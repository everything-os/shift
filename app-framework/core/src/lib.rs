use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use monitor_layout_engine::{
	MonitorPlacement, MonitorSpec, clamp_point_to_layout, is_valid_edge_contiguous_layout,
	layout_horizontal, move_cursor_no_tunnel,
};
use tab_client::{
	InputEvent as TabInputEvent, MonitorEvent as TabMonitorEvent, RenderEvent as TabRenderEvent,
};
use tab_client::{TabClient, TabClientConfig, TabClientError, TabSwapchain};
use tab_protocol::{BufferIndex, ButtonState, InputEventPayload, KeyState, TouchContact};
use thiserror::Error;
use tracing::{debug, info};
pub use tab_protocol::{SessionCreatedPayload, SessionInfo, SessionRole};

const BTN_LEFT: u32 = 272;

/// Frame scheduling policy used by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
	/// Continuously render whenever a buffer becomes available.
	Eager,
	/// Render only when explicitly scheduled by the application.
	Scheduled,
}

/// Runtime configuration used during framework initialization.
#[derive(Debug, Clone)]
pub struct Config {
	token: String,
	socket_path: PathBuf,
	render_node_path: Option<PathBuf>,
	render_mode: RenderMode,
	opengl_version: (u8, u8),
}

impl Config {
	/// Creates a configuration using an explicit session token.
	pub fn from_token(token: impl Into<String>) -> Self {
		Self {
			token: token.into(),
			socket_path: tab_protocol::DEFAULT_SOCKET_PATH.into(),
			render_node_path: None,
			render_mode: RenderMode::Scheduled,
			opengl_version: (3, 3),
		}
	}

	/// Creates a configuration from process environment.
	///
	/// Requires `SHIFT_SESSION_TOKEN`.
	pub fn from_env() -> Result<Self, FrameworkError> {
		let token = std::env::var("SHIFT_SESSION_TOKEN")
			.map_err(|_| FrameworkError::Config("missing SHIFT_SESSION_TOKEN".into()))?;
		Ok(Self::from_token(token))
	}

	/// Sets the session token used for authentication.
	pub fn set_token(&mut self, token: impl Into<String>) -> &mut Self {
		self.token = token.into();
		self
	}

	/// Sets the Unix socket path for server communication.
	pub fn set_socket_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
		self.socket_path = path.as_ref().to_path_buf();
		self
	}

	/// Sets the DRM render node path used by GBM allocation.
	pub fn set_render_node_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
		self.render_node_path = Some(path.as_ref().to_path_buf());
		self
	}

	/// Sets the render mode used by the main loop.
	pub fn set_render_mode(&mut self, mode: RenderMode) -> &mut Self {
		self.render_mode = mode;
		self
	}

	/// Requests a specific OpenGL/OpenGL ES version.
	pub fn opengl_version(&mut self, major: u8, minor: u8) -> &mut Self {
		self.opengl_version = (major, minor);
		self
	}

	/// Returns the configured render mode.
	pub fn render_mode(&self) -> RenderMode {
		self.render_mode
	}

	/// Returns the requested OpenGL/OpenGL ES version.
	pub fn requested_opengl_version(&self) -> (u8, u8) {
		self.opengl_version
	}

	/// Returns the configured session token.
	pub fn token(&self) -> &str {
		&self.token
	}

	/// Returns the configured render node path, if set.
	pub fn render_node_path(&self) -> Option<&Path> {
		self.render_node_path.as_deref()
	}
}

/// Top-level framework errors.
#[derive(Debug, Error)]
pub enum FrameworkError {
	#[error("framework config error: {0}")]
	Config(String),
	#[error("tab client error: {0}")]
	Client(#[from] TabClientError),
	#[error("poll failed: {0}")]
	Poll(std::io::Error),
	#[error("monitor not found: {0}")]
	MonitorNotFound(String),
}

/// Logical monitor metadata exposed to applications.
#[derive(Debug, Clone)]
pub struct Monitor {
	/// Stable monitor identifier used by the protocol.
	pub id: String,
	/// Human-readable monitor name.
	pub name: String,
	/// Logical width in pixels.
	pub width: i32,
	/// Logical height in pixels.
	pub height: i32,
	/// Nominal refresh rate in Hz.
	pub refresh_rate: i32,
	/// Monitor origin X in global layout space.
	pub x: i32,
	/// Monitor origin Y in global layout space.
	pub y: i32,
	/// Scale factor for logical-to-physical mapping.
	pub scale: f64,
}

impl Monitor {
	fn from_tab_monitor(state: &tab_client::MonitorState) -> Self {
		Self {
			id: state.info.id.clone(),
			name: state.info.name.clone(),
			width: state.info.width,
			height: state.info.height,
			refresh_rate: state.info.refresh_rate,
			x: 0,
			y: 0,
			scale: 1.0,
		}
	}
}

fn recompute_layout(monitors: &mut HashMap<String, MonitorRuntime>) {
	let specs: Vec<_> = monitors
		.values()
		.map(|m| MonitorSpec {
			id: m.monitor.id.clone(),
			width: m.monitor.width,
			height: m.monitor.height,
		})
		.collect();
	let placements = layout_horizontal(&specs);
	for p in placements {
		if let Some(m) = monitors.get_mut(&p.id) {
			m.monitor.x = p.x;
			m.monitor.y = p.y;
		}
	}
}

fn current_layout(monitors: &HashMap<String, MonitorRuntime>) -> Vec<MonitorPlacement> {
	monitors
		.values()
		.map(|m| MonitorPlacement {
			id: m.monitor.id.clone(),
			x: m.monitor.x,
			y: m.monitor.y,
			width: m.monitor.width,
			height: m.monitor.height,
		})
		.collect()
}

/// Render callback payload containing the acquired client buffer.
#[derive(Debug, Clone)]
pub struct RenderEvent {
	/// Target monitor id.
	pub monitor_id: String,
	/// Acquired swapchain buffer index.
	pub buffer_index: BufferIndex,
	/// DMA-BUF file descriptor for the render target.
	pub dmabuf_fd: RawFd,
	/// Buffer width in pixels.
	pub width: i32,
	/// Buffer height in pixels.
	pub height: i32,
	/// Buffer stride in bytes.
	pub stride: i32,
	/// Buffer offset in bytes.
	pub offset: i32,
	/// DRM fourcc pixel format.
	pub fourcc: i32,
}

/// Present callback payload emitted after a rendered buffer is released.
#[derive(Debug, Clone)]
pub struct PresentEvent {
	/// Target monitor id.
	pub monitor_id: String,
	/// Buffer index that reached presentation completion.
	pub buffer_index: BufferIndex,
}

/// Emitted when a monitor is added.
#[derive(Debug, Clone)]
pub struct MonitorAddedEvent {
	/// Added monitor metadata.
	pub monitor: Monitor,
}

/// Emitted when a monitor is removed.
#[derive(Debug, Clone)]
pub struct MonitorRemovedEvent {
	/// Removed monitor id.
	pub monitor_id: String,
	/// Removed monitor name.
	pub name: String,
}

/// Session state update payload.
#[derive(Debug, Clone)]
pub struct SessionEvent {
	/// Full session state snapshot.
	pub session: SessionInfo,
}

/// Emitted when a watched file descriptor becomes readable.
#[derive(Debug, Clone)]
pub struct FdReadyEvent {
	/// File descriptor that is ready.
	pub fd: RawFd,
}

/// Raw input payload forwarded from the server.
#[derive(Debug, Clone)]
pub struct InputEvent {
	/// Protocol input event payload.
	pub payload: InputEventPayload,
}

/// Keyboard event payload.
#[derive(Debug, Clone)]
pub struct KeyEvent {
	/// Input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Linux keycode.
	pub key: u32,
	/// Key state.
	pub state: KeyState,
}

impl KeyEvent {
	/// Returns `true` when this event is a key press.
	pub fn is_pressed(&self) -> bool {
		matches!(self.state, KeyState::Pressed)
	}
}

/// Text composition event payload.
#[derive(Debug, Clone)]
pub struct CharEvent {
	/// Composed UTF-8 text.
	pub text: String,
}

/// Pointer device class for pointer-style events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerType {
	/// Mouse-like pointer device (mouse/touchpad/trackpoint).
	Mouse,
	/// Pen/stylus input device.
	Pen,
	/// Touch contact input device.
	Touch,
	/// Device class could not be determined.
	Unknown,
}

