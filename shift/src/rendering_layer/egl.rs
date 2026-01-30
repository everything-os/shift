#[allow(clippy::all, warnings)]
pub(crate) mod ffi {
	use std::os::raw::c_void;

	pub type khronos_utime_nanoseconds_t = u64;
	pub type khronos_uint64_t = u64;
	pub type khronos_ssize_t = isize;
	pub type EGLNativeDisplayType = *const c_void;
	pub type EGLNativePixmapType = *const c_void;
	pub type EGLNativeWindowType = *const c_void;
	pub type EGLint = i32;
	pub type NativeDisplayType = *const c_void;
	pub type NativePixmapType = *const c_void;
	pub type NativeWindowType = *const c_void;

	include!(concat!(env!("OUT_DIR"), "/egl_bindings.rs"));
}

pub(crate) use ffi::*;
