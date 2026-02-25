//! OpenGL renderer integration for `tab-app-framework`.
//! Provides EGL/GBM context setup and DMA-BUF import helpers.

mod egl;
mod framework;

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::fs::OpenOptions;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::ptr;

use gbm::AsRaw as _;
use gbm::Device as GbmDevice;
use glow::HasContext;
use thiserror::Error;

pub use framework::{GlApplication, GlEventContext, GlInitContext, GlTabAppFramework};
pub use tab_app_framework_core::{SessionCreatedPayload, SessionInfo, SessionRole};

/// Requested OpenGL/OpenGL ES version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlVersion {
	/// Major version.
	pub major: u8,
	/// Minor version.
	pub minor: u8,
}

/// Errors produced by GL/EGL initialization and rendering helpers.
#[derive(Debug, Error)]
pub enum GlError {
	#[error("failed to open EGL library: {0}")]
	LoadEglLibrary(String),
	#[error("failed to open GL library: {0}")]
	LoadGlLibrary(String),
	#[error("invalid function name: {0}")]
	InvalidFunctionName(String),
	#[error("failed to open render node {path}: {source}")]
	RenderNodeOpen {
		path: PathBuf,
		source: std::io::Error,
	},
	#[error("gbm device initialization failed: {0}")]
	GbmInit(String),
	#[error("eglGetPlatformDisplay failed")]
	GetPlatformDisplayFailed,
	#[error("eglInitialize failed (error={0:#X})")]
	InitializeFailed(i32),
	#[error("eglBindAPI failed (error={0:#X})")]
	BindApiFailed(i32),
	#[error("eglChooseConfig failed (error={0:#X})")]
	ChooseConfigFailed(i32),
	#[error("no EGL config found")]
	MissingConfig,
	#[error("eglCreateContext failed (error={0:#X})")]
	CreateContextFailed(i32),
	#[error("failed to create EGL context for OpenGL/OpenGL ES: {0}")]
	ContextCreationFailed(String),
	#[error("eglMakeCurrent failed (error={0:#X})")]
	MakeCurrentFailed(i32),
	#[error(
		"required EGL image entrypoints are unavailable (need eglCreateImageKHR or eglCreateImage, and eglDestroyImageKHR or eglDestroyImage)"
	)]
	MissingEglImageExt,
	#[error("missing glEGLImageTargetTexture2DOES")]
	MissingGlEglImageTarget,
	#[error("missing eglDupNativeFenceFDANDROID")]
	MissingEglDupNativeFenceFd,
	#[error("eglCreateSync failed (error={0:#X})")]
	CreateSyncFailed(i32),
	#[error("eglDupNativeFenceFDANDROID failed (error={0:#X})")]
	DupNativeFenceFdFailed(i32),
	#[error("eglCreateImageKHR failed (error={0:#X})")]
	CreateImageFailed(i32),
}

type GlEglImageTargetTexture2DOes = unsafe extern "system" fn(u32, *const c_void);

/// OpenGL/EGL context and DMA-BUF render-target cache.
pub struct GlContext {
	egl: egl::Egl,
	display: egl::types::EGLDisplay,
	context: egl::types::EGLContext,
	_gbm_device: GbmDevice<std::fs::File>,
	egl_lib: libloading::Library,
	gl_lib: libloading::Library,
	glow: glow::Context,
	version: GlVersion,
	egl_image_target_texture_2d_oes: GlEglImageTargetTexture2DOes,
	dmabuf_targets: HashMap<RenderTargetKey, DmabufTarget>,
}

