use std::collections::HashMap;
#[cfg(feature = "easydrm")]
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::client::{Client, ServerEvent};
use crate::connection::TabConnection;
use crate::monitor::Monitor;
#[cfg(feature = "easydrm")]
use crate::monitor::MonitorIdStorage;
use crate::session::{CycleDirection, SessionRegistry};
use tab_protocol::{
	DEFAULT_SOCKET_PATH, FramebufferLinkPayload, InputEventPayload, MonitorAddedPayload, MonitorInfo,
	MonitorRemovedPayload, ProtocolError, SessionActivePayload, SessionInfo, SessionRole,
	SessionStatePayload, SessionSwitchPayload, TabMessageFrame, message_header,
};
use tracing::warn;

const DEFAULT_SWITCH_ANIMATION: &str = "slideleft";
const DEFAULT_SWITCH_DURATION: Duration = Duration::from_millis(1000);

/// Server-side error type.
#[derive(Debug, thiserror::Error)]
pub enum TabServerError {
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("protocol error: {0}")]
	Protocol(#[from] ProtocolError),
	#[error("texture load error: {0}")]
	Texture(String),
}

type LoaderFn<Texture> =
	Arc<dyn Fn(RawFd, &FramebufferLinkPayload) -> Result<Texture, TabServerError>>;
pub struct SessionTransitionState {
	last_switch_time: Instant,
	animation: String,
	duration: Duration,
	old_session_id: Option<String>,
}
impl SessionTransitionState {
	pub fn new(animation: String, duration: Duration, old_session_id: Option<String>) -> Self {
		Self {
			animation,
			old_session_id,
			duration,
			last_switch_time: Instant::now(),
		}
	}
	fn elapsed_secs_f64(&self) -> f64 {
		self.last_switch_time.elapsed().as_secs_f64()
	}
	pub fn progress(&self) -> f64 {
		(self.elapsed_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
	}
	pub fn animation(&self) -> &str {
		self.animation.as_str()
	}
	pub fn old_session_id(&self) -> Option<&str> {
		self.old_session_id.as_deref()
	}
}
pub struct RenderSnapshot<'a, Texture> {
	pub active_session_id: Option<&'a str>,
	pub transition: Option<RenderTransition<'a>>,
	pub monitors: Vec<MonitorRenderSnapshot<'a, Texture>>,
}

pub struct RenderTransition<'a> {
	pub animation: &'a str,
	pub progress: f64,
	pub previous_session_id: Option<&'a str>,
	pub new_session_id: Option<&'a str>,
}

pub struct MonitorRenderSnapshot<'a, Texture> {
	pub monitor_id: &'a str,
	pub info: &'a MonitorInfo,
	pub active_texture: Option<&'a Texture>,
	pub previous_texture: Option<&'a Texture>,
}
/// Headless Tab protocol server that coordinates client sessions.
pub struct TabServer<Texture> {
	path: PathBuf,
	listener: UnixListener,
	clients: Vec<Client<Texture>>,
	sessions: SessionRegistry,
	load_dmabuf: LoaderFn<Texture>,
	monitors: HashMap<String, Monitor<Texture>>,
	current_session_id: Option<String>,
	transition_state: Option<SessionTransitionState>,
}

impl<Texture> TabServer<Texture> {
	pub fn poll_fds(&self) -> Vec<RawFd> {
		std::iter::once(self.listener_fd())
			.chain(self.client_fds())
			.collect()
	}
	pub fn monitor_infos(&self) -> Vec<MonitorInfo> {
		self.monitors.values().map(|m| m.info().clone()).collect()
	}

	pub fn register_monitor(&mut self, info: MonitorInfo) {
		let id = info.id.clone();
		let mut changed = false;
		match self.monitors.get_mut(&id) {
			Some(existing) => {
				if existing.info() != &info {
					existing.update_info(info.clone());
					changed = true;
				}
			}
			None => {
				self.monitors.insert(id.clone(), Monitor::new(info.clone()));
				changed = true;
			}
		}
		if changed {
			self.broadcast_monitor_added(info);
		}
	}

	pub fn remove_monitor(&mut self, id: &str) {
		if let Some(monitor) = self.monitors.remove(id) {
			self.broadcast_monitor_removed(monitor.info().clone());
		}
	}

