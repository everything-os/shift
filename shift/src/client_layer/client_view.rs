use std::{rc::Rc, sync::Arc};

use crate::{
	auth::{self, Token},
	client_layer::client::{Client, ClientId},
	comms::{
		client2server::{C2SMsg, C2SRx, C2STx, C2SWeakTx},
		server2client::{BufferRelease, S2CMsg, S2CRx, S2CTx},
	},
	monitor::{Monitor, MonitorId},
	sessions::{PendingSession, Session, SessionId},
};
use tab_protocol::SessionInfo;

#[derive(Debug)]
pub struct ChannelsServerEnd(C2SRx, S2CTx);

impl ChannelsServerEnd {
	pub fn to_client(&self) -> &S2CTx {
		&self.1
	}
	pub fn from_client(&mut self) -> &mut C2SRx {
		&mut self.0
	}
}
#[derive(Debug)]
pub struct ChannelsClientEnd(S2CRx, C2STx);

impl ChannelsClientEnd {
	pub fn to_server(&self) -> &C2STx {
		&self.1
	}
	pub fn from_server(&mut self) -> &mut S2CRx {
		&mut self.0
	}
}
#[derive(Debug)]
pub struct Channels {
	pub client_end: ChannelsClientEnd,
	pub server_end: ChannelsServerEnd,
}
impl Channels {
	pub(super) fn new() -> Self {
		let c2s = tokio::sync::mpsc::channel(5000);
		let s2c = tokio::sync::mpsc::channel(5000);
		Self {
			client_end: ChannelsClientEnd(s2c.1, c2s.0),
			server_end: ChannelsServerEnd(c2s.1, s2c.0),
		}
	}
}
#[derive(Debug)]
pub struct ClientView {
	id: ClientId,
	pub(super) channels: ChannelsServerEnd,
	session_id: Option<SessionId>,
}

impl ClientView {
	pub(super) fn from_client(client: &Client, channels: ChannelsServerEnd) -> ClientView {
		Self {
			id: client.id(),
			channels,
			session_id: None,
		}
	}

	pub fn id(&self) -> ClientId {
		self.id
	}
	pub async fn read_message(&mut self) -> Option<C2SMsg> {
		self.channels.from_client().recv().await
	}
	pub fn running(&self) -> bool {
		!self.channels.1.is_closed() && self.has_messages()
	}
	pub fn has_messages(&self) -> bool {
		!self.channels.0.is_closed() || !self.channels.0.is_empty()
	}
	pub async fn notify_auth_error(&self, reason: auth::error::Error) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::AuthError(reason))
			.await
			.is_ok()
	}
	pub async fn notify_auth_success(&mut self, session: &Arc<Session>) -> bool {
		self.session_id = Some(session.id());
		self
			.channels
			.1
			.send(S2CMsg::BindToSession(Arc::clone(&session)))
			.await
			.is_ok()
	}
	pub async fn notify_session_created(&mut self, token: Token, session: PendingSession) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::SessionCreated(token, session))
			.await
			.is_ok()
	}

	pub async fn notify_error(
		&mut self,
		code: Arc<str>,
		error: Option<Arc<str>>,
		shutdown: bool,
	) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::Error {
				code,
				error,
				shutdown,
			})
			.await
			.is_ok()
	}

	pub fn authenticated_session(&self) -> Option<SessionId> {
		self.session_id
	}

	pub async fn notify_buffer_release(&mut self, buffers: Vec<BufferRelease>) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::BufferRelease { buffers })
			.await
			.is_ok()
	}

	pub async fn notify_buffer_request_ack(
		&mut self,
		monitor_id: MonitorId,
		buffer: tab_protocol::BufferIndex,
	) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::BufferRequestAck { monitor_id, buffer })
			.await
			.is_ok()
	}

	pub async fn notify_monitor_added(&mut self, monitor: Monitor) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::MonitorAdded { monitor })
			.await
			.is_ok()
	}

	pub async fn notify_monitor_removed(&mut self, monitor_id: MonitorId, name: Arc<str>) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::MonitorRemoved { monitor_id, name })
			.await
			.is_ok()
	}

	pub async fn notify_session_awake(&mut self, session_id: SessionId) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::SessionAwake { session_id })
			.await
			.is_ok()
	}

	pub async fn notify_session_active(&mut self, session_id: SessionId) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::SessionActive { session_id })
			.await
			.is_ok()
	}

	pub async fn notify_session_state(&mut self, session: SessionInfo) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::SessionState { session })
			.await
			.is_ok()
	}

	pub async fn notify_session_sleep(&mut self, session_id: SessionId) -> bool {
		self
			.channels
			.1
			.send(S2CMsg::SessionSleep { session_id })
			.await
			.is_ok()
	}
}
