use super::*;

// ============================================================================
// STRUCTURES - Session Info
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabSessionInfo {
	pub id: *const c_char,
	pub role: event::TabSessionRole,
	pub display_name: *const c_char, // NULL if not set
	pub state: event::TabSessionLifecycle,
}

// ============================================================================
// SESSION MANAGEMENT
// ============================================================================

/// Get session info as JSON string. Caller must free with `tab_client_string_free()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_session(handle: *mut TabClientHandle) -> TabSessionInfo {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return std::mem::zeroed(),
	};

	if let Some(session) = client.session() {
		let id_cstr = match CString::new(session.id.clone()) {
			Ok(s) => s,
			Err(_) => return std::mem::zeroed(),
		};
		let display_name_cstr = match &session.display_name {
			Some(name) => match CString::new(name.clone()) {
				Ok(s) => s,
				Err(_) => return std::mem::zeroed(),
			},
			None => CString::new("").unwrap(), // Placeholder, will use NULL pointer
		};

		TabSessionInfo {
			id: id_cstr.into_raw(),
			role: match session.role {
				SessionRole::Admin => event::TabSessionRole::TabSessionRoleAdmin,
				SessionRole::Session => event::TabSessionRole::TabSessionRoleSession,
			},
			display_name: if session.display_name.is_some() {
				display_name_cstr.into_raw()
			} else {
				std::ptr::null()
			},
			state: match session.state {
				SessionLifecycle::Pending => event::TabSessionLifecycle::TabSessionLifecyclePending,
				SessionLifecycle::Loading => event::TabSessionLifecycle::TabSessionLifecycleLoading,
				SessionLifecycle::Occupied => event::TabSessionLifecycle::TabSessionLifecycleOccupied,
				SessionLifecycle::Consumed => event::TabSessionLifecycle::TabSessionLifecycleConsumed,
			},
		}
	} else {
		std::mem::zeroed()
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_session_info(info: *mut TabSessionInfo) {
	if info.is_null() {
		return;
	}
	unsafe {
		if !(*info).id.is_null() {
			drop(CString::from_raw((*info).id as *mut _));
		}
		if !(*info).display_name.is_null() {
			drop(CString::from_raw((*info).display_name as *mut _));
		}
	}
}

/// Send session ready signal
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_send_ready(handle: *mut TabClientHandle) -> bool {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &mut h.inner,
		None => return false,
	};

	client.send_ready().is_ok()
}
