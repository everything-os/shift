use std::env;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::thread;
use std::time::{Duration, Instant};

use image::GenericImageView;
use tab_client::{FrameTarget, TabClient, TabClientError, TabEvent, gl};
use tab_protocol::InputEventPayload;

const CURSOR_BYTES: &[u8] = include_bytes!("penger.png");
const TWO_PI: f32 = std::f32::consts::PI * 2.0;

fn main() -> Result<(), Box<dyn Error>> {
	tracing_subscriber::fmt::try_init().ok();
	let token = env::args()
		.nth(1)
		.or_else(|| env::var("SHIFT_SESSION_TOKEN").ok())
		.expect("Provide a session token via SHIFT_SESSION_TOKEN or argv[1]");

	let mut client = TabClient::connect_default(token)?;
	println!(
		"Connected to Shift server '{}' speaking {}",
		client.hello().server,
		client.hello().protocol
	);

	let cursor_image = CursorImageData::load()?;
	let gl = client.gl().clone();
	// client.send_ready()?;

	let mut monitor_id = None;
	let mut cursor_tracker = CursorTracker::new();
	refresh_active_monitor(
		&mut client,
		&mut monitor_id,
		&cursor_image,
		&mut cursor_tracker,
		false,
	)?;
	if let Some(active) = monitor_id.as_ref() {
		println!("Using monitor {} for cursor demo", active);
	} else {
		println!("Waiting for monitor...");
	}

	let mut phase = 0.0f32;
	let mut last_frame = Instant::now();
	loop {
		if let Some(active) = monitor_id.clone() {
			if let Some(info) = client.monitor_info(&active) {
				let dt = last_frame.elapsed().as_secs_f32();
				last_frame = Instant::now();
				phase = (phase + dt * 0.35).fract();
				match render_frame(&mut client, &gl, &active, phase) {
					Ok(_) => {}
					Err(TabClientError::NoFreeBuffers(_)) => {}
					Err(TabClientError::UnknownMonitor(_)) => {
						monitor_id = None;
						refresh_active_monitor(
							&mut client,
							&mut monitor_id,
							&cursor_image,
							&mut cursor_tracker,
							true,
						)?;
						continue;
					}
					Err(err) => return Err(err.into()),
				}
			} else {
				monitor_id = None;
				refresh_active_monitor(
					&mut client,
					&mut monitor_id,
					&cursor_image,
					&mut cursor_tracker,
					true,
				)?;
			}
		}

		let blocking = monitor_id.is_none();
		let events = pump_events(&mut client, blocking)?;
		handle_events(
			&mut client,
			&events,
			&mut monitor_id,
			&cursor_image,
			&mut cursor_tracker,
		)?;
		if monitor_id.is_none() {
			println!("Waiting for monitor...");
		}
		thread::sleep(Duration::from_millis(8));
	}
}

fn render_frame(
	client: &mut TabClient,
	gl: &gl::Gles2,
	monitor_id: &str,
	phase: f32,
) -> Result<(), TabClientError> {
	match client.acquire_frame(monitor_id) {
		Ok(frame) => {
			draw_background(gl, &frame, phase);
			client.swap_buffers(monitor_id)?;
		}
		Err(TabClientError::NoFreeBuffers(_)) => {}
		Err(err) => return Err(err),
	}
	Ok(())
}

fn draw_background(gl: &gl::Gles2, target: &FrameTarget, phase: f32) {
	let (width, height) = target.size();
	let angle = phase * TWO_PI;
	let r = angle.cos() * 0.5 + 0.5;
	let g = angle.sin() * 0.5 + 0.5;
	let b = (angle * 0.5).sin() * 0.5 + 0.5;
	unsafe {
		gl.BindFramebuffer(gl::FRAMEBUFFER, target.framebuffer());
		gl.Viewport(0, 0, width, height);
		gl.ClearColor(r, g, b, 1.0);
		gl.Clear(gl::COLOR_BUFFER_BIT);
	}
}

fn apply_cursor_image(
	client: &mut TabClient,
	monitor_id: &str,
	cursor: &CursorImageData,
) -> Result<(), TabClientError> {
	println!(
		"Uploading cursor {}x{} to monitor {}",
		cursor.width, cursor.height, monitor_id
	);
	match client.set_cursor_framebuffer(
		monitor_id,
		cursor.width,
		cursor.height,
		cursor.hotspot_x,
		cursor.hotspot_y,
		&cursor.pixels,
	) {
		Ok(()) => {
			println!("Cursor upload sent");
			Ok(())
		}
		Err(err) => {
			eprintln!("Cursor upload failed: {err}");
			Err(err)
		}
	}
}

fn refresh_active_monitor(
	client: &mut TabClient,
	monitor_id: &mut Option<String>,
	cursor: &CursorImageData,
	cursor_tracker: &mut CursorTracker,
	force_reapply: bool,
) -> Result<(), TabClientError> {
	let mut ids = client.monitor_ids();
	ids.sort();
	if let Some(first) = ids.into_iter().next() {
		let changed = monitor_id.as_deref() != Some(first.as_str());
		if changed {
			println!("Switching to monitor {}", first);
		} else if force_reapply {
			println!("Reapplying cursor to monitor {}", first);
		}
		if changed || force_reapply {
			if let Some(info) = client.monitor_info(&first) {
				cursor_tracker.set_monitor(info.width, info.height);
			}
			*monitor_id = Some(first.clone());
			apply_cursor_image(client, &first, cursor)?;
		}
	} else if monitor_id.take().is_some() {
		cursor_tracker.clear();
		println!("All monitors disconnected; waiting for reconnection");
	}
	Ok(())
}

