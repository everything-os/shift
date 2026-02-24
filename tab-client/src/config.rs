use std::path::{Path, PathBuf};

use tab_protocol::DEFAULT_SOCKET_PATH;

/// Builder-style configuration for establishing a Tab connection.
#[derive(Debug, Clone)]
pub struct TabClientConfig {
	socket_path: PathBuf,
	token: String,
	render_node: Option<PathBuf>,
}

impl TabClientConfig {
	pub fn new(token: impl Into<String>) -> Self {
		Self {
			socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
			token: token.into(),
			render_node: None,
		}
	}

	pub fn socket_path(mut self, path: impl AsRef<Path>) -> Self {
		self.socket_path = path.as_ref().into();
		self
	}

	pub fn render_node(mut self, path: impl AsRef<Path>) -> Self {
		self.render_node = Some(path.as_ref().into());
		self
	}

	pub fn token(&self) -> &str {
		&self.token
	}

	pub fn socket_path_ref(&self) -> &Path {
		&self.socket_path
	}

	pub fn render_node_path(&self) -> Option<&Path> {
		self.render_node.as_deref()
	}
}
