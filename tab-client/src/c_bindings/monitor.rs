use super::*;

// ============================================================================
// STRUCTURES - Monitor Info
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabMonitorInfo {
	pub id: *const c_char,
	pub width: i32,
	pub height: i32,
	pub refresh_rate: i32,
	pub name: *const c_char,
}

// ============================================================================
// MONITOR MANAGEMENT
// ============================================================================

/// Get the number of monitors
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_count(handle: *mut TabClientHandle) -> usize {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return 0,
	};

	client.monitor_ids().len()
}

/// Get monitor ID by index. Returns owned C string, must free with `tab_client_string_free()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_id(
	handle: *mut TabClientHandle,
	index: usize,
) -> *mut c_char {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return std::ptr::null_mut(),
	};

	client
		.monitor_ids()
		.get(index)
		.and_then(|id| CString::new(id.clone()).ok())
		.map(|s| s.into_raw())
		.unwrap_or(std::ptr::null_mut())
}

/// Get monitor info by ID as JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_info(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
) -> TabMonitorInfo {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &h.inner,
		None => return std::mem::zeroed(),
	};

	let id = match unsafe { CStr::from_ptr(monitor_id) }.to_str() {
		Ok(s) => s,
		Err(_) => return std::mem::zeroed(),
	};
	let monitor_info = match client.monitor_info(id) {
		Some(info) => info,
		None => return std::mem::zeroed(),
	};
	TabMonitorInfo {
		id: CString::new(monitor_info.id).unwrap().into_raw(),
		width: monitor_info.width,
		height: monitor_info.height,
		refresh_rate: monitor_info.refresh_rate,
		name: CString::new(monitor_info.name).unwrap().into_raw(),
	}
}

// free monitor info
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_monitor_info(monitor_info: *mut TabMonitorInfo) {
	if monitor_info.is_null() {
		return;
	}
	unsafe {
		if !(*monitor_info).id.is_null() {
			drop(CString::from_raw((*monitor_info).id as *mut _));
		}
		if !(*monitor_info).name.is_null() {
			drop(CString::from_raw((*monitor_info).name as *mut _));
		}
	}
}
