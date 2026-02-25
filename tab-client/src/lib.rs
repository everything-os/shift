//! Tab client rewrite crate.

mod c_bindings;
mod config;
mod error;
mod events;
mod gbm_allocator;
mod monitor;
mod swapchain;

pub use config::TabClientConfig;
pub use error::TabClientError;
pub use events::{InputEvent, MonitorEvent, RenderEvent, SessionEvent};
pub use monitor::{MonitorId, MonitorState};
pub use swapchain::{TabBuffer, TabSwapchain};

use std::collections::HashMap;
use std::os::{
	fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd, RawFd},
	unix::net::UnixStream,
};
use std::time::{Duration, Instant};

use tab_protocol::message_frame::{TabMessageFrame, TabMessageFrameReader};
use tab_protocol::message_header;
use tab_protocol::{
	AuthErrorPayload, AuthOkPayload, AuthPayload, BufferIndex, BufferReleasePayload,
	BufferRequestAckPayload, InputEventPayload, MonitorInfo, SessionActivePayload,
	SessionAwakePayload, SessionCreatePayload, SessionCreatedPayload, SessionInfo,
	SessionReadyPayload, SessionRole, SessionSleepPayload, SessionStatePayload,
	SessionSwitchPayload, TabMessage,
};

use crate::gbm_allocator::GbmAllocator;

/// Primary synchronous Tab client handle.
pub struct TabClient {
	socket: UnixStream,
	reader: TabMessageFrameReader,
	session: SessionInfo,
	monitors: HashMap<MonitorId, MonitorState>,
	monitor_listeners: Vec<Box<dyn Fn(&MonitorEvent)>>,
	render_listeners: Vec<Box<dyn Fn(&RenderEvent)>>,
	session_listeners: Vec<Box<dyn Fn(&SessionEvent)>>,
	input_listeners: Vec<Box<dyn Fn(&InputEvent)>>,
	gbm: GbmAllocator,
}

impl TabClient {
	const BUFFER_REQUEST_ACK_TIMEOUT: Duration = Duration::from_millis(250);
	const SESSION_CREATE_TIMEOUT: Duration = Duration::from_millis(500);

	pub fn connect(config: TabClientConfig) -> Result<Self, TabClientError> {
		let socket = tab_protocol::unix_socket_utils::connect_seqpacket(config.socket_path_ref())?;
		let mut reader = TabMessageFrameReader::new();
		let hello = Self::read_message(&socket, &mut reader)?;
		let TabMessage::Hello(payload) = hello else {
			return Err(TabClientError::Unexpected("expected hello"));
		};
		if payload.protocol != tab_protocol::PROTOCOL_VERSION {
			return Err(TabClientError::Unexpected("protocol mismatch"));
		}
		let auth_frame = TabMessageFrame::json(
			message_header::AUTH,
			AuthPayload {
				token: config.token().to_string(),
			},
		);
		auth_frame.encode_and_send(&socket)?;
		let auth_ok = Self::wait_for_auth(&socket, &mut reader)?;
		let monitors = auth_ok
			.monitors
			.into_iter()
			.map(|info| (info.id.clone(), MonitorState::new(info)))
			.collect();
		let gbm = GbmAllocator::new(config.render_node_path())?;
		socket.set_nonblocking(true)?;
		Ok(Self {
			socket,
			reader,
			session: auth_ok.session,
			monitors,
			monitor_listeners: Vec::new(),
			render_listeners: Vec::new(),
			session_listeners: Vec::new(),
			input_listeners: Vec::new(),
			gbm,
		})
	}

	pub fn session(&self) -> &SessionInfo {
		&self.session
	}

	pub fn monitors(&self) -> impl Iterator<Item = &MonitorState> {
		self.monitors.values()
	}

	pub fn monitor(&self, id: &str) -> Option<&MonitorState> {
		self.monitors.get(id)
	}

