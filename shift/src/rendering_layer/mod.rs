#![allow(dead_code)]

pub mod channels;
pub mod dmabuf_import;
mod egl;
mod fence_scheduler;
mod ownership;
mod state;
mod surface_cache;

use easydrm::{
	EasyDRM,
	gl::{COLOR_BUFFER_BIT, DEPTH_BUFFER_BIT},
};
use skia_safe::gpu;
use std::{
	collections::HashMap,
	os::fd::{AsFd, FromRawFd, OwnedFd},
	sync::Arc,
	time::Duration,
};
#[cfg(debug_assertions)]
use std::{fs, time::Instant};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::warn;

use crate::{
	comms::{
		render2server::{RenderEvt, RenderEvtTx},
		server2render::{RenderCmd, RenderCmdRx},
	},
	monitor::{Monitor as ServerLayerMonitor, MonitorId},
	sessions::SessionId,
};
use channels::RenderingEnd;
use dmabuf_import::{DmaBufTexture, ImportParams as DmaBufImportParams, SkiaDmaBufTexture};
use fence_scheduler::{FenceScheduler, FenceTaskHandle, FenceWaitMode};
use ownership::OwnershipManager;
use state::{BufferSlot, FenceEvent, SlotKey, SlotOwner};
use surface_cache::{MonitorRenderState, current_framebuffer_binding};
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

	#[cfg(debug_assertions)]
	#[error("open fd guard exceeded: {count} > {limit}")]
	OpenFdGuardExceeded { count: usize, limit: usize },
}

// -----------------------------
// Rendering layer
// -----------------------------

