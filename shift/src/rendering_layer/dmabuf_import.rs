#![allow(dead_code)]

use std::{
	ffi::c_void,
	os::fd::{IntoRawFd, OwnedFd},
};

use easydrm::gl;
use nix::unistd::close;
use skia_safe::{Image, gpu};
use thiserror::Error;

use crate::rendering_layer::egl;

/// Metadata required to import a client-provided dmabuf as a GL texture.
#[derive(Debug)]
pub struct ImportParams {
	pub width: i32,
	pub height: i32,
	pub stride: i32,
	pub offset: i32,
	pub fourcc: i32,
	pub fd: OwnedFd,
}

#[derive(Debug, Error)]
pub enum DmaBufImportError {
	#[error("required EGL extension is unavailable")]
	EglLoadFailed,
	#[error("no current EGL display")]
	MissingDisplay,
	#[error("no current EGL context")]
	MissingContext,
	#[error("eglCreateImageKHR failed (error={0:#X})")]
	ImageCreationFailed(i32),
	#[error("failed to create GL texture")]
	TextureAllocationFailed,
	#[error("glEGLImageTargetTexture2DOES failed (error={0:#X})")]
	ImageBindFailed(u32),
}

/// RAII wrapper owning the imported GL texture + EGL image.
pub struct DmaBufTexture {
	gl: gl::Gles2,
	egl: egl::Egl,
	display: egl::types::EGLDisplay,
	image: egl::types::EGLImageKHR,
	texture_id: gl::types::GLuint,
	pub width: i32,
	pub height: i32,
	pub fourcc: i32,
}

impl DmaBufTexture {
	#[tracing::instrument(skip_all, fields(width = params.width, height = params.height, fourcc = params.fourcc))]
	pub fn import(
		gl: &gl::Gles2,
		proc_resolver: &dyn Fn(&str) -> *const c_void,
		params: ImportParams,
	) -> Result<Self, DmaBufImportError> {
		let resolver = |name: &'static str| (proc_resolver)(name);
		let egl = egl::Egl::load_with(|name| resolver(name));
		if !(egl.CreateImageKHR.is_loaded() && egl.DestroyImageKHR.is_loaded()) {
			return Err(DmaBufImportError::EglLoadFailed);
		}

		let display = unsafe { egl.GetCurrentDisplay() };
		if display.is_null() {
			return Err(DmaBufImportError::MissingDisplay);
		}
		let context = unsafe { egl.GetCurrentContext() };
		if context.is_null() {
			return Err(DmaBufImportError::MissingContext);
		}
		let raw_fd = params.fd.into_raw_fd();
		let mut attrs = [
			egl::LINUX_DRM_FOURCC_EXT as i32,
			params.fourcc,
			egl::DMA_BUF_PLANE0_FD_EXT as i32,
			raw_fd,
			egl::DMA_BUF_PLANE0_OFFSET_EXT as i32,
			params.offset,
			egl::DMA_BUF_PLANE0_PITCH_EXT as i32,
			params.stride,
			egl::WIDTH as i32,
			params.width,
			egl::HEIGHT as i32,
			params.height,
			egl::NONE as i32,
		];

		let image = unsafe {
			egl.CreateImageKHR(
				display,
				std::ptr::null(),
				egl::LINUX_DMA_BUF_EXT,
				std::ptr::null(),
				attrs.as_mut_ptr(),
			)
		};

		let _ = close(raw_fd);

		if image.is_null() {
			let egl_error = unsafe { egl.GetError() };
			return Err(DmaBufImportError::ImageCreationFailed(egl_error));
		}

		let mut texture = 0;
		unsafe {
			gl.GenTextures(1, &mut texture);
		}
		if texture == 0 {
			unsafe {
				egl.DestroyImageKHR(display, image);
			}
			return Err(DmaBufImportError::TextureAllocationFailed);
		}

		unsafe {
			gl.BindTexture(gl::TEXTURE_2D, texture);
			gl.TexParameteri(
				gl::TEXTURE_2D,
				gl::TEXTURE_MIN_FILTER,
				gl::LINEAR.try_into().unwrap(),
			);
			gl.TexParameteri(
				gl::TEXTURE_2D,
				gl::TEXTURE_MAG_FILTER,
				gl::LINEAR.try_into().unwrap(),
			);
			gl.TexParameteri(
				gl::TEXTURE_2D,
				gl::TEXTURE_WRAP_S,
				gl::CLAMP_TO_EDGE.try_into().unwrap(),
			);
			gl.TexParameteri(
				gl::TEXTURE_2D,
				gl::TEXTURE_WRAP_T,
				gl::CLAMP_TO_EDGE.try_into().unwrap(),
			);
			gl.EGLImageTargetTexture2DOES(gl::TEXTURE_2D, image.cast());
		}

		let gl_error = unsafe { gl.GetError() };
		if gl_error != gl::NO_ERROR {
			unsafe {
				gl.DeleteTextures(1, &texture);
				egl.DestroyImageKHR(display, image);
			}
			return Err(DmaBufImportError::ImageBindFailed(gl_error));
		}
		Ok(Self {
			gl: gl.clone(),
			egl,
			display,
			image,
			texture_id: texture,
			width: params.width,
			height: params.height,
			fourcc: params.fourcc,
		})
	}
	fn skia_tex_info(&self) -> gpu::gl::TextureInfo {
		gpu::gl::TextureInfo {
			target: gl::TEXTURE_2D as gpu::gl::Enum,
			id: self.texture_id as gpu::gl::Enum,
			format: gpu::gl::Format::RGBA8.into(),
			protected: gpu::Protected::No,
		}
	}
	#[tracing::instrument(skip_all)]
	pub fn to_skia(self, label: impl AsRef<str>) -> Result<SkiaDmaBufTexture, DmaBufImportError> {
		let backend_texture = unsafe {
			gpu::backend_textures::make_gl(
				(self.width, self.height),
				gpu::Mipmapped::No,
				self.skia_tex_info(),
				label,
			)
		};

		Ok(SkiaDmaBufTexture {
			backend_texture,
			source: self,
			cached_image: None,
		})
	}
}

