#![allow(dead_code)]

pub mod channels;

use easydrm::{EasyDRM, Monitor, MonitorContextCreationRequest, gl};
use skia_safe::{self as skia, gpu, gpu::gl::FramebufferInfo};
use thiserror::Error;
use tracing::warn;
use std::collections::HashMap;

use crate::{
	comms::{
		render2server::{RenderEvt, RenderEvtTx},
		server2render::{RenderCmd, RenderCmdRx},
	},
	monitor::{Monitor as ServerLayerMonitor, MonitorId},
};
use channels::RenderingEnd;

// -----------------------------
// Errors
// -----------------------------

#[derive(Debug, Error)]
pub enum RenderError {
	#[error("easydrm error: {0}")]
	EasyDrmError(#[from] easydrm::EasyDRMError),

	#[error("skia GL interface creation failed")]
	SkiaGlInterface,

	#[error("skia DirectContext creation failed")]
	SkiaDirectContext,

	#[error("skia surface creation failed")]
	SkiaSurface,
}

// -----------------------------
// Per-monitor render state
// -----------------------------

pub struct MonitorRenderState {
	pub gr: gpu::DirectContext,
	pub surface: skia::Surface,
	pub width: usize,
	pub height: usize,
	pub gl: gl::Gles2,
	pub id: MonitorId,
}

impl MonitorRenderState {
	fn new(req: &MonitorContextCreationRequest<'_>) -> Result<Self, RenderError> {
		let interface = gpu::gl::Interface::new_load_with(|s| (req.get_proc_address)(s))
			.ok_or(RenderError::SkiaGlInterface)?;

		let mut gr =
			gpu::direct_contexts::make_gl(interface, None).ok_or(RenderError::SkiaDirectContext)?;

		let surface = skia_surface_for_current_fbo(&mut gr, req.gl, req.width, req.height)?;

		Ok(Self {
			gr,
			surface,
			width: req.width,
			height: req.height,
			gl: req.gl.clone(),
			id: MonitorId::rand(),
		})
	}

	fn ensure_surface_size(&mut self, width: usize, height: usize) -> Result<(), RenderError> {
		if self.width == width && self.height == height {
			return Ok(());
		}
		self.width = width;
		self.height = height;
		self.surface = skia_surface_for_current_fbo(&mut self.gr, &self.gl, width, height)?;
		Ok(())
	}

	pub fn canvas(&mut self) -> &skia::Canvas {
		self.surface.canvas()
	}

	pub fn flush(&mut self) {
		self.gr.flush_and_submit();
	}

	pub fn get_server_layer_monitor(monitor: &Monitor<Self>) -> ServerLayerMonitor {
		crate::monitor::Monitor {
			height: monitor.size().0 as _,
			width: monitor.size().1 as _,
			id: monitor.context().id,
			name: format!("Monitor {}", u32::from(monitor.connector_id())),
			refresh_rate: monitor.active_mode().vrefresh(),
		}
	}
}

// -----------------------------
// Rendering layer
// -----------------------------

pub struct RenderingLayer {
	drm: EasyDRM<MonitorRenderState>,
	command_rx: Option<RenderCmdRx>,
	event_tx: RenderEvtTx,
	known_monitors: HashMap<MonitorId, ServerLayerMonitor>,
}

impl RenderingLayer {
	pub fn init(channels: RenderingEnd) -> Result<Self, RenderError> {
		let (command_rx, event_tx) = channels.into_parts();
		let drm = EasyDRM::init(|req| {
			// O EasyDRM chama isto com o contexto do monitor já válido/current.
			MonitorRenderState::new(req).expect("MonitorRenderState::new failed")
		})?;

		Ok(Self {
			drm,
			command_rx: Some(command_rx),
			event_tx,
			known_monitors: HashMap::new(),
		})
	}

