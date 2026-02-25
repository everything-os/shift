use std::os::fd::RawFd;
use std::time::Duration;

use anyhow::Context as _;
use tab_app_framework_core as core;
use tab_app_framework_xkb::XkbEngine;
use tracing::error;

use crate::{GlContext, GlError, GlVersion};

/// GL-specialized application trait.
///
/// This mirrors [`tab_app_framework_core::Application`] while exposing
/// OpenGL helpers through [`GlEventContext`].
pub trait GlApplication: Sized + 'static {
	/// Constructs the application with access to GL initialization state.
	fn init(ctx: &mut GlInitContext) -> anyhow::Result<Self>;

	/// Called after a buffer is acquired and bound as current render target.
	fn on_render(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::RenderEvent) {}
	/// Called when a rendered buffer is presented/released.
	fn on_present(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::PresentEvent) {}
	/// Called when a monitor is added.
	fn on_monitor_added(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::MonitorAddedEvent,
	) {
	}
	/// Called when a monitor is removed.
	fn on_monitor_removed(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::MonitorRemovedEvent,
	) {
	}
	/// Called when session state updates arrive.
	fn on_session_state(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::SessionEvent) {
	}
	/// Called for every raw input payload.
	fn on_input(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::InputEvent) {}
	/// Called for key events.
	fn on_key(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::KeyEvent) {}
	/// Called for composed text events.
	fn on_char(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::CharEvent) {}
	/// Called when any pointer device moves the cursor.
	fn on_pointer_move(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::PointerMoveEvent,
	) {
	}
	/// Called when a mouse-like device moves the cursor.
	fn on_mouse_move(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::MouseMoveEvent,
	) {
	}
	/// Called when any pointer device produces a down transition.
	fn on_pointer_down(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::PointerDownEvent,
	) {
	}
	/// Called when any pointer device produces an up transition.
	fn on_pointer_up(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::PointerUpEvent,
	) {
	}
	/// Called when a mouse-like device produces a down transition.
	fn on_mouse_down(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::MouseDownEvent,
	) {
	}
	/// Called when a mouse-like device produces an up transition.
	fn on_mouse_up(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: core::MouseUpEvent,
	) {
	}
	/// Called for multitouch contact lifecycle events.
	fn on_touch(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::TouchEvent) {}
	/// Called for high-level multi-finger gesture events.
	fn on_gesture(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::GestureEvent) {}
	/// Called when a watched FD is readable.
	fn on_fd_ready(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, _ev: core::FdReadyEvent) {}
	/// Called when framework errors are surfaced.
	fn on_error(&mut self, _ctx: &mut GlEventContext<'_, '_, Self>, error: &core::FrameworkError) {
		error!(target: "tab_app_framework.gl", error = %error, "tab-app-framework-gl error");
	}
}

/// Initialization context used by [`GlApplication::init`].
pub struct GlInitContext {
	gl: GlContext,
}

impl GlInitContext {
	fn new(gl: GlContext) -> Self {
		Self { gl }
	}

	/// Returns immutable access to the GL context.
	pub fn gl(&self) -> &GlContext {
		&self.gl
	}

	/// Returns mutable access to the GL context.
	pub fn gl_mut(&mut self) -> &mut GlContext {
		&mut self.gl
	}

	fn into_parts(self) -> GlContext {
		self.gl
	}
}

/// Callback context for GL applications.
pub struct GlEventContext<'c, 'g, A: GlApplication> {
	core: &'g mut core::Context<'c, GlBridge<A>>,
	gl: &'g mut GlContext,
}

impl<'c, 'g, A: GlApplication> GlEventContext<'c, 'g, A> {
	/// Schedules a frame for a specific monitor.
	pub fn schedule_frame(&mut self, monitor_id: impl Into<String>) {
		self.core.schedule_frame(monitor_id);
	}

	/// Schedules a frame for every monitor.
	pub fn schedule_all_frames(&mut self) {
		self.core.schedule_all_frames();
	}

	/// Adds a file descriptor to the readable watch set.
	pub fn watch_fd(&mut self, fd: RawFd) {
		self.core.watch_fd(fd);
	}