pub struct RenderingLayer {
	drm: EasyDRM<MonitorRenderState>,
	gr: gpu::DirectContext,
	command_rx: Option<RenderCmdRx>,
	event_tx: RenderEvtTx,
	known_monitors: HashMap<MonitorId, ServerLayerMonitor>,
	ownership: OwnershipManager,
	slots: HashMap<SlotKey, SkiaDmaBufTexture>,
	fence_event_tx: mpsc::UnboundedSender<FenceEvent>,
	fence_event_rx: mpsc::UnboundedReceiver<FenceEvent>,
	fence_scheduler: FenceScheduler,
	fence_tasks: HashMap<SlotKey, FenceTaskHandle>,
	#[cfg(debug_assertions)]
	fd_guard_limit: usize,
	#[cfg(debug_assertions)]
	fd_guard_last_check: Instant,
}

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	pub fn init(channels: RenderingEnd) -> Result<Self, RenderError> {
		let (command_rx, event_tx) = channels.into_parts();
		let drm = EasyDRM::init(|req| {
			// O EasyDRM chama isto com o contexto do monitor já válido/current.
			MonitorRenderState::new(req).expect("MonitorRenderState::new failed")
		})?;
		drm.make_current().map_err(|_| RenderError::SkiaGlInterface)?;
		let interface =
			gpu::gl::Interface::new_load_with(|s| drm.get_proc_address(s)).ok_or(RenderError::SkiaGlInterface)?;
		let gr = gpu::direct_contexts::make_gl(interface, None).ok_or(RenderError::SkiaDirectContext)?;
		let (fence_event_tx, fence_event_rx) = mpsc::unbounded_channel();

		Ok(Self {
			drm,
			gr,
			command_rx: Some(command_rx),
			event_tx,
			known_monitors: HashMap::new(),
			ownership: OwnershipManager::new(),
			slots: HashMap::new(),
			fence_event_tx,
			fence_event_rx,
			fence_scheduler: FenceScheduler::new(),
			fence_tasks: HashMap::new(),
			#[cfg(debug_assertions)]
			fd_guard_limit: std::env::var("SHIFT_MAX_OPEN_FDS")
				.ok()
				.and_then(|v| v.parse::<usize>().ok())
				.unwrap_or(4096),
			#[cfg(debug_assertions)]
			fd_guard_last_check: Instant::now(),
		})
	}

	#[tracing::instrument(skip_all)]
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
		self.known_monitors = current.into_iter().map(|m| (m.id, m)).collect();

		'e: loop {
			#[cfg(debug_assertions)]
			self.check_open_fd_guard()?;
			// Mantém as surfaces a seguir ao tamanho real do monitor
				let monitor_ids: Vec<MonitorId> = self.drm.monitors().map(|mon| mon.context().id).collect();
				self.ownership.ensure_current_session_monitors(&monitor_ids);
				for mon in self.drm.monitors_mut() {
					if !mon.can_render() {
					continue;
				}
				if let Err(e) = mon.make_current() {
					warn!(monitor_id = %mon.context().id, "make_current failed: {e:?}");
					continue;
				}
				{
					unsafe {
						mon.gl().ClearColor(1.0, 0.0, 0.0, 1.0);
					}
					unsafe {
						mon.gl().Clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT);
					};

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
						if let Some(texture) = texture {
							if let Err(e) = context.draw_texture(&mut self.gr, texture) {
								warn!(%monitor_id, "failed to draw client texture: {e:?}");
							}
						}

						context.flush(&mut self.gr);
					}
				}
			let committed_any = {
				let page_flip_span = tracing::span!(tracing::Level::TRACE, "drm_page_flip_ioctl");
				let _page_flip_enter = page_flip_span.enter();

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
				committed_any
			};
				'l: loop {
					tokio::select! {
						cmd = command_rx.recv() => {
						if let Some(cmd) = cmd {
							if !self.handle_command(cmd).await? {
								break 'e;
							}
						} else {
							warn!("server→renderer channel closed, shutting down renderer");
							break 'e;
						}
					}
					result = self.drm.poll_events_async() => {
						result?;
						self.sync_monitors().await;
						break 'l;
					}
					fence_evt = self.fence_event_rx.recv() => {
						if let Some(fence_evt) = fence_evt {
							self.handle_fence_event(fence_evt).await;
						}
					}
						scheduler_ok = self.fence_scheduler.recv_and_run() => {
							if !scheduler_ok {
								warn!("fence scheduler channel closed");
							}
						}
						_ = tokio::time::sleep(Duration::from_millis(2)), if !committed_any => {
							// No commit happened this iteration, so there may be no pageflip event to wake us up.
							// Avoid stalling forever waiting on drm events that won't arrive.
							break 'l;
						}
					}
				}
			}
		warn!("shutting down renderer");
		Ok(())
	}

	#[cfg(debug_assertions)]
	fn check_open_fd_guard(&mut self) -> Result<(), RenderError> {
		const FD_GUARD_INTERVAL: Duration = Duration::from_secs(1);
		if self.fd_guard_last_check.elapsed() < FD_GUARD_INTERVAL {
			return Ok(());
		}
		self.fd_guard_last_check = Instant::now();

		let Ok(entries) = fs::read_dir("/proc/self/fd") else {
			return Ok(());
		};
		let count = entries.count();
		if count > self.fd_guard_limit {
			debug_assert!(
				count <= self.fd_guard_limit,
				"open fd guard exceeded: {count} > {}",
				self.fd_guard_limit
			);
			return Err(RenderError::OpenFdGuardExceeded {
				count,
				limit: self.fd_guard_limit,
			});
		}
		Ok(())
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

	#[tracing::instrument(skip_all)]
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
		let removed_ids = self
			.known_monitors
			.keys()
			.filter(|removed_id| !current_map.contains_key(removed_id))
			.copied()
			.collect::<Vec<_>>();
			for removed_id in removed_ids {
				self
					.emit_event(RenderEvt::MonitorOffline {
						monitor_id: removed_id,
					})
					.await;
				self.cleanup_monitor_slots(removed_id);
			}
			self.known_monitors = current_map;
		}

	pub fn drm_mut(&mut self) -> &mut EasyDRM<MonitorRenderState> {
		&mut self.drm
	}

		fn cleanup_monitor_slots(&mut self, monitor_id: MonitorId) {
			self.slots.retain(|key, _| key.monitor_id != monitor_id);
			self.ownership.cleanup_monitor(monitor_id);
			let remove = self
				.fence_tasks
				.keys()
			.filter(|key| key.monitor_id == monitor_id)
			.copied()
			.collect::<Vec<_>>();
		for key in remove {
			self.cancel_fence_wait(key);
		}
	}

		fn cleanup_session_slots(&mut self, session_id: SessionId) {
			self.slots.retain(|key, _| key.session_id != session_id);
			self.ownership.cleanup_session(session_id);
			let remove = self
				.fence_tasks
				.keys()
			.filter(|key| key.session_id == session_id)
			.copied()
			.collect::<Vec<_>>();
		for key in remove {
			self.cancel_fence_wait(key);
		}
	}

	#[tracing::instrument(skip_all, fields(session_id = %session_id, monitor_id = %payload.monitor_id))]
	fn import_framebuffers(
		&mut self,
		payload: tab_protocol::FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
		session_id: SessionId,
	) {
		let Ok(monitor_id) = payload.monitor_id.parse::<MonitorId>() else {
			warn!(monitor_id = %payload.monitor_id, "invalid monitor id in framebuffer link");
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
				warn!(%monitor_id, "failed to make monitor current: {e:?}");
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
						warn!(%monitor_id, ?slot, "failed to import dmabuf: {e:?}");
					}
				}
			}
			break;
		}

		if !found_monitor {
			warn!(%monitor_id, "framebuffer link for unknown monitor");
			return;
		}

			for (slot, texture) in imported {
				let key = SlotKey::new(monitor_id, session_id, slot);
				self.slots.insert(key, texture);
				self.ownership.set_owner(key, SlotOwner::Client);
			}
		}

		async fn process_deferred_releases(&mut self, release_fence: i32) {
			for item in self.ownership.take_deferred_releases() {
				let key = SlotKey::new(item.monitor_id, item.session_id, item.buffer);
				self.ownership.set_owner(key, SlotOwner::Client);
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
}

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	async fn handle_command(&mut self, cmd: RenderCmd) -> Result<bool, RenderError> {
		match cmd {
			RenderCmd::Shutdown => {
				warn!("received shutdown request from server");
				return Ok(false);
			}
			RenderCmd::FramebufferLink {
				payload,
				dma_bufs,
				session_id,
			} => {
				self.import_framebuffers(payload, dma_bufs, session_id);
			}
				RenderCmd::SetActiveSession { session_id } => {
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
						if let Some(state) = self.ownership.state(monitor_id, session_id)
							&& let Some(pending) = state.pending_buffer
						{
							let pending_key = SlotKey::new(monitor_id, session_id, pending);
							if pending_key != slot_key {
								self.cancel_fence_wait(pending_key);
								self.ownership
									.queue_buffer_release(monitor_id, session_id, pending);
							}
						}
					if let Some(fence_fd) = acquire_fence {
						self.spawn_acquire_fence_waiter(slot_key, fence_fd);
					} else {
						self.cancel_fence_wait(slot_key);
					}
						self.ownership.set_owner(slot_key, SlotOwner::Shift);
						let previous = {
							let state = self.ownership.state_entry(monitor_id, session_id);
							let previous = state.current_buffer;
							state.pending_buffer = Some(slot);
							if !has_acquire_fence {
								state.current_buffer = Some(slot);
								state.pending_buffer = None;
							}
							previous
						};
						if !has_acquire_fence {
							if let Some(previous) = previous.filter(|prev| *prev != slot) {
								self.ownership
									.queue_buffer_release(monitor_id, session_id, previous);
							}
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

	#[tracing::instrument(skip_all)]
	async fn emit_event(&self, event: RenderEvt) {
		if let Err(e) = self.event_tx.send(event).await {
			warn!("failed to send renderer event to server: {e}");
		}
	}

	fn cancel_fence_wait(&mut self, key: SlotKey) {
		if let Some(handle) = self.fence_tasks.remove(&key) {
			self.fence_scheduler.cancel(handle);
		}
	}

	fn spawn_acquire_fence_waiter(&mut self, key: SlotKey, fence_fd: OwnedFd) {
		if let Some(existing) = self.fence_tasks.get(&key).copied() {
			if let Ok(cloned_fd) = fence_fd.as_fd().try_clone_to_owned()
				&& self
					.fence_scheduler
					.reschedule(existing, vec![cloned_fd], FenceWaitMode::All)
			{
				return;
			}
			// Recover from unexpected scheduler/task-map desync.
			self.fence_tasks.remove(&key);
		}
		let tx = self.fence_event_tx.clone();
		let handle = self.fence_scheduler.schedule(
			vec![fence_fd],
			FenceWaitMode::All,
			Box::new(move || {
				let _ = tx.send(FenceEvent::Signaled { key });
			}),
		);
		self.fence_tasks.insert(key, handle);
	}

	async fn handle_fence_event(&mut self, event: FenceEvent) {
			match event {
				FenceEvent::Signaled { key } => {
					self.fence_tasks.remove(&key);
					if let Some(state) = self.ownership.state_mut(key.monitor_id, key.session_id) {
						if state.pending_buffer == Some(key.buffer) {
							let previous = state.current_buffer;
							state.current_buffer = Some(key.buffer);
							state.pending_buffer = None;
							if let Some(previous) = previous.filter(|prev| *prev != key.buffer) {
								self
									.ownership
									.queue_buffer_release(key.monitor_id, key.session_id, previous);
							}
						}
					}
			}
		}
	}
}