/// Generic pointer movement event (browser-like `pointermove` semantics).
#[derive(Debug, Clone)]
pub struct PointerMoveEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Pointer class that moved the cursor.
	pub pointer_type: PointerType,
	/// Previous cursor position in global layout space.
	pub old_position: (f64, f64),
	/// New cursor position in global layout space.
	pub new_position: (f64, f64),
}

impl PointerMoveEvent {
	/// Returns `(dx, dy)` in global layout space.
	pub fn delta(&self) -> (f64, f64) {
		(
			self.new_position.0 - self.old_position.0,
			self.new_position.1 - self.old_position.1,
		)
	}
}

/// Mouse-only movement event (browser-like `mousemove` semantics).
#[derive(Debug, Clone)]
pub struct MouseMoveEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Previous cursor position in global layout space.
	pub old_position: (f64, f64),
	/// New cursor position in global layout space.
	pub new_position: (f64, f64),
}

impl MouseMoveEvent {
	/// Returns `(dx, dy)` in global layout space.
	pub fn delta(&self) -> (f64, f64) {
		(
			self.new_position.0 - self.old_position.0,
			self.new_position.1 - self.old_position.1,
		)
	}
}

/// Pointer down event (browser-like `pointerdown` semantics).
#[derive(Debug, Clone)]
pub struct PointerDownEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Pointer class for this event.
	pub pointer_type: PointerType,
	/// Logical button code.
	pub button: u32,
	/// Cursor position in global layout space.
	pub position: (f64, f64),
}

/// Pointer up event (browser-like `pointerup` semantics).
#[derive(Debug, Clone)]
pub struct PointerUpEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Pointer class for this event.
	pub pointer_type: PointerType,
	/// Logical button code.
	pub button: u32,
	/// Cursor position in global layout space.
	pub position: (f64, f64),
}

/// Mouse down event (browser-like `mousedown` semantics).
#[derive(Debug, Clone)]
pub struct MouseDownEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Mouse button code.
	pub button: u32,
	/// Cursor position in global layout space.
	pub position: (f64, f64),
}

/// Mouse up event (browser-like `mouseup` semantics).
#[derive(Debug, Clone)]
pub struct MouseUpEvent {
	/// Source input device id.
	pub device: u32,
	/// Event timestamp in microseconds.
	pub time_usec: u64,
	/// Mouse button code.
	pub button: u32,
	/// Cursor position in global layout space.
	pub position: (f64, f64),
}

/// High-level touch event stream preserving contact ids for multitouch.
#[derive(Debug, Clone)]
pub enum TouchEvent {
	/// New touch contact.
	Down {
		device: u32,
		time_usec: u64,
		contact: TouchContact,
	},
	/// Updated touch contact.
	Motion {
		device: u32,
		time_usec: u64,
		contact: TouchContact,
	},
	/// Touch contact ended.
	Up {
		device: u32,
		time_usec: u64,
		contact_id: i32,
	},
	/// End of touch event frame batch.
	Frame {
		time_usec: u64,
	},
	/// Touch sequence cancelled.
	Cancel {
		time_usec: u64,
	},
}

/// High-level multi-finger gesture event stream.
#[derive(Debug, Clone)]
pub enum GestureEvent {
	SwipeBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	SwipeUpdate {
		device: u32,
		time_usec: u64,
		fingers: u32,
		dx: f64,
		dy: f64,
	},
	SwipeEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},
	PinchBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	PinchUpdate {
		device: u32,
		time_usec: u64,
		fingers: u32,
		dx: f64,
		dy: f64,
		scale: f64,
		rotation: f64,
	},
	PinchEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},
	HoldBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	HoldEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},
}

/// Initialization context passed to [`Application::init`].
pub struct InitContext<A: Application> {
	config: Config,
	_marker: PhantomData<A>,
}

impl<A: Application> InitContext<A> {
	fn new(config: Config) -> Self {
		Self {
			config,
			_marker: PhantomData,
		}
	}

	/// Returns the current runtime configuration.
	pub fn config(&self) -> &Config {
		&self.config
	}

