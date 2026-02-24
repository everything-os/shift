use tab_protocol::SessionRole;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[repr(u8)]
pub enum Role {
	Normal = 0,
	Admin = 1,
}

impl From<SessionRole> for Role {
	fn from(value: SessionRole) -> Self {
		match value {
			SessionRole::Admin => Self::Admin,
			SessionRole::Session => Self::Normal,
		}
	}
}

impl From<Role> for SessionRole {
	fn from(value: Role) -> Self {
		match value {
			Role::Normal => Self::Session,
			Role::Admin => Self::Admin,
		}
	}
}