	pub fn socket_fd(&self) -> RawFd {
		self.socket.as_raw_fd()
	}

	pub fn poll_fds(&self) -> [RawFd; 2] {
		[self.socket.as_raw_fd(), self.drm_fd()]
	}

	pub fn drm_fd(&self) -> RawFd {
		self.gbm.drm_fd()
	}

	pub fn create_swapchain(&self, monitor_id: &str) -> Result<TabSwapchain, TabClientError> {
		let monitor = self
			.monitors
			.get(monitor_id)
			.ok_or_else(|| TabClientError::UnknownMonitor(monitor_id.to_string()))?;
		let swapchain = self.gbm.create_swapchain(monitor)?;
		self.framebuffer_link(&swapchain)?;
		Ok(swapchain)
	}

	pub fn framebuffer_link(&self, swapchain: &TabSwapchain) -> Result<(), TabClientError> {
		let payload = swapchain.framebuffer_link_payload();
		let mut frame = TabMessageFrame::json(message_header::FRAMEBUFFER_LINK, payload);
		let fds = swapchain.export_fds();
		frame.fds = Vec::from(fds);
		frame.encode_and_send(&self.socket)?;
		Ok(())
	}

	pub fn request_buffer(
		&mut self,
		monitor_id: &str,
		buffer: BufferIndex,
		acquire_fence: Option<RawFd>,
	) -> Result<(), TabClientError> {
		let payload = format!("{monitor_id} {}", buffer as u8);
		let frame = TabMessageFrame {
			header: message_header::BUFFER_REQUEST.into(),
			payload: Some(payload),
			fds: acquire_fence.map_or_else(Vec::new, |fd| vec![fd]),
		};
		frame.encode_and_send(&self.socket)?;
		self.wait_for_buffer_request_ack(monitor_id, buffer)?;
		Ok(())
	}

	pub fn send_ready(&self) -> Result<(), TabClientError> {
		let payload = SessionReadyPayload {
			session_id: self.session.id.clone(),
		};
		TabMessageFrame::json(message_header::SESSION_READY, payload).encode_and_send(&self.socket)?;
		Ok(())
	}

	pub fn create_session(
		&mut self,
		role: SessionRole,
		display_name: Option<String>,
	) -> Result<SessionCreatedPayload, TabClientError> {
		let payload = SessionCreatePayload { role, display_name };
		TabMessageFrame::json(message_header::SESSION_CREATE, payload)
			.encode_and_send(&self.socket)?;
		self.wait_for_session_created()
	}

	pub fn switch_session(
		&self,
		session_id: &str,
		animation: Option<String>,
		duration: Duration,
	) -> Result<(), TabClientError> {
		let payload = SessionSwitchPayload {
			session_id: session_id.to_string(),
			animation,
			duration,
		};
		TabMessageFrame::json(message_header::SESSION_SWITCH, payload)
			.encode_and_send(&self.socket)?;
		Ok(())
	}

	pub fn on_monitor_event<F>(&mut self, listener: F)
	where
		F: Fn(&MonitorEvent) + 'static,
	{
		self.monitor_listeners.push(Box::new(listener));
	}

	pub fn on_render_event<F>(&mut self, listener: F)
	where
		F: Fn(&RenderEvent) + 'static,
	{
		self.render_listeners.push(Box::new(listener));
	}

	pub fn on_session_event<F>(&mut self, listener: F)
	where
		F: Fn(&SessionEvent) + 'static,
	{
		self.session_listeners.push(Box::new(listener));
	}

	pub fn on_input_event<F>(&mut self, listener: F)
	where
		F: Fn(&InputEvent) + 'static,
	{
		self.input_listeners.push(Box::new(listener));
	}

	pub fn dispatch_events(&mut self) -> Result<(), TabClientError> {
		loop {
			match self.reader.read_framed(&self.socket) {
				Ok(frame) => {
					let message = TabMessage::try_from(frame)?;
					self.handle_message(message)?;
				}
				Err(tab_protocol::ProtocolError::WouldBlock) => break,
				Err(other) => return Err(other.into()),
			}
		}
		Ok(())
	}

