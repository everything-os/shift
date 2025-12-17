use super::*;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabInputEventKind {
	TabInputPointerMotion = 0,
	TabInputPointerMotionAbsolute = 1,
	TabInputPointerButton = 2,
	TabInputPointerAxis = 3,
	TabInputPointerAxisStop = 4,
	TabInputPointerAxisDiscrete = 5,
	TabInputKey = 6,
	TabInputTouchDown = 7,
	TabInputTouchUp = 8,
	TabInputTouchMotion = 9,
	TabInputTouchFrame = 10,
	TabInputTouchCancel = 11,
	TabInputTabletToolProximity = 12,
	TabInputTabletToolAxis = 13,
	TabInputTabletToolTip = 14,
	TabInputTabletToolButton = 15,
	TabInputTabletPadButton = 16,
	TabInputTabletPadRing = 17,
	TabInputTabletPadStrip = 18,
	TabInputSwitchToggle = 19,
}

// ============================================================================
// STRUCTURES - Input Events (Tagged Union)
// ============================================================================

/// Pointer motion event
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

/// Absolute pointer position event
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
pub enum TabButtonState {
	TabButtonPressed = 0,
	TabButtonReleased = 1,
}

/// Pointer button event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerButton {
	pub device: u32,
	pub time_usec: u64,
	pub button: u32,
	pub state: TabButtonState,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabAxisOrientation {
	TabAxisVertical = 0,
	TabAxisHorizontal = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabAxisSource {
	TabAxisSourceWheel = 0,
	TabAxisSourceFinger = 1,
	TabAxisSourceContinuous = 2,
	TabAxisSourceWheelTilt = 3,
}

/// Pointer axis event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerAxis {
	pub device: u32,
	pub time_usec: u64,
	pub orientation: TabAxisOrientation,
	pub delta: f64,
	pub delta_discrete: i32, // -1 means NULL/invalid
	pub source: TabAxisSource,
}

/// Pointer axis stop event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerAxisStop {
	pub device: u32,
	pub time_usec: u64,
	pub orientation: TabAxisOrientation,
}

/// Pointer discrete axis event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputPointerAxisDiscrete {
	pub device: u32,
	pub time_usec: u64,
	pub orientation: TabAxisOrientation,
	pub delta_discrete: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabKeyState {
	TabKeyPressed = 0,
	TabKeyReleased = 1,
}

/// Keyboard key event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputKey {
	pub device: u32,
	pub time_usec: u64,
	pub key: u32,
	pub state: TabKeyState,
}

/// Touch contact point
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTouchContact {
	pub id: i32,
	pub x: f64,
	pub y: f64,
	pub x_transformed: f64,
	pub y_transformed: f64,
}

/// Touch down event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchDown {
	pub device: u32,
	pub time_usec: u64,
	pub contact: TabTouchContact,
}

/// Touch motion event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchMotion {
	pub device: u32,
	pub time_usec: u64,
	pub contact: TabTouchContact,
}

/// Touch up event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchUp {
	pub device: u32,
	pub time_usec: u64,
	pub contact_id: i32,
}

/// Touch frame/sync event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchFrame {
	pub time_usec: u64,
}

/// Touch cancel event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTouchCancel {
	pub time_usec: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTabletTool {
	pub serial: u64,
	pub tool_type: u8, // encoded as u8 (0=pen, 1=eraser, etc)
}

/// Tablet tool proximity event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolProximity {
	pub device: u32,
	pub time_usec: u64,
	pub in_proximity: bool,
	pub tool: TabTabletTool,
}

/// Tablet tool axes state
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabTabletToolAxes {
	pub x: f64,
	pub y: f64,
	pub pressure: f64,      // -1.0 = invalid
	pub distance: f64,      // -1.0 = invalid
	pub tilt_x: f64,        // -1.0 = invalid
	pub tilt_y: f64,        // -1.0 = invalid
	pub rotation: f64,      // -1.0 = invalid
	pub slider: f64,        // -1.0 = invalid
	pub wheel_delta: f64,   // -1.0 = invalid
}

/// Tablet tool axis event
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
pub enum TabTipState {
	TabTipDown = 0,
	TabTipUp = 1,
}

/// Tablet tool tip event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolTip {
	pub device: u32,
	pub time_usec: u64,
	pub tool: TabTabletTool,
	pub state: TabTipState,
}

