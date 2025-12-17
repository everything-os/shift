use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::OwnedFd;
use std::path::Path;

use input::AsRaw;
use input::event::device::DeviceEvent;
use input::event::keyboard::{KeyState as LibinputKeyState, KeyboardEvent, KeyboardEventTrait};
#[allow(deprecated)]
use input::event::pointer::{
	Axis as PointerAxis, AxisSource as PointerAxisSource, ButtonState as PointerButtonState,
	PointerAxisEvent, PointerEvent, PointerEventTrait, PointerMotionAbsoluteEvent,
	PointerMotionEvent, PointerScrollContinuousEvent, PointerScrollEvent, PointerScrollFingerEvent,
	PointerScrollWheelEvent,
};
use input::event::switch::{
	Switch, SwitchEvent, SwitchEventTrait, SwitchState as LibinputSwitchState,
};
use input::event::touch::{
	TouchDownEvent, TouchEvent, TouchEventPosition, TouchEventSlot, TouchEventTrait,
	TouchMotionEvent, TouchUpEvent,
};
use input::event::{Event, EventTrait};
use input::{Device, Libinput, LibinputInterface};
use libc::{O_RDONLY, O_RDWR, O_WRONLY};
use tab_protocol::{
	AxisOrientation, AxisSource, ButtonState, InputEventPayload, KeyState, SwitchState, SwitchType,
	TouchContact,
};
use tracing::trace;

use crate::error::ShiftError;

pub struct InputManager {
	ctx: Libinput,
	cursor: CursorState,
	transform_size: (u32, u32),
	device_ids: HashMap<usize, u32>,
	next_device_id: u32,
}

impl InputManager {
	pub fn new() -> Result<Self, ShiftError> {
		let mut ctx = Libinput::new_with_udev(ShiftInputInterface::default());
		ctx
			.udev_assign_seat("seat0")
			.map_err(|_| ShiftError::Libinput("failed to assign libinput seat".into()))?;
		
		Ok(Self {
			ctx,
			cursor: CursorState::default(),
			transform_size: (1, 1),
			device_ids: HashMap::new(),
			next_device_id: 1,
		})
	}

	pub fn fd(&self) -> RawFd {
		self.ctx.as_raw_fd()
	}

	pub fn set_transform_size(&mut self, width: u32, height: u32) {
		self.transform_size = (width.max(1), height.max(1));
	}

	pub fn dispatch_events<F>(&mut self, mut handler: F) -> Result<(), ShiftError>
	where
		F: FnMut(InputEventPayload),
	{
		match self.ctx.dispatch() {
			Ok(()) => {}
			Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(()),
			Err(err) => return Err(err.into()),
		}
		let mut pending_events = Vec::new();
		for event in &mut self.ctx {
			pending_events.push(event);
		}
		for event in pending_events {
			self.handle_event(event, &mut handler);
		}
		Ok(())
	}

	fn handle_event<F>(&mut self, event: Event, handler: &mut F)
	where
		F: FnMut(InputEventPayload),
	{
		match event {
			Event::Device(device_event) => self.handle_device_event(device_event),
			Event::Keyboard(event) => {
				if let Some(payload) = self.convert_keyboard_event(event) {
					handler(payload);
				}
			}
			Event::Pointer(event) => {
				for payload in self.convert_pointer_event(event) {
					handler(payload);
				}
			}
			Event::Touch(event) => {
				for payload in self.convert_touch_event(event) {
					handler(payload);
				}
			}
			Event::Switch(event) => {
				if let Some(payload) = self.convert_switch_event(event) {
					handler(payload);
				}
			}
			other => {
				trace!(?other, "Unhandled libinput event");
			}
		}
	}

	fn handle_device_event(&mut self, event: DeviceEvent) {
		match event {
			DeviceEvent::Added(ev) => {
				let device_id = self.device_id_for(&ev.device());
				trace!(device_id, "Device added");
			}
			DeviceEvent::Removed(ev) => {
				let key = Self::device_key(&ev.device());
				self.device_ids.remove(&key);
				trace!(device_key = key, "Device removed");
			}
			_ => {}
		}
	}

	fn convert_keyboard_event(&mut self, event: KeyboardEvent) -> Option<InputEventPayload> {
		match event {
			KeyboardEvent::Key(ev) => {
				let device = self.device_id_for(&ev.device());
				Some(InputEventPayload::Key {
					device,
					time_usec: ev.time_usec(),
					key: ev.key(),
					state: match ev.key_state() {
						LibinputKeyState::Pressed => KeyState::Pressed,
						LibinputKeyState::Released => KeyState::Released,
					},
				})
			}
			_ => None,
		}
	}

