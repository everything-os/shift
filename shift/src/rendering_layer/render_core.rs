use easydrm::gl::{COLOR_BUFFER_BIT, DEPTH_BUFFER_BIT};
use tracing::warn;

use super::{RenderError, RenderEvt, RenderingLayer, current_framebuffer_binding};
use super::state::SlotOwner;

impl RenderingLayer {
	pub(super) fn draw_ready_monitors(&mut self) -> Result<(), RenderError> {
		let monitor_ids: Vec<_> = self.drm.monitors().map(|mon| mon.context().id).collect();
		self.ownership.ensure_current_session_monitors(&monitor_ids);

		for mon in self.drm.monitors_mut() {
			if !mon.can_render() {
				continue;
			}
			if let Err(e) = mon.make_current() {
				warn!(monitor_id = %mon.context().id, "make_current failed: {e:?}");
				continue;
			}

			unsafe {
				mon.gl().ClearColor(1.0, 0.0, 0.0, 1.0);
				mon.gl().Clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT);
			}

			let monitor_id = mon.context().id;
			let mode = mon.active_mode();
			let (w, h) = (mode.size().0 as usize, mode.size().1 as usize);
			let context = mon.context_mut();
			let target_fbo = current_framebuffer_binding(&context.gl);
			context.ensure_surface_target(&mut self.gr, w, h, target_fbo)?;

			let key = self.ownership.current_slot_key(monitor_id);
			let texture = key.and_then(|key| {
				if self.ownership.owner(key) != Some(SlotOwner::Shift) {
					return None;
				}
				self.slots.get_mut(&key)
			});

			if let Some(texture) = texture
				&& let Err(e) = context.draw_texture(&mut self.gr, texture)
			{
				warn!(%monitor_id, "failed to draw client texture: {e:?}");
			}

			context.flush(&mut self.gr);
		}

		Ok(())
	}

	pub(super) async fn render_and_commit(&mut self) -> Result<bool, RenderError> {
		self.draw_ready_monitors()?;

		let page_flipped_monitors = self
			.drm
			.monitors()
			.filter(|m| m.was_drawn())
			.map(|m| m.context().id)
			.collect::<Vec<_>>();

		let swap_result = self.drm.swap_buffers_with_result()?;
		let committed_any = !swap_result.committed_connectors.is_empty();
		self.process_deferred_releases(swap_result.render_fence).await;
		self
			.emit_event(RenderEvt::PageFlip {
				monitors: page_flipped_monitors,
			})
			.await;

		Ok(committed_any)
	}
}
