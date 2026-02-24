use std::{
	fs::OpenOptions,
	os::fd::{AsRawFd, RawFd},
	path::{Path, PathBuf},
};

use gbm::{BufferObjectFlags, Device, Format};
use tab_protocol::BufferIndex;

use crate::{
	error::TabClientError,
	monitor::MonitorState,
	swapchain::{TabBuffer, TabSwapchain},
};

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

pub struct GbmAllocator {
	device: Device<std::fs::File>,
	format: Format,
	preferred_usage: BufferObjectFlags,
	fallback_usage: BufferObjectFlags,
}

impl GbmAllocator {
	pub fn new(configured_node: Option<&Path>) -> Result<Self, TabClientError> {
		let mut last_error = None;
		for candidate in Self::render_node_candidates(configured_node) {
			match OpenOptions::new().read(true).write(true).open(&candidate) {
				Ok(file) => match Device::new(file) {
					Ok(device) => {
						return Ok(Self {
							device,
							format: Format::Xrgb8888,
							preferred_usage: BufferObjectFlags::RENDERING,
							fallback_usage: BufferObjectFlags::RENDERING,
						});
					}
					Err(err) => {
						last_error = Some(TabClientError::GbmInit(err.to_string()));
					}
				},
				Err(source) => {
					last_error = Some(TabClientError::RenderNodeOpen {
						path: candidate.clone(),
						source,
					});
				}
			}
		}
		Err(
			last_error.unwrap_or_else(|| TabClientError::GbmInit("no usable render nodes found".into())),
		)
	}

	pub fn drm_fd(&self) -> RawFd {
		self.device.as_raw_fd()
	}

	pub fn create_swapchain(&self, monitor: &MonitorState) -> Result<TabSwapchain, TabClientError> {
		let width =
			u32::try_from(monitor.info.width).map_err(|_| TabClientError::InvalidMonitorDimensions)?;
		let height =
			u32::try_from(monitor.info.height).map_err(|_| TabClientError::InvalidMonitorDimensions)?;
		let bo0 = self
			.device
			.create_buffer_object::<()>(width, height, self.format, self.preferred_usage)
			.or_else(|_| {
				self
					.device
					.create_buffer_object::<()>(width, height, self.format, self.fallback_usage)
			})?;
		let bo1 = self
			.device
			.create_buffer_object::<()>(width, height, self.format, self.preferred_usage)
			.or_else(|_| {
				self
					.device
					.create_buffer_object::<()>(width, height, self.format, self.fallback_usage)
			})?;
		let buffers = [
			TabBuffer::new(BufferIndex::Zero, bo0),
			TabBuffer::new(BufferIndex::One, bo1),
		];
		Ok(TabSwapchain::new(monitor.info.id.clone(), buffers))
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
}
