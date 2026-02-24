use crate::MonitorState;
use std::os::fd::RawFd;
use tab_protocol::{BufferIndex, SessionInfo};

/// Monitor lifecycle event emitted to listeners.
#[derive(Debug, Clone)]
pub enum MonitorEvent {
	Added(MonitorState),
	Removed(String),
}

/// Rendering-related notifications.
#[derive(Debug, Clone)]
pub enum RenderEvent {
	BufferReleased {
		monitor_id: String,
		buffer: BufferIndex,
		release_fence_fd: Option<RawFd>,
	},
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
	Active(String),
	Awake(String),
	Sleep(String),
	State(SessionInfo),
}