	/// Returns mutable runtime configuration.
	pub fn config_mut(&mut self) -> &mut Config {
		&mut self.config
	}
}

/// Core application trait implemented by framework users.
pub trait Application: Sized + 'static {
	/// Constructs the application instance.
	fn init(ctx: &mut InitContext<Self>) -> anyhow::Result<Self>;

	/// Called after a buffer is acquired and ready to be rendered into.
	fn on_render(&mut self, _ctx: &mut Context<Self>, _ev: RenderEvent) {}
	/// Called when a previously rendered buffer is presented/released.
	fn on_present(&mut self, _ctx: &mut Context<Self>, _ev: PresentEvent) {}
	/// Called when a monitor becomes available.
	fn on_monitor_added(&mut self, _ctx: &mut Context<Self>, _ev: MonitorAddedEvent) {}
	/// Called when a monitor is removed.
	fn on_monitor_removed(&mut self, _ctx: &mut Context<Self>, _ev: MonitorRemovedEvent) {}
	/// Called when session state changes.
	fn on_session_state(&mut self, _ctx: &mut Context<Self>, _ev: SessionEvent) {}
	/// Called for every raw input event.
	fn on_input(&mut self, _ctx: &mut Context<Self>, _ev: InputEvent) {}
	/// Called for key events.
	fn on_key(&mut self, _ctx: &mut Context<Self>, _ev: KeyEvent) {}
	/// Called for composed text events.
	fn on_char(&mut self, _ctx: &mut Context<Self>, _ev: CharEvent) {}
	/// Called when any pointer device moves the cursor.
	fn on_pointer_move(&mut self, _ctx: &mut Context<Self>, _ev: PointerMoveEvent) {}
	/// Called when a mouse-like device moves the cursor.
	fn on_mouse_move(&mut self, _ctx: &mut Context<Self>, _ev: MouseMoveEvent) {}
	/// Called when any pointer device produces a down transition.
	fn on_pointer_down(&mut self, _ctx: &mut Context<Self>, _ev: PointerDownEvent) {}
	/// Called when any pointer device produces an up transition.
	fn on_pointer_up(&mut self, _ctx: &mut Context<Self>, _ev: PointerUpEvent) {}
	/// Called when a mouse-like device produces a down transition.
	fn on_mouse_down(&mut self, _ctx: &mut Context<Self>, _ev: MouseDownEvent) {}
	/// Called when a mouse-like device produces an up transition.
	fn on_mouse_up(&mut self, _ctx: &mut Context<Self>, _ev: MouseUpEvent) {}
	/// Called for multitouch contact lifecycle events.
	fn on_touch(&mut self, _ctx: &mut Context<Self>, _ev: TouchEvent) {}
	/// Called for high-level multi-finger gesture events.
	fn on_gesture(&mut self, _ctx: &mut Context<Self>, _ev: GestureEvent) {}
	/// Called when a watched file descriptor is readable.
	fn on_fd_ready(&mut self, _ctx: &mut Context<Self>, _ev: FdReadyEvent) {}
	/// Called when the framework surfaces an error.
	fn on_error(&mut self, _ctx: &mut Context<Self>, _error: &FrameworkError) {}
}

/// Mutable runtime context passed into application callbacks.
pub struct Context<'a, A: Application> {
	client: &'a mut TabClient,
	monitors: &'a mut HashMap<String, MonitorRuntime>,
	scheduled: &'a mut HashSet<String>,
	watched_fds: &'a mut HashSet<RawFd>,
	next_acquire_fence: &'a mut Option<OwnedFd>,
	cursor_position: &'a mut (f64, f64),
	exiting: &'a mut bool,
	_marker: PhantomData<A>,
}

impl<'a, A: Application> Context<'a, A> {
	/// Schedules a frame for a specific monitor.
	pub fn schedule_frame(&mut self, monitor_id: impl Into<String>) {
		self.scheduled.insert(monitor_id.into());
	}

	/// Schedules a frame for every known monitor.
	pub fn schedule_all_frames(&mut self) {
		self.scheduled.extend(self.monitors.keys().cloned());
	}

	/// Returns an iterator over all known monitors.
	pub fn monitors(&self) -> impl Iterator<Item = &Monitor> {
		self.monitors.values().map(|m| &m.monitor)
	}

	/// Returns a monitor by id.
	pub fn monitor(&self, monitor_id: &str) -> Option<&Monitor> {
		self.monitors.get(monitor_id).map(|m| &m.monitor)
	}

	/// Sets monitor position in global layout space.
	///
	/// The resulting layout must remain edge-contiguous and non-overlapping.
	pub fn set_monitor_position(
		&mut self,
		monitor_id: &str,
		x: i32,
		y: i32,
	) -> Result<(), FrameworkError> {
		let old = {
			let Some(m) = self.monitors.get(monitor_id) else {
				return Err(FrameworkError::MonitorNotFound(monitor_id.to_string()));
			};
			(m.monitor.x, m.monitor.y)
		};
		if let Some(m) = self.monitors.get_mut(monitor_id) {
			m.monitor.x = x;
			m.monitor.y = y;
		}
		let placements = current_layout(self.monitors);
		if !is_valid_edge_contiguous_layout(&placements) {
			if let Some(m) = self.monitors.get_mut(monitor_id) {
				m.monitor.x = old.0;
				m.monitor.y = old.1;
			}
			return Err(FrameworkError::Config(
				"invalid monitor layout: monitors must edge-touch, must not overlap, and cannot form islands"
					.into(),
			));
		}
		let (cx, cy) = clamp_point_to_layout(&placements, self.cursor_position.0, self.cursor_position.1);
		*self.cursor_position = (cx, cy);
		Ok(())
	}

	/// Recomputes monitor positions using default horizontal packing.
	pub fn apply_horizontal_layout(&mut self) {
		recompute_layout(self.monitors);
		let placements = current_layout(self.monitors);
		let (cx, cy) = clamp_point_to_layout(&placements, self.cursor_position.0, self.cursor_position.1);
		*self.cursor_position = (cx, cy);
	}

	/// Returns current cursor position in global layout space.
	pub fn cursor_position(&self) -> (f64, f64) {
		*self.cursor_position
	}

