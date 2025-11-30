use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tab_protocol::{BufferIndex, MonitorInfo};

#[cfg(feature = "easydrm")]
pub trait MonitorIdStorage {
	fn monitor_id(&self) -> Option<&str>;
	fn set_monitor_id(&mut self, id: String);
}
#[derive(Clone)]
pub struct Cursor {
	width: u32,
	height: u32,
	hot_x: i32,
	hot_y: i32,
	image: Arc<[u8]>,
	position_x: i32,
	position_y: i32,
	image_hash: u64,
}
impl Cursor {
	pub fn new(
		width: u32,
		height: u32,
		hot_x: i32,
		hot_y: i32,
		image: Vec<u8>
	) -> Self {
		Self {
			width,
			height,
			hot_x,
			hot_y,
			image_hash: {
				use std::hash::{Hash, Hasher};
				let mut hasher = std::collections::hash_map::DefaultHasher::new();
				width.hash(&mut hasher);
				height.hash(&mut hasher);
				hot_x.hash(&mut hasher);
				hot_y.hash(&mut hasher);
				image.hash(&mut hasher);
				hasher.finish()
			},
			image: Arc::from(image),
			position_x: 0,
			position_y: 0,
		}
	}
	pub fn image(&self) -> &[u8] {
		&self.image
	}
	pub fn width(&self) -> u32 {
		self.width
	}
	pub fn height(&self) -> u32 {
		self.height
	}
	pub fn hot_x(&self) -> i32 {
		self.hot_x
	}
	pub fn hot_y(&self) -> i32 {
		self.hot_y
	}
	pub fn position_x(&self) -> i32 {
		self.position_x
	}
	pub fn position_y(&self) -> i32 {
		self.position_y
	}
	pub fn image_hash(&self) -> u64 {
		self.image_hash
	}
}
pub struct Output<Texture> {
	buffers: [Texture; 2],
	current: Option<BufferIndex>,
	queue: VecDeque<(BufferIndex, Instant)>,
	pending_page_flip: bool,
	current_swap_started: Option<Instant>,
	cursor: Option<Cursor>
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
				current_swap_started: None,
				cursor: None
			},
		);
	}
	pub fn swap_buffers(&mut self, session_id: &str, buffer: BufferIndex) -> bool {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return false;
		};
		if !o.pending_page_flip && o.queue.is_empty() {
			if let Some(current) = o.current {
				if current == buffer {
					tracing::error!(
						session_id = session_id,
						?buffer,
						"swap_buffers reused buffer before FRAME_DONE"
					);
				}
			}
			o.current = Some(buffer);
			o.current_swap_started = Some(Instant::now());
			o.pending_page_flip = true;
		} else {
			o.queue.push_back((buffer, Instant::now()));
		}
		true
	}
	pub fn current_buffer_for_session(&self, session_id: &str) -> Option<&Texture> {
		self.outputs.get(session_id)?.borrow_current_texture()
	}

	pub fn remove_session(&mut self, session_id: &str) -> Option<Texture> {
		self.outputs.remove(session_id)?.current_texture()
	}
	pub fn take_pending_page_flip(&mut self, session_id: &str) -> Option<Duration> {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return None;
		};
		if o.pending_page_flip {
			o.pending_page_flip = false;
			let latency = o
				.current_swap_started
				.map(|start| start.elapsed())
				.unwrap_or_default();
			o.current_swap_started = None;
			if let Some((next, started)) = o.queue.pop_front() {
				o.current = Some(next);
				o.current_swap_started = Some(started);
				o.pending_page_flip = true;
			}
			Some(latency)
		} else {
			None
		}
	}
	pub fn set_cursor(&mut self, session_id: &str, cursor: Cursor) -> bool {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return false;
		};
		o.cursor = Some(cursor);
		true
	}
	pub fn move_cursor(&mut self, session_id: &str, position_x: i32, position_y: i32) {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return;
		};
		if let Some(ref mut cursor) = o.cursor {
			cursor.position_x = position_x;
			cursor.position_y = position_y;
		}
	}
	pub fn clear_cursor(&mut self, session_id: &str) {
		let Some(o) = self.outputs.get_mut(session_id) else {
			return;
		};
		o.cursor.take();
	}
	pub fn get_cursor(&self, session_id: &str) -> Option<Cursor> {
		let Some(o) = self.outputs.get(session_id) else {
			return None;
		};
		// the image data is Arc, so cloning is cheap
		o.cursor.clone()
	}
}
