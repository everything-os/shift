use std::sync::Arc;

use crate::{define_id_type, sessions::Role};

define_id_type!(Session, "se_");

#[derive(Clone, Debug)]
pub struct Session {
	pub(super) id: SessionId,
	pub(super) role: Role,
	pub(super) ready: bool,
	pub(super) display_name: Arc<str>,
}

impl Session {
	pub fn with_ready(&self, ready: bool) -> Self {
		let mut cloned = self.clone();
		cloned.ready = ready;
		cloned
	}
	pub fn id(&self) -> SessionId {
		self.id
	}
	pub fn role(&self) -> Role {
		self.role
	}
	pub fn ready(&self) -> bool {
		self.ready
	}
	pub fn display_name(&self) -> &str {
		&self.display_name
	}
}
