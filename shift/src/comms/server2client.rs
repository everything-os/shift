use std::{borrow::Cow, sync::Arc};

use tab_protocol::SessionRole;

use crate::{
	auth::{self, Token},
	monitor::{Monitor, MonitorId},
	sessions::{self, PendingSession, Session, SessionId},
};

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
	FrameDone {
		monitors: Vec<MonitorId>,
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
