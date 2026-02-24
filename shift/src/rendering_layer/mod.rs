#![allow(dead_code)]

mod animation;
pub mod channels;
mod commands;
pub mod dmabuf_import;
mod egl;
mod fence_runtime;
mod fence_scheduler;
mod ownership;
mod render_core;
mod state;
mod surface_cache;

use easydrm::EasyDRM;
use skia_safe::gpu;
use std::{
	collections::HashMap,
	time::{Duration, Instant as StdInstant},
};
#[cfg(debug_assertions)]
use std::{fs, time::Instant};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::warn;

use crate::comms::server2render::SessionTransition;
use crate::{
	comms::{
		render2server::{RenderEvt, RenderEvtTx},
		server2render::RenderCmdRx,
	},
	monitor::{Monitor as ServerLayerMonitor, MonitorId},
	sessions::SessionId,
};
use animation::AnimationRegistry;
use channels::RenderingEnd;
use dmabuf_import::SkiaDmaBufTexture;
use fence_scheduler::{FenceScheduler, FenceTaskHandle, FenceWaitMode};
use ownership::OwnershipManager;
use state::{FenceEvent, SlotKey};
use surface_cache::{MonitorRenderState, current_framebuffer_binding};

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
	animations: AnimationRegistry,
	active_transition: Option<ActiveTransition>,
	#[cfg(debug_assertions)]
	fd_guard_limit: usize,
	#[cfg(debug_assertions)]
	fd_guard_last_check: Instant,
}

#[derive(Debug, Clone)]
struct ActiveTransition {
	from_session_id: SessionId,
	to_session_id: SessionId,
	animation: String,
	started_at: StdInstant,
	duration: Duration,
}

impl ActiveTransition {
	fn from_cmd(to_session_id: SessionId, transition: SessionTransition) -> Option<Self> {
		if transition.duration.is_zero() {
			return None;
		}
		Some(Self {
			from_session_id: transition.from_session_id,
			to_session_id,
			animation: transition.animation,
			started_at: StdInstant::now(),
			duration: transition.duration,
		})
	}

	fn progress(&self, now: StdInstant) -> f64 {
		if self.duration.is_zero() {
			return 1.0;
		}
		let elapsed = now.saturating_duration_since(self.started_at);
		(elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
	}
}

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	pub fn init(channels: RenderingEnd) -> Result<Self, RenderError> {
		let (command_rx, event_tx) = channels.into_parts();
		let drm =
			EasyDRM::init(|req| MonitorRenderState::new(req).expect("MonitorRenderState::new failed"))?;
		drm
			.make_current()
			.map_err(|_| RenderError::SkiaGlInterface)?;
		let interface = gpu::gl::Interface::new_load_with(|s| drm.get_proc_address(s))
			.ok_or(RenderError::SkiaGlInterface)?;
		let gr =
			gpu::direct_contexts::make_gl(interface, None).ok_or(RenderError::SkiaDirectContext)?;
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
			animations: AnimationRegistry::new(),
			active_transition: None,
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
			let committed_any = self.render_and_commit().await?;

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

	pub fn drm_mut(&mut self) -> &mut EasyDRM<MonitorRenderState> {
		&mut self.drm
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
}