impl GlContext {
	/// Creates a surfaceless EGL context backed by a GBM render node.
	pub fn new(version: GlVersion, render_node: Option<&Path>) -> Result<Self, GlError> {
		let egl_lib = unsafe { libloading::Library::new("libEGL.so.1") }
			.map_err(|e| GlError::LoadEglLibrary(e.to_string()))?;
		let gl_lib = unsafe { libloading::Library::new("libGL.so.1") }
			.map_err(|e| GlError::LoadGlLibrary(e.to_string()))?;

		// Bootstrap with dlsym first so we can use eglGetProcAddress for extension entrypoints.
		let egl_boot =
			egl::Egl::load_with(|name| load_symbol(&egl_lib, name).unwrap_or(ptr::null()));

		let gbm_device = open_render_node_gbm_device(render_node)?;
		const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;
		let display = if egl_boot.GetPlatformDisplay.is_loaded() {
			unsafe {
				egl_boot.GetPlatformDisplay(
					EGL_PLATFORM_GBM_KHR,
					gbm_device.as_raw_mut().cast(),
					ptr::null(),
				)
			}
		} else if egl_boot.GetPlatformDisplayEXT.is_loaded() {
			unsafe {
				egl_boot.GetPlatformDisplayEXT(
					EGL_PLATFORM_GBM_KHR,
					gbm_device.as_raw_mut().cast(),
					ptr::null(),
				)
			}
		} else {
			ptr::null()
		};
		if display.is_null() {
			return Err(GlError::GetPlatformDisplayFailed);
		}

		let mut major = 0;
		let mut minor = 0;
		let ok = unsafe { egl_boot.Initialize(display, &mut major, &mut minor) };
		if ok == 0 {
			return Err(GlError::InitializeFailed(unsafe { egl_boot.GetError() }));
		}
		let egl = egl::Egl::load_with(|name| {
			if let Some(sym) = load_symbol(&egl_lib, name) {
				return sym;
			}
			if !egl_boot.GetProcAddress.is_loaded() {
				return ptr::null();
			}
			let Ok(c_name) = CString::new(name) else {
				return ptr::null();
			};
			let ptr = unsafe { egl_boot.GetProcAddress(c_name.as_ptr()) };
			if ptr.is_null() { ptr::null() } else { ptr.cast() }
		});

		const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
		const EGL_CONTEXT_MINOR_VERSION: i32 = 0x30FB;
		const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
		const EGL_OPENGL_ES3_BIT_KHR: i32 = 0x0040;

		let mut last_error = String::new();
		let (config, context) = if unsafe { egl.BindAPI(egl::OPENGL_API as u32) } != 0 {
			let gl_config = choose_config(&egl, display, egl::OPENGL_BIT as i32)?;
			let gl_ctx_attribs = [
				EGL_CONTEXT_MAJOR_VERSION,
				version.major as i32,
				EGL_CONTEXT_MINOR_VERSION,
				version.minor as i32,
				egl::NONE as i32,
			];
			let context = unsafe {
				egl.CreateContext(
					display,
					gl_config,
					egl::NO_CONTEXT,
					gl_ctx_attribs.as_ptr() as *const _,
				)
			};
			if context.is_null() {
				last_error = format!("OpenGL context failed eglError={:#X}", unsafe {
					egl.GetError()
				});
				(ptr::null(), ptr::null())
			} else {
				(gl_config, context)
			}
		} else {
			last_error = format!("OpenGL BindAPI failed eglError={:#X}", unsafe {
				egl.GetError()
			});
			(ptr::null(), ptr::null())
		};

		let (_config, context) = if context.is_null() {
			if unsafe { egl.BindAPI(egl::OPENGL_ES_API as u32) } == 0 {
				return Err(GlError::ContextCreationFailed(format!(
					"{last_error}; OpenGL ES BindAPI failed eglError={:#X}",
					unsafe { egl.GetError() }
				)));
			}

			let es_bits = if version.major >= 3 {
				EGL_OPENGL_ES3_BIT_KHR
			} else {
				EGL_OPENGL_ES2_BIT
			};
			let es_config = choose_config(&egl, display, es_bits)?;
			let es_major = version.major.max(2);
			let es_ctx_attribs = [
				EGL_CONTEXT_MAJOR_VERSION,
				es_major as i32,
				EGL_CONTEXT_MINOR_VERSION,
				version.minor as i32,
				egl::NONE as i32,
			];
			let es_context = unsafe {
				egl.CreateContext(
					display,
					es_config,
					egl::NO_CONTEXT,
					es_ctx_attribs.as_ptr() as *const _,
				)
			};
			if es_context.is_null() {
				return Err(GlError::ContextCreationFailed(format!(
					"{last_error}; OpenGL ES context failed eglError={:#X}",
					unsafe { egl.GetError() }
				)));
			}
			(es_config, es_context)
		} else {
			(config, context)
		};

		let make_current_ok =
			unsafe { egl.MakeCurrent(display, egl::NO_SURFACE, egl::NO_SURFACE, context) };
		if make_current_ok == 0 {
			return Err(GlError::MakeCurrentFailed(unsafe { egl.GetError() }));
		}

		if !(egl.CreateImageKHR.is_loaded() || egl.CreateImage.is_loaded())
			|| !(egl.DestroyImageKHR.is_loaded() || egl.DestroyImage.is_loaded())
		{
			return Err(GlError::MissingEglImageExt);
		}

		let image_target_ptr = load_proc_raw(&egl, &egl_lib, &gl_lib, "glEGLImageTargetTexture2DOES")
			.ok_or(GlError::MissingGlEglImageTarget)?;
		let egl_image_target_texture_2d_oes: GlEglImageTargetTexture2DOes =
			unsafe { std::mem::transmute(image_target_ptr) };

		let glow = unsafe {
			glow::Context::from_loader_function(|name| {
				load_proc_raw(&egl, &egl_lib, &gl_lib, name).unwrap_or(ptr::null()) as *const _
			})
		};

		Ok(Self {
			egl,
			display,
			context,
			_gbm_device: gbm_device,
			egl_lib,
			gl_lib,
			glow,
			version,
			egl_image_target_texture_2d_oes,
			dmabuf_targets: HashMap::new(),
		})
	}

