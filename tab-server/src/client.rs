use std::os::fd::RawFd;
use std::sync::Arc;

use crate::connection::TabConnection;
use crate::server::TabServerError;
use crate::session::SessionRegistry;
use tab_protocol::{
	AuthErrorPayload, AuthOkPayload, ErrorPayload, FramebufferLinkPayload, MonitorInfo,
	SessionCreatePayload, SessionCreatedPayload, SessionInfo, SessionLifecycle, SessionReadyPayload,
	SessionRole, SessionSwitchPayload, SwapBuffersPayload, TabMessage, TabMessageFrame,
	message_header,
};
use tracing::{debug, error, info, warn};

type Loader<Texture> =
	Arc<dyn Fn(RawFd, &FramebufferLinkPayload) -> Result<Texture, TabServerError>>;

pub struct Client<Texture> {
	pub id: String,
	pub connection: TabConnection,
	pub session: ClientSession,
	load_texture: Loader<Texture>,
}

impl<Texture> Client<Texture> {
	pub fn new(connection: TabConnection, load_texture: Loader<Texture>) -> Self {
		Self {
			id: crate::generate_id("cli"),
			connection,
			session: ClientSession::default(),
			load_texture,
		}
	}

	pub fn handle_message(
		&mut self,
		message: TabMessage,
		sessions: &mut SessionRegistry,
		monitors: &[MonitorInfo],
	) -> Vec<ServerEvent<Texture>> {
		let mut events = Vec::new();
		match message {
			TabMessage::Auth(payload) => {
				if let Some(session_id) = sessions.authenticate_with_token(&payload.token) {
					self.session.authenticated = true;
					self.session.token = Some(payload.token);
					self.session.session_id = Some(session_id.clone());
					info!(client_id = %self.id, session_id = %session_id, "Client authenticated");
					if let Some(mut info) = sessions.session_info(&session_id) {
						if info.role == SessionRole::Admin {
							if let Some(updated) = sessions.set_state(&session_id, SessionLifecycle::Occupied) {
								info = updated;
							}
						}
						self.session.role = Some(info.role);
						let frame = TabMessageFrame::json(
							message_header::AUTH_OK,
							AuthOkPayload {
								session: info.clone(),
								monitors: monitors.to_vec(),
							},
						);
						if let Err(err) = self.connection.send_frame(&frame) {
							error!(client_id = %self.id, %err, "Failed to send auth_ok");
						}
						events.push(ServerEvent::SessionState {
							session: info,
							exclude_client_id: None,
						});
					}
				} else {
					let message = format!("Unknown or expired token {}", payload.token);
					warn!(client_id = %self.id, "Authentication failed: {}", message);
					let frame = TabMessageFrame::json(
						message_header::AUTH_ERROR,
						AuthErrorPayload { error: message },
					);
					if let Err(err) = self.connection.send_frame(&frame) {
						error!(client_id = %self.id, %err, "Failed to send auth_error");
					}
				}
			}
			TabMessage::SessionReady(payload) => {
				if let Some(event) = self.handle_session_ready(payload, sessions) {
					events.push(event);
				}
			}
			TabMessage::SessionCreate(payload) => {
				if let Some(event) = self.handle_session_create(payload, sessions) {
					events.push(event);
				}
			}
			TabMessage::FramebufferLink { payload, dma_bufs } => {
				let Some(session_id) = self.session.session_id.clone() else {
					warn!(
						client_id = %self.id,
						monitor_id = %payload.monitor_id,
						"Framebuffer link before authentication"
					);
					return events;
				};
				match dma_bufs
					.iter()
					.map(|&f| (self.load_texture)(f, &payload))
					.collect::<Result<Vec<Texture>, _>>()
				{
					Ok(mut buffers) => {
						assert_eq!(buffers.len(), 2);
						let buffers = [buffers.swap_remove(0), buffers.swap_remove(0)];
						info!(
							client_id = %self.id,
							monitor_id = %payload.monitor_id,
							"Client linked 2 buffers successfully"
						);
						events.push(ServerEvent::FramebufferLinked {
							monitor_id: payload.monitor_id.clone(),
							session_id,
							buffers,
						});
					}
					Err(err) => {
						error!(
							client_id = %self.id,
							monitor_id = %payload.monitor_id,
							%err,
							"Failed to load dma-buf"
						);
					}
				}
			}
			TabMessage::SessionSwitch(switch) => {
				events.push(ServerEvent::SessionSwitch(switch));
			}
			TabMessage::SwapBuffers { payload: swap } => {
				let Some(session_id) = self.session.session_id.clone() else {
					warn!(client_id = %self.id, "swap_buffers before authentication");
					return events;
				};
				events.push(ServerEvent::SwapBuffers {
					session_id,
					payload: swap,
				});
			}
			other => {
				debug!(client_id = %self.id, ?other, "Received message");
			}
		}
		events
	}
}