	/// Adds a file descriptor to the readable watch set.
	pub fn watch_fd(&mut self, fd: RawFd) {
		self.watched_fds.insert(fd);
	}

	/// Removes a file descriptor from the watch set.
	pub fn unwatch_fd(&mut self, fd: RawFd) {
		self.watched_fds.remove(&fd);
	}

	/// Requests graceful termination of the main loop.
	pub fn request_exit(&mut self) {
		*self.exiting = true;
	}

	/// Sets an acquire fence to be sent with the next buffer request.
	pub fn set_next_acquire_fence(&mut self, fence_fd: OwnedFd) {
		*self.next_acquire_fence = Some(fence_fd);
	}

	/// Returns current authenticated session information.
	pub fn session(&self) -> &SessionInfo {
		self.client.session()
	}

	/// Sends `session_ready` for the current session.
	pub fn session_ready(&mut self) -> Result<(), FrameworkError> {
		self.client.send_ready().map_err(FrameworkError::from)
	}

	/// Backward-compatible alias for [`Context::session_ready`].
	pub fn send_ready(&mut self) -> Result<(), FrameworkError> {
		self.session_ready()
	}

	/// Requests creation of a new session and waits for server response.
	pub fn create_session(
		&mut self,
		role: SessionRole,
		display_name: Option<String>,
	) -> Result<SessionCreatedPayload, FrameworkError> {
		self.client
			.create_session(role, display_name)
			.map_err(FrameworkError::from)
	}

	/// Requests switching to another session.
	pub fn switch_session(
		&mut self,
		session_id: &str,
		animation: Option<String>,
		duration: Duration,
	) -> Result<(), FrameworkError> {
		self.client
			.switch_session(session_id, animation, duration)
			.map_err(FrameworkError::from)
	}

	/// Returns direct mutable access to the underlying tab client.
	///
	/// Prefer high-level methods when possible.
	pub fn raw_client(&mut self) -> &mut TabClient {
		self.client
	}
}

/// Main application runtime.
pub struct TabAppFramework<A: Application> {
	app: A,
	client: TabClient,
	render_mode: RenderMode,
	monitors: HashMap<String, MonitorRuntime>,
	scheduled: HashSet<String>,
	watched_fds: HashSet<RawFd>,
	event_queue: Rc<RefCell<VecDeque<QueuedEvent>>>,
	exiting: bool,
	next_acquire_fence: Option<OwnedFd>,
	stats: LoopStats,
	cursor_position: (f64, f64),
	touch_contacts: HashMap<i32, (f64, f64)>,
	primary_touch_id: Option<i32>,
}

impl<A: Application> TabAppFramework<A> {
	/// Initializes the framework and application state.
	pub fn init(configure: impl FnOnce(&mut Config)) -> Result<Self, FrameworkError> {
		let mut init_ctx = InitContext::<A>::new(Config::from_env()?);
		configure(init_ctx.config_mut());
		let app = A::init(&mut init_ctx)
			.map_err(|e| FrameworkError::Config(format!("app init failed: {e:#}")))?;

		let cfg = init_ctx.config().clone();
		let mut client_cfg = TabClientConfig::new(cfg.token()).socket_path(cfg.socket_path.clone());
		if let Some(render_node) = cfg.render_node_path {
			client_cfg = client_cfg.render_node(render_node);
		}
		let mut client = TabClient::connect(client_cfg)?;
		let queue = Rc::new(RefCell::new(VecDeque::new()));
		Self::attach_event_queue(&mut client, Rc::clone(&queue));

		let mut monitors = HashMap::new();
		for tab_monitor in client.monitors() {
			let monitor = Monitor::from_tab_monitor(tab_monitor);
			let swapchain = client.create_swapchain(&monitor.id)?;
			monitors.insert(monitor.id.clone(), MonitorRuntime::new(monitor, swapchain));
		}
			recompute_layout(&mut monitors);
			let initial_cursor = {
				let placements = current_layout(&monitors);
				let seed = placements
					.iter()
					.min_by(|a, b| {
						(a.x, a.y, a.id.as_str()).cmp(&(b.x, b.y, b.id.as_str()))
					})
					.map(|m| {
						(
							m.x as f64 + (m.width.max(1) as f64 / 2.0),
							m.y as f64 + (m.height.max(1) as f64 / 2.0),
						)
					})
					.unwrap_or((0.0, 0.0));
				clamp_point_to_layout(&placements, seed.0, seed.1)
			};
			let scheduled = if cfg.render_mode == RenderMode::Eager {
				monitors.keys().cloned().collect()
			} else {
				HashSet::new()
			};

		Ok(Self {
			app,
			client,
			render_mode: cfg.render_mode,
			monitors,
			scheduled,
			watched_fds: HashSet::new(),
				event_queue: queue,
				exiting: false,
				next_acquire_fence: None,
				stats: LoopStats::new(),
				cursor_position: initial_cursor,
				touch_contacts: HashMap::new(),
				primary_touch_id: None,
			})
		}

	/// Runs the main event/render loop until exit is requested.
	pub fn run(&mut self) -> Result<(), FrameworkError> {
		while !self.exiting {
			let has_queued_events = !self.event_queue.borrow().is_empty();
			let timeout_ms = if self.scheduled.is_empty() && !has_queued_events {
				-1
			} else {
				0
			};
			let (tab_ready, ready_fds) = self.poll_once(timeout_ms)?;
			if tab_ready {
				self.client.dispatch_events()?;
			}
			self.flush_pending_releases();
			for fd in ready_fds {
				let ev = FdReadyEvent { fd };
				self.call_app(|app, ctx| app.on_fd_ready(ctx, ev));
			}
			self.drain_tab_events()?;
			self.flush_pending_releases();
			self.render_scheduled()?;
			self.stats.maybe_log();
		}
		Ok(())
	}

	fn attach_event_queue(client: &mut TabClient, queue: Rc<RefCell<VecDeque<QueuedEvent>>>) {
		let q = Rc::clone(&queue);
		client.on_monitor_event(move |ev| {
			q.borrow_mut().push_back(QueuedEvent::Monitor(ev.clone()));
		});
		let q = Rc::clone(&queue);
		client.on_render_event(move |ev| {
			q.borrow_mut().push_back(QueuedEvent::Render(ev.clone()));
		});
		let q = Rc::clone(&queue);
		client.on_input_event(move |ev| {
			q.borrow_mut().push_back(QueuedEvent::Input(ev.clone()));
		});
		let q = Rc::clone(&queue);
		client.on_session_event(move |ev| {
			q.borrow_mut().push_back(QueuedEvent::Session(ev.clone()));
		});
	}

