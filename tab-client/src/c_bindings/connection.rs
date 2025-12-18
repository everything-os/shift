use super::*;

// ============================================================================
// CONNECTION AND BASIC OPERATIONS
// ============================================================================

/// Connect to a Tab socket at an explicit path and authenticate with a token.
/// Returns NULL on failure. Use `tab_client_take_error()` to get error details.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_connect(
	socket_path: *const c_char,
	token: *const c_char,
) -> *mut TabClientHandle {
	let path = if socket_path.is_null() {
		DEFAULT_SOCKET_PATH.to_string()
	} else {
		match unsafe { CStr::from_ptr(socket_path) }.to_str() {
			Ok(s) => s.to_string(),
			Err(_) => return std::ptr::null_mut(),
		}
	};

	if token.is_null() {
		return std::ptr::null_mut();
	}

	let token_str = match unsafe { CStr::from_ptr(token) }.to_str() {
		Ok(s) => s.to_string(),
		Err(_) => return std::ptr::null_mut(),
	};

	match TabClient::connect(path, token_str) {
		Ok(client) => Box::into_raw(Box::new(TabClientHandle {
			inner: Box::new(client),
			event_queue: VecDeque::new(),
		})),
		Err(_) => std::ptr::null_mut(),
	}
}

/// Connect to the default `/tmp/shift.sock` socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_connect_default(token: *const c_char) -> *mut TabClientHandle {
	unsafe { tab_client_connect(std::ptr::null(), token) }
}

/// Disconnect and free the client handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_disconnect(handle: *mut TabClientHandle) {
	if !handle.is_null() {
		unsafe {
			drop(Box::from_raw(handle));
		}
	}
}

/// Free a string returned by C bindings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_string_free(s: *mut c_char) {
	if !s.is_null() {
		unsafe {
			drop(CString::from_raw(s));
		}
	}
}

/// Take and clear the last error message. Caller must free with `tab_client_string_free()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_take_error(handle: *mut TabClientHandle) -> *mut c_char {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &mut h.inner,
		None => return std::ptr::null_mut(),
	};

	if let Some(err) = client.last_error.take() {
		match CString::new(err) {
			Ok(s) => s.into_raw(),
			Err(_) => std::ptr::null_mut(),
		}
	} else {
		std::ptr::null_mut()
	}
}

// ============================================================================
// HELLO / SERVER INFO
// ============================================================================

/// Get server name from hello payload
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_server_name(handle: *mut TabClientHandle) -> *mut c_char {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return std::ptr::null_mut(),
	};

	match CString::new(client.hello().server.clone()) {
		Ok(s) => s.into_raw(),
		Err(_) => std::ptr::null_mut(),
	}
}

/// Get protocol version from hello payload
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_protocol_name(handle: *mut TabClientHandle) -> *mut c_char {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return std::ptr::null_mut(),
	};

	match CString::new(client.hello().protocol.clone()) {
		Ok(s) => s.into_raw(),
		Err(_) => std::ptr::null_mut(),
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_socket_fd(handle: *mut TabClientHandle) -> i32 {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return -1,
	};

	client.socket_fd().as_raw_fd()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_swap_fd(handle: *mut TabClientHandle) -> i32 {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return -1,
	};
	client.swap_notifier_fd().as_raw_fd()
}