	#[allow(deprecated)]
	fn convert_pointer_event(&mut self, event: PointerEvent) -> Vec<InputEventPayload> {
		match event {
			PointerEvent::Motion(ev) => self.pointer_motion(ev),
			PointerEvent::MotionAbsolute(ev) => self.pointer_motion_absolute(ev),
			PointerEvent::Button(ev) => self.pointer_button(ev),
			PointerEvent::Axis(ev) => self.pointer_axis(ev),
			PointerEvent::ScrollWheel(ev) => self.pointer_scroll_wheel(ev),
			PointerEvent::ScrollFinger(ev) => self.pointer_scroll_finger(ev),
			PointerEvent::ScrollContinuous(ev) => self.pointer_scroll_continuous(ev),
			_ => Vec::new(),
		}
	}

	fn pointer_motion(&mut self, event: PointerMotionEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		let dx = event.dx();
		let dy = event.dy();
		let unaccel_dx = event.dx_unaccelerated();
		let unaccel_dy = event.dy_unaccelerated();
		let (x, y) = self.cursor.update_relative(dx, dy);
		
		vec![InputEventPayload::PointerMotion {
			device,
			time_usec: event.time_usec(),
			x,
			y,
			dx,
			dy,
			unaccel_dx,
			unaccel_dy,
		}]
	}

	fn pointer_motion_absolute(
		&mut self,
		event: PointerMotionAbsoluteEvent,
	) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		let width = self.transform_size.0.max(1);
		let height = self.transform_size.1.max(1);
		let x_transformed = event.absolute_x_transformed(width);
		let y_transformed = event.absolute_y_transformed(height);
		let x = event.absolute_x();
		let y = event.absolute_y();
		self.cursor.update_absolute(x_transformed, y_transformed);
		vec![InputEventPayload::PointerMotionAbsolute {
			device,
			time_usec: event.time_usec(),
			x,
			y,
			x_transformed,
			y_transformed,
		}]
	}

	fn pointer_button(
		&mut self,
		event: input::event::pointer::PointerButtonEvent,
	) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		let state = match event.button_state() {
			PointerButtonState::Pressed => ButtonState::Pressed,
			PointerButtonState::Released => ButtonState::Released,
		};
		vec![InputEventPayload::PointerButton {
			device,
			time_usec: event.time_usec(),
			button: event.button(),
			state,
		}]
	}

	#[allow(deprecated)]
	fn pointer_axis(&mut self, event: PointerAxisEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		self.collect_axis_payloads(
			device,
			event.time_usec(),
			axis_source_from_pointer(event.axis_source()),
			|axis| event.has_axis(axis),
			|axis| event.axis_value(axis),
			|axis| event.axis_value_discrete(axis).map(|v| v.round() as i32),
		)
	}

	fn pointer_scroll_wheel(&mut self, event: PointerScrollWheelEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		self.collect_axis_payloads(
			device,
			event.time_usec(),
			AxisSource::Wheel,
			|axis| event.has_axis(axis),
			|axis| event.scroll_value(axis),
			|axis| {
				let v120 = event.scroll_value_v120(axis);
				if v120.abs() < f64::EPSILON {
					None
				} else {
					Some((v120 / 120.0).round() as i32)
				}
			},
		)
	}

	fn pointer_scroll_finger(&mut self, event: PointerScrollFingerEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		self.collect_axis_payloads(
			device,
			event.time_usec(),
			AxisSource::Finger,
			|axis| event.has_axis(axis),
			|axis| event.scroll_value(axis),
			|_| None,
		)
	}

	fn pointer_scroll_continuous(
		&mut self,
		event: PointerScrollContinuousEvent,
	) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		self.collect_axis_payloads(
			device,
			event.time_usec(),
			AxisSource::Continuous,
			|axis| event.has_axis(axis),
			|axis| event.scroll_value(axis),
			|_| None,
		)
	}

	fn collect_axis_payloads<FHas, FValue, FDiscrete>(
		&self,
		device: u32,
		time_usec: u64,
		source: AxisSource,
		has_axis: FHas,
		value: FValue,
		discrete: FDiscrete,
	) -> Vec<InputEventPayload>
	where
		FHas: Fn(PointerAxis) -> bool,
		FValue: Fn(PointerAxis) -> f64,
		FDiscrete: Fn(PointerAxis) -> Option<i32>,
	{
		let mut events = Vec::new();
		for axis in [PointerAxis::Vertical, PointerAxis::Horizontal] {
			if !has_axis(axis) {
				continue;
			}
			let delta = value(axis);
			if delta.abs() < f64::EPSILON {
				events.push(InputEventPayload::PointerAxisStop {
					device,
					time_usec,
					orientation: axis_orientation_from_pointer(axis),
				});
			} else {
				events.push(InputEventPayload::PointerAxis {
					device,
					time_usec,
					orientation: axis_orientation_from_pointer(axis),
					delta,
					delta_discrete: discrete(axis),
					source: source.clone(),
				});
			}
			if let Some(steps) = discrete(axis) {
				if steps != 0 {
					events.push(InputEventPayload::PointerAxisDiscrete {
						device,
						time_usec,
						orientation: axis_orientation_from_pointer(axis),
						delta_discrete: steps,
					});
				}
			}
		}
		events
	}

	fn convert_touch_event(&mut self, event: TouchEvent) -> Vec<InputEventPayload> {
		match event {
			TouchEvent::Down(ev) => self.touch_down(ev),
			TouchEvent::Up(ev) => self.touch_up(ev),
			TouchEvent::Motion(ev) => self.touch_motion(ev),
			TouchEvent::Cancel(ev) => vec![InputEventPayload::TouchCancel {
				time_usec: ev.time_usec(),
			}],
			TouchEvent::Frame(ev) => vec![InputEventPayload::TouchFrame {
				time_usec: ev.time_usec(),
			}],
			_ => Vec::new(),
		}
	}

	fn touch_down(&mut self, event: TouchDownEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		let contact = self.make_touch_contact(&event);
		vec![InputEventPayload::TouchDown {
			device,
			time_usec: event.time_usec(),
			contact,
		}]
	}

	fn touch_up(&mut self, event: TouchUpEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		vec![InputEventPayload::TouchUp {
			device,
			time_usec: event.time_usec(),
			contact_id: event.seat_slot() as i32,
		}]
	}

	fn touch_motion(&mut self, event: TouchMotionEvent) -> Vec<InputEventPayload> {
		let device = self.device_id_for(&event.device());
		let contact = self.make_touch_contact(&event);
		vec![InputEventPayload::TouchMotion {
			device,
			time_usec: event.time_usec(),
			contact,
		}]
	}

	fn make_touch_contact<T>(&self, event: &T) -> TouchContact
	where
		T: TouchEventPosition + TouchEventSlot,
	{
		let width = self.transform_size.0.max(1);
		let height = self.transform_size.1.max(1);
		TouchContact {
			id: event.seat_slot() as i32,
			x: event.x(),
			y: event.y(),
			x_transformed: event.x_transformed(width),
			y_transformed: event.y_transformed(height),
		}
	}

	fn convert_switch_event(&mut self, event: SwitchEvent) -> Option<InputEventPayload> {
		match event {
			SwitchEvent::Toggle(ev) => {
				let switch = ev.switch()?;
				let device = self.device_id_for(&ev.device());
				Some(InputEventPayload::SwitchToggle {
					device,
					time_usec: ev.time_usec(),
					switch: match switch {
						Switch::Lid => SwitchType::Lid,
						Switch::TabletMode => SwitchType::TabletMode,
						_ => return None,
					},
					state: match ev.switch_state() {
						LibinputSwitchState::On => SwitchState::On,
						LibinputSwitchState::Off => SwitchState::Off,
					},
				})
			}
			_ => None,
		}
	}

	fn device_id_for(&mut self, device: &Device) -> u32 {
		let key = Self::device_key(device);
		*self.device_ids.entry(key).or_insert_with(|| {
			let id = self.next_device_id;
			self.next_device_id += 1;
			id
		})
	}

	fn device_key(device: &Device) -> usize {
		device.as_raw() as usize
	}
}

