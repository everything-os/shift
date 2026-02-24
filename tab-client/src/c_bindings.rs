#![allow(non_camel_case_types)]

use std::{
	cell::RefCell,
	collections::{HashMap, VecDeque},
	env,
	ffi::{CStr, CString},
	os::raw::{c_char, c_int},
	ptr,
	rc::Rc,
};

use crate::{
	TabClient,
	config::TabClientConfig,
	error::TabClientError,
	events::{MonitorEvent, RenderEvent, SessionEvent},
	monitor::MonitorState,
	swapchain::TabSwapchain,
};
use tab_protocol::BufferIndex;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabDmabuf {
	pub fd: c_int,
	pub stride: c_int,
	pub offset: c_int,
	pub fourcc: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabFrameTarget {
	pub framebuffer: u32,
	pub texture: u32,
	pub width: i32,
	pub height: i32,
	pub buffer_index: u32,
	pub dmabuf: TabDmabuf,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabBufferRelease {
	pub monitor_id: *mut c_char,
	pub buffer_index: u32,
	pub release_fence_fd: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabMonitorInfo {
	pub id: *mut c_char,
	pub width: i32,
	pub height: i32,
	pub refresh_rate: i32,
	pub name: *mut c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabAcquireResult {
	TAB_ACQUIRE_OK = 0,
	TAB_ACQUIRE_NO_BUFFERS = 1,
	TAB_ACQUIRE_ERROR = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabEventType {
	TAB_EVENT_BUFFER_RELEASED = 0,
	TAB_EVENT_MONITOR_ADDED = 1,
	TAB_EVENT_MONITOR_REMOVED = 2,
	TAB_EVENT_SESSION_STATE = 3,
	TAB_EVENT_INPUT = 4,
	TAB_EVENT_SESSION_CREATED = 5,
	TAB_EVENT_SESSION_AWAKE = 6,
	TAB_EVENT_SESSION_SLEEP = 7,
	TAB_EVENT_SESSION_ACTIVE = 8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabSessionRole {
	TAB_SESSION_ROLE_ADMIN = 0,
	TAB_SESSION_ROLE_SESSION = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabSessionLifecycle {
	TAB_SESSION_LIFECYCLE_PENDING = 0,
	TAB_SESSION_LIFECYCLE_LOADING = 1,
	TAB_SESSION_LIFECYCLE_OCCUPIED = 2,
	TAB_SESSION_LIFECYCLE_CONSUMED = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabSessionInfo {
	pub id: *mut c_char,
	pub role: TabSessionRole,
	pub display_name: *mut c_char,
	pub state: TabSessionLifecycle,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TabEvent {
	pub event_type: TabEventType,
	pub data: TabEventData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union TabEventData {
	pub buffer_released: TabBufferRelease,
	pub monitor_added: TabMonitorInfo,
	pub monitor_removed: *mut c_char,
	pub session_state: TabSessionInfo,
	pub session_awake: *mut c_char,
	pub session_sleep: *mut c_char,
	pub session_active: *mut c_char,
	pub input: TabInputEvent,
	pub session_created_token: *mut c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabInputEventKind {
	TAB_INPUT_KIND_POINTER_MOTION = 0,
	TAB_INPUT_KIND_POINTER_MOTION_ABSOLUTE = 1,
	TAB_INPUT_KIND_POINTER_BUTTON = 2,
	TAB_INPUT_KIND_POINTER_AXIS = 3,
	TAB_INPUT_KIND_KEY = 6,
	TAB_INPUT_KIND_TOUCH_DOWN = 7,
	TAB_INPUT_KIND_TOUCH_UP = 8,
	TAB_INPUT_KIND_TOUCH_MOTION = 9,
	TAB_INPUT_KIND_TOUCH_FRAME = 10,
	TAB_INPUT_KIND_TOUCH_CANCEL = 11,
	TAB_INPUT_KIND_TABLET_TOOL_PROXIMITY = 12,
	TAB_INPUT_KIND_TABLET_TOOL_AXIS = 13,
	TAB_INPUT_KIND_TABLET_TOOL_TIP = 14,
	TAB_INPUT_KIND_TABLET_TOOL_BUTTON = 15,
	TAB_INPUT_KIND_TABLET_PAD_BUTTON = 16,
	TAB_INPUT_KIND_TABLET_PAD_RING = 17,
	TAB_INPUT_KIND_TABLET_PAD_STRIP = 18,
	TAB_INPUT_KIND_SWITCH_TOGGLE = 19,
	TAB_INPUT_KIND_GESTURE_SWIPE_BEGIN = 20,
	TAB_INPUT_KIND_GESTURE_SWIPE_UPDATE = 21,
	TAB_INPUT_KIND_GESTURE_SWIPE_END = 22,
	TAB_INPUT_KIND_GESTURE_PINCH_BEGIN = 23,
	TAB_INPUT_KIND_GESTURE_PINCH_UPDATE = 24,
	TAB_INPUT_KIND_GESTURE_PINCH_END = 25,
	TAB_INPUT_KIND_GESTURE_HOLD_BEGIN = 26,
	TAB_INPUT_KIND_GESTURE_HOLD_END = 27,
}

// Various input structs (layout compatibility)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerMotion {
	pub device: u32,
	pub time_usec: u64,
	pub x: f64,
	pub y: f64,
	pub dx: f64,
	pub dy: f64,
	pub unaccel_dx: f64,
	pub unaccel_dy: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerMotionAbsolute {
	pub device: u32,
	pub time_usec: u64,
	pub x: f64,
	pub y: f64,
	pub x_transformed: f64,
	pub y_transformed: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerButton {
	pub device: u32,
	pub time_usec: u64,
	pub button: u32,
	pub state: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerAxis {
	pub device: u32,
	pub time_usec: u64,
	pub orientation: u32,
	pub delta: f64,
	pub delta_discrete: i32,
	pub source: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputKey {
	pub device: u32,
	pub time_usec: u64,
	pub key: u32,
	pub state: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTouchContact {
	pub id: i32,
	pub x: f64,
	pub y: f64,
	pub x_transformed: f64,
	pub y_transformed: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchDown {
	pub device: u32,
	pub time_usec: u64,
	pub contact: TabTouchContact,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchMotion {
	pub device: u32,
	pub time_usec: u64,
	pub contact: TabTouchContact,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchUp {
	pub device: u32,
	pub time_usec: u64,
	pub contact_id: i32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchFrame {
	pub time_usec: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchCancel {
	pub time_usec: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTabletTool {
	pub serial: u64,
	pub tool_type: u8,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolProximity {
	pub device: u32,
	pub time_usec: u64,
	pub in_proximity: bool,
	pub tool: TabTabletTool,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTabletToolAxes {
	pub x: f64,
	pub y: f64,
	pub pressure: f64,
	pub distance: f64,
	pub tilt_x: f64,
	pub tilt_y: f64,
	pub rotation: f64,
	pub slider: f64,
	pub wheel_delta: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolAxis {
	pub device: u32,
	pub time_usec: u64,
	pub tool: TabTabletTool,
	pub axes: TabTabletToolAxes,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolTip {
	pub device: u32,
	pub time_usec: u64,
	pub tool: TabTabletTool,
	pub state: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolButton {
	pub device: u32,
	pub time_usec: u64,
	pub tool: TabTabletTool,
	pub button: u32,
	pub state: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadButton {
	pub device: u32,
	pub time_usec: u64,
	pub button: u32,
	pub state: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadRing {
	pub device: u32,
	pub time_usec: u64,
	pub ring: u32,
	pub position: f64,
	pub source: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadStrip {
	pub device: u32,
	pub time_usec: u64,
	pub strip: u32,
	pub position: f64,
	pub source: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputSwitchToggle {
	pub device: u32,
	pub time_usec: u64,
	pub switch_type: u32,
	pub state: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGestureSwipeBegin {
	pub device: u32,
	pub time_usec: u64,
	pub fingers: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGestureSwipeUpdate {
	pub device: u32,
	pub time_usec: u64,
	pub fingers: u32,
	pub dx: f64,
	pub dy: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGestureSwipeEnd {
	pub device: u32,
	pub time_usec: u64,
	pub cancelled: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGesturePinchBegin {
	pub device: u32,
	pub time_usec: u64,
	pub fingers: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGesturePinchUpdate {
	pub device: u32,
	pub time_usec: u64,
	pub fingers: u32,
	pub dx: f64,
	pub dy: f64,
	pub scale: f64,
	pub rotation: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGesturePinchEnd {
	pub device: u32,
	pub time_usec: u64,
	pub cancelled: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGestureHoldBegin {
	pub device: u32,
	pub time_usec: u64,
	pub fingers: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputGestureHoldEnd {
	pub device: u32,
	pub time_usec: u64,
	pub cancelled: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union TabInputEventData {
	pub pointer_motion: TabInputPointerMotion,
	pub pointer_motion_absolute: TabInputPointerMotionAbsolute,
	pub pointer_button: TabInputPointerButton,
	pub pointer_axis: TabInputPointerAxis,
	pub key: TabInputKey,
	pub touch_down: TabInputTouchDown,
	pub touch_up: TabInputTouchUp,
	pub touch_motion: TabInputTouchMotion,
	pub touch_frame: TabInputTouchFrame,
	pub touch_cancel: TabInputTouchCancel,
	pub tablet_tool_proximity: TabInputTabletToolProximity,
	pub tablet_tool_axis: TabInputTabletToolAxis,
	pub tablet_tool_tip: TabInputTabletToolTip,
	pub tablet_tool_button: TabInputTabletToolButton,
	pub tablet_pad_button: TabInputTabletPadButton,
	pub tablet_pad_ring: TabInputTabletPadRing,
	pub tablet_pad_strip: TabInputTabletPadStrip,
	pub switch_toggle: TabInputSwitchToggle,
	pub swipe_begin: TabInputGestureSwipeBegin,
	pub swipe_update: TabInputGestureSwipeUpdate,
	pub swipe_end: TabInputGestureSwipeEnd,
	pub pinch_begin: TabInputGesturePinchBegin,
	pub pinch_update: TabInputGesturePinchUpdate,
	pub pinch_end: TabInputGesturePinchEnd,
	pub hold_begin: TabInputGestureHoldBegin,
	pub hold_end: TabInputGestureHoldEnd,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TabInputEvent {
	pub kind: TabInputEventKind,
	pub data: TabInputEventData,
}

struct MonitorEntry {
	state: MonitorState,
	swapchain: TabSwapchain,
	pending: Option<BufferIndex>,
}

enum PendingEvent {
	BufferReleased(String, BufferIndex, Option<c_int>),
	MonitorAdded(MonitorState),
	MonitorRemoved(String),
	SessionState(tab_protocol::SessionInfo),
	SessionActive(String),
	SessionAwake(String),
	SessionSleep(String),
}

pub struct TabClientHandle {
	client: TabClient,
	events: Rc<RefCell<VecDeque<PendingEvent>>>,
	monitors: HashMap<String, MonitorEntry>,
	monitor_order: Vec<String>,
	last_error: Option<CString>,
}

impl TabClientHandle {
	fn new(mut client: TabClient) -> Result<Self, TabClientError> {
		let queue = Rc::new(RefCell::new(VecDeque::new()));

		{
			let q = queue.clone();
			client.on_monitor_event(move |evt| {
				let mut guard = q.borrow_mut();
				match evt {
					MonitorEvent::Added(state) => guard.push_back(PendingEvent::MonitorAdded(state.clone())),
					MonitorEvent::Removed(id) => guard.push_back(PendingEvent::MonitorRemoved(id.clone())),
				}
			});
		}
		{
			let q = queue.clone();
			client.on_render_event(move |evt| {
				let mut guard = q.borrow_mut();
				match evt {
					RenderEvent::BufferReleased {
						monitor_id,
						buffer,
						release_fence_fd,
					} => guard.push_back(PendingEvent::BufferReleased(
						monitor_id.clone(),
						*buffer,
						*release_fence_fd,
					)),
				}
			});
		}
		{
			let q = queue.clone();
			client.on_session_event(move |evt| {
				let mut guard = q.borrow_mut();
				match evt {
					SessionEvent::Active(session_id) => {
						guard.push_back(PendingEvent::SessionActive(session_id.clone()))
					}
					SessionEvent::Awake(session_id) => {
						guard.push_back(PendingEvent::SessionAwake(session_id.clone()))
					}
					SessionEvent::Sleep(session_id) => {
						guard.push_back(PendingEvent::SessionSleep(session_id.clone()))
					}
					SessionEvent::State(session) => {
						guard.push_back(PendingEvent::SessionState(session.clone()))
					}
				}
			});
		}

		let mut handle = Self {
			client,
			events: queue,
			monitors: HashMap::new(),
			monitor_order: Vec::new(),
			last_error: None,
		};

		let monitor_ids: Vec<String> = handle
			.client
			.monitors()
			.map(|m| m.info.id.clone())
			.collect();
		for id in monitor_ids {
			if let Some(state) = handle.client.monitor(&id).cloned() {
				handle.insert_monitor(state)?;
			}
		}

		Ok(handle)
	}

	fn insert_monitor(&mut self, state: MonitorState) -> Result<(), TabClientError> {
		let id = state.info.id.clone();
		if self.monitors.contains_key(&id) {
			return Ok(());
		}
		let swapchain = self.client.create_swapchain(&id)?;
		self.monitor_order.push(id.clone());
		self.monitors.insert(
			id,
			MonitorEntry {
				state,
				swapchain,
				pending: None,
			},
		);
		Ok(())
	}

	fn remove_monitor(&mut self, id: &str) {
		self.monitors.remove(id);
		self.monitor_order.retain(|item| item != id);
	}

	fn record_error(&mut self, err: impl ToString) {
		if let Ok(cs) = CString::new(err.to_string()) {
			self.last_error = Some(cs);
		}
	}
}

fn dup_string(s: &str) -> *mut c_char {
	CString::new(s)
		.map(|c| c.into_raw())
		.unwrap_or(ptr::null_mut())
}

fn cstring_to_string(ptr: *const c_char) -> Option<String> {
	if ptr.is_null() {
		return None;
	}
	unsafe { CStr::from_ptr(ptr) }
		.to_str()
		.ok()
		.map(|s| s.to_string())
}

fn resolve_token(token: *const c_char) -> Option<String> {
	cstring_to_string(token).or_else(|| env::var("SHIFT_SESSION_TOKEN").ok())
}

fn monitor_info_to_c(state: &MonitorState) -> TabMonitorInfo {
	TabMonitorInfo {
		id: dup_string(&state.info.id),
		width: state.info.width,
		height: state.info.height,
		refresh_rate: state.info.refresh_rate,
		name: dup_string(&state.info.name),
	}
}

fn tab_session_role(role: tab_protocol::SessionRole) -> TabSessionRole {
	match role {
		tab_protocol::SessionRole::Admin => TabSessionRole::TAB_SESSION_ROLE_ADMIN,
		tab_protocol::SessionRole::Session => TabSessionRole::TAB_SESSION_ROLE_SESSION,
	}
}

fn tab_session_lifecycle(lifecycle: tab_protocol::SessionLifecycle) -> TabSessionLifecycle {
	match lifecycle {
		tab_protocol::SessionLifecycle::Pending => TabSessionLifecycle::TAB_SESSION_LIFECYCLE_PENDING,
		tab_protocol::SessionLifecycle::Loading => TabSessionLifecycle::TAB_SESSION_LIFECYCLE_LOADING,
		tab_protocol::SessionLifecycle::Occupied => TabSessionLifecycle::TAB_SESSION_LIFECYCLE_OCCUPIED,
		tab_protocol::SessionLifecycle::Consumed => TabSessionLifecycle::TAB_SESSION_LIFECYCLE_CONSUMED,
	}
}

fn tab_session_info_to_c(session: &tab_protocol::SessionInfo) -> TabSessionInfo {
	TabSessionInfo {
		id: dup_string(&session.id),
		role: tab_session_role(session.role),
		display_name: session
			.display_name
			.as_deref()
			.map(dup_string)
			.unwrap_or(ptr::null_mut()),
		state: tab_session_lifecycle(session.state),
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_connect(
	socket_path: *const c_char,
	token: *const c_char,
) -> *mut TabClientHandle {
	let token = match resolve_token(token) {
		Some(t) => t,
		None => return ptr::null_mut(),
	};
	let mut config = TabClientConfig::new(token);
	if let Some(path) = cstring_to_string(socket_path) {
		config = config.socket_path(path);
	}
	let client = match TabClient::connect(config) {
		Ok(client) => client,
		Err(err) => {
			eprintln!("tab_client_connect failed: {err}");
			return ptr::null_mut();
		}
	};
	match TabClientHandle::new(client) {
		Ok(handle) => Box::into_raw(Box::new(handle)),
		Err(err) => {
			eprintln!("tab_client_connect handle init failed: {err}");
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_connect_default(token: *const c_char) -> *mut TabClientHandle {
	unsafe { tab_client_connect(ptr::null(), token) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_disconnect(handle: *mut TabClientHandle) {
	unsafe {
		if !handle.is_null() {
			drop(Box::from_raw(handle));
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_string_free(s: *mut c_char) {
	unsafe {
		if !s.is_null() {
			drop(CString::from_raw(s));
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_take_error(handle: *mut TabClientHandle) -> *mut c_char {
	unsafe {
		let handle = match handle.as_mut() {
			Some(h) => h,
			None => return ptr::null_mut(),
		};
		if let Some(err) = handle.last_error.take() {
			err.into_raw()
		} else {
			ptr::null_mut()
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_socket_fd(handle: *mut TabClientHandle) -> c_int {
	unsafe { handle.as_ref().map(|h| h.client.socket_fd()).unwrap_or(-1) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_swap_fd(_handle: *mut TabClientHandle) -> c_int {
	-1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_drm_fd(handle: *mut TabClientHandle) -> c_int {
	unsafe { handle.as_ref().map(|h| h.client.drm_fd()).unwrap_or(-1) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_count(handle: *mut TabClientHandle) -> usize {
	unsafe { handle.as_ref().map(|h| h.monitor_order.len()).unwrap_or(0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_id(
	handle: *mut TabClientHandle,
	index: usize,
) -> *mut c_char {
	unsafe {
		let handle = match handle.as_ref() {
			Some(h) => h,
			None => return ptr::null_mut(),
		};
		handle
			.monitor_order
			.get(index)
			.map(|id| dup_string(id))
			.unwrap_or(ptr::null_mut())
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_monitor_info(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
) -> TabMonitorInfo {
	unsafe {
		let handle = match handle.as_ref() {
			Some(h) => h,
			None => {
				return TabMonitorInfo {
					id: ptr::null_mut(),
					width: 0,
					height: 0,
					refresh_rate: 0,
					name: ptr::null_mut(),
				};
			}
		};
		let id = match cstring_to_string(monitor_id) {
			Some(id) => id,
			None => {
				return TabMonitorInfo {
					id: ptr::null_mut(),
					width: 0,
					height: 0,
					refresh_rate: 0,
					name: ptr::null_mut(),
				};
			}
		};
		match handle.monitors.get(&id) {
			Some(entry) => monitor_info_to_c(&entry.state),
			None => TabMonitorInfo {
				id: ptr::null_mut(),
				width: 0,
				height: 0,
				refresh_rate: 0,
				name: ptr::null_mut(),
			},
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_monitor_info(info: *mut TabMonitorInfo) {
	unsafe {
		if info.is_null() {
			return;
		}
		if !(*info).id.is_null() {
			drop(CString::from_raw((*info).id));
			(*info).id = ptr::null_mut();
		}
		if !(*info).name.is_null() {
			drop(CString::from_raw((*info).name));
			(*info).name = ptr::null_mut();
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_poll_events(handle: *mut TabClientHandle) -> usize {
	unsafe {
		let handle = match handle.as_mut() {
			Some(h) => h,
			None => return 0,
		};
		match handle.client.dispatch_events() {
			Ok(()) => (),
			Err(err) => {
				handle.record_error(err);
				return 0;
			}
		}
		handle.events.borrow().len()
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_next_event(
	handle: *mut TabClientHandle,
	event: *mut TabEvent,
) -> bool {
	unsafe {
		let handle = match handle.as_mut() {
			Some(h) => h,
			None => return false,
		};
		if event.is_null() {
			return false;
		}
		let pending = handle.events.borrow_mut().pop_front();
		let Some(evt) = pending else {
			return false;
		};
		match evt {
			PendingEvent::BufferReleased(monitor_id, buffer, release_fence_fd) => {
				if let Some(entry) = handle.monitors.get_mut(&monitor_id) {
					entry.swapchain.mark_released(buffer);
				}
				(*event).event_type = TabEventType::TAB_EVENT_BUFFER_RELEASED;
				(*event).data.buffer_released = TabBufferRelease {
					monitor_id: dup_string(&monitor_id),
					buffer_index: buffer as u32,
					release_fence_fd: release_fence_fd.unwrap_or(-1),
				};
				true
			}
			PendingEvent::MonitorRemoved(monitor_id) => {
				handle.remove_monitor(&monitor_id);
				(*event).event_type = TabEventType::TAB_EVENT_MONITOR_REMOVED;
				(*event).data.monitor_removed = dup_string(&monitor_id);
				true
			}
			PendingEvent::MonitorAdded(state) => {
				if let Err(err) = handle.insert_monitor(state.clone()) {
					handle.record_error(err);
					// requeue and signal failure
					handle
						.events
						.borrow_mut()
						.push_front(PendingEvent::MonitorAdded(state));
					false
				} else {
					(*event).event_type = TabEventType::TAB_EVENT_MONITOR_ADDED;
					(*event).data.monitor_added = monitor_info_to_c(&state);
					true
				}
			}
			PendingEvent::SessionAwake(session_id) => {
				(*event).event_type = TabEventType::TAB_EVENT_SESSION_AWAKE;
				(*event).data.session_awake = dup_string(&session_id);
				true
			}
			PendingEvent::SessionActive(session_id) => {
				(*event).event_type = TabEventType::TAB_EVENT_SESSION_ACTIVE;
				(*event).data.session_active = dup_string(&session_id);
				true
			}
			PendingEvent::SessionSleep(session_id) => {
				(*event).event_type = TabEventType::TAB_EVENT_SESSION_SLEEP;
				(*event).data.session_sleep = dup_string(&session_id);
				true
			}
			PendingEvent::SessionState(session) => {
				(*event).event_type = TabEventType::TAB_EVENT_SESSION_STATE;
				(*event).data.session_state = tab_session_info_to_c(&session);
				true
			}
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_event_strings(event: *mut TabEvent) {
	unsafe {
		if event.is_null() {
			return;
		}
		match (*event).event_type {
			TabEventType::TAB_EVENT_BUFFER_RELEASED => {
				if !(*event).data.buffer_released.monitor_id.is_null() {
					drop(CString::from_raw((*event).data.buffer_released.monitor_id));
					(*event).data.buffer_released.monitor_id = ptr::null_mut();
				}
				if (*event).data.buffer_released.release_fence_fd >= 0 {
					libc::close((*event).data.buffer_released.release_fence_fd);
					(*event).data.buffer_released.release_fence_fd = -1;
				}
			}
			TabEventType::TAB_EVENT_MONITOR_REMOVED => {
				if !(*event).data.monitor_removed.is_null() {
					drop(CString::from_raw((*event).data.monitor_removed));
					(*event).data.monitor_removed = ptr::null_mut();
				}
			}
			TabEventType::TAB_EVENT_SESSION_AWAKE => {
				if !(*event).data.session_awake.is_null() {
					drop(CString::from_raw((*event).data.session_awake));
					(*event).data.session_awake = ptr::null_mut();
				}
			}
			TabEventType::TAB_EVENT_SESSION_SLEEP => {
				if !(*event).data.session_sleep.is_null() {
					drop(CString::from_raw((*event).data.session_sleep));
					(*event).data.session_sleep = ptr::null_mut();
				}
			}
			TabEventType::TAB_EVENT_SESSION_ACTIVE => {
				if !(*event).data.session_active.is_null() {
					drop(CString::from_raw((*event).data.session_active));
					(*event).data.session_active = ptr::null_mut();
				}
			}
			TabEventType::TAB_EVENT_SESSION_STATE => {
				if !(*event).data.session_state.id.is_null() {
					drop(CString::from_raw((*event).data.session_state.id));
					(*event).data.session_state.id = ptr::null_mut();
				}
				if !(*event).data.session_state.display_name.is_null() {
					drop(CString::from_raw((*event).data.session_state.display_name));
					(*event).data.session_state.display_name = ptr::null_mut();
				}
			}
			TabEventType::TAB_EVENT_MONITOR_ADDED => {
				let mut info = (*event).data.monitor_added;
				tab_client_free_monitor_info(&mut info as *mut _);
			}
			_ => {}
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_acquire_frame(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	target: *mut TabFrameTarget,
) -> TabAcquireResult {
	unsafe {
		let handle = match handle.as_mut() {
			Some(h) => h,
			None => return TabAcquireResult::TAB_ACQUIRE_ERROR,
		};
		let id = match cstring_to_string(monitor_id) {
			Some(id) => id,
			None => return TabAcquireResult::TAB_ACQUIRE_ERROR,
		};
		let entry = match handle.monitors.get_mut(&id) {
			Some(entry) => entry,
			None => return TabAcquireResult::TAB_ACQUIRE_ERROR,
		};
		let Some((buffer, index)) = entry.swapchain.acquire_next() else {
			return TabAcquireResult::TAB_ACQUIRE_NO_BUFFERS;
		};
		let fd = buffer.fd();
		entry.pending = Some(index);
		if target.is_null() {
			return TabAcquireResult::TAB_ACQUIRE_ERROR;
		}
		(*target).framebuffer = 0;
		(*target).texture = 0;
		(*target).width = buffer.width();
		(*target).height = buffer.height();
		(*target).buffer_index = index as u32;
		(*target).dmabuf = TabDmabuf {
			fd,
			stride: buffer.stride(),
			offset: buffer.offset(),
			fourcc: buffer.fourcc(),
		};
		TabAcquireResult::TAB_ACQUIRE_OK
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_request_buffer(
	handle: *mut TabClientHandle,
	monitor_id: *const c_char,
	acquire_fence_fd: c_int,
) -> bool {
	unsafe {
		let handle = match handle.as_mut() {
			Some(h) => h,
			None => return false,
		};
		let id = match cstring_to_string(monitor_id) {
			Some(id) => id,
			None => return false,
		};
		let entry = match handle.monitors.get_mut(&id) {
			Some(entry) => entry,
			None => return false,
		};
		let buffer = match entry.pending.take() {
			Some(idx) => idx,
			None => return false,
		};
		let acquire_fence = if acquire_fence_fd >= 0 {
			Some(acquire_fence_fd)
		} else {
			None
		};
		if let Err(err) = handle.client.request_buffer(&id, buffer, acquire_fence) {
			entry.swapchain.rollback();
			handle.record_error(err);
			return false;
		}
		entry.swapchain.mark_busy(buffer);
		true
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_server_name(_handle: *mut TabClientHandle) -> *mut c_char {
	ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_protocol_name(
	_handle: *mut TabClientHandle,
) -> *mut c_char {
	ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_get_session(handle: *mut TabClientHandle) -> TabSessionInfo {
	unsafe {
		let Some(handle) = handle.as_ref() else {
			return TabSessionInfo {
				id: ptr::null_mut(),
				role: TabSessionRole::TAB_SESSION_ROLE_SESSION,
				display_name: ptr::null_mut(),
				state: TabSessionLifecycle::TAB_SESSION_LIFECYCLE_PENDING,
			};
		};
		tab_session_info_to_c(handle.client.session())
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_free_session_info(info: *mut TabSessionInfo) {
	unsafe {
		if info.is_null() {
			return;
		}
		if !(*info).id.is_null() {
			drop(CString::from_raw((*info).id));
			(*info).id = ptr::null_mut();
		}
		if !(*info).display_name.is_null() {
			drop(CString::from_raw((*info).display_name));
			(*info).display_name = ptr::null_mut();
		}
	}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tab_client_send_ready(handle: *mut TabClientHandle) -> bool {
	unsafe {
		let Some(handle) = handle.as_mut() else {
			return false;
		};
		if let Err(err) = handle.client.send_ready() {
			handle.record_error(err);
			return false;
		}
		true
	}
}
