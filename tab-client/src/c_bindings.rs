use std::{
	ffi::{CStr, CString},
	os::{fd::AsRawFd, raw::c_char},
};

use tab_protocol::DEFAULT_SOCKET_PATH;

use crate::{FrameTarget, TabClient};

#[repr(C)]
pub struct TabFrameTarget {
	pub framebuffer: u32,
	pub texture: u32,
	pub width: i32,
	pub height: i32,
}

#[repr(C)]
pub enum TabAcquireResult {
	TabAcquireOk = 0,
	TabAcquireNoBuffers = 1,
	TabAcquireError = 2,
}

impl From<FrameTarget> for TabFrameTarget {
	fn from(target: FrameTarget) -> Self {
		Self {
			framebuffer: target.framebuffer(),
			texture: target.texture(),
			width: target.size().0,
			height: target.size().1,
		}
	}
}

/// C-friendly opaque handle.
#[repr(C)]
pub struct TabClientHandle {
	inner: TabClient,
}

/// Connect to a Tab socket and authenticate immediately. Returns NULL on failure.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_connect(
	socket_path: *const c_char,
	token: *const c_char,
) -> *mut TabClientHandle {
	let path = unsafe {
		if socket_path.is_null() {
			DEFAULT_SOCKET_PATH.to_string()
		} else {
			match CStr::from_ptr(socket_path).to_str() {
				Ok(s) => s.to_string(),
				Err(_) => return std::ptr::null_mut(),
			}
		}
	};

	if token.is_null() {
		return std::ptr::null_mut();
	}
	let token = match unsafe { CStr::from_ptr(token) }.to_str() {
		Ok(s) => s.to_string(),
		Err(_) => return std::ptr::null_mut(),
	};

	match TabClient::connect(path, token) {
		Ok(client) => Box::into_raw(Box::new(TabClientHandle { inner: client })),
		Err(_) => std::ptr::null_mut(),
	}
}

/// Disconnect and free the handle.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_disconnect(handle: *mut TabClientHandle) {
	if handle.is_null() {
		return;
	}
	unsafe {
		drop(Box::from_raw(handle));
	}
}
macro_rules! to_cstr {
	($s:expr) => {
		CString::new($s)
			.map(CString::into_raw)
			.unwrap_or(std::ptr::null_mut())
	};
}
macro_rules! unwrap_handle {
	($client:expr) => {{
		let Some(client) = (unsafe { $client.as_mut() }) else {
			panic!("NullPointerException: tab client cannot be a null pointer");
		};
		&mut client.inner
	}};
}

fn c_str_to_string(ptr: *const c_char) -> Option<String> {
	if ptr.is_null() {
		return None;
	}
	unsafe { CStr::from_ptr(ptr) }
		.to_str()
		.ok()
		.map(|s| s.to_string())
}
/// Retrieve and clear the last error as an owned C string. Caller must free via `tab_client_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_protocol_name(handle: *mut TabClientHandle) -> *mut c_char {
	let client = unwrap_handle!(handle);
	to_cstr!(client.hello.protocol.as_str())
}
/// Retrieve and clear the last error as an owned C string. Caller must free via `tab_client_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_server_name(handle: *mut TabClientHandle) -> *mut c_char {
	let client = unwrap_handle!(handle);
	to_cstr!(client.hello.server.as_str())
}
/// Retrieve and clear the last error as an owned C string. Caller must free via `tab_client_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_take_error(handle: *mut TabClientHandle) -> *mut c_char {
	let client = unwrap_handle!(handle);
	let err = client.last_error.take();
	match err {
		Some(msg) => to_cstr!(msg),
		None => 0 as _,
	}
}

/// Retrieve the authenticated session JSON blob (caller must free with `tab_client_string_free`).
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_session_json(handle: *mut TabClientHandle) -> *mut c_char {
	let client = unwrap_handle!(handle);
	match client.session() {
		Some(session) => match serde_json::to_string(session) {
			Ok(json) => to_cstr!(json),
			Err(err) => {
				client.record_error(err.to_string());
				std::ptr::null_mut()
			}
		},
		None => std::ptr::null_mut(),
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_monitor_count(handle: *mut TabClientHandle) -> libc::size_t {
	let client = unwrap_handle!(handle);
	client.monitor_ids().len() as libc::size_t
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_monitor_id(
	handle: *mut TabClientHandle,
	index: libc::size_t,
) -> *mut c_char {
	let client = unwrap_handle!(handle);
	match client.monitor_ids().into_iter().nth(index as usize) {
		Some(id) => to_cstr!(id),
		None => std::ptr::null_mut(),
	}
}

/// Notify Shift that this session is ready to present.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_send_ready(handle: *mut TabClientHandle) -> bool {
	let client = unwrap_handle!(handle);
	match client.send_ready() {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_socket_fd(handle: *mut TabClientHandle) -> libc::c_int {
	let client = unwrap_handle!(handle);
	client.socket_fd().as_raw_fd()
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_get_swap_fd(handle: *mut TabClientHandle) -> libc::c_int {
	let client = unwrap_handle!(handle);
	client.swap_notifier_fd().as_raw_fd()
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_process_socket_events(handle: *mut TabClientHandle) -> bool {
	let client = unwrap_handle!(handle);
	match client.process_socket_events() {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_set_cursor_fb(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	width: u32,
	height: u32,
	hotspot_x: i32,
	hotspot_y: i32,
	data: *const u8,
	data_len: libc::size_t,
) -> bool {
	let client = unwrap_handle!(handle);
	let Some(monitor) = c_str_to_string(monitor_id) else {
		return false;
	};
	if data.is_null() {
		return false;
	}
	let pixels = unsafe { std::slice::from_raw_parts(data, data_len as usize) };
	match client.set_cursor_framebuffer(&monitor, width, height, hotspot_x, hotspot_y, pixels) {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_set_cursor_pos(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	x: i32,
	y: i32,
) -> bool {
	let client = unwrap_handle!(handle);
	let Some(monitor) = c_str_to_string(monitor_id) else {
		return false;
	};
	match client.set_cursor_position(&monitor, x, y) {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_process_swap_events(handle: *mut TabClientHandle) -> bool {
	let client = unwrap_handle!(handle);
	match client.process_ready_swaps() {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_acquire_frame(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	out_target: *mut TabFrameTarget,
) -> TabAcquireResult {
	let client = unwrap_handle!(handle);
	let Some(id) = c_str_to_string(monitor_id) else {
		return TabAcquireResult::TabAcquireError;
	};
	let Some(out_ptr) = (unsafe { out_target.as_mut() }) else {
		return TabAcquireResult::TabAcquireError;
	};
	match client.acquire_frame(&id) {
		Ok(target) => {
			*out_ptr = target.into();
			TabAcquireResult::TabAcquireOk
		}
		Err(crate::TabClientError::NoFreeBuffers(_)) => TabAcquireResult::TabAcquireNoBuffers,
		Err(err) => {
			client.record_error(err.to_string());
			TabAcquireResult::TabAcquireError
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn tab_client_swap_buffers(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
) -> bool {
	let client = unwrap_handle!(handle);
	let Some(id) = c_str_to_string(monitor_id) else {
		return false;
	};
	match client.swap_buffers(&id) {
		Ok(_) => true,
		Err(err) => {
			client.record_error(err.to_string());
			false
		}
	}
}

/// Free a string returned by `tab_client_take_error`.
#[unsafe(no_mangle)]
pub extern "C" fn tab_client_string_free(s: *mut c_char) {
	if s.is_null() {
		return;
	}
	unsafe {
		let _ = CString::from_raw(s);
	}
}
