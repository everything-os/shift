//! Shared Tab v1 protocol definitions and helpers for both client and server sides.
//! - Message framing over Unix domain sockets (sendmsg/recvmsg + SCM_RIGHTS)
//! - Raw TabMessageFrame representation (header + payload string + FDs)
//! - Parsing helpers into typed TabMessage variants

use serde::{Deserialize, Serialize};
use std::{
	os::fd::{FromRawFd, OwnedFd},
	str::FromStr,
	time::Duration,
};

pub mod message_frame;
pub mod unix_socket_utils;
/// Default Unix domain socket for Tab connections.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/shift.sock";
/// Protocol identifier string expected in `hello` payloads. Used to check if the client and server are compatible.
pub const PROTOCOL_VERSION: &str = const_str::concat!("tab/v", env!("CARGO_PKG_VERSION"));
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum BufferIndex {
	Zero = 0,
	One = 1,
}
impl FromStr for BufferIndex {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, ()> {
		match s {
			"0" => Ok(Self::Zero),
			"1" => Ok(Self::One),
			_ => Err(()),
		}
	}
}
/// Parsed, semantic Tab message.
#[derive(Debug)]
pub enum TabMessage {
	Hello(HelloPayload),
	Auth(AuthPayload),
	AuthOk(AuthOkPayload),
	AuthError(AuthErrorPayload),
	FramebufferLink {
		payload: FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
	},
	BufferRequest {
		payload: BufferRequestPayload,
		acquire_fence: Option<OwnedFd>,
	},
	BufferRequestAck(BufferRequestAckPayload),
	BufferRelease {
		payload: BufferReleasePayload,
		release_fence: Option<OwnedFd>,
	},
	InputEvent(InputEventPayload),
	MonitorAdded(MonitorAddedPayload),
	MonitorRemoved(MonitorRemovedPayload),
	SessionSwitch(SessionSwitchPayload),
	SessionCreate(SessionCreatePayload),
	SessionCreated(SessionCreatedPayload),
	SessionReady(SessionReadyPayload),
	SessionState(SessionStatePayload),
	SessionActive(SessionActivePayload),
	SessionAwake(SessionAwakePayload),
	SessionSleep(SessionSleepPayload),
	Error(ErrorPayload),
	Ping,
	Pong,
	Unknown(TabMessageFrame),
}
impl TryFrom<TabMessageFrame> for TabMessage {
	type Error = ProtocolError;
	fn try_from(value: TabMessageFrame) -> Result<Self, ProtocolError> {
		Self::parse_message_frame(value)
	}
}