	/// Returns the actual GL version requested for this context.
	pub fn version(&self) -> GlVersion {
		self.version
	}

	/// Makes this context current on the calling thread.
	pub fn make_current(&self) -> Result<(), GlError> {
		let ok = unsafe {
			self
				.egl
				.MakeCurrent(self.display, egl::NO_SURFACE, egl::NO_SURFACE, self.context)
		};
		if ok == 0 {
			return Err(GlError::MakeCurrentFailed(unsafe { self.egl.GetError() }));
		}
		Ok(())
	}

	/// Resolves an OpenGL/EGL symbol by name.
	pub fn load_proc(&self, name: &str) -> Result<*const c_void, GlError> {
		if name.as_bytes().contains(&0) {
			return Err(GlError::InvalidFunctionName(name.to_string()));
		}
		Ok(load_proc_raw(&self.egl, &self.egl_lib, &self.gl_lib, name).unwrap_or(ptr::null()))
	}

	/// Returns the underlying `glow` context.
	pub fn glow(&self) -> &glow::Context {
		&self.glow
	}

	/// Returns the currently bound framebuffer object id.
	pub fn current_fbo(&self) -> i32 {
		unsafe { self.glow.get_parameter_i32(glow::FRAMEBUFFER_BINDING) }
	}

	/// Creates an EGL native fence FD representing queued GL work.
	pub fn create_acquire_fence_fd(&self) -> Result<OwnedFd, GlError> {
		if !self.egl.DupNativeFenceFDANDROID.is_loaded() {
			return Err(GlError::MissingEglDupNativeFenceFd);
		}
		let attribs: [egl::types::EGLAttrib; 3] = [
			egl::SYNC_NATIVE_FENCE_FD_ANDROID as egl::types::EGLAttrib,
			egl::NO_NATIVE_FENCE_FD_ANDROID as egl::types::EGLAttrib,
			egl::NONE as egl::types::EGLAttrib,
		];
		let sync = unsafe {
			self.egl.CreateSync(
				self.display,
				egl::SYNC_NATIVE_FENCE_ANDROID,
				attribs.as_ptr(),
			)
		};
		if sync.is_null() {
			return Err(GlError::CreateSyncFailed(unsafe { self.egl.GetError() }));
		}

		unsafe { self.glow.flush() };
		let fd = unsafe { self.egl.DupNativeFenceFDANDROID(self.display, sync as egl::types::EGLSyncKHR) };
		unsafe {
			self.egl.DestroySync(self.display, sync);
		}
		if fd < 0 {
			return Err(GlError::DupNativeFenceFdFailed(unsafe {
				self.egl.GetError()
			}));
		}

		Ok(unsafe { OwnedFd::from_raw_fd(fd) })
	}

	/// Imports/binds the render target for a render event and sets viewport.
	pub fn prepare_render_target(
		&mut self,
		ev: &tab_app_framework_core::RenderEvent,
	) -> Result<(), GlError> {
		let key = RenderTargetKey::new(&ev.monitor_id, ev.buffer_index as u8);
		if !self.dmabuf_targets.contains_key(&key) {
			let target = self.import_target(ev)?;
			self.dmabuf_targets.insert(key.clone(), target);
		}

		let target = self
			.dmabuf_targets
			.get(&key)
			.expect("dmabuf target cache unexpectedly missing");
		unsafe {
			self
				.glow
				.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
			self.glow.viewport(0, 0, ev.width, ev.height);
		}
		Ok(())
	}

