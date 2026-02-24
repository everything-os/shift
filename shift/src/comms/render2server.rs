use std::os::fd::OwnedFd;
use std::sync::Arc;

use tab_protocol::BufferIndex;

use crate::{
	monitor::{Monitor, MonitorId},
	sessions::SessionId,
};

/// Events emitted by the rendering layer back into the server core.
#[derive(Debug)]
pub enum RenderEvt {
	/// Rendering layer has started successfully
	Started {
		/// Initial monitors when shift started
		monitors: Vec<Monitor>,
	},
	/// The user plugged in a new monitor
	MonitorOnline { monitor: Monitor },
	/// The user unplugged a monitor
	MonitorOffline { monitor_id: MonitorId },
	/// Rendering reported an unrecoverable condition.
	FatalError { reason: Arc<str> },
	/// Some monitors just page flipped and are ready to be commited to again
	PageFlip { monitors: Vec<MonitorId> },
	/// Renderer has accepted and applied a buffer request to its internal state.
	BufferRequestAck {
		session_id: SessionId,
		monitor_id: MonitorId,
		buffer: BufferIndex,
	},
	/// Renderer switched to a newer buffer and no longer needs the previous one.
	BufferConsumed {
		session_id: SessionId,
		monitor_id: MonitorId,
		buffer: BufferIndex,
		release_fence: Option<OwnedFd>,
	},
	/// Renderer rejected a buffer request after inspecting local state.
	BufferRequestRejected {
		session_id: SessionId,
		monitor_id: MonitorId,
		buffer: BufferIndex,
		reason: Arc<str>,
	},
}

pub type RenderEvtRx = tokio::sync::mpsc::Receiver<RenderEvt>;
pub type RenderEvtTx = tokio::sync::mpsc::Sender<RenderEvt>;
pub type RenderEvtWeakTx = tokio::sync::mpsc::WeakSender<RenderEvt>;
