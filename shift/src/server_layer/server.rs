use std::{
	collections::{HashMap, HashSet},
	fs::Permissions,
	future::pending,
	io,
	os::unix::fs::PermissionsExt,
	path::{Path, PathBuf},
	process::Command,
	sync::Arc,
	time::Duration,
};

use futures::future::select_all;
use tab_protocol::TabMessageFrame;
use thiserror::Error;
use tokio::{
	io::unix::AsyncFd,
	net::{UnixListener, UnixStream, unix::SocketAddr},
	task::JoinHandle as TokioJoinHandle,
	time::Instant,
};
use tracing::error;

use crate::auth::error::Error as AuthError;
use crate::{
	auth::Token,
	client_layer::{
		client::{Client, ClientId},
		client_view::{self, ClientView},
	},
	comms::{
		client2server::C2SMsg,
		render2server::{RenderEvt, RenderEvtRx},
		server2client::BufferRelease,
		server2render::{RenderCmd, RenderCmdTx, SessionTransition},
	},
	monitor::{Monitor, MonitorId},
	rendering_layer::channels::ServerEnd as RenderServerChannels,
	sessions::{PendingSession, Role, Session, SessionId},
};
use tab_protocol::{SessionInfo, SessionLifecycle, SessionRole};

#[derive(Debug, Clone, Copy)]
struct PendingFlip {
	session_id: SessionId,
	monitor_id: MonitorId,
	buffer: tab_protocol::BufferIndex,
}

#[derive(Debug, Clone, Copy)]
struct PendingBufferRequest {
	client_id: ClientId,
	session_id: SessionId,
	monitor_id: MonitorId,
	buffer: tab_protocol::BufferIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferOwner {
	Client,
	Shift,
}
struct ConnectedClient {
	client_view: ClientView,
	join_handle: TokioJoinHandle<()>,
}
impl Drop for ConnectedClient {
	fn drop(&mut self) {
		self.join_handle.abort();
	}
}
pub struct ShiftServer {
	listener: Option<UnixListener>,
	current_session: Option<SessionId>,
	pending_sessions: HashMap<Token, PendingSession>,
	active_sessions: HashMap<SessionId, Arc<Session>>,
	loading_sessions: HashSet<SessionId>,
	awake_sessions: HashSet<SessionId>,
	awake_until: HashMap<SessionId, Instant>,
	connected_clients: HashMap<ClientId, ConnectedClient>,
	render_commands: RenderCmdTx,
	render_events: RenderEvtRx,
	monitors: HashMap<MonitorId, Monitor>,
	pending_buffer_requests: Vec<PendingBufferRequest>,
	waiting_flip: Vec<PendingFlip>,
	front_buffers: HashMap<(SessionId, MonitorId), tab_protocol::BufferIndex>,
	buffer_ownership: HashMap<(SessionId, MonitorId, tab_protocol::BufferIndex), BufferOwner>,
	swap_buffers_received: u64,
	frame_done_emitted: u64,
	debug_second_session_cmd: Option<String>,
	debug_second_session_spawned: bool,
	debug_admin_session_id: Option<SessionId>,
	debug_second_session_id: Option<SessionId>,
	debug_auto_switch_interval: Option<Duration>,
}
#[derive(Error, Debug)]
pub enum BindError {
	#[error("io error: {0}")]
	IOError(#[from] std::io::Error),
}
impl ShiftServer {
	#[tracing::instrument(level= "info", skip(path), fields(path = ?path.as_ref().display()))]
	pub async fn bind(
		path: impl AsRef<Path>,
		render_channels: RenderServerChannels,
	) -> Result<Self, BindError> {
		std::fs::remove_file(&path).ok();
		let listener = UnixListener::bind(&path)?;
		std::fs::set_permissions(&path, Permissions::from_mode(0o7777)).ok();
		let (render_events, render_commands) = render_channels.into_parts();
		let debug_second_session_cmd = std::env::var("SHIFT_DEBUG_SECOND_SESSION_CMD")
			.ok()
			.map(|v| v.trim().to_string())
			.filter(|v| !v.is_empty());
		let debug_auto_switch_interval = std::env::var("SHIFT_DEBUG_AUTO_SWITCH_INTERVAL_MS")
			.ok()
			.and_then(|raw| match raw.parse::<u64>() {
				Ok(ms) if ms > 0 => Some(Duration::from_millis(ms)),
				Ok(_) => None,
				Err(e) => {
					tracing::warn!(
						value = %raw,
						"invalid SHIFT_DEBUG_AUTO_SWITCH_INTERVAL_MS: {e}"
					);
					None
				}
			});
		Ok(Self {
			listener: Some(listener),
			current_session: Default::default(),
			pending_sessions: Default::default(),
			active_sessions: Default::default(),
			loading_sessions: Default::default(),
			awake_sessions: Default::default(),
			awake_until: Default::default(),
			connected_clients: Default::default(),
			render_commands,
			render_events,
			monitors: Default::default(),
			pending_buffer_requests: Default::default(),
			waiting_flip: Default::default(),
			front_buffers: Default::default(),
			buffer_ownership: Default::default(),
			swap_buffers_received: 0,
			frame_done_emitted: 0,
			debug_second_session_cmd,
			debug_second_session_spawned: false,
			debug_admin_session_id: None,
			debug_second_session_id: None,
			debug_auto_switch_interval,
		})
	}

