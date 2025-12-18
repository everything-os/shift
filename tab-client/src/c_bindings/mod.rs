use std::{
	collections::VecDeque,
	ffi::{CStr, CString},
	os::fd::AsRawFd,
	os::raw::c_char,
};

use tab_protocol::{
	AxisOrientation, AxisSource, ButtonState, DEFAULT_SOCKET_PATH, InputEventPayload, KeyState,
	MonitorInfo, SessionInfo, SessionLifecycle, SessionRole,
};

use crate::{TabClient, TabEvent as RustTabEvent};

pub mod connection;
pub mod event;
pub mod frame;
pub mod input;
pub mod monitor;
pub mod session;

// ============================================================================
// OPAQUE HANDLES
// ============================================================================

/// Opaque handle to a TabClient instance
#[repr(C)]
pub struct TabClientHandle {
	inner: Box<TabClient>,
	event_queue: VecDeque<RustTabEvent>,
}
