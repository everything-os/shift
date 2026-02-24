use easydrm::gl::{COLOR_BUFFER_BIT, DEPTH_BUFFER_BIT};
use skia_safe::{FilterMode, MipmapMode, Paint, SamplingOptions};
use std::collections::HashMap;
use tracing::warn;

use super::state::SlotOwner;
use super::{RenderError, RenderEvt, RenderingLayer, current_framebuffer_binding};
use super::{SkiaDmaBufTexture, SlotKey};

impl RenderingLayer {
	fn slot_image(
		slots: &mut HashMap<SlotKey, SkiaDmaBufTexture>,
		gr: &mut skia_safe::gpu::DirectContext,
		key: SlotKey,
	) -> Option<skia_safe::Image> {
		let texture = slots.get_mut(&key)?;
		texture.image(gr).cloned()
	}

	fn draw_image_fullscreen(context: &mut super::MonitorRenderState, image: &skia_safe::Image) {
		let rect = skia_safe::Rect::from_wh(context.width as f32, context.height as f32);
		let sampling = SamplingOptions::new(FilterMode::Nearest, MipmapMode::Nearest);
		let mut paint = Paint::default();
		paint.set_argb(255, 255, 255, 255);
		context
			.canvas()
			.draw_image_rect_with_sampling_options(image, None, rect, sampling, &paint);
	}

	pub(super) fn draw_ready_monitors(&mut self) -> Result<(), RenderError> {
		let monitor_ids: Vec<_> = self.drm.monitors().map(|mon| mon.context().id).collect();
		self.ownership.ensure_current_session_monitors(&monitor_ids);
		let now = std::time::Instant::now();
		let transition_snapshot = self.active_transition.clone();
		let transition_done = transition_snapshot
			.as_ref()
			.map(|transition| transition.progress(now) >= 1.0)
			.unwrap_or(false);

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

			let mut drew = false;
			if let Some(transition) = transition_snapshot.as_ref()
				&& let Some(animation) = self.animations.get(&transition.animation)
			{
				let old_key = self
					.ownership
					.current_slot_key_for_session(monitor_id, transition.from_session_id);
				let new_key = self
					.ownership
					.current_slot_key_for_session(monitor_id, transition.to_session_id);
				let old_image = old_key
					.filter(|key| self.ownership.owner(*key) == Some(SlotOwner::ShiftOwned))
					.and_then(|key| Self::slot_image(&mut self.slots, &mut self.gr, key));
				let new_image = new_key
					.filter(|key| self.ownership.owner(*key) == Some(SlotOwner::ShiftOwned))
					.and_then(|key| Self::slot_image(&mut self.slots, &mut self.gr, key));
				match (old_image, new_image) {
					(Some(old_image), Some(new_image)) => {
						let width = context.width as f32;
						let height = context.height as f32;
						animation.draw(
							context.canvas(),
							&old_image,
							&new_image,
							transition.progress(now),
							width,
							height,
						);
						drew = true;
					}
					(_, Some(new_image)) => {
						Self::draw_image_fullscreen(context, &new_image);
						drew = true;
					}
					_ => {}
				}
			}

			if !drew {
				let key = self.ownership.current_slot_key(monitor_id);
				let image = key
					.filter(|key| self.ownership.owner(*key) == Some(SlotOwner::ShiftOwned))
					.and_then(|key| Self::slot_image(&mut self.slots, &mut self.gr, key));
				if let Some(image) = image {
					Self::draw_image_fullscreen(context, &image);
				}
			}

			context.flush(&mut self.gr);
		}

		if transition_done {
			self.active_transition = None;
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
		self
			.process_deferred_releases(swap_result.render_fence)
			.await;
		self
			.emit_event(RenderEvt::PageFlip {
				monitors: page_flipped_monitors,
			})
			.await;

		Ok(committed_any)
	}
}