impl TabMessage {
	/// Parse the raw TabMessageFrame into a typed `TabMessage` variant.
	#[tracing::instrument(skip_all, fields(header = %msg.header.0))]
	pub fn parse_message_frame(msg: TabMessageFrame) -> Result<Self, ProtocolError> {
		let header = msg.header.0.as_str();

		match header {
			message_header::HELLO => {
				let payload: HelloPayload = msg.expect_payload_json()?;
				Ok(TabMessage::Hello(payload))
			}
			message_header::AUTH => {
				let payload: AuthPayload = msg.expect_payload_json()?;
				Ok(TabMessage::Auth(payload))
			}
			message_header::AUTH_OK => {
				let payload: AuthOkPayload = msg.expect_payload_json()?;
				Ok(TabMessage::AuthOk(payload))
			}
			message_header::AUTH_ERROR => {
				let payload: AuthErrorPayload = msg.expect_payload_json()?;
				Ok(TabMessage::AuthError(payload))
			}
			message_header::FRAMEBUFFER_LINK => {
				let payload: FramebufferLinkPayload = msg.expect_payload_json()?;
				msg.expect_n_fds(2)?;
				let dma_bufs = unsafe {
					[
						OwnedFd::from_raw_fd(msg.fds[0]),
						OwnedFd::from_raw_fd(msg.fds[1]),
					]
				};
				Ok(TabMessage::FramebufferLink { payload, dma_bufs })
			}
			message_header::BUFFER_REQUEST => {
				let payload = msg.payload.clone().ok_or(ProtocolError::ExpectedPayload)?;
				let err = ProtocolError::InvalidPayload(
					r#""buffer_request" request requires 2 arguments: <monitor_id> <0 or 1 (buffer index)>"#
						.into(),
				);
				let split = payload.split_ascii_whitespace().collect::<Vec<_>>();
				let [monitor_id, buffer_index_str] = split[..] else {
					return Err(err);
				};
				let buffer_index = buffer_index_str.parse().map_err(|_| err)?;
				let payload = BufferRequestPayload {
					monitor_id: monitor_id.into(),
					buffer: buffer_index,
				};
				let acquire_fence = match msg.fds.len() {
					0 => None,
					1 => Some(unsafe { OwnedFd::from_raw_fd(msg.fds[0]) }),
					found => {
						return Err(ProtocolError::ExpectedFds {
							expected: 1,
							found: found as u32,
						});
					}
				};
				Ok(TabMessage::BufferRequest {
					payload,
					acquire_fence,
				})
			}
			message_header::BUFFER_REQUEST_ACK => {
				let payload = msg.payload.clone().ok_or(ProtocolError::ExpectedPayload)?;
				let err = ProtocolError::InvalidPayload(
					r#""buffer_request_ack" event requires 2 arguments: <monitor_id> <0 or 1 (buffer index)>"#
						.into(),
				);
				let split = payload.split_ascii_whitespace().collect::<Vec<_>>();
				let [monitor_id, buffer_index_str] = split[..] else {
					return Err(err);
				};
				let buffer_index = buffer_index_str.parse().map_err(|_| err)?;
				Ok(TabMessage::BufferRequestAck(BufferRequestAckPayload {
					monitor_id: monitor_id.into(),
					buffer: buffer_index,
				}))
			}
			message_header::BUFFER_RELEASE => {
				let payload = msg.payload.clone().ok_or(ProtocolError::ExpectedPayload)?;
				let err = ProtocolError::InvalidPayload(
					r#""buffer_release" event requires 2 arguments: <monitor_id> <0 or 1 (buffer index)>"#
						.into(),
				);
				let split = payload.split_ascii_whitespace().collect::<Vec<_>>();
				let [monitor_id, buffer_index_str] = split[..] else {
					return Err(err);
				};
				let buffer_index = buffer_index_str.parse().map_err(|_| err)?;
				let release_fence = match msg.fds.len() {
					0 => None,
					1 => Some(unsafe { OwnedFd::from_raw_fd(msg.fds[0]) }),
					found => {
						return Err(ProtocolError::ExpectedFds {
							expected: 1,
							found: found as u32,
						});
					}
				};
				Ok(TabMessage::BufferRelease {
					payload: BufferReleasePayload {
						monitor_id: monitor_id.into(),
						buffer: buffer_index,
					},
					release_fence,
				})
			}
			message_header::INPUT_EVENT => {
				let payload: InputEventPayload = msg.expect_payload_json()?;
				Ok(TabMessage::InputEvent(payload))
			}
			message_header::MONITOR_ADDED => {
				let payload: MonitorAddedPayload = msg.expect_payload_json()?;
				Ok(TabMessage::MonitorAdded(payload))
			}
			message_header::MONITOR_REMOVED => {
				let payload: MonitorRemovedPayload = msg.expect_payload_json()?;
				Ok(TabMessage::MonitorRemoved(payload))
			}
			message_header::SESSION_SWITCH => {
				let payload: SessionSwitchPayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionSwitch(payload))
			}
			message_header::SESSION_CREATE => {
				let payload: SessionCreatePayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionCreate(payload))
			}
			message_header::SESSION_CREATED => {
				let payload: SessionCreatedPayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionCreated(payload))
			}
			message_header::SESSION_READY => {
				let payload: SessionReadyPayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionReady(payload))
			}
			message_header::SESSION_STATE => {
				let payload: SessionStatePayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionState(payload))
			}
			message_header::SESSION_ACTIVE => {
				let payload: SessionActivePayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionActive(payload))
			}
			message_header::SESSION_AWAKE => {
				let payload: SessionAwakePayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionAwake(payload))
			}
			message_header::SESSION_SLEEP => {
				let payload: SessionSleepPayload = msg.expect_payload_json()?;
				Ok(TabMessage::SessionSleep(payload))
			}
			message_header::ERROR => {
				let payload: ErrorPayload = msg.expect_payload_json()?;
				Ok(TabMessage::Error(payload))
			}
			message_header::PING => Ok(TabMessage::Ping),
			message_header::PONG => Ok(TabMessage::Pong),
			_ => Ok(TabMessage::Unknown(msg)),
		}
	}
}
/// Typed payloads
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloPayload {
	pub server: String,
	pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPayload {
	pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
	pub id: String,
	pub width: i32,
	pub height: i32,
	pub refresh_rate: i32,
	pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
	pub id: String,
	pub role: SessionRole,
	pub display_name: Option<String>,
	pub state: SessionLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
	Pending,
	Loading,
	Occupied,
	Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
	Admin,
	Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthOkPayload {
	pub session: SessionInfo,
	pub monitors: Vec<MonitorInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthErrorPayload {
	pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FramebufferLinkPayload {
	pub monitor_id: String,
	pub width: i32,
	pub height: i32,
	pub stride: i32,
	pub offset: i32,
	pub fourcc: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferRequestPayload {
	pub monitor_id: String,
	pub buffer: BufferIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferRequestAckPayload {
	pub monitor_id: String,
	pub buffer: BufferIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferReleasePayload {
	pub monitor_id: String,
	pub buffer: BufferIndex,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEventPayload {
	PointerMotion {
		device: u32,
		time_usec: u64,
		x: f64,
		y: f64,
		dx: f64,
		dy: f64,
		unaccel_dx: f64,
		unaccel_dy: f64,
	},
	PointerMotionAbsolute {
		device: u32,
		time_usec: u64,
		x: f64,
		y: f64,
		x_transformed: f64,
		y_transformed: f64,
	},
	PointerButton {
		device: u32,
		time_usec: u64,
		button: u32,
		state: ButtonState,
	},
	PointerAxis {
		device: u32,
		time_usec: u64,
		orientation: AxisOrientation,
		delta: f64,
		delta_discrete: Option<i32>,
		source: AxisSource,
	},
	Key {
		device: u32,
		time_usec: u64,
		key: u32,
		state: KeyState,
	},
	TouchDown {
		device: u32,
		time_usec: u64,
		contact: TouchContact,
	},
	TouchUp {
		device: u32,
		time_usec: u64,
		contact_id: i32,
	},
	TouchMotion {
		device: u32,
		time_usec: u64,
		contact: TouchContact,
	},
	TouchFrame {
		time_usec: u64,
	},
	TouchCancel {
		time_usec: u64,
	},
	TableToolProximity {
		device: u32,
		time_usec: u64,
		in_proximity: bool,
		tool: TabletTool,
	},
	TabletToolAxis {
		device: u32,
		time_usec: u64,
		tool: TabletTool,
		axes: TabletToolAxes,
	},
	TabletToolTip {
		device: u32,
		time_usec: u64,
		tool: TabletTool,
		state: TipState,
	},
	TabletToolButton {
		device: u32,
		time_usec: u64,
		tool: TabletTool,
		button: u32,
		state: ButtonState,
	},
	TablePadButton {
		device: u32,
		time_usec: u64,
		button: u32,
		state: ButtonState,
	},
	TablePadRing {
		device: u32,
		time_usec: u64,
		ring: u32,
		position: f64,
		source: AxisSource,
	},
	TablePadStrip {
		device: u32,
		time_usec: u64,
		strip: u32,
		position: f64,
		source: AxisSource,
	},
	SwitchToggle {
		device: u32,
		time_usec: u64,
		switch: SwitchType,
		state: SwitchState,
	},

	// ======================
	// Gestures (NEW)
	// ======================
	GestureSwipeBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	GestureSwipeUpdate {
		device: u32,
		time_usec: u64,
		fingers: u32,
		dx: f64,
		dy: f64,
	},
	GestureSwipeEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},

	GesturePinchBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	GesturePinchUpdate {
		device: u32,
		time_usec: u64,
		fingers: u32,
		dx: f64,
		dy: f64,
		scale: f64,
		rotation: f64,
	},
	GesturePinchEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},

	GestureHoldBegin {
		device: u32,
		time_usec: u64,
		fingers: u32,
	},
	GestureHoldEnd {
		device: u32,
		time_usec: u64,
		cancelled: bool,
	},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
	Pressed,
	Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyState {
	Pressed,
	Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TipState {
	Down,
	Up,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchContact {
	pub id: i32,
	pub x: f64,
	pub y: f64,
	pub x_transformed: f64,
	pub y_transformed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabletTool {
	pub serial: u64,
	pub tool_type: TabletToolType,
	pub capability: TabletToolCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabletToolType {
	Pen,
	Eraser,
	Brush,
	Pencil,
	Airbrush,
	Finger,
	Mouse,
	Lens,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabletToolCapability {
	pub pressure: bool,
	pub distance: bool,
	pub tilt: bool,
	pub rotation: bool,
	pub slider: bool,
	pub wheel: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabletToolAxes {
	pub x: f64,
	pub y: f64,
	pub pressure: Option<f64>,
	pub distance: Option<f64>,
	pub tilt_x: Option<f64>,
	pub tilt_y: Option<f64>,
	pub rotation: Option<f64>,
	pub slider: Option<f64>,
	pub wheel_delta: Option<f64>,
	pub buttons: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisOrientation {
	Vertical,
	Horizontal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisSource {
	Wheel,
	Finger,
	Continuous,
	WheelTilt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchType {
	Lid,
	TabletMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchState {
	On,
	Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorAddedPayload {
	pub monitor: MonitorInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorRemovedPayload {
	pub monitor_id: String,
	pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSwitchPayload {
	pub session_id: String,
	pub animation: Option<String>,
	pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCreatePayload {
	pub role: SessionRole,
	pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCreatedPayload {
	pub session: SessionInfo,
	pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReadyPayload {
	pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatePayload {
	pub session: SessionInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionActivePayload {
	pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAwakePayload {
	pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSleepPayload {
	pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
	pub code: String,
	pub message: Option<String>,
}

pub use message_header::MessageHeader;
pub mod message_header;

mod error;
pub use error::*;

pub use crate::message_frame::{TabMessageFrame, TabMessageFrameReader};
