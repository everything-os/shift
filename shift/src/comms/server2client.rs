use std::os::fd::OwnedFd;
use std::sync::Arc;

use tab_protocol::{BufferIndex, SessionInfo};

use crate::{
	auth::{self, Token},
	monitor::{Monitor, MonitorId},
	sessions::{PendingSession, Session, SessionId},
};

#[derive(Debug)]
pub struct BufferRelease {
	pub monitor_id: MonitorId,
	pub buffer: BufferIndex,
	pub release_fence: Option<OwnedFd>,
}

#[derive(Debug)]
pub enum S2CMsg {
	BindToSession(Arc<Session>),
	AuthError(auth::error::Error),
	SessionCreated(Token, PendingSession),
	Error {
		code: Arc<str>,
		error: Option<Arc<str>>,
		shutdown: bool,
	},
	BufferRelease {
		buffers: Vec<BufferRelease>,
	},
	BufferRequestAck {
		monitor_id: MonitorId,
		buffer: BufferIndex,
	},
	SessionActive {
		session_id: SessionId,
	},
	SessionState {
		session: SessionInfo,
	},
	SessionAwake {
		session_id: SessionId,
	},
	SessionSleep {
		session_id: SessionId,
	},
	MonitorAdded {
		monitor: Monitor,
	},
	MonitorRemoved {
		monitor_id: MonitorId,
		name: Arc<str>,
	},
}

pub type S2CRx = tokio::sync::mpsc::Receiver<S2CMsg>;
pub type S2CTx = tokio::sync::mpsc::Sender<S2CMsg>;
pub type S2CWeakTx = tokio::sync::mpsc::WeakSender<S2CMsg>;