	fn poll_once(&self, timeout_ms: i32) -> Result<(bool, Vec<RawFd>), FrameworkError> {
		let mut pending_release_fds = Vec::new();
		for monitor in self.monitors.values() {
			for fence in &monitor.pending_release_fences {
				if let Some(fd) = fence {
					pending_release_fds.push(std::os::fd::AsRawFd::as_raw_fd(fd));
				}
			}
		}

		let watched_count = self.watched_fds.len();
		let mut pollfds = Vec::with_capacity(1 + watched_count + pending_release_fds.len());
		pollfds.push(libc::pollfd {
			fd: self.client.socket_fd(),
			events: libc::POLLIN,
			revents: 0,
		});
		for fd in &self.watched_fds {
			pollfds.push(libc::pollfd {
				fd: *fd,
				events: libc::POLLIN,
				revents: 0,
			});
		}
		for fd in pending_release_fds {
			pollfds.push(libc::pollfd {
				fd,
				events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
				revents: 0,
			});
		}
		let rc = unsafe {
			libc::poll(
				pollfds.as_mut_ptr(),
				pollfds.len() as libc::nfds_t,
				timeout_ms,
			)
		};
		if rc < 0 {
			return Err(FrameworkError::Poll(std::io::Error::last_os_error()));
		}
		if rc == 0 {
			return Ok((false, Vec::new()));
		}
		let tab_ready = (pollfds[0].revents & libc::POLLIN) != 0;
		let mut ready_fds = Vec::new();
		for pfd in pollfds.iter().skip(1).take(watched_count) {
			if (pfd.revents & libc::POLLIN) != 0 {
				ready_fds.push(pfd.fd);
			}
		}
		Ok((tab_ready, ready_fds))
	}

