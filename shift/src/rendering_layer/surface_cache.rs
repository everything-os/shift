use std::collections::HashMap;

use easydrm::{Monitor, MonitorContextCreationRequest, gl};
use skia_safe::{
	self as skia, FilterMode, MipmapMode, Paint, SamplingOptions, gpu, gpu::gl::FramebufferInfo,
};

use crate::monitor::{Monitor as ServerLayerMonitor, MonitorId};

use super::{RenderError, dmabuf_import::SkiaDmaBufTexture};

pub struct MonitorRenderState {
	pub surfaces_by_fbo: HashMap<i32, skia::Surface>,
	pub width: usize,
	pub height: usize,
	pub target_fbo: i32,
	pub gl: gl::Gles2,
	pub id: MonitorId,
}

impl MonitorRenderState {
	#[tracing::instrument(skip_all)]
	pub fn new(req: &MonitorContextCreationRequest<'_>) -> Result<Self, RenderError> {
		let target_fbo = current_framebuffer_binding(req.gl);

		Ok(Self {
			surfaces_by_fbo: HashMap::new(),
			width: req.width,
			height: req.height,
			target_fbo,
			gl: req.gl.clone(),
			id: MonitorId::rand(),
		})
	}

	#[tracing::instrument(skip_all, fields(width = width, height = height, fbo = fbo))]
	pub fn ensure_surface_target(
		&mut self,
		gr: &mut gpu::DirectContext,
		width: usize,
		height: usize,
		fbo: i32,
	) -> Result<(), RenderError> {
		let size_changed = self.width != width || self.height != height;
		if size_changed {
			self.surfaces_by_fbo.clear();
			self.width = width;
			self.height = height;
		}
		self.target_fbo = fbo;
		if !self.surfaces_by_fbo.contains_key(&fbo) {
			self
				.surfaces_by_fbo
				.insert(fbo, skia_surface_for_fbo(gr, width, height, fbo)?);
		}
		Ok(())
	}

	pub fn canvas(&mut self) -> &skia::Canvas {
		self
			.surfaces_by_fbo
			.get_mut(&self.target_fbo)
			.expect("active target fbo surface missing")
			.canvas()
	}

	pub fn flush(&mut self, gr: &mut gpu::DirectContext) {
		gr.flush(None);
	}

	pub fn get_server_layer_monitor(monitor: &Monitor<Self>) -> ServerLayerMonitor {
		crate::monitor::Monitor {
			height: monitor.size().1 as _,
			width: monitor.size().0 as _,
			id: monitor.context().id,
			name: format!("Monitor {}", u32::from(monitor.connector_id())),
			refresh_rate: monitor.active_mode().vrefresh(),
		}
	}

	#[tracing::instrument(skip_all, fields(monitor_id = %self.id))]
	pub fn draw_texture(
		&mut self,
		gr: &mut gpu::DirectContext,
		texture: &mut SkiaDmaBufTexture,
	) -> Result<(), RenderError> {
		let Some(image) = texture.image(gr) else {
			return Err(RenderError::SkiaSurface);
		};
		let rect = skia::Rect::from_wh(self.width as f32, self.height as f32);
		let sampling = SamplingOptions::new(FilterMode::Nearest, MipmapMode::Nearest);
		let mut paint = Paint::default();
		paint.set_argb(255, 255, 255, 255);
		self
			.canvas()
			.draw_image_rect_with_sampling_options(image, None, rect, sampling, &paint);
		Ok(())
	}
}

fn skia_surface_for_fbo(
	gr: &mut gpu::DirectContext,
	width: usize,
	height: usize,
	fbo: i32,
) -> Result<skia::Surface, RenderError> {
	let fb_info = FramebufferInfo {
		fboid: fbo as u32,
		format: gpu::gl::Format::RGBA8.into(),
		protected: gpu::Protected::No,
	};

	let backend_rt = gpu::backend_render_targets::make_gl(
		(width as i32, height as i32),
		0, // samples
		8, // stencil
		fb_info,
	);

	gpu::surfaces::wrap_backend_render_target(
		gr,
		&backend_rt,
		gpu::SurfaceOrigin::TopLeft,
		skia::ColorType::RGBA8888,
		None,
		None,
	)
	.ok_or(RenderError::SkiaSurface)
}

pub fn current_framebuffer_binding(gl: &gl::Gles2) -> i32 {
	let mut fbo: i32 = 0;
	unsafe {
		gl.GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fbo);
	}
	fbo
}