	pub fn monitor_texture(&self, monitor_id: &str, session_id: &str) -> Option<&Texture> {
		self
			.monitors
			.get(monitor_id)?
			.current_buffer_for_session(session_id)
	}
	pub fn render_snapshot(&self) -> RenderSnapshot<'_, Texture> {
		let active_session_id = self.current_session_id.as_deref();
		let transition_state = self.transition_state.as_ref();
		let transition = transition_state.map(|state| RenderTransition {
			animation: state.animation(),
			progress: state.progress(),
			previous_session_id: state.old_session_id(),
			new_session_id: active_session_id,
		});
		let previous_session_id = transition_state.and_then(|state| state.old_session_id());
		let mut monitors = Vec::with_capacity(self.monitors.len());
		for (monitor_id, monitor) in &self.monitors {
			let active_texture =
				active_session_id.and_then(|session_id| monitor.current_buffer_for_session(session_id));
			let previous_texture =
				previous_session_id.and_then(|session_id| monitor.current_buffer_for_session(session_id));
			monitors.push(MonitorRenderSnapshot {
				monitor_id: monitor_id.as_str(),
				info: monitor.info(),
				active_texture,
				previous_texture,
			});
		}
		RenderSnapshot {
			active_session_id,
			transition,
			monitors,
		}
	}

	pub fn notify_frame_rendered<'a, I>(&mut self, frames: I)
	where
		I: IntoIterator<Item = (&'a str, &'a str)>,
	{
		for (monitor_id, session_id) in frames.into_iter() {
			let Some(monitor) = self.monitors.get_mut(monitor_id) else {
				warn!(monitor_id = %monitor_id, "frame_done for unknown monitor");
				continue;
			};
			if let Some(latency) = monitor.take_pending_page_flip(session_id) {
				tracing::trace!(
					monitor_id = monitor_id,
					session_id = session_id,
					ms = latency.as_secs_f32() * 1000.0,
					"frame_latency"
				);
				let frame = TabMessageFrame::raw(message_header::FRAME_DONE, monitor_id);
				self.send_to_session(&frame, session_id);
			}
		}
	}

	fn cleanup_session(&mut self, session_id: &str) {
		for monitor in self.monitors.values_mut() {
			monitor.remove_session(session_id);
		}
	}

	pub fn forward_input_event(&mut self, event: InputEventPayload) {
		let Some(session_id) = self.current_session_id.clone() else {
			return;
		};
		let frame = TabMessageFrame::json(message_header::INPUT_EVENT, event);
		self.send_to_session(&frame, &session_id);
	}

	pub fn cycle_next_session(&mut self) -> Option<String> {
		self.cycle_session(CycleDirection::Forward)
	}

	pub fn cycle_previous_session(&mut self) -> Option<String> {
		self.cycle_session(CycleDirection::Backward)
	}

	fn cycle_session(&mut self, direction: CycleDirection) -> Option<String> {
		let target = self
			.sessions
			.cycle_session(self.current_session_id.as_deref(), direction)?;
		let payload = SessionSwitchPayload {
			session_id: target.clone(),
			animation: Some(DEFAULT_SWITCH_ANIMATION.to_string()),
			duration: DEFAULT_SWITCH_DURATION,
		};
		self.dispatch_event(ServerEvent::SessionSwitch(payload));
		Some(target)
	}

	#[cfg(feature = "easydrm")]
	pub fn ensure_monitors_are_up_to_date_with_easydrm<C>(
		&mut self,
		easydrm: &mut easydrm::EasyDRM<C>,
	) where
		C: MonitorIdStorage,
	{
		let mut seen = HashSet::new();
		for monitor in easydrm.monitors_mut() {
			let ctx = monitor.context_mut();
			let id = match ctx.monitor_id() {
				Some(existing) => existing.to_string(),
				None => {
					let id = crate::generate_id("mon");
					ctx.set_monitor_id(id.clone());
					id
				}
			};
			let (width, height) = monitor.size();
			let refresh_rate = monitor.active_mode().vrefresh() as i32;
			let connector_raw: u32 = monitor.connector_id().into();
			let info = MonitorInfo {
				id: id.clone(),
				width: width as i32,
				height: height as i32,
				refresh_rate,
				name: format!("Connector {connector_raw}"),
			};
			seen.insert(id.clone());
			self.register_monitor(info);
		}
		let existing: Vec<String> = self.monitors.keys().cloned().collect();
		for id in existing {
			if !seen.contains(&id) {
				self.remove_monitor(&id);
			}
		}
	}

	/// Create and bind a Tab server socket, cleaning up any stale path.
	pub fn bind(
		path: impl AsRef<Path>,
		load_dmabuf: impl Fn(RawFd, &FramebufferLinkPayload) -> Result<Texture, TabServerError> + 'static,
	) -> Result<Self, TabServerError> {
		let path = path.as_ref();
		if path.exists() {
			fs::remove_file(path)?;
		}
		let listener = UnixListener::bind(path)?;
		listener.set_nonblocking(true)?;
		Ok(Self {
			path: path.to_path_buf(),
			listener,
			clients: Vec::new(),
			sessions: SessionRegistry::new(),
			load_dmabuf: Arc::new(load_dmabuf),
			monitors: HashMap::new(),
			current_session_id: None,
			transition_state: None,
		})
	}

	/// Convenience helper to bind to the default `/tmp/shift.sock`.
	pub fn bind_default(
		load_dmabuf: impl Fn(RawFd, &FramebufferLinkPayload) -> Result<Texture, TabServerError> + 'static,
	) -> Result<Self, TabServerError> {
		Self::bind(DEFAULT_SOCKET_PATH, load_dmabuf)
	}

	/// Drive acceptance and message processing without blocking.
	pub fn pump(&mut self) -> Result<(), TabServerError> {
		self.accept_new_clients()?;
		self.process_clients()?;
		Ok(())
	}

	/// Register a pending session/token pair waiting for a client connection.
	pub fn register_session(
		&mut self,
		session_id: impl Into<String>,
		token: impl Into<String>,
		role: SessionRole,
		display_name: Option<String>,
	) {
		self
			.sessions
			.insert_pending(session_id.into(), token.into(), role, display_name);
	}

	/// Raw file descriptor for the listening socket (for poll integration).
	pub fn listener_fd(&self) -> RawFd {
		self.listener.as_raw_fd()
	}

	/// Raw file descriptors for all connected clients (for poll integration).
	pub fn client_fds(&self) -> impl Iterator<Item = RawFd> + '_ {
		self.clients.iter().map(|c| c.connection.as_raw_fd())
	}

	/// Immutable view of connected clients.
	pub fn clients(&self) -> impl Iterator<Item = &Client<Texture>> {
		self.clients.iter()
	}

	/// Path currently bound by this server.
	pub fn path(&self) -> &Path {
		&self.path
	}

	fn dispatch_event(&mut self, event: ServerEvent<Texture>) {
		match event {
			ServerEvent::SessionState {
				session,
				exclude_client_id,
			} => self.broadcast_session_state(session, exclude_client_id),
			ServerEvent::FramebufferLinked {
				monitor_id,
				session_id,
				buffers,
			} => {
				if let Some(monitor) = self.monitors.get_mut(&monitor_id) {
					monitor.framebuffer_link(session_id, buffers);
				} else {
					warn!(monitor_id = %monitor_id, "Framebuffer link for unknown monitor");
				}
			}
			ServerEvent::SwapBuffers {
				session_id,
				payload,
			} => {
				let Some(monitor) = self.monitors.get_mut(&payload.monitor_id) else {
					warn!(monitor_id = %payload.monitor_id, "swap_buffers for unknown monitor");
					return;
				};
				if !monitor.swap_buffers(&session_id, payload.buffer) {
					warn!(
						monitor_id = %payload.monitor_id,
						session_id = %session_id,
						buffer = ?payload.buffer,
						"swap_buffers for unknown session"
					);
				} else if self.current_session_id.is_none() {
					self.activate_session(
						session_id,
						Some(DEFAULT_SWITCH_ANIMATION.to_string()),
						Some(DEFAULT_SWITCH_DURATION),
					);
				}
			}
			ServerEvent::SessionSwitch(payload) => {
				let SessionSwitchPayload {
					session_id,
					animation,
					duration,
				} = payload;
				self.activate_session(session_id, animation, Some(duration));
			}
		}
	}

	fn broadcast_monitor_added(&self, info: MonitorInfo) {
		let frame = TabMessageFrame::json(
			message_header::MONITOR_ADDED,
			MonitorAddedPayload { monitor: info },
		);
		self.broadcast_to_sessions(&frame);
	}

	fn broadcast_monitor_removed(&self, info: MonitorInfo) {
		let frame = TabMessageFrame::json(
			message_header::MONITOR_REMOVED,
			MonitorRemovedPayload {
				monitor_id: info.id,
				name: info.name,
			},
		);
		self.broadcast_to_sessions(&frame);
	}

	fn send_to_session(&self, frame: &TabMessageFrame, session_id: &str) {
		for client in self.clients.iter() {
			if !client.session.authenticated
				|| client
					.session
					.session_id
					.as_ref()
					.is_none_or(|s| s != session_id)
			{
				continue;
			}
			if let Err(err) = client.connection.send_frame(frame) {
				warn!(client_id = %client.id, %err, "Failed to send event to session {session_id}");
			}
			return;
		}
	}

	fn broadcast_to_sessions(&self, frame: &TabMessageFrame) {
		for client in self.clients.iter() {
			if !client.session.authenticated {
				continue;
			}
			if let Err(err) = client.connection.send_frame(frame) {
				warn!(client_id = %client.id, %err, "Failed to broadcast event");
			}
		}
	}

	fn broadcast_session_state(&mut self, session: SessionInfo, exclude_client_id: Option<String>) {
		let frame = TabMessageFrame::json(
			message_header::SESSION_STATE,
			SessionStatePayload {
				session: session.clone(),
			},
		);
		for client in self.clients.iter_mut() {
			if client.session.role == Some(SessionRole::Admin) {
				if exclude_client_id
					.as_ref()
					.is_some_and(|skip| skip == &client.id)
				{
					continue;
				}
				if let Err(err) = client.connection.send_frame(&frame) {
					warn!(client_id = %client.id, %err, "Failed to send session_state");
				}
			}
		}
	}

	fn accept_new_clients(&mut self) -> Result<(), TabServerError> {
		loop {
			match self.listener.accept() {
				Ok((stream, _)) => {
					let mut connection = TabConnection::new(stream)?;
					connection.send_hello("Shift dev")?;
					let client = Client::new(connection, Arc::clone(&self.load_dmabuf));
					self.clients.push(client);
				}
				Err(e) if e.kind() == ErrorKind::WouldBlock => break,
				Err(e) => return Err(TabServerError::Io(e)),
			}
		}
		Ok(())
	}

	fn process_clients(&mut self) -> Result<(), TabServerError> {
		let mut idx = 0;
		while idx < self.clients.len() {
			let mut remove = false;
			match self.clients[idx].connection.read_message_nonblocking() {
				Ok(Some(msg)) => {
					let monitors = self.monitor_infos();
					let events = self.clients[idx].handle_message(msg, &mut self.sessions, &monitors);
					for event in events {
						self.dispatch_event(event);
					}
				}
				Ok(None) => {}
				Err(ProtocolError::WouldBlock) => {}
				Err(err) => {
					warn!(
						client_id = %self.clients[idx].id,
						%err,
						"Dropping client due to protocol error"
					);
					remove = true;
				}
			}

			if remove {
				if let Some(session_id) = self.clients[idx].session.session_id.clone() {
					self.cleanup_session(&session_id);
					if let Some(info) = self.sessions.mark_consumed(&session_id) {
						self.dispatch_event(ServerEvent::SessionState {
							session: info,
							exclude_client_id: None,
						});
					}
				}
				self.clients.swap_remove(idx);
			} else {
				idx += 1;
			}
		}
		Ok(())
	}

	fn broadcast_active_session(&self, session_id: &str) {
		let frame = TabMessageFrame::json(
			message_header::SESSION_ACTIVE,
			SessionActivePayload {
				session_id: session_id.to_string(),
			},
		);
		self.broadcast_to_sessions(&frame);
	}

	fn activate_session(
		&mut self,
		session_id: String,
		animation: Option<String>,
		duration: Option<Duration>,
	) {
		let already_active = self.current_session_id.as_deref() == Some(session_id.as_str());
		if already_active {
			return;
		}
		let previous = std::mem::replace(&mut self.current_session_id, Some(session_id.clone()));
		if let (Some(anim), Some(dur)) = (animation, duration) {
			if previous.is_some() {
				self.transition_state = Some(SessionTransitionState::new(anim, dur, previous));
			} else {
				self.transition_state = None;
			}
		} else {
			self.transition_state = None;
		}
		self.broadcast_active_session(&session_id);
	}
}