	fn drain_tab_events(&mut self) -> Result<(), FrameworkError> {
		loop {
			let maybe_event = self.event_queue.borrow_mut().pop_front();
			let Some(event) = maybe_event else {
				break;
			};
			match event {
				QueuedEvent::Monitor(ev) => match ev {
					TabMonitorEvent::Added(state) => {
						let monitor = Monitor::from_tab_monitor(&state);
						let swapchain = self.client.create_swapchain(&monitor.id)?;
						if self.render_mode == RenderMode::Eager {
							self.scheduled.insert(monitor.id.clone());
						}
						self.monitors.insert(
							monitor.id.clone(),
							MonitorRuntime::new(monitor.clone(), swapchain),
						);
						recompute_layout(&mut self.monitors);
						let placements = current_layout(&self.monitors);
						self.cursor_position =
							clamp_point_to_layout(&placements, self.cursor_position.0, self.cursor_position.1);
						let monitor = self
							.monitors
							.get(&state.info.id)
							.map(|m| m.monitor.clone())
							.unwrap_or(monitor);
						self.call_app(|app, ctx| {
							app.on_monitor_added(
								ctx,
								MonitorAddedEvent {
									monitor: monitor.clone(),
								},
							)
						});
					}
					TabMonitorEvent::Removed { monitor_id, name } => {
						self.monitors.remove(&monitor_id);
						recompute_layout(&mut self.monitors);
						let placements = current_layout(&self.monitors);
						self.cursor_position =
							clamp_point_to_layout(&placements, self.cursor_position.0, self.cursor_position.1);
						self.scheduled.remove(&monitor_id);
						self.call_app(|app, ctx| {
							app.on_monitor_removed(
								ctx,
								MonitorRemovedEvent {
									monitor_id: monitor_id.clone(),
									name: name.clone(),
								},
							)
						});
					}
				},
				QueuedEvent::Render(ev) => {
					self.stats.buffer_release_events += 1;
					let TabRenderEvent::BufferReleased {
						monitor_id,
						buffer,
						release_fence_fd,
					} = ev;
					self.stats.instant_log(&format!(
						"buffer_release event monitor={monitor_id} buffer={} fence={}",
						buffer as u8,
						if release_fence_fd.is_some() { "yes" } else { "no" }
					));
					let mut should_emit_present = false;
					if let Some(monitor) = self.monitors.get_mut(&monitor_id) {
						if let Some(fd) = release_fence_fd {
							monitor.pending_release_fences[buffer as usize] =
								Some(unsafe { OwnedFd::from_raw_fd(fd) });
						} else {
							if monitor.pending_present[buffer as usize] {
								monitor.pending_present[buffer as usize] = false;
								should_emit_present = true;
							}
							monitor.swapchain.mark_released(buffer);
							if self.render_mode == RenderMode::Eager {
								self.scheduled.insert(monitor_id.clone());
							}
						}
					}
					if should_emit_present {
						self.stats.present_callbacks += 1;
						self.call_app(|app, ctx| {
							app.on_present(
								ctx,
								PresentEvent {
									monitor_id: monitor_id.clone(),
									buffer_index: buffer,
								},
							)
						});
					}
				}
				QueuedEvent::Input(ev) => {
					let TabInputEvent::Event(payload) = ev;
					self.call_app(|app, ctx| {
						app.on_input(
							ctx,
							InputEvent {
								payload: payload.clone(),
							},
						)
					});
						match payload {
							InputEventPayload::Key {
								device,
								time_usec,
								key,
								state,
							} => {
								self.call_app(|app, ctx| {
									app.on_key(
										ctx,
										KeyEvent {
											device,
											time_usec,
											key,
											state,
										},
									)
								});
							}
							InputEventPayload::PointerMotion {
								device,
								time_usec,
								dx,
								dy,
								..
							} => {
								let old_position = self.cursor_position;
								let placements = current_layout(&self.monitors);
								self.cursor_position = move_cursor_no_tunnel(
									&placements,
									self.cursor_position.0,
									self.cursor_position.1,
									dx,
									dy,
								);
								self.emit_cursor_move(
									PointerMoveEvent {
										device,
										time_usec,
										pointer_type: PointerType::Mouse,
										old_position,
										new_position: self.cursor_position,
									},
									true,
								);
							}
							InputEventPayload::PointerButton {
								device,
								time_usec,
								button,
								state,
							} => match state {
								ButtonState::Pressed => self.emit_pointer_down(
									PointerDownEvent {
										device,
										time_usec,
										pointer_type: PointerType::Mouse,
										button,
										position: self.cursor_position,
									},
									true,
								),
								ButtonState::Released => self.emit_pointer_up(
									PointerUpEvent {
										device,
										time_usec,
										pointer_type: PointerType::Mouse,
										button,
										position: self.cursor_position,
									},
									true,
								),
							},
							InputEventPayload::PointerMotionAbsolute {
								device,
								time_usec,
								x_transformed,
								y_transformed,
								..
							} => {
								let old_position = self.cursor_position;
								let placements = current_layout(&self.monitors);
								self.cursor_position =
									clamp_point_to_layout(&placements, x_transformed, y_transformed);
								self.emit_cursor_move(
									PointerMoveEvent {
										device,
										time_usec,
										pointer_type: PointerType::Mouse,
										old_position,
										new_position: self.cursor_position,
									},
									true,
								);
							}
							InputEventPayload::TabletToolAxis {
								device,
								time_usec,
								axes,
								..
							} => {
								let old_position = self.cursor_position;
								let placements = current_layout(&self.monitors);
								let (mut x, mut y) = (axes.x, axes.y);
								if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
									let max_x = placements
										.iter()
										.map(|m| m.x.saturating_add(m.width))
										.max()
										.unwrap_or(0)
										.max(1) as f64;
									let max_y = placements
										.iter()
										.map(|m| m.y.saturating_add(m.height))
										.max()
										.unwrap_or(0)
										.max(1) as f64;
									x *= max_x;
									y *= max_y;
								}
								self.cursor_position = clamp_point_to_layout(&placements, x, y);
								self.emit_cursor_move(
									PointerMoveEvent {
										device,
										time_usec,
										pointer_type: PointerType::Pen,
										old_position,
										new_position: self.cursor_position,
									},
									false,
								);
							}
							InputEventPayload::TouchDown {
								device,
								time_usec,
								contact,
							} => {
								let placements = current_layout(&self.monitors);
								let mut x = contact.x_transformed;
								let mut y = contact.y_transformed;
								if x > 1.0 || y > 1.0 {
									x /= 65535.0;
									y /= 65535.0;
								}
								let max_x = placements
									.iter()
									.map(|m| m.x.saturating_add(m.width))
									.max()
									.unwrap_or(0)
									.max(1) as f64;
								let max_y = placements
									.iter()
									.map(|m| m.y.saturating_add(m.height))
									.max()
									.unwrap_or(0)
									.max(1) as f64;
								let old_position = self.cursor_position;
								self.cursor_position =
									clamp_point_to_layout(&placements, x * max_x, y * max_y);
								self.touch_contacts
									.insert(contact.id, self.cursor_position);
								self.emit_touch(TouchEvent::Down {
									device,
									time_usec,
									contact: contact.clone(),
								});
								if self.primary_touch_id.is_none() {
									self.primary_touch_id = Some(contact.id);
									self.emit_cursor_move(
										PointerMoveEvent {
											device,
											time_usec,
											pointer_type: PointerType::Touch,
											old_position,
											new_position: self.cursor_position,
										},
										false,
									);
									self.emit_pointer_down(
										PointerDownEvent {
											device,
											time_usec,
											pointer_type: PointerType::Touch,
											button: BTN_LEFT,
											position: self.cursor_position,
										},
										false,
									);
								}
							}
							InputEventPayload::TouchMotion {
								device,
								time_usec,
								contact,
							} => {
								let placements = current_layout(&self.monitors);
								let mut x = contact.x_transformed;
								let mut y = contact.y_transformed;
								if x > 1.0 || y > 1.0 {
									x /= 65535.0;
									y /= 65535.0;
								}
								let max_x = placements
									.iter()
									.map(|m| m.x.saturating_add(m.width))
									.max()
									.unwrap_or(0)
									.max(1) as f64;
								let max_y = placements
									.iter()
									.map(|m| m.y.saturating_add(m.height))
									.max()
									.unwrap_or(0)
									.max(1) as f64;
								let next =
									clamp_point_to_layout(&placements, x * max_x, y * max_y);
								self.touch_contacts.insert(contact.id, next);
								self.emit_touch(TouchEvent::Motion {
									device,
									time_usec,
									contact: contact.clone(),
								});
								if self.primary_touch_id == Some(contact.id) {
									let old_position = self.cursor_position;
									self.cursor_position = next;
									self.emit_cursor_move(
										PointerMoveEvent {
											device,
											time_usec,
											pointer_type: PointerType::Touch,
											old_position,
											new_position: self.cursor_position,
										},
										false,
									);
								}
							}
							InputEventPayload::TouchUp {
								device,
								time_usec,
								contact_id,
							} => {
								self.touch_contacts.remove(&contact_id);
								self.emit_touch(TouchEvent::Up {
									device,
									time_usec,
									contact_id,
								});
								if self.primary_touch_id == Some(contact_id) {
									self.emit_pointer_up(
										PointerUpEvent {
											device,
											time_usec,
											pointer_type: PointerType::Touch,
											button: BTN_LEFT,
											position: self.cursor_position,
										},
										false,
									);
									self.primary_touch_id = self.touch_contacts.keys().next().copied();
								}
							}
							InputEventPayload::TouchFrame { time_usec } => {
								self.emit_touch(TouchEvent::Frame { time_usec });
							}
							InputEventPayload::TouchCancel { time_usec } => {
								self.emit_touch(TouchEvent::Cancel { time_usec });
								if self.primary_touch_id.take().is_some() {
									self.emit_pointer_up(
										PointerUpEvent {
											device: 0,
											time_usec,
											pointer_type: PointerType::Touch,
											button: BTN_LEFT,
											position: self.cursor_position,
										},
										false,
									);
								}
								self.touch_contacts.clear();
							}
							InputEventPayload::GestureSwipeBegin {
								device,
								time_usec,
								fingers,
							} => self.emit_gesture(GestureEvent::SwipeBegin {
								device,
								time_usec,
								fingers,
							}),
							InputEventPayload::GestureSwipeUpdate {
								device,
								time_usec,
								fingers,
								dx,
								dy,
							} => self.emit_gesture(GestureEvent::SwipeUpdate {
								device,
								time_usec,
								fingers,
								dx,
								dy,
							}),
							InputEventPayload::GestureSwipeEnd {
								device,
								time_usec,
								cancelled,
							} => self.emit_gesture(GestureEvent::SwipeEnd {
								device,
								time_usec,
								cancelled,
							}),
							InputEventPayload::GesturePinchBegin {
								device,
								time_usec,
								fingers,
							} => self.emit_gesture(GestureEvent::PinchBegin {
								device,
								time_usec,
								fingers,
							}),
							InputEventPayload::GesturePinchUpdate {
								device,
								time_usec,
								fingers,
								dx,
								dy,
								scale,
								rotation,
							} => self.emit_gesture(GestureEvent::PinchUpdate {
								device,
								time_usec,
								fingers,
								dx,
								dy,
								scale,
								rotation,
							}),
							InputEventPayload::GesturePinchEnd {
								device,
								time_usec,
								cancelled,
							} => self.emit_gesture(GestureEvent::PinchEnd {
								device,
								time_usec,
								cancelled,
							}),
							InputEventPayload::GestureHoldBegin {
								device,
								time_usec,
								fingers,
							} => self.emit_gesture(GestureEvent::HoldBegin {
								device,
								time_usec,
								fingers,
							}),
							InputEventPayload::GestureHoldEnd {
								device,
								time_usec,
								cancelled,
							} => self.emit_gesture(GestureEvent::HoldEnd {
								device,
								time_usec,
								cancelled,
							}),
							_ => (),
						}
					}
				QueuedEvent::Session(ev) => {
					if let tab_client::SessionEvent::State(session) = ev {
						self.call_app(|app, ctx| {
							app.on_session_state(
								ctx,
								SessionEvent {
									session: session.clone(),
								},
							)
						});
					}
				}
			}
		}
		Ok(())
	}

	fn render_scheduled(&mut self) -> Result<(), FrameworkError> {
		let targets: Vec<_> = self.scheduled.drain().collect();
		for monitor_id in targets {
			self.stats
				.instant_log(&format!("render_scheduled begin monitor={monitor_id}"));
			let Some((buffer_idx, render_ev)) = (|| {
				let monitor_rt = self.monitors.get_mut(&monitor_id)?;
				let (buffer, buffer_idx) = monitor_rt.swapchain.acquire_next()?;
				self.stats.acquire_ok += 1;
				let render_ev = RenderEvent {
					monitor_id: monitor_id.clone(),
					buffer_index: buffer_idx,
					dmabuf_fd: buffer.fd(),
					width: buffer.width(),
					height: buffer.height(),
					stride: buffer.stride(),
					offset: buffer.offset(),
					fourcc: buffer.fourcc(),
				};
				Some((buffer_idx, render_ev))
			})() else {
				self.stats.acquire_miss += 1;
				continue;
			};
			self.next_acquire_fence = None;
			self.call_app(|app, ctx| app.on_render(ctx, render_ev.clone()));
			let acquire_fence = self
				.next_acquire_fence
				.as_ref()
				.map(|fd| fd.as_raw_fd());
			self.stats.instant_log(&format!(
				"request_buffer send monitor={monitor_id} buffer={} fence={}",
				buffer_idx as u8,
				acquire_fence
					.map(|fd| fd.to_string())
					.unwrap_or_else(|| "none".to_string())
			));

				match self.client.request_buffer(&monitor_id, buffer_idx, acquire_fence) {
					Ok(()) => {
						self.stats.request_ok += 1;
						self.stats.instant_log(&format!(
							"request_buffer ack monitor={monitor_id} buffer={}",
							buffer_idx as u8
						));
						if let Some(monitor_rt) = self.monitors.get_mut(&monitor_id) {
							monitor_rt.swapchain.mark_busy(buffer_idx);
							monitor_rt.pending_present[buffer_idx as usize] = true;
						}
						if self.render_mode == RenderMode::Eager {
							// Keep requesting while another client-owned buffer exists.
							// This avoids deadlocking on the first frame in double-buffering.
							self.scheduled.insert(monitor_id.clone());
						}
					}
				Err(err) => {
					self.stats.request_err += 1;
					self.stats.instant_log(&format!(
						"request_buffer err monitor={monitor_id} buffer={} err={}",
						buffer_idx as u8,
						err
					));
					if let Some(monitor_rt) = self.monitors.get_mut(&monitor_id) {
						monitor_rt.swapchain.rollback();
					}
					if self.render_mode == RenderMode::Eager {
						let err_text = err.to_string();
						let ownership_related = err_text.contains("ownership_violation")
							|| err_text.contains("buffer_request_inflight")
							|| err_text.contains("session_sleeping")
							|| err_text.contains("not client-owned");
						if !ownership_related {
							self.scheduled.insert(monitor_id.clone());
						}
					}
					let ferr: FrameworkError = err.into();
					self.call_app(|app, ctx| app.on_error(ctx, &ferr));
				}
			}
		}
		Ok(())
	}

	fn flush_pending_releases(&mut self) {
		let mut errors = Vec::new();
		let mut presents = Vec::new();
		let mut ready_monitors = Vec::new();
		for monitor_rt in self.monitors.values_mut() {
			for buffer_idx in 0..monitor_rt.pending_release_fences.len() {
				let Some(fence) = monitor_rt.pending_release_fences[buffer_idx].as_ref() else {
					continue;
				};
				let signaled = match fd_readable_now(fence) {
					Ok(v) => v,
					Err(err) => {
						errors.push(err);
						true
					}
				};
				if signaled {
					monitor_rt.pending_release_fences[buffer_idx] = None;
					self.stats.release_fence_signaled += 1;
					let buffer = match buffer_idx {
						0 => BufferIndex::Zero,
						1 => BufferIndex::One,
						_ => continue,
					};
					self.stats.instant_log(&format!(
						"release_fence signaled monitor={} buffer={}",
						monitor_rt.monitor.id, buffer_idx
					));
					monitor_rt.swapchain.mark_released(buffer);
					if monitor_rt.pending_present[buffer_idx] {
						monitor_rt.pending_present[buffer_idx] = false;
						presents.push(PresentEvent {
							monitor_id: monitor_rt.monitor.id.clone(),
							buffer_index: buffer,
						});
					}
					if self.render_mode == RenderMode::Eager {
						ready_monitors.push(monitor_rt.monitor.id.clone());
					}
				}
			}
		}
		for monitor_id in ready_monitors {
			self.scheduled.insert(monitor_id);
		}
		for ev in presents {
			self.stats.present_callbacks += 1;
			self.call_app(|app, ctx| app.on_present(ctx, ev));
		}
		for err in errors {
			self.call_app(|app, ctx| app.on_error(ctx, &err));
		}
	}

	fn emit_cursor_move(&mut self, ev: PointerMoveEvent, also_mouse: bool) {
		if ev.old_position == ev.new_position {
			return;
		}
		let mouse_ev = MouseMoveEvent {
			device: ev.device,
			time_usec: ev.time_usec,
			old_position: ev.old_position,
			new_position: ev.new_position,
		};
		self.call_app(|app, ctx| app.on_pointer_move(ctx, ev.clone()));
		if also_mouse {
			self.call_app(|app, ctx| app.on_mouse_move(ctx, mouse_ev));
		}
	}

	fn emit_pointer_down(&mut self, ev: PointerDownEvent, also_mouse: bool) {
		let mouse_ev = MouseDownEvent {
			device: ev.device,
			time_usec: ev.time_usec,
			button: ev.button,
			position: ev.position,
		};
		self.call_app(|app, ctx| app.on_pointer_down(ctx, ev));
		if also_mouse {
			self.call_app(|app, ctx| app.on_mouse_down(ctx, mouse_ev));
		}
	}

	fn emit_pointer_up(&mut self, ev: PointerUpEvent, also_mouse: bool) {
		let mouse_ev = MouseUpEvent {
			device: ev.device,
			time_usec: ev.time_usec,
			button: ev.button,
			position: ev.position,
		};
		self.call_app(|app, ctx| app.on_pointer_up(ctx, ev));
		if also_mouse {
			self.call_app(|app, ctx| app.on_mouse_up(ctx, mouse_ev));
		}
	}

	fn emit_touch(&mut self, ev: TouchEvent) {
		self.call_app(|app, ctx| app.on_touch(ctx, ev));
	}

	fn emit_gesture(&mut self, ev: GestureEvent) {
		self.call_app(|app, ctx| app.on_gesture(ctx, ev));
	}

	fn call_app<F>(&mut self, f: F)
	where
		F: FnOnce(&mut A, &mut Context<A>),
	{
		let mut ctx = Context::<A> {
			client: &mut self.client,
			monitors: &mut self.monitors,
			scheduled: &mut self.scheduled,
			watched_fds: &mut self.watched_fds,
			next_acquire_fence: &mut self.next_acquire_fence,
			cursor_position: &mut self.cursor_position,
			exiting: &mut self.exiting,
			_marker: PhantomData,
		};
		f(&mut self.app, &mut ctx);
	}
}