	fn read_message(
		socket: &UnixStream,
		reader: &mut TabMessageFrameReader,
	) -> Result<TabMessage, TabClientError> {
		let frame = reader.read_framed(socket)?;
		Ok(TabMessage::try_from(frame)?)
	}

	fn wait_for_auth(
		socket: &UnixStream,
		reader: &mut TabMessageFrameReader,
	) -> Result<AuthOkPayload, TabClientError> {
		loop {
			match Self::read_message(socket, reader)? {
				TabMessage::AuthOk(payload) => return Ok(payload),
				TabMessage::AuthError(AuthErrorPayload { error }) => {
					return Err(TabClientError::Auth(error));
				}
				other => {
					return Err(TabClientError::Unexpected(match other {
						TabMessage::Hello(_) => "duplicate hello",
						TabMessage::Auth(_) => "unexpected auth from server",
						_ => "unexpected pre-auth message",
					}));
				}
			}
		}
	}

	fn handle_message(&mut self, message: TabMessage) -> Result<(), TabClientError> {
		match message {
			TabMessage::MonitorAdded(payload) => {
				self.handle_monitor_added(payload.monitor);
			}
			TabMessage::MonitorRemoved(payload) => {
				self.handle_monitor_removed(payload.monitor_id, payload.name);
			}
			TabMessage::SessionCreated(payload) => {
				self.handle_session_created(payload.session, payload.token);
			}
			TabMessage::BufferRelease {
				payload,
				release_fence,
			} => {
				self.handle_buffer_release(payload, release_fence);
			}
			TabMessage::SessionAwake(SessionAwakePayload { session_id }) => {
				self.handle_session_awake(session_id);
			}
			TabMessage::SessionSleep(SessionSleepPayload { session_id }) => {
				self.handle_session_sleep(session_id);
			}
			TabMessage::SessionActive(SessionActivePayload { session_id }) => {
				self.handle_session_active(session_id);
			}
			TabMessage::SessionState(SessionStatePayload { session }) => {
				self.handle_session_state(session);
			}
			TabMessage::InputEvent(payload) => {
				self.handle_input_event(payload);
			}
			_ => {}
		}
		Ok(())
	}

	fn handle_monitor_added(&mut self, info: MonitorInfo) {
		let state = MonitorState::new(info);
		self.monitors.insert(state.info.id.clone(), state.clone());
		let event = MonitorEvent::Added(state);
		for listener in &self.monitor_listeners {
			listener(&event);
		}
	}

	fn handle_monitor_removed(&mut self, monitor_id: String, name: String) {
		self.monitors.remove(&monitor_id);
		let event = MonitorEvent::Removed { monitor_id, name };
		for listener in &self.monitor_listeners {
			listener(&event);
		}
	}

	fn handle_buffer_release(
		&mut self,
		payload: BufferReleasePayload,
		release_fence: Option<OwnedFd>,
	) {
		let monitor_id = payload.monitor_id;
		let buffer = payload.buffer;
		for listener in &self.render_listeners {
			let release_fence_fd = release_fence
				.as_ref()
				.and_then(|fd| fd.as_fd().try_clone_to_owned().ok())
				.map(|fd| fd.into_raw_fd());
			let event = RenderEvent::BufferReleased {
				monitor_id: monitor_id.clone(),
				buffer,
				release_fence_fd,
			};
			listener(&event);
		}
	}

	fn handle_session_awake(&mut self, session_id: String) {
		let event = SessionEvent::Awake(session_id);
		for listener in &self.session_listeners {
			listener(&event);
		}
	}

	fn handle_session_active(&mut self, session_id: String) {
		let event = SessionEvent::Active(session_id);
		for listener in &self.session_listeners {
			listener(&event);
		}
	}

