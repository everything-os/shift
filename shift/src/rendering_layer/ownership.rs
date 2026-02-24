use std::collections::HashMap;

use crate::{monitor::MonitorId, sessions::SessionId};

use super::state::{BufferSlot, DeferredRelease, MonitorSurfaceState, SlotKey, SlotOwner};

pub(super) struct SwapApplyResult {
	pub canceled_pending: Option<BufferSlot>,
	pub previous_to_release: Option<BufferSlot>,
}

pub(super) struct OwnershipManager {
	current_session: Option<SessionId>,
	monitor_state: HashMap<(MonitorId, SessionId), MonitorSurfaceState>,
	slot_ownership: HashMap<SlotKey, SlotOwner>,
	deferred_releases: Vec<DeferredRelease>,
}

impl OwnershipManager {
	pub fn new() -> Self {
		Self {
			current_session: None,
			monitor_state: HashMap::new(),
			slot_ownership: HashMap::new(),
			deferred_releases: Vec::new(),
		}
	}

	pub fn current_session(&self) -> Option<SessionId> {
		self.current_session
	}

	pub fn set_current_session(&mut self, session_id: Option<SessionId>) {
		self.current_session = session_id;
	}

	pub fn ensure_current_session_monitors(&mut self, monitor_ids: &[MonitorId]) {
		if let Some(session_id) = self.current_session {
			for monitor_id in monitor_ids {
				self
					.monitor_state
					.entry((*monitor_id, session_id))
					.or_default();
			}
		}
	}

	pub fn current_slot_key(&self, monitor_id: MonitorId) -> Option<SlotKey> {
		let session_id = self.current_session?;
		self.current_slot_key_for_session(monitor_id, session_id)
	}

	pub fn current_slot_key_for_session(
		&self,
		monitor_id: MonitorId,
		session_id: SessionId,
	) -> Option<SlotKey> {
		let state = self.monitor_state.get(&(monitor_id, session_id))?;
		let buffer = state.current_buffer?;
		Some(SlotKey::new(monitor_id, session_id, buffer))
	}

	fn state_mut(
		&mut self,
		monitor_id: MonitorId,
		session_id: SessionId,
	) -> Option<&mut MonitorSurfaceState> {
		self.monitor_state.get_mut(&(monitor_id, session_id))
	}

	fn state_entry(
		&mut self,
		monitor_id: MonitorId,
		session_id: SessionId,
	) -> &mut MonitorSurfaceState {
		self
			.monitor_state
			.entry((monitor_id, session_id))
			.or_default()
	}

	pub fn owner(&self, key: SlotKey) -> Option<SlotOwner> {
		self.slot_ownership.get(&key).copied()
	}

	pub fn mark_slot_client_owned(&mut self, key: SlotKey) {
		self.slot_ownership.insert(key, SlotOwner::ClientOwned);
	}

	pub fn mark_slot_shift_owned(&mut self, key: SlotKey) {
		self.slot_ownership.insert(key, SlotOwner::ShiftOwned);
	}

	pub fn apply_swap_request(
		&mut self,
		monitor_id: MonitorId,
		session_id: SessionId,
		slot: BufferSlot,
		has_acquire_fence: bool,
	) -> SwapApplyResult {
		let canceled_pending = self
			.monitor_state
			.get(&(monitor_id, session_id))
			.and_then(|state| state.pending_buffer)
			.filter(|pending| *pending != slot);

		self.mark_slot_shift_owned(SlotKey::new(monitor_id, session_id, slot));

		let state = self.state_entry(monitor_id, session_id);
		let previous = state.current_buffer;
		state.pending_buffer = Some(slot);

		let previous_to_release = if has_acquire_fence {
			None
		} else {
			state.current_buffer = Some(slot);
			state.pending_buffer = None;
			previous.filter(|prev| *prev != slot)
		};

		SwapApplyResult {
			canceled_pending,
			previous_to_release,
		}
	}

	pub fn apply_acquire_fence_signaled(&mut self, key: SlotKey) -> Option<BufferSlot> {
		let state = self.state_mut(key.monitor_id, key.session_id)?;
		if state.pending_buffer != Some(key.buffer) {
			return None;
		}
		let previous = state.current_buffer;
		state.current_buffer = Some(key.buffer);
		state.pending_buffer = None;
		previous.filter(|prev| *prev != key.buffer)
	}

	pub fn queue_buffer_release(
		&mut self,
		monitor_id: MonitorId,
		session_id: SessionId,
		buffer: BufferSlot,
	) {
		if self.deferred_releases.iter().any(|item| {
			item.monitor_id == monitor_id && item.session_id == session_id && item.buffer == buffer
		}) {
			return;
		}
		self.deferred_releases.push(DeferredRelease {
			monitor_id,
			session_id,
			buffer,
		});
	}

	pub fn take_deferred_releases(&mut self) -> Vec<DeferredRelease> {
		self.deferred_releases.drain(..).collect()
	}

	pub fn cleanup_monitor(&mut self, monitor_id: MonitorId) {
		self
			.slot_ownership
			.retain(|key, _| key.monitor_id != monitor_id);
		self
			.deferred_releases
			.retain(|item| item.monitor_id != monitor_id);
		self.monitor_state.retain(|(mon, _), _| *mon != monitor_id);
	}

	pub fn cleanup_session(&mut self, session_id: SessionId) {
		self
			.slot_ownership
			.retain(|key, _| key.session_id != session_id);
		self
			.monitor_state
			.retain(|(_, sess), _| *sess != session_id);
		self
			.deferred_releases
			.retain(|item| item.session_id != session_id);
	}
}