#[derive(Debug)]
struct LoopStats {
	enabled: bool,
	last_log: Instant,
	acquire_ok: u64,
	acquire_miss: u64,
	request_ok: u64,
	request_err: u64,
	buffer_release_events: u64,
	release_fence_signaled: u64,
	present_callbacks: u64,
}

impl LoopStats {
	fn new() -> Self {
		let enabled = std::env::var("TAB_APP_FRAMEWORK_TRACE")
			.ok()
			.map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
			.unwrap_or(false);
		Self {
			enabled,
			last_log: Instant::now(),
			acquire_ok: 0,
			acquire_miss: 0,
			request_ok: 0,
			request_err: 0,
			buffer_release_events: 0,
			release_fence_signaled: 0,
			present_callbacks: 0,
		}
	}

	fn maybe_log(&mut self) {
		if !self.enabled || self.last_log.elapsed() < Duration::from_secs(1) {
			return;
		}
		info!(
			target: "tab_app_framework.core",
			acquire_ok = self.acquire_ok,
			acquire_miss = self.acquire_miss,
			request_ok = self.request_ok,
			request_err = self.request_err,
			releases = self.buffer_release_events,
			fence_ready = self.release_fence_signaled,
			present = self.present_callbacks,
			"taf stats"
		);
		self.last_log = Instant::now();
		self.acquire_ok = 0;
		self.acquire_miss = 0;
		self.request_ok = 0;
		self.request_err = 0;
		self.buffer_release_events = 0;
		self.release_fence_signaled = 0;
		self.present_callbacks = 0;
	}

