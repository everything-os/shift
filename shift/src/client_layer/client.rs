use std::{
	fmt::{Debug, Display},
	os::{fd::AsRawFd, unix::net::UnixStream},
	sync::Arc,
};

use tab_protocol::{
	AuthErrorPayload, AuthOkPayload, ErrorPayload, MonitorAddedPayload, MonitorRemovedPayload,
	SessionActivePayload, SessionAwakePayload, SessionCreatedPayload, SessionInfo,
	SessionSleepPayload, SessionStatePayload, TabMessage, TabMessageFrame, TabMessageFrameReader,
	message_header,
};
use tokio::{io::unix::AsyncFd, task::JoinHandle};
use tracing::{Instrument, Span};

use crate::{
	auth::Token,
	client_layer::client_view::{self, ChannelsClientEnd, ClientView},
	comms::{
		client2server::{C2SMsg, C2STx},
		server2client::S2CMsg,
	},
	define_id_type,
	monitor::{Monitor, MonitorId},
	sessions::{Role, Session, SessionId},
};
pub type AsyncUnixStream = AsyncFd<UnixStream>;

pub struct Client {
	id: ClientId,
	socket: AsyncUnixStream,
	frame_reader: TabMessageFrameReader,
	channel_client_end: ChannelsClientEnd,
	connected_session: Option<Arc<Session>>,
	shutdown: bool,
	initial_monitors: Vec<Monitor>,
}