	/// Releases cached render targets for a monitor.
	pub fn release_monitor_targets(&mut self, monitor_id: &str) {
		let keys: Vec<_> = self
			.dmabuf_targets
			.keys()
			.filter(|k| k.monitor_id == monitor_id)
			.cloned()
			.collect();
		for key in keys {
			if let Some(target) = self.dmabuf_targets.remove(&key) {
				unsafe {
					self.glow.delete_framebuffer(target.framebuffer);
					self.glow.delete_texture(target.texture);
				}
				self.destroy_egl_image(target.egl_image);
			}
		}
	}

	fn import_target(
		&self,
		ev: &tab_app_framework_core::RenderEvent,
	) -> Result<DmabufTarget, GlError> {
		let attrs = [
			egl::LINUX_DRM_FOURCC_EXT as i32,
			ev.fourcc,
			egl::DMA_BUF_PLANE0_FD_EXT as i32,
			ev.dmabuf_fd,
			egl::DMA_BUF_PLANE0_OFFSET_EXT as i32,
			ev.offset,
			egl::DMA_BUF_PLANE0_PITCH_EXT as i32,
			ev.stride,
			egl::WIDTH as i32,
			ev.width,
			egl::HEIGHT as i32,
			ev.height,
			egl::NONE as i32,
		];

		let image = self.create_egl_image(&attrs)?;
		if image == egl::NO_IMAGE_KHR {
			return Err(GlError::CreateImageFailed(unsafe { self.egl.GetError() }));
		}

		let texture = unsafe {
			self
				.glow
				.create_texture()
				.expect("failed to create texture")
		};
		let framebuffer = unsafe {
			self
				.glow
				.create_framebuffer()
				.expect("failed to create framebuffer")
		};

		unsafe {
			self.glow.bind_texture(glow::TEXTURE_2D, Some(texture));
			self.glow.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_MIN_FILTER,
				glow::LINEAR as i32,
			);
			self.glow.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_MAG_FILTER,
				glow::LINEAR as i32,
			);
			self.glow.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_WRAP_S,
				glow::CLAMP_TO_EDGE as i32,
			);
			self.glow.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_WRAP_T,
				glow::CLAMP_TO_EDGE as i32,
			);
			(self.egl_image_target_texture_2d_oes)(glow::TEXTURE_2D, image.cast());

			self
				.glow
				.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
			self.glow.framebuffer_texture_2d(
				glow::FRAMEBUFFER,
				glow::COLOR_ATTACHMENT0,
				glow::TEXTURE_2D,
				Some(texture),
				0,
			);
			self.glow.bind_texture(glow::TEXTURE_2D, None);
			self.glow.bind_framebuffer(glow::FRAMEBUFFER, None);
		}

		Ok(DmabufTarget {
			egl_image: image,
			texture,
			framebuffer,
		})
	}

	fn create_egl_image(&self, attrs: &[i32]) -> Result<egl::types::EGLImageKHR, GlError> {
		if self.egl.CreateImageKHR.is_loaded() {
			let image = unsafe {
				self.egl.CreateImageKHR(
					self.display,
					egl::NO_CONTEXT,
					egl::LINUX_DMA_BUF_EXT,
					ptr::null_mut(),
					attrs.as_ptr(),
				)
			};
			return Ok(image);
		}

		if self.egl.CreateImage.is_loaded() {
			let attrs64: Vec<egl::types::EGLAttrib> =
				attrs.iter().map(|v| *v as egl::types::EGLAttrib).collect();
			let image = unsafe {
				self.egl.CreateImage(
					self.display,
					egl::NO_CONTEXT,
					egl::LINUX_DMA_BUF_EXT,
					ptr::null_mut(),
					attrs64.as_ptr(),
				)
			};
			return Ok(image as egl::types::EGLImageKHR);
		}

		Err(GlError::MissingEglImageExt)
	}

	fn destroy_egl_image(&self, image: egl::types::EGLImageKHR) {
		unsafe {
			if self.egl.DestroyImageKHR.is_loaded() {
				self.egl.DestroyImageKHR(self.display, image);
			} else if self.egl.DestroyImage.is_loaded() {
				self
					.egl
					.DestroyImage(self.display, image as egl::types::EGLImage);
			}
		}
	}
}