	fn maybe_spawn_debug_second_session(&mut self, admin_session_id: SessionId) {
		let Some(cmdline) = self.debug_second_session_cmd.clone() else {
			return;
		};
		if self.debug_second_session_spawned {
			return;
		}
		self.debug_second_session_spawned = true;
		self.debug_admin_session_id.get_or_insert(admin_session_id);
		let (token, pending_session) = PendingSession::normal(Some("Debug Session 2".into()));
		let session_id = pending_session.id();
		self.pending_sessions.insert(token.clone(), pending_session);
		let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
		let mut cmd = Command::new(shell);
		cmd.args(["-c", &cmdline]);
		cmd.env("SHIFT_SESSION_TOKEN", token.to_string());
		match cmd.spawn() {
			Ok(child) => {
				self.debug_second_session_id = Some(session_id);
				tracing::info!(
					%session_id,
					pid = child.id(),
					"spawned SHIFT_DEBUG_SECOND_SESSION_CMD"
				);
			}
			Err(e) => {
				self.debug_second_session_spawned = false;
				self.debug_second_session_id = None;
				self.pending_sessions.remove(&token);
				tracing::error!("failed to spawn SHIFT_DEBUG_SECOND_SESSION_CMD: {e}");
			}
		}
	}

	async fn handle_debug_auto_switch_tick(&mut self) {
		let Some(admin_session_id) = self.debug_admin_session_id else {
			return;
		};
		let Some(second_session_id) = self.debug_second_session_id else {
			return;
		};
		if !self.active_sessions.contains_key(&admin_session_id)
			|| !self.active_sessions.contains_key(&second_session_id)
		{
			return;
		}
		let target = match self.current_session {
			Some(id) if id == admin_session_id => second_session_id,
			Some(id) if id == second_session_id => admin_session_id,
			_ => second_session_id,
		};
		if self.current_session == Some(target) {
			return;
		}
		let previous = self.current_session;
		tracing::info!(%target, "debug auto-switch session");
		let transition = previous.and_then(|from_session_id| {
			if from_session_id == target {
				return None;
			}
			Some(SessionTransition {
				from_session_id,
				animation: "blur".to_string(),
				duration: Duration::from_millis(500),
			})
		});
		if let Some(from_session_id) = previous
			&& from_session_id != target
		{
			self
				.keep_session_awake_for(from_session_id, Duration::from_millis(500))
				.await;
		}
		self.update_active_session(Some(target), transition).await;
	}

	async fn notify_session_awake_change(&mut self, session_id: SessionId, awake: bool) {
		let target_clients = self
			.connected_clients
			.iter()
			.filter_map(|(id, client)| {
				(client.client_view.authenticated_session() == Some(session_id)).then_some(*id)
			})
			.collect::<Vec<_>>();
		for id in target_clients {
			let Some(client) = self.connected_clients.get_mut(&id) else {
				continue;
			};
			let notified = if awake {
				client.client_view.notify_session_awake(session_id).await
			} else {
				client.client_view.notify_session_sleep(session_id).await
			};
			if !notified {
				tracing::warn!(%id, %session_id, awake, "failed to notify session awake state");
			}
		}
	}