impl Client {
	pub fn wrap_socket(
		socket: AsyncUnixStream,
		initial_monitors: Vec<Monitor>,
	) -> (Self, ClientView) {
		let channels = client_view::Channels::new();
		let client = Self {
			socket,
			frame_reader: TabMessageFrameReader::new(),
			id: ClientId::rand(),
			channel_client_end: channels.client_end,
			connected_session: None,
			shutdown: false,
			initial_monitors,
		};
		let client_view = ClientView::from_client(&client, channels.server_end);
		(client, client_view)
	}
	pub fn id(&self) -> ClientId {
		self.id
	}
	#[tracing::instrument(level = "error", skip(self), fields(client.id = self.id().to_string()))]
	async fn send_error(&self, code: &str, error: Option<impl Display + Debug>) {
		tracing::warn!("sending error to the client");
		let tab_message = TabMessageFrame::json(
			message_header::ERROR,
			ErrorPayload {
				code: code.into(),
				message: error.as_ref().map(|e| e.to_string()),
			},
		);
		let result = tab_message.send_frame_to_async_fd(&self.socket).await;
		if let Err(e) = result {
			tracing::warn!(
				"failed to send error message to client {:?}: {e}",
				error.map(|e| e.to_string())
			);
		}
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn send_auth_error(&mut self, cause: impl Display + Debug) {
		let tab_message = TabMessageFrame::json(
			message_header::AUTH_ERROR,
			AuthErrorPayload {
				error: cause.to_string(),
			},
		);

		let result = tab_message.send_frame_to_async_fd(&self.socket).await;
		if let Err(e) = result {
			tracing::warn!(
				"failed to send auth error message to client ({}): {e}",
				cause.to_string()
			);
		}
	}

	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn handle_unknown_msg(&mut self, message_name: impl Display + Debug) {
		self.send_error("unknown_message", Some(message_name)).await;
		self.schedule_client_shutdown().await;
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn handle_packet(&mut self, tab_message: TabMessage) {
		macro_rules! check_admin {
			($action:literal) => {
				if !self
					.connected_session
					.as_deref()
					.is_some_and(|session| session.role() == Role::Admin)
				{
					self
						.send_error(
							"forbidden",
							Some(format!(
								"you need to authenticate as an admin client before being able to {}",
								$action
							)),
						)
						.await;
					return;
				};
			};
		}

		macro_rules! check_session {
			($action:literal, $var:ident) => {
				let Some($var) = self.connected_session.as_deref() else {
					self
						.send_error(
							"forbidden",
							Some(format!(
								"you need to authenticate before being able to {}",
								$action
							)),
						)
						.await;
					return;
				};
			};
		}
		macro_rules! send_server_msg {
			($send:expr) => {
				let send_result = self.channel_client_end.to_server().send($send).await;
				if send_result.is_err() {
					tracing::debug!("C2S channel closed, terminating client");
					self.schedule_client_shutdown().await;
					return;
				}
			};
		}
		match tab_message {
			TabMessage::Auth(auth) => {
				let token = auth.token.parse::<Token>();
				let token = match token {
					Ok(token) => token,
					Err(error) => {
						return self
							.send_auth_error(format!("token parse error: {error:?}"))
							.await;
					}
				};
				tracing::info!(?token, "sending auth request to the server");
				send_server_msg!(C2SMsg::Auth(token));
			}
			TabMessage::SessionSwitch(session_switch_payload) => {
				check_admin!("switch session");
				send_server_msg!(C2SMsg::SwitchSession(session_switch_payload));
			}
			TabMessage::BufferRequest {
				payload,
				acquire_fence,
			} => {
				let monitor_id = payload.monitor_id.parse::<MonitorId>();
				let monitor_id = match monitor_id {
					Ok(monitor_id) => monitor_id,
					Err(error) => {
						return self
							.send_error(
								"unknown_monitor",
								Some(format!("monitor id parse error: {error:?}")),
							)
							.await;
					}
				};
				send_server_msg!(C2SMsg::BufferRequest {
					monitor_id: monitor_id,
					buffer: payload.buffer,
					acquire_fence,
				});
			}
			TabMessage::SessionCreate(session_create_req) => {
				check_admin!("create a session");
				send_server_msg!(C2SMsg::CreateSession(session_create_req));
			}
			TabMessage::Ping => {
				tracing::debug!("received ping");

				let send_result = TabMessageFrame::no_payload(message_header::PONG)
					.send_frame_to_async_fd(&self.socket)
					.await;
				if let Err(e) = send_result {
					tracing::warn!("failed to send pong message back: {e}");
					return;
				}
			}
			TabMessage::FramebufferLink {
				payload: fb_info,
				dma_bufs,
			} => {
				tracing::debug!(?fb_info, ?dma_bufs, "received link framebuffer request");
				check_session!("link framebuffer", _session);
				send_server_msg!(C2SMsg::FramebufferLink {
					payload: fb_info,
					dma_bufs
				});
			}

			TabMessage::Hello(_hello_payload) => self.handle_unknown_msg("Hello").await,
			TabMessage::AuthOk(_auth_ok_payload) => self.handle_unknown_msg("AuthOk").await,
			TabMessage::AuthError(_auth_error_payload) => self.handle_unknown_msg("AuthError").await,
			TabMessage::BufferRelease { .. } => self.handle_unknown_msg("BufferRelease").await,
			TabMessage::BufferRequestAck(_buffer_request_ack_payload) => {
				self.handle_unknown_msg("BufferRequestAck").await
			}
			TabMessage::InputEvent(_input_event_payload) => self.handle_unknown_msg("InputEvent").await,
			TabMessage::MonitorAdded(_monitor_added_payload) => {
				self.handle_unknown_msg("MonitorAdded").await
			}
			TabMessage::MonitorRemoved(_monitor_removed_payload) => {
				self.handle_unknown_msg("MonitorRemoved").await
			}
			TabMessage::SessionCreated(_session_created_payload) => {
				self.handle_unknown_msg("SessionCreated").await
			}
			TabMessage::SessionReady(_session_ready_payload) => {
				send_server_msg!(C2SMsg::SessionReady(_session_ready_payload));
			}
			TabMessage::SessionState(_session_state_payload) => {
				self.handle_unknown_msg("SessionState").await
			}
			TabMessage::SessionActive(_session_active_payload) => {
				self.handle_unknown_msg("SessionActive").await
			}
			TabMessage::SessionAwake(_payload) => self.handle_unknown_msg("SessionAwake").await,
			TabMessage::SessionSleep(_payload) => self.handle_unknown_msg("SessionSleep").await,
			TabMessage::Error(_error_payload) => self.handle_unknown_msg("Error").await,
			TabMessage::Pong => self.handle_unknown_msg("Pong").await,
			TabMessage::Unknown(tab_message_frame) => {
				self.handle_unknown_msg(tab_message_frame.header.0).await
			}
		}
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn handle_server_layer_msg(&mut self, s2c_message: Option<S2CMsg>) {
		let Some(s2c_message) = s2c_message else {
			self.schedule_client_shutdown().await;
			return;
		};
		match s2c_message {
			S2CMsg::AuthError(e) => {
				tracing::info!(
					?e,
					"server says authentication didn't work, forwarding it to the client"
				);
				self.send_auth_error(e).await;
			}
			S2CMsg::BindToSession(session) => {
				tracing::info!(
					?session,
					"server says authentication went well, forwarding auth ok to the client"
				);
				let auth_ok = TabMessageFrame::json(
					message_header::AUTH_OK,
					AuthOkPayload {
						monitors: self
							.initial_monitors
							.iter()
							.map(|m| m.to_protocol_info())
							.collect(), // TODO: add monitors,
						session: SessionInfo {
							display_name: Some(session.display_name().to_string()),
							id: session.id().to_string(),
							role: session.role().into(),
							state: if session.ready() {
								tab_protocol::SessionLifecycle::Occupied
							} else {
								tab_protocol::SessionLifecycle::Loading
							},
						},
					},
				);
				self.connected_session = Some(session);
				let send_result = auth_ok.send_frame_to_async_fd(&self.socket).await;

				if let Err(e) = send_result {
					tracing::warn!("failed to send auth ok message to client: {e}");
					return;
				}
			}
			S2CMsg::SessionCreated(token, session) => {
				tracing::debug!(
					?session,
					?token,
					"server says it created a new session sucessfully"
				);
				let send_result = TabMessageFrame::json(
					message_header::SESSION_CREATED,
					SessionCreatedPayload {
						session: SessionInfo {
							display_name: session.display_name().map(String::from),
							id: session.id().to_string(),
							role: session.role().into(),
							state: tab_protocol::SessionLifecycle::Pending,
						},
						token: token.to_string(),
					},
				)
				.send_frame_to_async_fd(&self.socket)
				.await;
				if let Err(e) = send_result {
					tracing::warn!("failed to send session created message to client: {e}");
					return;
				}
			}
			S2CMsg::Error {
				code,
				error,
				shutdown,
			} => {
				self.send_error(&code, error.as_deref()).await;
				if shutdown {
					self.schedule_client_shutdown().await;
				}
			}
			S2CMsg::BufferRelease { buffers } => {
				for buffer in buffers {
					let payload = format!("{} {}", buffer.monitor_id, buffer.buffer as u8);
					let mut frame = TabMessageFrame::raw(message_header::BUFFER_RELEASE, payload);
					if let Some(fd) = buffer.release_fence.as_ref() {
						frame.fds.push(fd.as_raw_fd());
					}
					let send_result = frame.send_frame_to_async_fd(&self.socket).await;
					if let Err(e) = send_result {
						tracing::warn!(monitor_id = %buffer.monitor_id, buffer = buffer.buffer as u8, "failed to send buffer_release: {e}");
						break;
					}
				}
			}
			S2CMsg::BufferRequestAck { monitor_id, buffer } => {
				let payload = format!("{monitor_id} {}", buffer as u8);
				if let Err(e) = TabMessageFrame::raw(message_header::BUFFER_REQUEST_ACK, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!(%monitor_id, buffer = buffer as u8, "failed to send buffer_request_ack: {e}");
				}
			}
			S2CMsg::SessionAwake { session_id } => {
				let payload = SessionAwakePayload {
					session_id: session_id.to_string(),
				};
				if let Err(e) = TabMessageFrame::json(message_header::SESSION_AWAKE, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send session awake: {e}");
				}
			}
			S2CMsg::SessionActive { session_id } => {
				let payload = SessionActivePayload {
					session_id: session_id.to_string(),
				};
				if let Err(e) = TabMessageFrame::json(message_header::SESSION_ACTIVE, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send session active: {e}");
				}
			}
			S2CMsg::SessionState { session } => {
				let payload = SessionStatePayload { session };
				if let Err(e) = TabMessageFrame::json(message_header::SESSION_STATE, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send session state: {e}");
				}
			}
			S2CMsg::SessionSleep { session_id } => {
				let payload = SessionSleepPayload {
					session_id: session_id.to_string(),
				};
				if let Err(e) = TabMessageFrame::json(message_header::SESSION_SLEEP, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send session sleep: {e}");
				}
			}
			S2CMsg::MonitorAdded { monitor } => {
				let payload = MonitorAddedPayload {
					monitor: monitor.to_protocol_info(),
				};
				if let Err(e) = TabMessageFrame::json(message_header::MONITOR_ADDED, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send monitor added: {e}");
				}
			}
			S2CMsg::MonitorRemoved { monitor_id, name } => {
				let payload = MonitorRemovedPayload {
					monitor_id: monitor_id.to_string(),
					name: name.to_string(),
				};
				if let Err(e) = TabMessageFrame::json(message_header::MONITOR_REMOVED, payload)
					.send_frame_to_async_fd(&self.socket)
					.await
				{
					tracing::warn!("failed to send monitor removed: {e}");
				}
			}
		}
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn schedule_client_shutdown(&mut self) {
		tracing::info!("terminating client");
		let _ = self
			.channel_client_end
			.to_server()
			.send(C2SMsg::Shutdown)
			.await;
		self.shutdown = true;
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	async fn run(mut self) {
		loop {
			tokio::select! {
					read_frame_result = self.frame_reader.read_frame_from_async_fd(&self.socket) => match read_frame_result.and_then(TabMessage::try_from) {
							Ok(packet) => self.handle_packet(packet).await,
							Err(e) => {
									self.send_error("protocol_violation", Some(e)).await;
									self.schedule_client_shutdown().await;
							}
					},
					server_layer_message = self.channel_client_end.from_server().recv() => self.handle_server_layer_msg(server_layer_message).await
			}
			if self.shutdown {
				return;
			}
		}
	}
	#[tracing::instrument(skip(self), fields(client.id = self.id().to_string()))]
	pub async fn spawn(self) -> JoinHandle<()> {
		tokio::spawn(self.run().instrument(Span::current()))
	}
}
define_id_type!(Client, "cl_");