	/// Removes a file descriptor from the watch set.
	pub fn unwatch_fd(&mut self, fd: RawFd) {
		self.core.unwatch_fd(fd);
	}

	/// Requests framework shutdown.
	pub fn request_exit(&mut self) {
		self.core.request_exit();
	}

	/// Returns current session information.
	pub fn session(&self) -> &core::SessionInfo {
		self.core.session()
	}

	/// Returns all monitors.
	pub fn monitors(&self) -> impl Iterator<Item = &core::Monitor> {
		self.core.monitors()
	}

	/// Returns monitor metadata by id.
	pub fn monitor(&self, monitor_id: &str) -> Option<&core::Monitor> {
		self.core.monitor(monitor_id)
	}

	/// Sets monitor position in the global monitor layout.
	pub fn set_monitor_position(
		&mut self,
		monitor_id: &str,
		x: i32,
		y: i32,
	) -> Result<(), core::FrameworkError> {
		self.core.set_monitor_position(monitor_id, x, y)
	}

	/// Applies default horizontal monitor layout.
	pub fn apply_horizontal_layout(&mut self) {
		self.core.apply_horizontal_layout();
	}

	/// Returns current cursor position in global layout space.
	pub fn cursor_position(&self) -> (f64, f64) {
		self.core.cursor_position()
	}

	/// Returns immutable access to GL context.
	pub fn gl(&self) -> &GlContext {
		self.gl
	}

	/// Returns mutable access to GL context.
	pub fn gl_mut(&mut self) -> &mut GlContext {
		self.gl
	}

	/// Resolves a GL procedure by name.
	pub fn load_proc(&self, name: &str) -> Result<*const std::ffi::c_void, GlError> {
		self.gl.load_proc(name)
	}

	/// Returns currently bound framebuffer object id.
	pub fn current_fbo(&self) -> i32 {
		self.gl.current_fbo()
	}

	/// Sends `session_ready` for the current session.
	pub fn session_ready(&mut self) -> Result<(), core::FrameworkError> {
		self.core.session_ready()
	}

	/// Backward-compatible alias for [`GlEventContext::session_ready`].
	pub fn send_ready(&mut self) -> Result<(), core::FrameworkError> {
		self.session_ready()
	}

	/// Requests creation of a new session and waits for completion.
	pub fn create_session(
		&mut self,
		role: core::SessionRole,
		display_name: Option<String>,
	) -> Result<core::SessionCreatedPayload, core::FrameworkError> {
		self.core.create_session(role, display_name)
	}

	/// Requests switching to another session.
	pub fn switch_session(
		&mut self,
		session_id: &str,
		animation: Option<String>,
		duration: Duration,
	) -> Result<(), core::FrameworkError> {
		self.core.switch_session(session_id, animation, duration)
	}
}

/// High-level GL framework wrapper around the core runtime.
pub struct GlTabAppFramework<A: GlApplication> {
	inner: core::TabAppFramework<GlBridge<A>>,
}

impl<A: GlApplication> GlTabAppFramework<A> {
	/// Initializes a GL application runtime.
	pub fn init(configure: impl FnOnce(&mut core::Config)) -> Result<Self, core::FrameworkError> {
		let inner = core::TabAppFramework::<GlBridge<A>>::init(configure)?;
		Ok(Self { inner })
	}

	/// Runs the application loop until exit.
	pub fn run(&mut self) -> Result<(), core::FrameworkError> {
		self.inner.run()
	}
}

struct GlBridge<A: GlApplication> {
	app: A,
	gl: GlContext,
	xkb: XkbEngine,
}

impl<A: GlApplication> core::Application for GlBridge<A> {
	fn init(ctx: &mut core::InitContext<Self>) -> anyhow::Result<Self> {
		let (major, minor) = ctx.config().requested_opengl_version();
		let version = GlVersion { major, minor };
		let gl = GlContext::new(version, ctx.config().render_node_path())
			.context("failed to create GL context")?;
		let mut init = GlInitContext::new(gl);
		let app = A::init(&mut init)?;
		let xkb = XkbEngine::new().context("failed to initialize xkb engine")?;
		Ok(Self {
			app,
			gl: init.into_parts(),
			xkb,
		})
	}