	pub async fn run(mut self) -> Result<(), RenderError> {
		let mut command_rx = self
			.command_rx
			.take()
			.expect("render command channel missing");
		let current = self.collect_monitors();
		self
			.emit_event(RenderEvt::Started {
				monitors: current.clone(),
			})
			.await;
		self.known_monitors = current
			.into_iter()
			.map(|m| (m.id, m))
			.collect();
		loop {
			// Mantém as surfaces a seguir ao tamanho real do monitor
			for mon in self.drm.monitors_mut() {
				if mon.can_render() && mon.make_current().is_ok() {
					let mode = mon.active_mode();
					let (w, h) = (mode.size().0 as usize, mode.size().1 as usize);
					mon.context_mut().ensure_surface_size(w, h)?;

					// Render stub (intencionalmente vazio por agora)
					// Quando fores começar a desenhar:
					let canvas = mon.context_mut().canvas();
					canvas.clear(skia::Color::BLACK);
					mon.context_mut().flush();
				}
			}
			self.drm.swap_buffers()?;
			tokio::select! {
				cmd = command_rx.recv() => {
					if let Some(cmd) = cmd {
						if !self.handle_command(cmd).await? {
							return Ok(());
						}
					} else {
						warn!("server→renderer channel closed, shutting down renderer");
						return Ok(());
					}
				}
				result = self.drm.poll_events_async() => {
					result?;
					self.on_after_poll_events().await;
				}
			}
		}
	}
	async fn on_after_poll_events(&mut self) {
		let page_flipped_monitors = self
			.drm
			.monitors()
			.filter(|m| m.can_render())
			.map(|m| m.context().id)
			.collect::<Vec<_>>();
		self
			.emit_event(RenderEvt::PageFlip {
				monitors: page_flipped_monitors,
			})
			.await;
		self.sync_monitors().await;
	}
	pub fn drm(&self) -> &EasyDRM<MonitorRenderState> {
		&self.drm
	}

	fn collect_monitors(&self) -> Vec<ServerLayerMonitor> {
		self
			.drm
			.monitors()
			.map(MonitorRenderState::get_server_layer_monitor)
			.collect()
	}

	async fn sync_monitors(&mut self) {
		let current_list = self.collect_monitors();
		let mut current_map = HashMap::new();
		for monitor in current_list {
			if !self.known_monitors.contains_key(&monitor.id) {
				self
					.emit_event(RenderEvt::MonitorOnline {
						monitor: monitor.clone(),
					})
					.await;
			}
			current_map.insert(monitor.id, monitor);
		}
		for (removed_id, _) in self.known_monitors.iter() {
			if !current_map.contains_key(removed_id) {
				self
					.emit_event(RenderEvt::MonitorOffline {
						monitor_id: *removed_id,
					})
					.await;
			}
		}
		self.known_monitors = current_map;
	}

	pub fn drm_mut(&mut self) -> &mut EasyDRM<MonitorRenderState> {
		&mut self.drm
	}
}

impl RenderingLayer {
	async fn handle_command(&mut self, cmd: RenderCmd) -> Result<bool, RenderError> {
		match cmd {
			RenderCmd::Shutdown => {
				warn!("received shutdown request from server");
				return Ok(false);
			}
			RenderCmd::FramebufferLink { payload, .. } => {
				warn!(
					?payload,
					"framebuffer link command received but renderer handling is not implemented yet"
				);
			}
			RenderCmd::SwapBuffers { monitor_id, buffer } => {
				warn!(%monitor_id, ?buffer, "swap buffers command received but renderer handling is not implemented yet");
			}
		}

		Ok(true)
	}

	async fn emit_event(&self, event: RenderEvt) {
		if let Err(e) = self.event_tx.send(event).await {
			warn!("failed to send renderer event to server: {e}");
		}
	}
}

// -----------------------------
// Skia surface helper
// -----------------------------

fn skia_surface_for_current_fbo(
	gr: &mut gpu::DirectContext,
	gl: &gl::Gles2,
	width: usize,
	height: usize,
) -> Result<skia::Surface, RenderError> {
	let mut fbo: i32 = 0;
	unsafe {
		gl.GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fbo);
	}

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
		gpu::SurfaceOrigin::BottomLeft,
		skia::ColorType::RGBA8888,
		None,
		None,
	)
	.ok_or(RenderError::SkiaSurface)
}
