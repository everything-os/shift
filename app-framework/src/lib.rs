//! Convenience re-export crate for the tab app framework.
//!
//! This crate re-exports core runtime, GL integration, XKB helpers, and
//! monitor layout utilities from subcrates.

/// Core runtime APIs.
pub use tab_app_framework_core as core;
/// OpenGL integration APIs.
pub use tab_app_framework_gl as gl;
/// XKB composition APIs.
pub use tab_app_framework_xkb as xkb;
/// Monitor layout utilities.
pub use monitor_layout_engine as monitor_layout;

/// Re-exported core runtime types.
pub use tab_app_framework_core::{
	Application, CharEvent, Config, Context, FdReadyEvent, FrameworkError, GestureEvent,
	InitContext, InputEvent, KeyEvent, Monitor, MonitorAddedEvent, MonitorRemovedEvent, MouseDownEvent,
	MouseMoveEvent, MouseUpEvent, PointerDownEvent, PointerMoveEvent, PointerType, PointerUpEvent,
	PresentEvent, RenderEvent, RenderMode, SessionCreatedPayload, SessionEvent, SessionInfo,
	SessionRole, TabAppFramework, TouchEvent,
};
/// Re-exported GL runtime types.
pub use tab_app_framework_gl::{
	GlApplication, GlContext, GlError, GlEventContext, GlInitContext, GlTabAppFramework, GlVersion,
};
/// Re-exported XKB helper types.
pub use tab_app_framework_xkb::{KeyComposition, Modifiers, XkbEngine, XkbError};
