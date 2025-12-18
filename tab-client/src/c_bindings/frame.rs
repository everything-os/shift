use std::os::fd::RawFd;

use crate::BorrowedDmabuf;

use super::*;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct CDmabuf {
	pub fd: i32,
	pub stride: i32,
	pub offset: i32,
	pub fourcc: i32,
}

// ============================================================================
// STRUCTURES - Frame Targets
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabFrameTarget {
	pub framebuffer: u32,
	pub texture: u32,
	pub width: i32,
	pub height: i32,
	pub dmabuf: CDmabuf,
}

// ============================================================================
// FRAME OPERATIONS
// ============================================================================

/// Acquire a frame for rendering on a monitor
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_acquire_frame(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	target: *mut TabFrameTarget,
) -> event::TabAcquireResult {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &mut h.inner,
		None => return event::TabAcquireResult::TabAcquireError,
	};

	if monitor_id.is_null() || target.is_null() {
		return event::TabAcquireResult::TabAcquireError;
	}

	let id = match unsafe { CStr::from_ptr(monitor_id) }.to_str() {
		Ok(s) => s,
		Err(_) => return event::TabAcquireResult::TabAcquireError,
	};

	match client.acquire_frame(id) {
		Ok(frame) => {
			unsafe {
				(*target).framebuffer = frame.framebuffer();
				(*target).texture = frame.texture();
				let (w, h) = frame.size();
				(*target).width = w;
				(*target).height = h;
				(*target).dmabuf = CDmabuf {
					fd: frame.dmabuf().fd.as_raw_fd(),
					stride: frame.dmabuf.stride,
					offset: frame.dmabuf.offset,
					fourcc: frame.dmabuf.fourcc,
				};
			}
			event::TabAcquireResult::TabAcquireOk
		}
		Err(crate::TabClientError::NoFreeBuffers(_)) => event::TabAcquireResult::TabAcquireNoBuffers,
		Err(e) => {
			client.record_error(e);
			event::TabAcquireResult::TabAcquireError
		}
	}
}

/// Submit a frame for display
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_swap_buffers(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
) -> bool {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &mut h.inner,
		None => return false,
	};

	if monitor_id.is_null() {
		return false;
	}

	let id = match unsafe { CStr::from_ptr(monitor_id) }.to_str() {
		Ok(s) => s,
		Err(_) => return false,
	};

	client.swap_buffers(id).is_ok()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_drm_fd(handle: *mut TabClientHandle) -> RawFd {
	let client = match unsafe { handle.as_mut() } {
		Some(h) => &mut h.inner,
		None => return -1,
	};

	client.drm_fd()
}