	fn instant_log(&self, msg: &str) {
		if self.enabled {
			debug!(target: "tab_app_framework.core", "{msg}");
		}
	}
}

#[derive(Debug)]
struct MonitorRuntime {
	monitor: Monitor,
	swapchain: TabSwapchain,
	pending_release_fences: [Option<OwnedFd>; 2],
	pending_present: [bool; 2],
}

impl MonitorRuntime {
	fn new(monitor: Monitor, swapchain: TabSwapchain) -> Self {
		Self {
			monitor,
			swapchain,
			pending_release_fences: [None, None],
			pending_present: [false, false],
		}
	}
}

#[derive(Debug, Clone)]
enum QueuedEvent {
	Monitor(TabMonitorEvent),
	Render(TabRenderEvent),
	Input(TabInputEvent),
	Session(tab_client::SessionEvent),
}

fn fd_readable_now(fd: &OwnedFd) -> Result<bool, FrameworkError> {
	let mut pfd = libc::pollfd {
		fd: std::os::fd::AsRawFd::as_raw_fd(fd),
		events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
		revents: 0,
	};
	loop {
		let rc = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, 0) };
		if rc > 0 {
			return Ok(
				(pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL))
					!= 0,
			);
		}
		if rc == 0 {
			return Ok(false);
		}
		let err = std::io::Error::last_os_error();
		if err.kind() == std::io::ErrorKind::Interrupted {
			continue;
		}
		return Err(FrameworkError::Poll(err));
	}
}
