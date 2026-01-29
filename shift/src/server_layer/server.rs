use std::{collections::HashMap, future::pending, io, path::Path, sync::Arc};

use futures::future::select_all;
use tab_protocol::TabMessageFrame;
use thiserror::Error;
use tokio::{
	io::unix::AsyncFd,
	net::{UnixListener, UnixStream, unix::SocketAddr},
	task::JoinHandle as TokioJoinHandle,
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
		server2render::{RenderCmd, RenderCmdTx},
	},
	monitor::{Monitor, MonitorId},
	rendering_layer::channels::ServerEnd as RenderServerChannels,
	sessions::{PendingSession, Role, Session, SessionId},
};
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
	connected_clients: HashMap<ClientId, ConnectedClient>,
	render_commands: RenderCmdTx,
	render_events: RenderEvtRx,
	monitors: HashMap<MonitorId, Monitor>,
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
		let listener = UnixListener::bind(path)?;
		let (render_events, render_commands) = render_channels.into_parts();
		Ok(Self {
			listener: Some(listener),
			current_session: Default::default(),
			pending_sessions: Default::default(),
			active_sessions: Default::default(),
			connected_clients: Default::default(),
			render_commands,
			render_events,
			monitors: Default::default(),
		})
	}
	#[tracing::instrument(level= "info", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	pub fn add_initial_session(&mut self) -> Token {
		let (token, session) = PendingSession::admin(Some("Admin".into()));
		let id = session.id();
		self.pending_sessions.insert(token.clone(), session);
		tracing::info!(?token, %id, "added initial admin session");
		token
	}
	#[tracing::instrument(level= "trace", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	pub async fn start(mut self) {
		let listener = self.listener.take().unwrap();
		loop {
			tokio::select! {
					client_message = Self::read_clients_messages(&mut self.connected_clients) => self.handle_client_message(client_message.0, client_message.1).await,
					accept_result = listener.accept() => self.handle_accept(accept_result).await,
					render_event = self.render_events.recv() => {
							if let Some(event) = render_event {
									self.handle_render_event(event).await;
							} else {
									tracing::warn!("render layer event channel closed");
									return;
							}
					}
			}
		}
	}

	#[tracing::instrument(level= "trace", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
	async fn handle_client_message(&mut self, client_id: ClientId, message: C2SMsg) {
		let Some(connected_client) = self.connected_clients.get_mut(&client_id) else {
			tracing::warn!("tried handling message from a non-existing client");
			return;
		};
		let client_session = connected_client
			.client_view
			.authenticated_session()
			.and_then(|s| self.active_sessions.get(&s))
			.map(Arc::clone);
		match message {
			C2SMsg::Shutdown => {
				self.connected_clients.remove(&client_id);
			}
			C2SMsg::Auth(token) => {
				let Some(pending_session) = self.pending_sessions.remove(&token) else {
					connected_client
						.client_view
						.notify_auth_error(AuthError::NotFound)
						.await;
					return;
				};
				let session = Arc::new(pending_session.promote());
				if !connected_client
					.client_view
					.notify_auth_success(&session)
					.await
				{
					self.connected_clients.remove(&client_id);
					tracing::warn!("failed to notify auth success, removing client");
					return;
				}
				self
					.active_sessions
					.insert(session.id(), Arc::clone(&session));
				if session.role() == Role::Admin && self.current_session.is_none() {
					self.current_session = Some(session.id());
				}
			}
			C2SMsg::CreateSession(req) => {
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
					self.connected_clients.remove(&client_id);
					return;
				}
			}
			C2SMsg::SwapBuffers { monitor_id, buffer } => {
				if let Err(e) = self
					.render_commands
					.send(RenderCmd::SwapBuffers { monitor_id, buffer })
					.await
				{
					tracing::error!("failed to forward SwapBuffers to renderer: {e}");
					let code = Arc::<str>::from("render_unavailable");
					let detail = Some(Arc::<str>::from("renderer unavailable"));
					connected_client
						.client_view
						.notify_error(code, detail, true)
						.await;
				}
			}
			C2SMsg::FramebufferLink { payload, dma_bufs } => {
				if let Err(e) = self
					.render_commands
					.send(RenderCmd::FramebufferLink { payload, dma_bufs })
					.await
				{
					tracing::error!("failed to forward FramebufferLink to renderer: {e}");
					let code = Arc::<str>::from("render_unavailable");
					let detail = Some(Arc::<str>::from("renderer unavailable"));
					connected_client
						.client_view
						.notify_error(code, detail, true)
						.await;
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
			}
			RenderEvt::FatalError { reason } => {
				tracing::error!(?reason, "renderer fatal error");
				// TODO: Shutdown server
			}
			RenderEvt::PageFlip { monitors } => {
				if monitors.is_empty() {
					return;
				}
				let Some(active_session) = self.current_session else {
					tracing::trace!("page flip ignored: no active session");
					return;
				};
				let Some((_id, client)) = self
					.connected_clients
					.iter_mut()
					.find(|(_, c)| c.client_view.authenticated_session() == Some(active_session))
				else {
					tracing::trace!(%active_session, "page flip ignored: no client bound to active session");
					return;
				};
				if !client.client_view.notify_frame_done(monitors).await {
					tracing::warn!(%active_session, "failed to forward frame_done to client");
				}
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
			Ok((client_socket, ip)) => {
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
				let (new_client, new_client_view) = Client::wrap_socket(client_async_fd);
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
}