/// Tablet tool button event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletToolButton {
	pub device: u32,
	pub time_usec: u64,
	pub tool: TabTabletTool,
	pub button: u32,
	pub state: TabButtonState,
}

/// Tablet pad button event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadButton {
	pub device: u32,
	pub time_usec: u64,
	pub button: u32,
	pub state: TabButtonState,
}

/// Tablet pad ring event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadRing {
	pub device: u32,
	pub time_usec: u64,
	pub ring: u32,
	pub position: f64,
	pub source: TabAxisSource,
}

/// Tablet pad strip event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputTabletPadStrip {
	pub device: u32,
	pub time_usec: u64,
	pub strip: u32,
	pub position: f64,
	pub source: TabAxisSource,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabSwitchType {
	TabSwitchLid = 0,
	TabSwitchTabletMode = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum TabSwitchState {
	TabSwitchOn = 0,
	TabSwitchOff = 1,
}

/// Switch toggle event
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TabInputSwitchToggle {
	pub device: u32,
	pub time_usec: u64,
	pub switch_type: TabSwitchType,
	pub state: TabSwitchState,
}

// ============================================================================
// TAGGED UNION FOR INPUT EVENTS
// ============================================================================

#[repr(C)]
#[derive(Clone, Copy)]
pub union TabInputEventData {
	pub pointer_motion: TabInputPointerMotion,
	pub pointer_motion_absolute: TabInputPointerMotionAbsolute,
	pub pointer_button: TabInputPointerButton,
	pub pointer_axis: TabInputPointerAxis,
	pub pointer_axis_stop: TabInputPointerAxisStop,
	pub pointer_axis_discrete: TabInputPointerAxisDiscrete,
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
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TabInputEvent {
	pub kind: TabInputEventKind,
	pub data: TabInputEventData,
}


#[allow(dead_code)]
pub(super) fn convert_input_event(payload: &InputEventPayload) -> TabInputEvent {
	let kind: TabInputEventKind;
	let mut data: TabInputEventData = unsafe { std::mem::zeroed() };

	match payload {
		InputEventPayload::PointerMotion {
			device,
			time_usec,
			x,
			y,
			dx,
			dy,
			unaccel_dx,
			unaccel_dy,
		} => {
			kind = TabInputEventKind::TabInputPointerMotion;
			unsafe {
                data.pointer_motion = TabInputPointerMotion {
                    device: *device,
                    time_usec: *time_usec,
                    x: *x,
                    y: *y,
                    dx: *dx,
                    dy: *dy,
					unaccel_dx: *unaccel_dx,
					unaccel_dy: *unaccel_dy,
                };
            }
		}
		InputEventPayload::PointerMotionAbsolute {
			device,
			time_usec,
			x,
			y,
			x_transformed,
			y_transformed,
		} => {
			kind = TabInputEventKind::TabInputPointerMotionAbsolute;
			unsafe {
                data.pointer_motion_absolute = TabInputPointerMotionAbsolute {
                    device: *device,
                    time_usec: *time_usec,
                    x: *x,
                    y: *y,
                    x_transformed: *x_transformed,
                    y_transformed: *y_transformed,
                };
            }
		}
		InputEventPayload::PointerButton {
			device,
			time_usec,
			button,
			state,
		} => {
			let btn_state = match state {
				ButtonState::Pressed => TabButtonState::TabButtonPressed,
				ButtonState::Released => TabButtonState::TabButtonReleased,
			};
			kind = TabInputEventKind::TabInputPointerButton;
			unsafe {
                data.pointer_button = TabInputPointerButton {
                    device: *device,
                    time_usec: *time_usec,
                    button: *button,
                    state: btn_state,
                };
            }
		}
		InputEventPayload::PointerAxis {
			device,
			time_usec,
			orientation,
			delta,
			delta_discrete,
			source,
		} => {
			let axis_orientation = match orientation {
				AxisOrientation::Vertical => TabAxisOrientation::TabAxisVertical,
				AxisOrientation::Horizontal => TabAxisOrientation::TabAxisHorizontal,
			};
			let axis_source = match source {
				AxisSource::Wheel => TabAxisSource::TabAxisSourceWheel,
				AxisSource::Finger => TabAxisSource::TabAxisSourceFinger,
				AxisSource::Continuous => TabAxisSource::TabAxisSourceContinuous,
				AxisSource::WheelTilt => TabAxisSource::TabAxisSourceWheelTilt,
			};
			kind = TabInputEventKind::TabInputPointerAxis;
			unsafe {
                data.pointer_axis = TabInputPointerAxis {
                    device: *device,
                    time_usec: *time_usec,
                    orientation: axis_orientation,
                    delta: *delta,
                    delta_discrete: delta_discrete.unwrap_or(-1),
                    source: axis_source,
                };
            }
		}
		InputEventPayload::PointerAxisStop {
			device,
			time_usec,
			orientation,
		} => {
			let axis_orientation = match orientation {
				AxisOrientation::Vertical => TabAxisOrientation::TabAxisVertical,
				AxisOrientation::Horizontal => TabAxisOrientation::TabAxisHorizontal,
			};
			kind = TabInputEventKind::TabInputPointerAxisStop;
			unsafe {
                data.pointer_axis_stop = TabInputPointerAxisStop {
                    device: *device,
                    time_usec: *time_usec,
                    orientation: axis_orientation,
                };
            }
		}
		InputEventPayload::PointerAxisDiscrete {
			device,
			time_usec,
			orientation,
			delta_discrete,
		} => {
			let axis_orientation = match orientation {
				AxisOrientation::Vertical => TabAxisOrientation::TabAxisVertical,
				AxisOrientation::Horizontal => TabAxisOrientation::TabAxisHorizontal,
			};
			kind = TabInputEventKind::TabInputPointerAxisDiscrete;
			unsafe {
                data.pointer_axis_discrete = TabInputPointerAxisDiscrete {
                    device: *device,
                    time_usec: *time_usec,
                    orientation: axis_orientation,
                    delta_discrete: *delta_discrete,
                };
            }
		}
		InputEventPayload::Key {
			device,
			time_usec,
			key,
			state,
		} => {
			let key_state = match state {
				KeyState::Pressed => TabKeyState::TabKeyPressed,
				KeyState::Released => TabKeyState::TabKeyReleased,
			};
			kind = TabInputEventKind::TabInputKey;
			unsafe {
                data.key = TabInputKey {
                    device: *device,
                    time_usec: *time_usec,
                    key: *key,
                    state: key_state,
                };
            }
		}
		InputEventPayload::TouchDown { device, time_usec, contact } => {
			kind = TabInputEventKind::TabInputTouchDown;
			unsafe {
                data.touch_down = TabInputTouchDown {
                    device: *device,
                    time_usec: *time_usec,
                    contact: TabTouchContact {
                        id: contact.id,
                        x: contact.x,
                        y: contact.y,
                        x_transformed: contact.x_transformed,
                        y_transformed: contact.y_transformed,
                    },
                };
            }
		}
		InputEventPayload::TouchUp { device, time_usec, contact_id } => {
			kind = TabInputEventKind::TabInputTouchUp;
			unsafe {
                data.touch_up = TabInputTouchUp {
                    device: *device,
                    time_usec: *time_usec,
                    contact_id: *contact_id,
                };
            }
		}
		InputEventPayload::TouchMotion { device, time_usec, contact } => {
			kind = TabInputEventKind::TabInputTouchMotion;
			unsafe {
                data.touch_motion = TabInputTouchMotion {
                    device: *device,
                    time_usec: *time_usec,
                    contact: TabTouchContact {
                        id: contact.id,
                        x: contact.x,
                        y: contact.y,
                        x_transformed: contact.x_transformed,
                        y_transformed: contact.y_transformed,
                    },
                };
            }
		}
		InputEventPayload::TouchFrame { time_usec } => {
			kind = TabInputEventKind::TabInputTouchFrame;
			unsafe {
                data.touch_frame = TabInputTouchFrame {
                    time_usec: *time_usec,
                };
            }
		}
		InputEventPayload::TouchCancel { time_usec } => {
			kind = TabInputEventKind::TabInputTouchCancel;
			unsafe {
                data.touch_cancel = TabInputTouchCancel {
                    time_usec: *time_usec,
                };
            }
		}
		_ => {
			unimplemented!("Input event conversion not implemented for this variant");
		}
	};

	TabInputEvent { kind, data }
}