#[derive(Debug, Default)]
pub struct ClientSession {
	pub authenticated: bool,
	pub token: Option<String>,
	pub session_id: Option<String>,
	pub role: Option<SessionRole>,
}

#[derive(Debug, Clone)]
pub enum ServerEvent<Texture> {
	SessionState {
		session: SessionInfo,
		exclude_client_id: Option<String>,
	},
	FramebufferLinked {
		monitor_id: String,
		session_id: String,
		buffers: [Texture; 2],
	},
	SessionSwitch(SessionSwitchPayload),
	SwapBuffers {
		session_id: String,
		payload: SwapBuffersPayload,
	},
}

impl<Texture> Client<Texture> {
	fn handle_session_create(
		&mut self,
		payload: SessionCreatePayload,
		sessions: &mut SessionRegistry,
	) -> Option<ServerEvent<Texture>> {
		if !self.session.authenticated {
			self.send_error("not_authenticated", Some("Authenticate first".into()));
			return None;
		}
		if self.session.role != Some(SessionRole::Admin) {
			self.send_error(
				"not_admin",
				Some("Only admin sessions may create sessions".into()),
			);
			return None;
		}

		let (session_info, session_id, token) =
			sessions.create_pending(payload.role, payload.display_name.clone());
		info!(
			client_id = %self.id,
			new_session = %session_id,
			role = ?payload.role,
			"Admin created new session"
		);
		let frame = TabMessageFrame::json(
			message_header::SESSION_CREATED,
			SessionCreatedPayload {
				session: session_info.clone(),
				token,
			},
		);
		if let Err(err) = self.connection.send_frame(&frame) {
			error!(client_id = %self.id, %err, "Failed to send session_created");
		}
		Some(ServerEvent::SessionState {
			session: session_info,
			exclude_client_id: Some(self.id.clone()),
		})
	}

	fn handle_session_ready(
		&mut self,
		payload: SessionReadyPayload,
		sessions: &mut SessionRegistry,
	) -> Option<ServerEvent<Texture>> {
		if !self.session.authenticated {
			self.send_error("not_authenticated", Some("Authenticate first".into()));
			return None;
		}
		if self.session.role != Some(SessionRole::Session) {
			self.send_error(
				"invalid_role",
				Some("Only session clients may send session_ready".into()),
			);
			return None;
		}
		let Some(expected) = &self.session.session_id else {
			self.send_error("unknown_session", Some("Server missing session id".into()));
			return None;
		};
		if expected != &payload.session_id {
			self.send_error("session_mismatch", Some("Session id mismatch".into()));
			return None;
		}
		match sessions.set_state(expected, SessionLifecycle::Occupied) {
			Some(info) => {
				info!(
					client_id = %self.id,
					session_id = expected.as_str(),
					"Session reported ready"
				);
				Some(ServerEvent::SessionState {
					session: info,
					exclude_client_id: None,
				})
			}
			None => {
				self.send_error("unknown_session", Some("Session not found".into()));
				None
			}
		}
	}

	fn send_error(&mut self, code: &str, message: Option<String>) {
		let payload = ErrorPayload {
			code: code.to_string(),
			message,
		};
		let frame = TabMessageFrame::json(message_header::ERROR, payload);
		if let Err(err) = self.connection.send_frame(&frame) {
			error!(client_id = %self.id, %err, "Failed to send error response");
		}
	}
}
