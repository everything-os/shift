use std::collections::{HashMap, VecDeque};

use tab_protocol::{BufferIndex, MonitorInfo};

#[cfg(feature = "easydrm")]
pub trait MonitorIdStorage {
	fn monitor_id(&self) -> Option<&str>;
	fn set_monitor_id(&mut self, id: String);
}
pub struct Output<Texture> {
	buffers: [Texture; 2],
	current: Option<BufferIndex>,
	queue: VecDeque<BufferIndex>,
	pending_page_flip: bool,
}
impl<Texture> Output<Texture> {
	pub fn current_texture(self) -> Option<Texture> {
		self.buffers.into_iter().nth(self.current? as usize)
	}
	pub fn borrow_current_texture(&self) -> Option<&Texture> {
		self.buffers.get(self.current? as usize)
	}
}
pub struct Monitor<Texture> {
	info: MonitorInfo,
	outputs: HashMap<String, Output<Texture>>,
}

impl<Texture> Monitor<Texture> {
	pub fn new(info: MonitorInfo) -> Self {
		Self {
			info,
			outputs: HashMap::new(),
		}
	}

	pub fn info(&self) -> &MonitorInfo {
		&self.info
	}

	pub fn update_info(&mut self, info: MonitorInfo) {
		self.info = info;
	}

	pub fn framebuffer_link(&mut self, session_id: String, buffers: [Texture; 2]) {
		self.outputs.insert(
			session_id,
			Output {
				buffers,
				current: None,
				queue: VecDeque::new(),
				pending_page_flip: false,
			},
		);
	}
	pub fn swap_buffers(&mut self, session_id: &str, buffer: BufferIndex) -> bool {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return false;
		};
		if !o.pending_page_flip && o.queue.is_empty() {
			o.current = Some(buffer);
			o.pending_page_flip = true;
		} else {
			o.queue.push_back(buffer);
		}
		true
	}
	pub fn current_buffer_for_session(&self, session_id: &str) -> Option<&Texture> {
		self.outputs.get(session_id)?.borrow_current_texture()
	}

	pub fn remove_session(&mut self, session_id: &str) -> Option<Texture> {
		self.outputs.remove(session_id)?.current_texture()
	}
	pub fn take_pending_page_flip(&mut self, session_id: &str) -> bool {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return false;
		};
		if o.pending_page_flip {
			o.pending_page_flip = false;
			if let Some(next) = o.queue.pop_front() {
				o.current = Some(next);
				o.pending_page_flip = true;
			}
			true
		} else {
			false
		}
	}
}
