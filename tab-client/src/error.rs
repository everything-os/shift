use std::path::PathBuf;

use gbm::InvalidFdError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TabClientError {
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("protocol error: {0}")]
	Protocol(#[from] tab_protocol::ProtocolError),
	#[error("nix error: {0}")]
	Nix(#[from] nix::Error),
	#[error("authentication failed: {0}")]
	Auth(String),
	#[error("server rejected request: {0}")]
	Server(String),
	#[error("unexpected message: {0}")]
	Unexpected(&'static str),
	#[error("failed to open render node {path}: {source}")]
	RenderNodeOpen {
		path: PathBuf,
		source: std::io::Error,
	},
	#[error("gbm device initialization failed: {0}")]
	GbmInit(String),
	#[error("monitor has invalid dimensions")]
	InvalidMonitorDimensions,
	#[error("unknown monitor: {0}")]
	UnknownMonitor(String),
	#[error("failed to export dma-buf fd: {0}")]
	BufferExport(#[from] InvalidFdError),
}
