use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::{auth::Token, sessions::Session};

use super::{Role, SessionId};

#[derive(Debug, Clone)]
pub struct PendingSession {
	id: SessionId,
	role: Role,
	created_at: DateTime<Utc>,
	display_name: Option<Arc<str>>,
}
impl PendingSession {
	pub fn id(&self) -> SessionId {
		self.id
	}
	pub fn role(&self) -> Role {
		self.role
	}

	pub fn display_name(&self) -> Option<&str> {
		self.display_name.as_deref()
	}

	pub fn new(display_name: Option<Arc<str>>, role: Role) -> (Token, Self) {
		(
			Token::generate().expect("getrandom to be available"),
			Self {
				id: SessionId::rand(),
				role,
				created_at: Utc::now(),
				display_name,
			},
		)
	}

	pub fn admin(display_name: Option<Arc<str>>) -> (Token, Self) {
		Self::new(display_name, Role::Admin)
	}
	pub fn normal(display_name: Option<Arc<str>>) -> (Token, Self) {
		Self::new(display_name, Role::Normal)
	}

	pub fn promote(self) -> Session {
		Session {
			id: self.id,
			role: self.role,
			ready: self.role == Role::Admin,
			display_name: self
				.display_name
				.as_ref()
				.map(Arc::clone)
				.unwrap_or_else(|| self.default_session_name().into()),
		}
	}
	pub fn default_session_name(&self) -> String {
		format!("Session: {}", self.id.to_string())
	}
}
