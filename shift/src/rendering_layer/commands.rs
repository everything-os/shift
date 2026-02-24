use std::{
	os::fd::{FromRawFd, OwnedFd},
	sync::Arc,
};

use crate::comms::server2render::RenderCmd;

use super::dmabuf_import::{DmaBufTexture, ImportParams as DmaBufImportParams};
use super::state::BufferSlot;
use super::{RenderError, RenderEvt, RenderingLayer, SlotKey};

impl RenderingLayer {
	#[tracing::instrument(skip_all, fields(session_id = %session_id, monitor_id = %payload.monitor_id))]
	pub(super) fn import_framebuffers(
		&mut self,
		payload: tab_protocol::FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
		session_id: crate::sessions::SessionId,
	) {
		let Ok(monitor_id) = payload.monitor_id.parse::<crate::monitor::MonitorId>() else {
			tracing::warn!(monitor_id = %payload.monitor_id, "invalid monitor id in framebuffer link");
			return;
		};

		let mut imported = Vec::new();
		let mut found_monitor = false;
		let egl_context = self.drm.egl_context();
		for mon in self.drm.monitors_mut() {
			if mon.context().id != monitor_id {
				continue;
			}
			found_monitor = true;
			if let Err(e) = mon.make_current() {
				tracing::warn!(%monitor_id, "failed to make monitor current: {e:?}");
				break;
			}
			let gl = mon.context().gl.clone();
			let proc_loader = |symbol: &str| {
				egl_context
					.lock()
					.map(|ctx| ctx.get_proc_address(symbol))
					.unwrap_or(std::ptr::null())
			};
			for (idx, fd) in dma_bufs.into_iter().enumerate() {
				let Some(slot) = BufferSlot::from_index(idx) else {
					continue;
				};
				let params = DmaBufImportParams {
					width: payload.width,
					height: payload.height,
					stride: payload.stride,
					offset: payload.offset,
					fourcc: payload.fourcc,
					fd,
				};
				match DmaBufTexture::import(&gl, &proc_loader, params).and_then(|texture| {
					texture.to_skia(format!(
						"session_{}_monitor_{}_buffer_{}",
						session_id, monitor_id, idx
					))
				}) {
					Ok(texture) => imported.push((slot, texture)),
					Err(e) => {
						tracing::warn!(%monitor_id, ?slot, "failed to import dmabuf: {e:?}");
					}
				}
			}
			break;
		}

		if !found_monitor {
			tracing::warn!(%monitor_id, "framebuffer link for unknown monitor");
			return;
		}

		for (slot, texture) in imported {
			let key = SlotKey::new(monitor_id, session_id, slot);
			self.slots.insert(key, texture);
			self.ownership.mark_slot_client_owned(key);
		}
	}

	pub(super) async fn process_deferred_releases(&mut self, release_fence: i32) {
		for item in self.ownership.take_deferred_releases() {
			let key = SlotKey::new(item.monitor_id, item.session_id, item.buffer);
			self.ownership.mark_slot_client_owned(key);
			let release_fence = if release_fence >= 0 {
				let dup_fd = unsafe { libc::dup(release_fence) };
				if dup_fd >= 0 {
					Some(unsafe { OwnedFd::from_raw_fd(dup_fd) })
				} else {
					None
				}
			} else {
				None
			};
			self
				.emit_event(RenderEvt::BufferConsumed {
					session_id: item.session_id,
					monitor_id: item.monitor_id,
					buffer: item.buffer.into(),
					release_fence,
				})
				.await;
		}
	}

	#[tracing::instrument(skip_all)]
	pub(super) async fn handle_command(&mut self, cmd: RenderCmd) -> Result<bool, RenderError> {
		match cmd {
			RenderCmd::Shutdown => {
				tracing::warn!("received shutdown request from server");
				return Ok(false);
			}
			RenderCmd::FramebufferLink {
				payload,
				dma_bufs,
				session_id,
			} => {
				self.import_framebuffers(payload, dma_bufs, session_id);
			}
			RenderCmd::SetActiveSession {
				session_id,
				transition,
			} => {
				self.active_transition = None;
				if let Some(to_session_id) = session_id
					&& let Some(transition) = transition
				{
					self.active_transition = super::ActiveTransition::from_cmd(to_session_id, transition);
				}
				self.ownership.set_current_session(session_id);
			}
			RenderCmd::SessionRemoved { session_id } => {
				self.cleanup_session_slots(session_id);
				if self.ownership.current_session() == Some(session_id) {
					self.ownership.set_current_session(None);
				}
			}
			RenderCmd::SwapBuffers {
				monitor_id,
				buffer,
				session_id,
				acquire_fence,
			} => {
				let slot = BufferSlot::from(buffer);
				let monitor_known = self.known_monitors.contains_key(&monitor_id);
				let slot_key = SlotKey::new(monitor_id, session_id, slot);
				let slot_known = self.slots.contains_key(&slot_key);
				if !monitor_known || !slot_known {
					let reason: Arc<str> = if !monitor_known {
						"unknown_monitor"
					} else {
						"unlinked_buffer"
					}
					.into();
					self
						.emit_event(RenderEvt::BufferRequestRejected {
							session_id,
							monitor_id,
							buffer,
							reason,
						})
						.await;
				} else {
					let has_acquire_fence = acquire_fence.is_some();
					let transition =
						self
							.ownership
							.apply_swap_request(monitor_id, session_id, slot, has_acquire_fence);
					if let Some(pending) = transition.canceled_pending {
						let pending_key = SlotKey::new(monitor_id, session_id, pending);
						self.cancel_fence_wait(pending_key);
						self
							.ownership
							.queue_buffer_release(monitor_id, session_id, pending);
					}
					if let Some(fence_fd) = acquire_fence {
						self.spawn_acquire_fence_waiter(slot_key, fence_fd);
					} else {
						self.cancel_fence_wait(slot_key);
					}
					if let Some(previous) = transition.previous_to_release {
						self
							.ownership
							.queue_buffer_release(monitor_id, session_id, previous);
					}
					self
						.emit_event(RenderEvt::BufferRequestAck {
							session_id,
							monitor_id,
							buffer,
						})
						.await;
				}
			}
		}

		Ok(true)
	}
}
