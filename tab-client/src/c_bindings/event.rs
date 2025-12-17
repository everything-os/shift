use super::*;

// ============================================================================
// ENUMS AND CONSTANTS
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAcquireResult {
	TabAcquireOk = 0,
	TabAcquireNoBuffers = 1,
	TabAcquireError = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSessionRole {
	TabSessionRoleAdmin = 0,
	TabSessionRoleSession = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSessionLifecycle {
	TabSessionLifecyclePending = 0,
	TabSessionLifecycleLoading = 1,
	TabSessionLifecycleOccupied = 2,
	TabSessionLifecycleConsumed = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabEventType {
	TabEventFrameDone = 0,
	TabEventMonitorAdded = 1,
	TabEventMonitorRemoved = 2,
	TabEventSessionState = 3,
	TabEventInput = 4,
	TabEventSessionCreated = 5,
}

// ============================================================================
// TAGGED UNION FOR ALL EVENTS
// ============================================================================

#[repr(C)]
pub union TabEventData {
	pub frame_done: *const c_char, // monitor_id (owned)
	pub monitor_added: monitor::TabMonitorInfo,
	pub monitor_removed: *const c_char, // monitor_id (owned)
	pub session_state: session::TabSessionInfo,
	pub input: input::TabInputEvent,
	pub session_created_token: *const c_char, // token (owned)
}

#[repr(C)]
pub struct TabEvent {
    pub event_type: TabEventType,
    pub data: TabEventData,
}

// ============================================================================
// EVENT PROCESSING
// ============================================================================

/// Poll events with optional blocking. Returns event count.
/// Call `tab_client_get_event()` multiple times to retrieve events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_poll_events(
	handle: *mut TabClientHandle,
) -> usize {
	let client = match unsafe{ handle.as_mut() } {
		Some(h) => h,
		None => return 0,
	};

	match client.inner.poll_events() {
		Ok(events) => {
			let count = events.len();
			client.event_queue.extend(events);
			count
		}
		Err(e) => {
			client.inner.record_error(e);
			0
		}
	}
}

/// Retrieve the next pending event.
/// Returns true if an event was retrieved, false otherwise.
/// The `event` pointer must be valid.
/// The strings in the event are owned by the caller and must be freed with `tab_client_free_event_strings`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_next_event(handle: *mut TabClientHandle, event: *mut TabEvent) -> bool {
    let client = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return false,
    };

    if let Some(rust_event) = client.event_queue.pop_front() {

        unsafe { *event = convert_event(rust_event) };
        true
    } else {
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_event_strings(event: *mut TabEvent) {
    if event.is_null() {
        return;
    }

    unsafe {
        match (*event).event_type {
            TabEventType::TabEventFrameDone => {
                if !(*event).data.frame_done.is_null() {
                    drop(CString::from_raw((*event).data.frame_done as *mut _));
                }
            }
            TabEventType::TabEventMonitorAdded => {
                if !(*event).data.monitor_added.id.is_null() {
                    drop(CString::from_raw((*event).data.monitor_added.id as *mut _));
                }
                if !(*event).data.monitor_added.name.is_null() {
                    drop(CString::from_raw((*event).data.monitor_added.name as *mut _));
                }
            }
            TabEventType::TabEventMonitorRemoved => {
                if !(*event).data.monitor_removed.is_null() {
                    drop(CString::from_raw((*event).data.monitor_removed as *mut _));
                }
            }
            TabEventType::TabEventSessionState => {
                if !(*event).data.session_state.id.is_null() {
                    drop(CString::from_raw((*event).data.session_state.id as *mut _));
                }
                if !(*event).data.session_state.display_name.is_null() {
                    drop(CString::from_raw((*event).data.session_state.display_name as *mut _));
                }
            }
            TabEventType::TabEventSessionCreated => {
                if !(*event).data.session_created_token.is_null() {
                    drop(CString::from_raw((*event).data.session_created_token as *mut _));
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// HELPER: Convert Rust event to C event
// ============================================================================

fn convert_event(rust_event: RustTabEvent) -> TabEvent {
    let mut c_event = TabEvent {
        event_type: TabEventType::TabEventFrameDone, // dummy
        data: unsafe { std::mem::zeroed() },
    };

    match rust_event {
        RustTabEvent::FrameDone { monitor_id } => {
            c_event.event_type = TabEventType::TabEventFrameDone;
            let c_str = CString::new(monitor_id).unwrap();
            unsafe {
                c_event.data.frame_done = c_str.into_raw();
            }
        }
        RustTabEvent::MonitorAdded(monitor) => {
            c_event.event_type = TabEventType::TabEventMonitorAdded;
            let c_str_id = CString::new(monitor.id).unwrap();
            let c_str_name = CString::new(monitor.name).unwrap();
            let c_monitor = monitor::TabMonitorInfo {
                id: c_str_id.into_raw(),
                width: monitor.width,
                height: monitor.height,
                refresh_rate: monitor.refresh_rate,
                name: c_str_name.into_raw(),
            };
            unsafe {
                c_event.data.monitor_added = c_monitor;
            }
        }
        RustTabEvent::MonitorRemoved(monitor_id) => {
            c_event.event_type = TabEventType::TabEventMonitorRemoved;
            let c_str = CString::new(monitor_id).unwrap();
            unsafe {
                c_event.data.monitor_removed = c_str.into_raw();
            }
        }
        RustTabEvent::SessionState(session) => {
            c_event.event_type = TabEventType::TabEventSessionState;
            let c_str_id = CString::new(session.id).unwrap();
            let c_str_display_name = CString::new(session.display_name.unwrap_or_default()).unwrap();
            let c_session = session::TabSessionInfo {
                id: c_str_id.into_raw(),
                role: match session.role {
                    SessionRole::Admin => TabSessionRole::TabSessionRoleAdmin,
                    SessionRole::Session => TabSessionRole::TabSessionRoleSession,
                },
                display_name: c_str_display_name.into_raw(),
                state: match session.state {
                    SessionLifecycle::Pending => TabSessionLifecycle::TabSessionLifecyclePending,
                    SessionLifecycle::Loading => TabSessionLifecycle::TabSessionLifecycleLoading,
                    SessionLifecycle::Occupied => TabSessionLifecycle::TabSessionLifecycleOccupied,
                    SessionLifecycle::Consumed => TabSessionLifecycle::TabSessionLifecycleConsumed,
                },
            };
            unsafe {
                c_event.data.session_state = c_session;
            }
        }
        RustTabEvent::Input(payload) => {
            c_event.event_type = TabEventType::TabEventInput;
            unsafe {
                c_event.data.input = input::convert_input_event(&payload);
            }
        }
        RustTabEvent::SessionCreated(payload) => {
            c_event.event_type = TabEventType::TabEventSessionCreated;
            let c_str = CString::new(payload.token).unwrap();
            unsafe {
                c_event.data.session_created_token = c_str.into_raw();
            }
        }
    }

    c_event
}