impl Drop for GlContext {
	fn drop(&mut self) {
		let targets: Vec<_> = self.dmabuf_targets.drain().map(|(_, t)| t).collect();
		for target in targets {
			unsafe {
				self.glow.delete_framebuffer(target.framebuffer);
				self.glow.delete_texture(target.texture);
			}
			self.destroy_egl_image(target.egl_image);
		}

		unsafe {
			let _ = self.egl.MakeCurrent(
				self.display,
				egl::NO_SURFACE,
				egl::NO_SURFACE,
				egl::NO_CONTEXT,
			);
			if !self.context.is_null() {
				self.egl.DestroyContext(self.display, self.context);
			}
			if !self.display.is_null() {
				self.egl.Terminate(self.display);
			}
		}
	}
}

const DEFAULT_RENDER_NODES: &[&str] = &[
	"/dev/dri/renderD128",
	"/dev/dri/renderD129",
	"/dev/dri/renderD130",
	"/dev/dri/renderD131",
	"/dev/dri/renderD132",
	"/dev/dri/renderD133",
	"/dev/dri/renderD134",
	"/dev/dri/renderD135",
];

fn open_render_node_gbm_device(
	configured: Option<&Path>,
) -> Result<GbmDevice<std::fs::File>, GlError> {
	let mut last_error = None;
	for candidate in render_node_candidates(configured) {
		match OpenOptions::new().read(true).write(true).open(&candidate) {
			Ok(file) => match GbmDevice::new(file) {
				Ok(device) => return Ok(device),
				Err(err) => {
					last_error = Some(GlError::GbmInit(err.to_string()));
				}
			},
			Err(source) => {
				last_error = Some(GlError::RenderNodeOpen {
					path: candidate.clone(),
					source,
				});
			}
		}
	}
	Err(last_error.unwrap_or_else(|| GlError::GbmInit("no usable render nodes found".into())))
}

fn render_node_candidates(configured: Option<&Path>) -> Vec<PathBuf> {
	if let Some(path) = configured {
		vec![path.to_path_buf()]
	} else if let Ok(env) = std::env::var("TAB_CLIENT_RENDER_NODE") {
		vec![PathBuf::from(env)]
	} else {
		DEFAULT_RENDER_NODES
			.iter()
			.map(|p| PathBuf::from(p))
			.collect()
	}
}

fn load_proc_raw(
	egl: &egl::Egl,
	egl_lib: &libloading::Library,
	gl_lib: &libloading::Library,
	name: &str,
) -> Option<*const c_void> {
	let c_name = CString::new(name).ok()?;
	let egl_ptr = unsafe { egl.GetProcAddress(c_name.as_ptr()) };
	if !egl_ptr.is_null() {
		return Some(egl_ptr.cast());
	}
	if let Some(sym) = load_symbol(gl_lib, name) {
		return Some(sym);
	}
	if let Some(sym) = load_symbol(egl_lib, name) {
		return Some(sym);
	}
	None
}

fn choose_config(
	egl: &egl::Egl,
	display: egl::types::EGLDisplay,
	renderable_type: i32,
) -> Result<egl::types::EGLConfig, GlError> {
	let config_attribs = [
		egl::RENDERABLE_TYPE as i32,
		renderable_type,
		egl::RED_SIZE as i32,
		8,
		egl::GREEN_SIZE as i32,
		8,
		egl::BLUE_SIZE as i32,
		8,
		egl::ALPHA_SIZE as i32,
		8,
		egl::NONE as i32,
	];
	let mut config: egl::types::EGLConfig = ptr::null();
	let mut num_config = 0;
	let choose_ok = unsafe {
		egl.ChooseConfig(
			display,
			config_attribs.as_ptr(),
			&mut config as *mut egl::types::EGLConfig,
			1,
			&mut num_config,
		)
	};
	if choose_ok == 0 {
		return Err(GlError::ChooseConfigFailed(unsafe { egl.GetError() }));
	}
	if num_config <= 0 || config.is_null() {
		return Err(GlError::MissingConfig);
	}
	Ok(config)
}

fn load_symbol(lib: &libloading::Library, name: &str) -> Option<*const c_void> {
	let c_name = CString::new(name).ok()?;
	let symbol = unsafe { lib.get::<*const c_void>(c_name.as_bytes_with_nul()) }.ok()?;
	Some(*symbol)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RenderTargetKey {
	monitor_id: String,
	buffer_index: u8,
}

impl RenderTargetKey {
	fn new(monitor_id: &str, buffer_index: u8) -> Self {
		Self {
			monitor_id: monitor_id.to_string(),
			buffer_index,
		}
	}
}

struct DmabufTarget {
	egl_image: egl::types::EGLImageKHR,
	texture: glow::NativeTexture,
	framebuffer: glow::NativeFramebuffer,
}