	async fn prune_expired_awake_sessions(&mut self) {
		let now = Instant::now();
		let mut expired = Vec::new();
		for (session_id, deadline) in &self.awake_until {
			if *deadline <= now {
				expired.push(*session_id);
			}
		}
		for session_id in expired {
			self.awake_until.remove(&session_id);
			if self.current_session != Some(session_id) {
				if self.awake_sessions.remove(&session_id) {
					self.notify_session_awake_change(session_id, false).await;
				}
			}
		}
	}

	async fn set_awake_sessions(&mut self, sessions: impl IntoIterator<Item = SessionId>) {
		let now = Instant::now();
		let old_awake = self.awake_sessions.clone();
		self.awake_sessions.clear();
		for session_id in sessions {
			self.awake_sessions.insert(session_id);
		}
		for session_id in &self.loading_sessions {
			self.awake_sessions.insert(*session_id);
		}
		for (session_id, deadline) in &self.awake_until {
			if *deadline > now {
				self.awake_sessions.insert(*session_id);
			}
		}
		self
			.awake_until
			.retain(|session_id, _| self.awake_sessions.contains(session_id));
		let went_to_sleep = old_awake
			.difference(&self.awake_sessions)
			.copied()
			.collect::<Vec<_>>();
		let woke_up = self
			.awake_sessions
			.difference(&old_awake)
			.copied()
			.collect::<Vec<_>>();
		for session_id in went_to_sleep {
			self.notify_session_awake_change(session_id, false).await;
		}
		for session_id in woke_up {
			self.notify_session_awake_change(session_id, true).await;
		}
	}

	async fn keep_session_awake_for(&mut self, session_id: SessionId, duration: Duration) {
		if duration.is_zero() {
			return;
		}
		let was_awake = self.awake_sessions.insert(session_id);
		self
			.awake_until
			.insert(session_id, Instant::now() + duration);
		if !was_awake {
			self.notify_session_awake_change(session_id, true).await;
		}
	}

	async fn is_session_awake(&mut self, session_id: SessionId) -> bool {
		self.prune_expired_awake_sessions().await;
		self.awake_sessions.contains(&session_id)
	}

	fn session_info_from(session: &Session) -> SessionInfo {
		SessionInfo {
			id: session.id().to_string(),
			role: match session.role() {
				Role::Admin => SessionRole::Admin,
				Role::Normal => SessionRole::Session,
			},
			display_name: Some(session.display_name().to_string()),
			state: if session.ready() {
				SessionLifecycle::Occupied
			} else {
				SessionLifecycle::Loading
			},
		}
	}

	async fn notify_admins_session_state(&mut self, session: &Session) {
		let info = Self::session_info_from(session);
		let admin_client_ids = self
			.connected_clients
			.iter()
			.filter_map(|(id, client)| {
				let session_id = client.client_view.authenticated_session()?;
				let session = self.active_sessions.get(&session_id)?;
				(session.role() == Role::Admin).then_some(*id)
			})
			.collect::<Vec<_>>();
		for id in admin_client_ids {
			let Some(client) = self.connected_clients.get_mut(&id) else {
				continue;
			};
			if !client.client_view.notify_session_state(info.clone()).await {
				tracing::warn!(%id, session_id = %session.id(), "failed to notify session state");
			}
		}
	}

	#[tracing::instrument(level= "info", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	pub fn add_initial_session(&mut self) -> Token {
		let (token, session) = PendingSession::admin(Some("Admin".into()));
		let id = session.id();
		self.pending_sessions.insert(token.clone(), session);