impl Drop for DmaBufTexture {
	fn drop(&mut self) {
		unsafe {
			self.gl.DeleteTextures(1, &self.texture_id);
			if !self.image.is_null() {
				self.egl.DestroyImageKHR(self.display, self.image);
			}
		}
	}
}

/// Helper struct that keeps the GL/EGL resources alive for as long as Skia needs them.
pub struct SkiaDmaBufTexture {
	pub backend_texture: gpu::BackendTexture,
	source: DmaBufTexture,
	cached_image: Option<Image>,
}

impl SkiaDmaBufTexture {
	pub fn texture(&self) -> &gpu::BackendTexture {
		&self.backend_texture
	}

	pub fn image<'a>(&'a mut self, gr: &mut gpu::DirectContext) -> Option<&'a Image> {
		if self.cached_image.is_none() {
			self.cached_image = Image::from_texture(
				gr,
				&self.backend_texture,
				gpu::SurfaceOrigin::TopLeft,
				skia_safe::ColorType::RGBA8888,
				skia_safe::AlphaType::Opaque,
				None,
			);
		}
		self.cached_image.as_ref()
	}
	/// Splits into the skia texture and inner opengl texture
	///
	/// # Safety
	/// The caller is responsible for keeping `DmaBufTexture` alive while `BackendTexture` is alive.
	pub unsafe fn into_inner(self) -> (gpu::BackendTexture, DmaBufTexture) {
		(self.backend_texture, self.source)
	}

	pub fn update(&mut self) {
		self.backend_texture = unsafe {
			gpu::backend_textures::make_gl(
				(self.source.width, self.source.height),
				gpu::Mipmapped::No,
				self.source.skia_tex_info(),
				self.backend_texture.label(),
			)
		};
		self.cached_image = None;
	}

	pub fn gl_texture_id(&self) -> gl::types::GLuint {
		self.source.texture_id
	}
}