	fn on_render(&mut self, ctx: &mut core::Context<Self>, ev: core::RenderEvent) {
		if let Err(err) = self.gl.make_current() {
			let ferr = core::FrameworkError::Config(format!("gl make current failed: {err}"));
			self.on_error(ctx, &ferr);
			return;
		}
		if let Err(err) = self.gl.prepare_render_target(&ev) {
			let ferr = core::FrameworkError::Config(format!("prepare render target failed: {err}"));
			self.on_error(ctx, &ferr);
			return;
		}
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_render(&mut ctx, ev);
		match ctx.gl.create_acquire_fence_fd() {
			Ok(fence_fd) => ctx.core.set_next_acquire_fence(fence_fd),
			Err(err) => {
				let ferr = core::FrameworkError::Config(format!("create acquire fence failed: {err}"));
				self.app.on_error(&mut ctx, &ferr);
			}
		}
	}

	fn on_present(&mut self, ctx: &mut core::Context<Self>, ev: core::PresentEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_present(&mut ctx, ev);
	}

	fn on_monitor_added(&mut self, ctx: &mut core::Context<Self>, ev: core::MonitorAddedEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_monitor_added(&mut ctx, ev);
	}

	fn on_monitor_removed(&mut self, ctx: &mut core::Context<Self>, ev: core::MonitorRemovedEvent) {
		self.gl.release_monitor_targets(&ev.monitor_id);
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_monitor_removed(&mut ctx, ev);
	}

	fn on_session_state(&mut self, ctx: &mut core::Context<Self>, ev: core::SessionEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_session_state(&mut ctx, ev);
	}

	fn on_input(&mut self, ctx: &mut core::Context<Self>, ev: core::InputEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_input(&mut ctx, ev);
	}

	fn on_key(&mut self, ctx: &mut core::Context<Self>, ev: core::KeyEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		let compose = self.xkb.process_key(ev.key, ev.is_pressed());
		self.app.on_key(&mut ctx, ev.clone());
		if let Some(text) = compose.text {
			self.app.on_char(&mut ctx, core::CharEvent { text });
		}
	}

	fn on_char(&mut self, ctx: &mut core::Context<Self>, ev: core::CharEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_char(&mut ctx, ev);
	}

	fn on_pointer_move(&mut self, ctx: &mut core::Context<Self>, ev: core::PointerMoveEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_pointer_move(&mut ctx, ev);
	}

	fn on_mouse_move(&mut self, ctx: &mut core::Context<Self>, ev: core::MouseMoveEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_mouse_move(&mut ctx, ev);
	}

	fn on_pointer_down(&mut self, ctx: &mut core::Context<Self>, ev: core::PointerDownEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_pointer_down(&mut ctx, ev);
	}

	fn on_pointer_up(&mut self, ctx: &mut core::Context<Self>, ev: core::PointerUpEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_pointer_up(&mut ctx, ev);
	}

	fn on_mouse_down(&mut self, ctx: &mut core::Context<Self>, ev: core::MouseDownEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_mouse_down(&mut ctx, ev);
	}

	fn on_mouse_up(&mut self, ctx: &mut core::Context<Self>, ev: core::MouseUpEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_mouse_up(&mut ctx, ev);
	}

	fn on_touch(&mut self, ctx: &mut core::Context<Self>, ev: core::TouchEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_touch(&mut ctx, ev);
	}

	fn on_gesture(&mut self, ctx: &mut core::Context<Self>, ev: core::GestureEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_gesture(&mut ctx, ev);
	}

	fn on_fd_ready(&mut self, ctx: &mut core::Context<Self>, ev: core::FdReadyEvent) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_fd_ready(&mut ctx, ev);
	}

	fn on_error(&mut self, ctx: &mut core::Context<Self>, error: &core::FrameworkError) {
		let mut ctx = GlEventContext {
			core: ctx,
			gl: &mut self.gl,
		};
		self.app.on_error(&mut ctx, error);
	}
}