fn pump_events(client: &mut TabClient, blocking: bool) -> Result<Vec<TabEvent>, TabClientError> {
	let socket_fd = client.socket_fd().as_raw_fd();
	let swap_fd = client.swap_notifier_fd().as_raw_fd();
	let mut pfds = [
		libc::pollfd {
			fd: socket_fd,
			events: libc::POLLIN,
			revents: 0,
		},
		libc::pollfd {
			fd: swap_fd,
			events: libc::POLLIN,
			revents: 0,
		},
	];
	let timeout = if blocking { -1 } else { 0 };
	let ready = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, timeout) };
	if ready < 0 {
		let err = std::io::Error::last_os_error();
		if err.kind() == std::io::ErrorKind::Interrupted {
			return Ok(Vec::new());
		}
		return Err(TabClientError::Io(err));
	}
	let mut events = Vec::new();
	if ready == 0 {
		return Ok(events);
	}
	if pfds[0].revents & libc::POLLIN != 0 {
		events.extend(client.process_socket_events()?);
	}
	if pfds[1].revents & libc::POLLIN != 0 {
		client.process_ready_swaps()?;
	}
	Ok(events)
}

fn handle_events(
	client: &mut TabClient,
	events: &[TabEvent],
	monitor_id: &mut Option<String>,
	cursor: &CursorImageData,
	cursor_tracker: &mut CursorTracker,
) -> Result<(), TabClientError> {
	let mut cursor_movement = (0, 0);
	let mut monitor_event = false;
	for event in events {
		match event {
			TabEvent::MonitorAdded(info) => {
				println!("Monitor {} added", info.id);
				monitor_event = true;
			}
			TabEvent::MonitorRemoved(id) => {
				println!("Monitor {} removed", id);
				monitor_event = true;
			}
			TabEvent::SessionState(state) => {
				println!("Session {} is now {:?}", state.id, state.state);
			}
			TabEvent::Input(payload) => {
				if let Some((x, y)) = cursor_tracker.update_from_input(payload) {
					cursor_movement.0 = x;
					cursor_movement.1 = y;
				}
			}
			TabEvent::FrameDone { .. } => {}
			TabEvent::SessionCreated(_) => {}
			TabEvent::Error(err) => {
				eprintln!(
					"[Shift error] code={} message={}",
					err.code,
					err.message.clone().unwrap_or_else(|| "unknown".into())
				);
			}
		}
	}
	if let Some(active) = monitor_id.clone() {
		client.set_cursor_position(&active, cursor_movement.0, cursor_movement.1)?;
	}

	if monitor_event {
		refresh_active_monitor(client, monitor_id, cursor, cursor_tracker, true)?;
	}
	Ok(())
}

struct CursorImageData {
	width: u32,
	height: u32,
	hotspot_x: i32,
	hotspot_y: i32,
	pixels: Vec<u8>,
}

impl CursorImageData {
	fn load() -> Result<Self, Box<dyn Error>> {
		let image = image::load_from_memory(CURSOR_BYTES)?.to_rgba8();
		let (width, height) = image.dimensions();
		let pixels = image.into_raw();
		Ok(Self {
			width,
			height,
			hotspot_x: (width / 2) as i32,
			hotspot_y: (height / 2) as i32,
			pixels,
		})
	}
}

struct CursorTracker {
	width: i32,
	height: i32,
	x: f64,
	y: f64,
}

impl CursorTracker {
	fn new() -> Self {
		Self {
			width: 0,
			height: 0,
			x: 0.0,
			y: 0.0,
		}
	}

	fn set_monitor(&mut self, width: i32, height: i32) {
		self.width = width.max(1);
		self.height = height.max(1);
		self.x = (self.width / 2) as f64;
		self.y = (self.height / 2) as f64;
	}

	fn clear(&mut self) {
		self.width = 0;
		self.height = 0;
		self.x = 0.0;
		self.y = 0.0;
	}

	fn update_from_input(&mut self, payload: &tab_protocol::InputEventPayload) -> Option<(i32, i32)> {
		match payload {
			tab_protocol::InputEventPayload::PointerMotion { x, y, .. } => self.add_position(*x, *y),
			tab_protocol::InputEventPayload::PointerMotionAbsolute {
				x_transformed,
				y_transformed,
				..
			} => self.set_position(*x_transformed, *y_transformed),
			tab_protocol::InputEventPayload::TouchMotion { contact, .. }
			| tab_protocol::InputEventPayload::TouchDown { contact, .. } => {
				self.set_position(contact.x_transformed, contact.y_transformed)
			}
			_ => None,
		}
	}

	fn set_position(&mut self, x: f64, y: f64) -> Option<(i32, i32)> {
		if self.width == 0 || self.height == 0 {
			return None;
		}
		self.x = x.clamp(0., self.width as _);
		self.y = y.clamp(0., self.height as _);
		Some((self.x as _, self.y as _))
	}
	fn add_position(&mut self, x: f64, y: f64) -> Option<(i32, i32)> {
		if self.width == 0 || self.height == 0 {
			return None;
		}
		self.x += x.clamp(0., self.width as _);
		self.y += y.clamp(0., self.height as _);
		Some((self.x as _, self.y as _))
	}

	fn current(&self) -> (i32, i32) {
		(self.x as _, self.y as _)
	}
}
