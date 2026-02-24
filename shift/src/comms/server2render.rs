use std::os::fd::OwnedFd;
use std::time::Duration;

use tab_protocol::{BufferIndex, FramebufferLinkPayload};

use crate::{monitor::MonitorId, sessions::SessionId};

#[derive(Debug, Clone)]
pub struct SessionTransition {
	pub from_session_id: SessionId,
	pub animation: String,
	pub duration: Duration,
}

#[derive(Debug)]
pub enum RenderCmd {
	/// Request the renderer to clean up and exit.
	Shutdown,
	/// Ask the renderer to associate a client-provided framebuffer with internal GPU state.
	FramebufferLink {
		payload: FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
		session_id: SessionId,
	},
	/// Update which session should be displayed globally.
	SetActiveSession {
		session_id: Option<SessionId>,
		transition: Option<SessionTransition>,
	},
	/// Drop all GPU resources associated with a disconnected session.
	SessionRemoved { session_id: SessionId },
	/// Present a framebuffer on a given monitor.
	SwapBuffers {
		monitor_id: MonitorId,
		buffer: BufferIndex,
		session_id: SessionId,
		acquire_fence: Option<OwnedFd>,
	},
}

pub type RenderCmdRx = tokio::sync::mpsc::Receiver<RenderCmd>;
pub type RenderCmdTx = tokio::sync::mpsc::Sender<RenderCmd>;
pub type RenderCmdWeakTx = tokio::sync::mpsc::WeakSender<RenderCmd>;
