use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use gbm::BufferObject;
use tab_protocol::{BufferIndex, FramebufferLinkPayload};

/// Metadata describing a DMA-BUF-backed buffer.
#[derive(Debug)]
pub struct TabBuffer {
	pub index: BufferIndex,
	bo: BufferObject<()>,
	fd: OwnedFd,
}

impl TabBuffer {
	pub fn new(index: BufferIndex, bo: BufferObject<()>) -> Self {
		Self {
			index,
			fd: bo.fd().unwrap(),
			bo,
		}
	}

	pub fn width(&self) -> i32 {
		self.bo.width() as i32
	}

	pub fn height(&self) -> i32 {
		self.bo.height() as i32
	}

	pub fn stride(&self) -> i32 {
		self.bo.stride() as i32
	}

	pub fn offset(&self) -> i32 {
		self.bo.offset(0) as i32
	}

	pub fn fourcc(&self) -> i32 {
		self.bo.format() as u32 as i32
	}

	pub fn fd(&self) -> RawFd {
		self.fd.as_raw_fd()
	}
}

/// Double-buffer swapchain model.
#[derive(Debug)]
pub struct TabSwapchain {
	pub monitor_id: String,
	pub buffers: [TabBuffer; 2],
	current: BufferIndex,
	last_acquired: Option<BufferIndex>,
	busy: [bool; 2],
}

impl TabSwapchain {
	pub fn new(monitor_id: impl Into<String>, buffers: [TabBuffer; 2]) -> Self {
		Self {
			monitor_id: monitor_id.into(),
			buffers,
			current: BufferIndex::Zero,
			last_acquired: None,
			busy: [false, false],
		}
	}

	pub fn acquire_next(&mut self) -> Option<(&TabBuffer, BufferIndex)> {
		let preferred = match self.current {
			BufferIndex::Zero => BufferIndex::One,
			BufferIndex::One => BufferIndex::Zero,
		};
		let candidate = [preferred, self.current]
			.into_iter()
			.find(|idx| !self.busy[*idx as usize])?;
		self.current = candidate;
		self.last_acquired = Some(candidate);
		Some((&self.buffers[candidate as usize], candidate))
	}

	pub fn rollback(&mut self) {
		if let Some(last) = self.last_acquired.take() {
			self.current = match last {
				BufferIndex::Zero => BufferIndex::One,
				BufferIndex::One => BufferIndex::Zero,
			};
		}
	}

	pub fn current(&self) -> (&TabBuffer, BufferIndex) {
		(&self.buffers[self.current as usize], self.current)
	}

	pub fn mark_busy(&mut self, idx: BufferIndex) {
		self.busy[idx as usize] = true;
		self.last_acquired = None;
	}

	pub fn mark_released(&mut self, idx: BufferIndex) {
		self.busy[idx as usize] = false;
	}

	pub fn framebuffer_link_payload(&self) -> FramebufferLinkPayload {
		let buffer = &self.buffers[0];
		FramebufferLinkPayload {
			monitor_id: self.monitor_id.clone(),
			width: buffer.width(),
			height: buffer.height(),
			stride: buffer.stride(),
			offset: buffer.offset(),
			fourcc: buffer.fourcc(),
		}
	}

	pub fn export_fds(&self) -> [RawFd; 2] {
		let fd0 = self.buffers[0].fd();
		let fd1 = self.buffers[1].fd();
		[fd0, fd1]
	}
}