#[derive(Default)]
struct CursorState {
	x: f64,
	y: f64,
}

impl CursorState {
	fn update_relative(&mut self, dx: f64, dy: f64) -> (f64, f64) {
		self.x += dx;
		self.y += dy;
		(self.x, self.y)
	}

	fn update_absolute(&mut self, x: f64, y: f64) -> (f64, f64) {
		self.x = x;
		self.y = y;
		(x, y)
	}
}

#[derive(Default)]
struct ShiftInputInterface;

impl LibinputInterface for ShiftInputInterface {
	fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
		let mut options = OpenOptions::new();
		options.custom_flags(flags);
		if flags & O_RDWR != 0 {
			options.read(true).write(true);
		} else if flags & O_WRONLY != 0 {
			options.write(true);
		} else if flags & O_RDONLY != 0 {
			options.read(true);
		} else {
			options.read(true);
		}
		options
			.open(path)
			.map(|file| file.into())
			.map_err(|err| err.raw_os_error().unwrap_or(libc::EINVAL))
	}

	fn close_restricted(&mut self, fd: OwnedFd) {
		drop(fd);
	}
}

#[allow(deprecated)]
fn axis_source_from_pointer(value: PointerAxisSource) -> AxisSource {
	match value {
		PointerAxisSource::Wheel => AxisSource::Wheel,
		PointerAxisSource::Finger => AxisSource::Finger,
		PointerAxisSource::Continuous => AxisSource::Continuous,
		PointerAxisSource::WheelTilt => AxisSource::WheelTilt,
	}
}

fn axis_orientation_from_pointer(axis: PointerAxis) -> AxisOrientation {
	match axis {
		PointerAxis::Vertical => AxisOrientation::Vertical,
		PointerAxis::Horizontal => AxisOrientation::Horizontal,
	}
}