	fn handle_session_sleep(&mut self, session_id: String) {
		let event = SessionEvent::Sleep(session_id);
		for listener in &self.session_listeners {
			listener(&event);
		}
	}

	fn handle_session_created(&mut self, session: SessionInfo, token: String) {
		let event = SessionEvent::Created { session, token };
		for listener in &self.session_listeners {
			listener(&event);
		}
	}

	fn handle_session_state(&mut self, session: SessionInfo) {
		let event = SessionEvent::State(session);
		for listener in &self.session_listeners {
			listener(&event);
		}
	}

	fn handle_input_event(&mut self, payload: InputEventPayload) {
		let event = InputEvent::Event(payload);
		for listener in &self.input_listeners {
			listener(&event);
		}
	}

	fn wait_for_buffer_request_ack(
		&mut self,
		monitor_id: &str,
		buffer: BufferIndex,
	) -> Result<(), TabClientError> {
		let deadline = Instant::now() + Self::BUFFER_REQUEST_ACK_TIMEOUT;
		loop {
			if Instant::now() >= deadline {
				return Err(TabClientError::Unexpected("buffer_request_ack timeout"));
			}
			match self.reader.read_framed(&self.socket) {
				Ok(frame) => {
					let message = TabMessage::try_from(frame)?;
					match message {
						TabMessage::BufferRequestAck(BufferRequestAckPayload {
							monitor_id: ack_monitor,
							buffer: ack_buffer,
						}) => {
							if ack_monitor == monitor_id && ack_buffer == buffer {
								return Ok(());
							}
						}
						TabMessage::Error(err) => {
							let details = err
								.message
								.map(|m| format!("{}: {m}", err.code))
								.unwrap_or(err.code);
							return Err(TabClientError::Server(details));
						}
						other => self.handle_message(other)?,
					}
				}
				Err(tab_protocol::ProtocolError::WouldBlock) => {
					self.poll_socket_until(deadline)?;
				}
				Err(other) => return Err(other.into()),
			}
		}
	}

	fn wait_for_session_created(&mut self) -> Result<SessionCreatedPayload, TabClientError> {
		let deadline = Instant::now() + Self::SESSION_CREATE_TIMEOUT;
		loop {
			if Instant::now() >= deadline {
				return Err(TabClientError::Unexpected("session_created timeout"));
			}
			match self.reader.read_framed(&self.socket) {
				Ok(frame) => {
					let message = TabMessage::try_from(frame)?;
					match message {
						TabMessage::SessionCreated(payload) => {
							self.handle_session_created(payload.session.clone(), payload.token.clone());
							return Ok(payload);
						}
						TabMessage::Error(err) => {
							let details = err
								.message
								.map(|m| format!("{}: {m}", err.code))
								.unwrap_or(err.code);
							return Err(TabClientError::Server(details));
						}
						other => self.handle_message(other)?,
					}
				}
				Err(tab_protocol::ProtocolError::WouldBlock) => {
					self.poll_socket_until(deadline)?;
				}
				Err(other) => return Err(other.into()),
			}
		}
	}

	fn poll_socket_until(&self, deadline: Instant) -> Result<(), TabClientError> {
		let now = Instant::now();
		if now >= deadline {
			return Ok(());
		}
		let remaining = deadline.saturating_duration_since(now);
		let timeout_ms = (remaining.as_millis().max(1).min(i32::MAX as u128)) as i32;
		let mut pfd = libc::pollfd {
			fd: self.socket.as_raw_fd(),
			events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
			revents: 0,
		};
		loop {
			let rc = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, timeout_ms) };
			if rc >= 0 {
				return Ok(());
			}
			let err = std::io::Error::last_os_error();
			if err.kind() == std::io::ErrorKind::Interrupted {
				continue;
			}
			return Err(TabClientError::Io(err));
		}
	}
}