		let admin_launch_cmd = std::env::var("ADMIN_LAUNCH_CMD").ok();
		let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
		if let Some(admin_launch_cmd) = admin_launch_cmd {
			let mut cmd = Command::new(shell);
			cmd.args(["-c", &admin_launch_cmd]);
			cmd.env("SHIFT_SESSION_TOKEN", token.to_string());
			if let Err(e) = cmd.spawn() {
				panic!("Failed to start admin session process: {e}");
			}
		}
		tracing::info!(?token, %id, "added initial admin session");
		token
	}
	pub async fn start(mut self) {
		let listener = self.listener.take().unwrap();
		let mut stats_tick = tokio::time::interval(std::time::Duration::from_secs(1));
		let mut debug_auto_switch_tick = self.debug_auto_switch_interval.map(tokio::time::interval);
		loop {
			let span = tracing::trace_span!(
				"server_loop",
				connected_clients = self.connected_clients.len(),
				active_sessions = self.active_sessions.len(),
				pending_sessions = self.pending_sessions.len(),
				current_session = ?self.current_session,
				waiting_flip = self.waiting_flip.len(),
			);
			let _span = span.enter();
			tokio::select! {
					client_message = Self::read_clients_messages(&mut self.connected_clients) => self.handle_client_message(client_message.0, client_message.1).await,
					accept_result = listener.accept() => self.handle_accept(accept_result).await,
						_ = stats_tick.tick() => {
								self.prune_expired_awake_sessions().await;
								if self.swap_buffers_received > 0 || self.frame_done_emitted > 0 {
									tracing::trace!(
											swap_buffers_received = self.swap_buffers_received,
											frame_done_emitted = self.frame_done_emitted,
											"server stats per second"
									);
							}
							self.swap_buffers_received = 0;
							self.frame_done_emitted = 0;
					}
					render_event = self.render_events.recv() => {
							if let Some(event) = render_event {
									self.handle_render_event(event).await;
							} else {
									tracing::warn!("render layer event channel closed");
									return;
							}
					}
					_ = async {
						if let Some(tick) = &mut debug_auto_switch_tick {
							tick.tick().await;
						} else {
							pending::<()>().await;
						}
					} => {
						self.handle_debug_auto_switch_tick().await;
					}
			}
		}
	}

