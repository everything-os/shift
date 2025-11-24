use std::collections::HashMap;

use easydrm::{EasyDRM, Monitor, gl};

use crate::dma_buf_importer::ExternalTexture;
use crate::error::{FrameAck, RenderError};
use crate::output::OutputContext;
use tab_server::{MonitorRenderSnapshot, RenderSnapshot, RenderTransition};
use tracing::info;

pub struct FramePresenter;

impl FramePresenter {
	pub fn new() -> Self {
		Self
	}

	pub fn render(
		&self,
		snapshot: &RenderSnapshot<'_, ExternalTexture>,
		easydrm: &mut EasyDRM<OutputContext>,
	) -> Result<FrameAck, RenderError> {
		let mut rendered = Vec::new();
		let monitor_lookup: HashMap<_, _> = snapshot
			.monitors
			.iter()
			.map(|m| (m.monitor_id, m))
			.collect();
		for monitor in easydrm.monitors_mut() {
			if !monitor.can_render() {
				continue;
			}
			let monitor_id = match monitor.context().monitor_id() {
				Some(id) => id.to_string(),
				None => continue,
			};
			let Some(snapshot_monitor) = monitor_lookup.get(monitor_id.as_str()) else {
				continue;
			};
			let sessions = render_single_monitor(
				monitor_id.as_str(),
				monitor,
				snapshot_monitor,
				snapshot.transition.as_ref(),
				snapshot.active_session_id,
			)?;
			for session_id in sessions {
				rendered.push((monitor_id.clone(), session_id));
			}
		}
		Ok(rendered)
	}
}

fn render_single_monitor(
	monitor_id: &str,
	monitor: &mut Monitor<OutputContext>,
	snapshot_monitor: &MonitorRenderSnapshot<'_, ExternalTexture>,
	transition: Option<&RenderTransition>,
	active_session_id: Option<&str>,
) -> Result<Vec<String>, RenderError> {
	let mut presented = Vec::new();
	let mut primary = None;
	let mut secondary = None;
	let mut mix = 0.0f32;
	let previous_session_id = transition.and_then(|t| t.previous_session_id);
	if let Some(trans) = transition {
		if trans.progress < 1.0 {
			if let (Some(prev_tex), Some(new_tex), Some(prev_id), Some(new_id)) = (
				snapshot_monitor.previous_texture,
				snapshot_monitor.active_texture,
				trans.previous_session_id,
				active_session_id,
			) {
				primary = Some(prev_tex);
				secondary = Some(new_tex);
				mix = trans.progress.clamp(0.0, 1.0) as f32;
				presented.push(prev_id.to_string());
				if prev_id != new_id {
					presented.push(new_id.to_string());
				}
			}
		}
	}
	if primary.is_none() {
		if let (Some(texture), Some(session_id)) = (snapshot_monitor.active_texture, active_session_id)
		{
			primary = Some(texture);
			if !presented.iter().any(|s| s == session_id) {
				presented.push(session_id.to_string());
			}
		}
	}
	if primary.is_none() {
		if let (Some(texture), Some(session_id)) =
			(snapshot_monitor.previous_texture, previous_session_id)
		{
			primary = Some(texture);
			if !presented.iter().any(|s| s == session_id) {
				presented.push(session_id.to_string());
			}
		}
	}
	if primary.is_none() {
		clear_monitor(monitor)?;
		return Ok(Vec::new());
	}
	monitor
		.make_current()
		.map_err(|e| RenderError::MakeCurrent(e.to_string()))?;
	let (width, height) = monitor.size();
	let gl = monitor.gl();
	gl!(gl, Viewport(0, 0, width as i32, height as i32));
	gl!(gl, ClearColor(0.3, 0.3, 0.3, 1.0));
	gl!(gl, Clear(gl::COLOR_BUFFER_BIT));
	monitor
		.context()
		.renderer
		.draw(primary.unwrap(), secondary, mix);
	if let Some(fps) = monitor.context_mut().record_frame() {
		info!(monitor_id = monitor_id, fps = fps, "Shift FPS");
	}
	Ok(presented)
}

fn clear_monitor(monitor: &mut Monitor<OutputContext>) -> Result<(), RenderError> {
	monitor
		.make_current()
		.map_err(|e| RenderError::MakeCurrent(e.to_string()))?;
	let (width, height) = monitor.size();
	let gl = monitor.gl();
	gl!(gl, Viewport(0, 0, width as i32, height as i32));
	gl!(gl, ClearColor(0.3, 0.3, 0.3, 1.0));
	gl!(gl, Clear(gl::COLOR_BUFFER_BIT));
	Ok(())
}
