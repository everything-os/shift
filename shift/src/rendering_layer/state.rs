use tab_protocol::BufferIndex;

use crate::{monitor::MonitorId, sessions::SessionId};

#[derive(Default, Debug)]
pub(super) struct MonitorSurfaceState {
	pub current_buffer: Option<BufferSlot>,
	pub pending_buffer: Option<BufferSlot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SlotKey {
	pub monitor_id: MonitorId,
	pub session_id: SessionId,
	pub buffer: BufferSlot,
}

impl SlotKey {
	pub fn new(monitor_id: MonitorId, session_id: SessionId, buffer: BufferSlot) -> Self {
		Self {
			monitor_id,
			session_id,
			buffer,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum BufferSlot {
	Zero,
	One,
}

#[derive(Debug)]
pub(super) enum FenceEvent {
	Signaled { key: SlotKey },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DeferredRelease {
	pub monitor_id: MonitorId,
	pub session_id: SessionId,
	pub buffer: BufferSlot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotOwner {
	ClientOwned,
	ShiftOwned,
}

impl BufferSlot {
	pub fn from_index(idx: usize) -> Option<Self> {
		match idx {
			0 => Some(Self::Zero),
			1 => Some(Self::One),
			_ => None,
		}
	}
}

impl From<BufferIndex> for BufferSlot {
	fn from(value: BufferIndex) -> Self {
		match value {
			BufferIndex::Zero => BufferSlot::Zero,
			BufferIndex::One => BufferSlot::One,
		}
	}
}

impl From<BufferSlot> for BufferIndex {
	fn from(value: BufferSlot) -> Self {
		match value {
			BufferSlot::Zero => BufferIndex::Zero,
			BufferSlot::One => BufferIndex::One,
		}
	}
}