	#[tracing::instrument(level= "trace", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	async fn handle_client_message(&mut self, client_id: ClientId, message: C2SMsg) {
		match message {
			C2SMsg::Shutdown => {
				self.disconnect_client(client_id).await;
			}
			C2SMsg::Auth(token) => {
				let Some(pending_session) = self.pending_sessions.remove(&token) else {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_auth_error(AuthError::NotFound)
							.await;
					}
					return;
				};
				let session = Arc::new(pending_session.promote());
				let notify_succeeded = {
					let Some(connected_client) = self.connected_clients.get_mut(&client_id) else {
						tracing::warn!("tried handling message from a non-existing client");
						return;
					};
					connected_client
						.client_view
						.notify_auth_success(&session)
						.await
				};
				if !notify_succeeded {
					self.disconnect_client(client_id).await;
					tracing::warn!("failed to notify auth success, removing client");
					return;
				}
				self
					.active_sessions
					.insert(session.id(), Arc::clone(&session));
				if session.role() == Role::Normal && !session.ready() {
					self.loading_sessions.insert(session.id());
					self
						.set_awake_sessions(self.current_session.into_iter())
						.await;
				}
				if session.role() == Role::Admin {
					self.debug_admin_session_id.get_or_insert(session.id());
					self.maybe_spawn_debug_second_session(session.id());
				}
				if session.role() == Role::Admin && self.current_session.is_none() {
					self.update_active_session(Some(session.id()), None).await;
				} else if self.awake_sessions.contains(&session.id()) {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client.client_view.notify_session_awake(session.id()).await;
					}
				} else if let Some(client) = self.connected_clients.get_mut(&client_id) {
					client.client_view.notify_session_sleep(session.id()).await;
				}
				if let Some(active_session_id) = self.current_session {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_session_active(active_session_id)
							.await;
					}
				}
				if session.role() == Role::Admin {
					let session_infos = self
						.active_sessions
						.values()
						.filter(|s| s.role() == Role::Normal)
						.map(|s| Self::session_info_from(s))
						.collect::<Vec<_>>();
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						for info in session_infos {
							client.client_view.notify_session_state(info).await;
						}
					}
				}
				if session.role() == Role::Normal {
					self.notify_admins_session_state(&session).await;
				}
			}
			C2SMsg::CreateSession(req) => {
				let mut remove_client = false;
				{
					let Some(connected_client) = self.connected_clients.get_mut(&client_id) else {
						tracing::warn!("tried handling message from a non-existing client");
						return;
					};
					let client_session = connected_client
						.client_view
						.authenticated_session()
						.and_then(|s| self.active_sessions.get(&s))
						.map(Arc::clone);
					let Some(client_session) = client_session else {
						connected_client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
						return;
					};
					if client_session.role() != Role::Admin {
						connected_client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
						return;
					}
					let (token, pending_session) = PendingSession::new(
						req.display_name.map(Arc::from),
						match req.role {
							tab_protocol::SessionRole::Admin => Role::Admin,
							tab_protocol::SessionRole::Session => Role::Normal,
						},
					);
					self
						.pending_sessions
						.insert(token.clone(), pending_session.clone());
					if !connected_client
						.client_view
						.notify_session_created(token, pending_session)
						.await
					{
						tracing::warn!("failed to notify session created, removing client");
						remove_client = true;
					}
				}
				if remove_client {
					self.disconnect_client(client_id).await;
				}
			}
			C2SMsg::SwitchSession(payload) => {
				let target_session = match payload.session_id.parse::<SessionId>() {
					Ok(session_id) => session_id,
					Err(e) => {
						if let Some(client) = self.connected_clients.get_mut(&client_id) {
							client
								.client_view
								.notify_error(
									"invalid_session_id".into(),
									Some(Arc::<str>::from(e.to_string())),
									false,
								)
								.await;
						}
						return;
					}
				};
				let Some(connected_client) = self.connected_clients.get(&client_id) else {
					tracing::warn!("tried handling message from a non-existing client");
					return;
				};
				let requester_session = connected_client
					.client_view
					.authenticated_session()
					.and_then(|s| self.active_sessions.get(&s))
					.map(Arc::clone);
				let Some(requester_session) = requester_session else {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
					}
					return;
				};
				if requester_session.role() != Role::Admin {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
					}
					return;
				}
				if !self.active_sessions.contains_key(&target_session) {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"unknown_session".into(),
								Some(Arc::<str>::from("target session is not active")),
								false,
							)
							.await;
					}
					return;
				}
				if let Some(target) = self.active_sessions.get(&target_session)
					&& target.role() != Role::Admin
					&& !target.ready()
				{
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"session_loading".into(),
								Some(Arc::<str>::from(
									"target session is still loading and cannot become active",
								)),
								false,
							)
							.await;
					}
					return;
				}
				let previous = self.current_session;
				let transition = match (previous, payload.animation.clone()) {
					(Some(from_session_id), Some(animation))
						if from_session_id != target_session && payload.duration > Duration::ZERO =>
					{
						self
							.keep_session_awake_for(from_session_id, payload.duration)
							.await;
						Some(SessionTransition {
							from_session_id,
							animation,
							duration: payload.duration,
						})
					}
					_ => None,
				};
				self
					.update_active_session(Some(target_session), transition)
					.await;
			}
			C2SMsg::SessionReady(payload) => {
				let Some(connected_client) = self.connected_clients.get(&client_id) else {
					tracing::warn!("tried handling message from a non-existing client");
					return;
				};
				let requester_session_id = connected_client.client_view.authenticated_session();
				let Some(requester_session_id) = requester_session_id else {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
					}
					return;
				};
				if payload.session_id != requester_session_id.to_string() {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"invalid_session_id".into(),
								Some(Arc::<str>::from(
									"session_ready session_id does not match authenticated session",
								)),
								false,
							)
							.await;
					}
					return;
				}
				let Some(existing) = self.active_sessions.get(&requester_session_id).cloned() else {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
					}
					return;
				};
				if existing.role() == Role::Admin {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"invalid_transition".into(),
								Some(Arc::<str>::from(
									"admin session does not use loading/ready lifecycle",
								)),
								false,
							)
							.await;
					}
					return;
				}
				if existing.ready() {
					return;
				}

				let ready_session = Arc::new(existing.with_ready(true));
				self
					.active_sessions
					.insert(requester_session_id, Arc::clone(&ready_session));
				self.loading_sessions.remove(&requester_session_id);
				self.notify_admins_session_state(&ready_session).await;
				self
					.set_awake_sessions(self.current_session.into_iter())
					.await;
			}
			C2SMsg::BufferRequest {
				monitor_id,
				buffer,
				acquire_fence,
			} => {
				let Some(connected_client) = self.connected_clients.get(&client_id) else {
					tracing::warn!("tried handling message from a non-existing client");
					return;
				};
				let client_session = connected_client
					.client_view
					.authenticated_session()
					.and_then(|s| self.active_sessions.get(&s))
					.map(Arc::clone);
				let Some(client_session) = client_session else {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
					}
					return;
				};
				if !self.is_session_awake(client_session.id()).await {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"session_sleeping".into(),
								Some("session is not awake".into()),
								false,
							)
							.await;
					}
					return;
				}
				let owner_key = (client_session.id(), monitor_id, buffer);
				let current_owner = self
					.buffer_ownership
					.get(&owner_key)
					.copied()
					.unwrap_or(BufferOwner::Client);
				if current_owner != BufferOwner::Client {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"ownership_violation".into(),
								Some("requested buffer is not client-owned".into()),
								false,
							)
							.await;
					}
					return;
				}
				if self.pending_buffer_requests.iter().any(|pending| {
					pending.session_id == client_session.id() && pending.monitor_id == monitor_id
				}) {
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client
							.client_view
							.notify_error(
								"buffer_request_inflight".into(),
								Some("monitor already has an in-flight buffer request".into()),
								false,
							)
							.await;
					}
					return;
				}
				if let Err(e) = self
					.render_commands
					.send(RenderCmd::SwapBuffers {
						monitor_id,
						buffer,
						session_id: client_session.id(),
						acquire_fence,
					})
					.await
				{
					tracing::error!("failed to forward SwapBuffers to renderer: {e}");
					let code = Arc::<str>::from("render_unavailable");
					let detail = Some(Arc::<str>::from("renderer unavailable"));
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client.client_view.notify_error(code, detail, true).await;
					}
				} else {
					self.pending_buffer_requests.push(PendingBufferRequest {
						client_id,
						session_id: client_session.id(),
						monitor_id,
						buffer,
					});
				}
			}
			C2SMsg::FramebufferLink { payload, dma_bufs } => {
				let monitor_id_raw = payload.monitor_id.clone();
				let session_id = {
					let Some(client) = self.connected_clients.get_mut(&client_id) else {
						tracing::warn!("tried handling message from a non-existing client");
						return;
					};
					let Some(session_id) = client.client_view.authenticated_session() else {
						client
							.client_view
							.notify_error("forbidden".into(), None, false)
							.await;
						return;
					};
					session_id
				};
				if let Err(e) = self
					.render_commands
					.send(RenderCmd::FramebufferLink {
						payload,
						dma_bufs,
						session_id,
					})
					.await
				{
					tracing::error!("failed to forward FramebufferLink to renderer: {e}");
					let code = Arc::<str>::from("render_unavailable");
					let detail = Some(Arc::<str>::from("renderer unavailable"));
					if let Some(client) = self.connected_clients.get_mut(&client_id) {
						client.client_view.notify_error(code, detail, true).await;
					}
				} else {
					let Ok(monitor_id) = monitor_id_raw.parse::<MonitorId>() else {
						return;
					};
					self.waiting_flip.retain(|pending| {
						!(pending.session_id == session_id && pending.monitor_id == monitor_id)
					});
					self.pending_buffer_requests.retain(|pending| {
						!(pending.session_id == session_id && pending.monitor_id == monitor_id)
					});
					self.front_buffers.remove(&(session_id, monitor_id));
					self.buffer_ownership.insert(
						(session_id, monitor_id, tab_protocol::BufferIndex::Zero),
						BufferOwner::Client,
					);
					self.buffer_ownership.insert(
						(session_id, monitor_id, tab_protocol::BufferIndex::One),
						BufferOwner::Client,
					);
				}
			}
		}
	}
	async fn handle_render_event(&mut self, event: RenderEvt) {
		match event {
			RenderEvt::Started { monitors } => {
				self.monitors = monitors.into_iter().map(|m| (m.id, m)).collect();
			}
			RenderEvt::MonitorOnline { monitor } => {
				tracing::info!(?monitor, "renderer reports monitor online");
				self.broadcast_monitor_added(&monitor).await;
				self.monitors.insert(monitor.id, monitor);
			}
			RenderEvt::MonitorOffline { monitor_id } => {
				tracing::info!(%monitor_id, "renderer reports monitor offline");
				if let Some(monitor) = self.monitors.remove(&monitor_id) {
					self.broadcast_monitor_removed(&monitor).await;
				}
				self
					.waiting_flip
					.retain(|pending| pending.monitor_id != monitor_id);
				self
					.pending_buffer_requests
					.retain(|pending| pending.monitor_id != monitor_id);
				self.front_buffers.retain(|(_, mon), _| *mon != monitor_id);
				self
					.buffer_ownership
					.retain(|(_, mon, _), _| *mon != monitor_id);
			}
			RenderEvt::BufferRequestAck {
				session_id,
				monitor_id,
				buffer,
			} => {
				let Some(pos) = self.pending_buffer_requests.iter().position(|pending| {
					pending.session_id == session_id
						&& pending.monitor_id == monitor_id
						&& pending.buffer == buffer
				}) else {
					tracing::warn!(%session_id, %monitor_id, buffer = buffer as u8, "renderer acked unknown pending request");
					return;
				};
				let pending = self.pending_buffer_requests.remove(pos);
				self
					.buffer_ownership
					.insert((session_id, monitor_id, buffer), BufferOwner::Shift);
				self.swap_buffers_received = self.swap_buffers_received.saturating_add(1);

				let mut should_disconnect = false;
				if let Some(client) = self.connected_clients.get_mut(&pending.client_id) {
					if !client
						.client_view
						.notify_buffer_request_ack(monitor_id, buffer)
						.await
					{
						should_disconnect = true;
					}
				}
				if should_disconnect {
					self.disconnect_client(pending.client_id).await;
				}
			}
			RenderEvt::BufferRequestRejected {
				session_id,
				monitor_id,
				buffer,
				reason,
			} => {
				let Some(pos) = self.pending_buffer_requests.iter().position(|pending| {
					pending.session_id == session_id
						&& pending.monitor_id == monitor_id
						&& pending.buffer == buffer
				}) else {
					tracing::warn!(%session_id, %monitor_id, buffer = buffer as u8, %reason, "renderer rejected unknown pending request");
					return;
				};
				let pending = self.pending_buffer_requests.remove(pos);
				if let Some(client) = self.connected_clients.get_mut(&pending.client_id) {
					client
						.client_view
						.notify_error("buffer_request_rejected".into(), Some(reason), false)
						.await;
				}
			}
			RenderEvt::BufferConsumed {
				session_id,
				monitor_id,
				buffer,
				release_fence,
			} => {
				self
					.buffer_ownership
					.insert((session_id, monitor_id, buffer), BufferOwner::Client);
				let Some((_id, client)) = self
					.connected_clients
					.iter_mut()
					.find(|(_, c)| c.client_view.authenticated_session() == Some(session_id))
				else {
					return;
				};
				if !client
					.client_view
					.notify_buffer_release(vec![BufferRelease {
						monitor_id,
						buffer,
						release_fence,
					}])
					.await
				{
					tracing::warn!(%session_id, %monitor_id, buffer = buffer as u8, "failed to send early buffer_release");
				} else {
					self.frame_done_emitted = self.frame_done_emitted.saturating_add(1);
				}
			}
			RenderEvt::FatalError { reason } => {
				tracing::error!(?reason, "renderer fatal error");
				// TODO: Shutdown server
			}
			RenderEvt::PageFlip { monitors } => {
				let _ = monitors;
			}
		}
	}
	async fn read_clients_messages(
		connected_clients: &mut HashMap<ClientId, ConnectedClient>,
	) -> (ClientId, C2SMsg) {
		connected_clients.retain(|_, c| c.client_view.has_messages());
		let futures = connected_clients
			.iter_mut()
			.map(|c| {
				Box::pin(async {
					let Some(msg) = c.1.client_view.read_message().await else {
						return pending().await;
					};
					(*c.0, msg)
				})
			})
			.collect::<Vec<_>>();
		if futures.is_empty() {
			return pending().await;
		}
		select_all(futures).await.0
	}
	#[tracing::instrument(level= "info", skip(self, accept_result), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	async fn handle_accept(&mut self, accept_result: io::Result<(UnixStream, SocketAddr)>) {
		match accept_result {
			Ok((client_socket, _ip)) => {
				macro_rules! or_continue {
                    ($expr:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {
                        match $expr {
                            Ok(val) => val,
                            Err(e) => {
                                tracing::error!($fmt $(, $arg)*, e);
                                return;
                            }
                        }
                    };
                }

				let hellopkt = TabMessageFrame::hello("shift 0.1.0-alpha");
				let client_async_fd = or_continue!(
					client_socket.into_std().and_then(AsyncFd::new),
					"failed to accept connection: AsyncFd creation from client_socket failed: {}"
				);

				or_continue!(
					hellopkt.send_frame_to_async_fd(&client_async_fd).await,
					"failed to send hello packet: {}"
				);
				let (new_client, mut new_client_view) =
					Client::wrap_socket(client_async_fd, self.monitors.values().cloned().collect());
				let client_id = new_client_view.id();

				self.connected_clients.insert(
					new_client_view.id(),
					ConnectedClient {
						client_view: new_client_view,
						join_handle: new_client.spawn().await,
					},
				);
				tracing::info!(%client_id, "client successfully connected");
			}
			Err(e) => {
				tracing::error!("failed to accept connection: {e}");
			}
		}
	}

	async fn broadcast_monitor_added(&mut self, monitor: &crate::monitor::Monitor) {
		for (id, client) in self.connected_clients.iter_mut() {
			if !client
				.client_view
				.notify_monitor_added(monitor.clone())
				.await
			{
				tracing::warn!(%id, "failed to notify monitor added");
			}
		}
	}

	async fn broadcast_monitor_removed(&mut self, monitor: &crate::monitor::Monitor) {
		let name: Arc<str> = monitor.name.clone().into();
		for (id, client) in self.connected_clients.iter_mut() {
			if !client
				.client_view
				.notify_monitor_removed(monitor.id, Arc::clone(&name))
				.await
			{
				tracing::warn!(%id, "failed to notify monitor removed");
			}
		}
	}

	async fn disconnect_client(&mut self, client_id: ClientId) {
		let Some(client) = self.connected_clients.remove(&client_id) else {
			return;
		};
		if let Some(session_id) = client.client_view.authenticated_session() {
			self.active_sessions.remove(&session_id);
			self.loading_sessions.remove(&session_id);
			self.awake_sessions.remove(&session_id);
			self.awake_until.remove(&session_id);
			self
				.pending_buffer_requests
				.retain(|pending| pending.client_id != client_id && pending.session_id != session_id);
			self
				.waiting_flip
				.retain(|pending| pending.session_id != session_id);
			self
				.front_buffers
				.retain(|(sess, _), _| *sess != session_id);
			self
				.buffer_ownership
				.retain(|(sess, _, _), _| *sess != session_id);
			if let Err(e) = self
				.render_commands
				.send(RenderCmd::SessionRemoved { session_id })
				.await
			{
				tracing::error!("failed to notify renderer about session removal: {e}");
			}
			if self.current_session == Some(session_id) {
				self.update_active_session(None, None).await;
			}
		}
	}

	async fn update_active_session(
		&mut self,
		next: Option<SessionId>,
		transition: Option<SessionTransition>,
	) {
		self.current_session = next;
		self.prune_expired_awake_sessions().await;
		self.set_awake_sessions(next.into_iter()).await;
		if let Some(active_session_id) = next {
			let target_clients = self
				.connected_clients
				.iter()
				.filter_map(|(id, client)| client.client_view.authenticated_session().map(|_| *id))
				.collect::<Vec<_>>();
			for id in target_clients {
				if let Some(client) = self.connected_clients.get_mut(&id) {
					client
						.client_view
						.notify_session_active(active_session_id)
						.await;
				}
			}
		}
		if let Err(e) = self
			.render_commands
			.send(RenderCmd::SetActiveSession {
				session_id: next,
				transition,
			})
			.await
		{
			tracing::error!("failed to notify renderer about active session change: {e}");
		}
	}
}
