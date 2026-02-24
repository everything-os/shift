use std::os::fd::{AsFd, OwnedFd};

use super::{FenceEvent, FenceWaitMode, RenderEvt, RenderingLayer, SlotKey};

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	pub(super) async fn emit_event(&self, event: RenderEvt) {
		if let Err(e) = self.event_tx.send(event).await {
			tracing::warn!("failed to send renderer event to server: {e}");
		}
	}

	pub(super) fn cancel_fence_wait(&mut self, key: SlotKey) {
		if let Some(handle) = self.fence_tasks.remove(&key) {
			self.fence_scheduler.cancel(handle);
		}
	}

	pub(super) fn spawn_acquire_fence_waiter(&mut self, key: SlotKey, fence_fd: OwnedFd) {
		if let Some(existing) = self.fence_tasks.get(&key).copied() {
			if let Ok(cloned_fd) = fence_fd.as_fd().try_clone_to_owned()
				&& self
					.fence_scheduler
					.reschedule(existing, vec![cloned_fd], FenceWaitMode::All)
			{
				return;
			}
			// Recover from unexpected scheduler/task-map desync.
			self.fence_tasks.remove(&key);
		}
		let tx = self.fence_event_tx.clone();
		let handle = self.fence_scheduler.schedule(
			vec![fence_fd],
			FenceWaitMode::All,
			Box::new(move || {
				let _ = tx.send(FenceEvent::Signaled { key });
			}),
		);
		self.fence_tasks.insert(key, handle);
	}

	pub(super) async fn handle_fence_event(&mut self, event: FenceEvent) {
		match event {
			FenceEvent::Signaled { key } => {
				self.fence_tasks.remove(&key);
				if let Some(previous) = self.ownership.apply_acquire_fence_signaled(key) {
					self
						.ownership
						.queue_buffer_release(key.monitor_id, key.session_id, previous);
				}
			}
		}
	}
}
