use std::{
	collections::HashMap,
	io::ErrorKind,
	os::fd::{AsRawFd, OwnedFd},
	sync::{Arc, Mutex},
};

use futures::future::{join_all, select_all};
use tokio::{io::unix::AsyncFd, sync::mpsc, task::JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct FenceTaskHandle(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FenceWaitMode {
	Any,
	All,
}

type TaskCallback = Box<dyn FnOnce() + Send + 'static>;
type SharedCallback = Arc<Mutex<Option<TaskCallback>>>;

struct CompletedTask {
	handle: FenceTaskHandle,
	callback: SharedCallback,
}

pub(super) struct FenceScheduler {
	next_id: u64,
	tasks: HashMap<FenceTaskHandle, JoinHandle<()>>,
	callbacks: HashMap<FenceTaskHandle, SharedCallback>,
	tx: mpsc::UnboundedSender<CompletedTask>,
	rx: mpsc::UnboundedReceiver<CompletedTask>,
}

impl FenceScheduler {
	pub fn new() -> Self {
		let (tx, rx) = mpsc::unbounded_channel();
		Self {
			next_id: 1,
			tasks: HashMap::new(),
			callbacks: HashMap::new(),
			tx,
			rx,
		}
	}

	pub fn schedule(
		&mut self,
		fences: Vec<OwnedFd>,
		mode: FenceWaitMode,
		callback: TaskCallback,
	) -> FenceTaskHandle {
		let handle = FenceTaskHandle(self.next_id);
		self.next_id = self.next_id.saturating_add(1);
		let callback = Arc::new(Mutex::new(Some(callback)));
		let task = spawn_wait_task(handle, fences, mode, Arc::clone(&callback), self.tx.clone());
		self.tasks.insert(handle, task);
		self.callbacks.insert(handle, callback);
		handle
	}

	pub fn reschedule(
		&mut self,
		handle: FenceTaskHandle,
		fences: Vec<OwnedFd>,
		mode: FenceWaitMode,
	) -> bool {
		let Some(callback) = self.callbacks.get(&handle).cloned() else {
			return false;
		};
		if let Some(task) = self.tasks.remove(&handle) {
			task.abort();
		}
		let task = spawn_wait_task(handle, fences, mode, callback, self.tx.clone());
		self.tasks.insert(handle, task);
		true
	}

	pub fn cancel(&mut self, handle: FenceTaskHandle) -> bool {
		if let Some(task) = self.tasks.remove(&handle) {
			task.abort();
		}
		self.callbacks.remove(&handle).is_some()
	}

	pub async fn recv_and_run(&mut self) -> bool {
		let Some(completed) = self.rx.recv().await else {
			return false;
		};
		self.tasks.remove(&completed.handle);
		self.callbacks.remove(&completed.handle);
		if let Ok(mut guard) = completed.callback.lock()
			&& let Some(callback) = guard.take()
		{
			callback();
		}
		true
	}
}

fn spawn_wait_task(
	handle: FenceTaskHandle,
	fences: Vec<OwnedFd>,
	mode: FenceWaitMode,
	callback: SharedCallback,
	tx: mpsc::UnboundedSender<CompletedTask>,
) -> JoinHandle<()> {
	tokio::spawn(async move {
		let wait_ok = wait_many_fences(fences, mode).await;
		if wait_ok {
			let _ = tx.send(CompletedTask { handle, callback });
		}
	})
}

async fn wait_many_fences(fences: Vec<OwnedFd>, mode: FenceWaitMode) -> bool {
	if fences.is_empty() {
		return true;
	}

	match mode {
		FenceWaitMode::Any => {
			let waiters = fences
				.into_iter()
				.map(|fd| Box::pin(wait_one_fence(fd)))
				.collect::<Vec<_>>();
			let (result, _idx, _rest) = select_all(waiters).await;
			result
		}
		FenceWaitMode::All => {
			let results = join_all(fences.into_iter().map(wait_one_fence)).await;
			results.into_iter().all(|ok| ok)
		}
	}
}

async fn wait_one_fence(fd: OwnedFd) -> bool {
	let afd = match AsyncFd::new(fd) {
		Ok(afd) => afd,
		Err(_) => return false,
	};

	loop {
		let mut guard = match afd.readable().await {
			Ok(guard) => guard,
			Err(_) => return false,
		};
		match guard.try_io(|inner| {
			let raw = inner.as_raw_fd();
			let mut poll_fd = nix::libc::pollfd {
				fd: raw,
				events: (nix::libc::POLLIN | nix::libc::POLLERR | nix::libc::POLLHUP) as i16,
				revents: 0,
			};
			let result = unsafe { nix::libc::poll(&mut poll_fd as *mut nix::libc::pollfd, 1, 0) };
			if result > 0
				&& (poll_fd.revents & (nix::libc::POLLIN | nix::libc::POLLERR | nix::libc::POLLHUP) as i16)
					!= 0
			{
				Ok(())
			} else {
				Err(std::io::Error::new(
					ErrorKind::WouldBlock,
					"fence not signaled yet",
				))
			}
		}) {
			Ok(Ok(())) => return true,
			Ok(Err(_)) => return false,
			Err(_) => continue,
		}
	}
}
