use std::time::Instant;

use glow::HasContext;
use tab_app_framework::{
	Config, GlApplication, GlEventContext, GlInitContext, GlTabAppFramework, MouseDownEvent,
	MouseUpEvent, RenderEvent, RenderMode,
};
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

struct App {
	start: Instant,
	last_log: Instant,
	frames: u64,
	left_down: bool,
}

impl GlApplication for App {
	fn init(_ctx: &mut GlInitContext) -> anyhow::Result<Self> {
		Ok(Self {
			start: Instant::now(),
			last_log: Instant::now(),
			frames: 0,
			left_down: false,
		})
	}

	fn on_render(&mut self, ctx: &mut GlEventContext<'_, '_, Self>, ev: RenderEvent) {
		let _ = ctx.session();
		self.frames = self.frames.saturating_add(1);
		if self.last_log.elapsed().as_secs_f32() >= 1.0 {
			info!(
				target: "tab_app_framework.example.minimal_gl",
				fps = self.frames,
				"example on_render"
			);
			self.frames = 0;
			self.last_log = Instant::now();
		}

		let t = self.start.elapsed().as_secs_f32();

		let gl = ctx.gl().glow();
		unsafe {
			gl.clear_color(t % 1.0, t % 1.0, t % 1.0, 1.0);
			gl.clear(glow::COLOR_BUFFER_BIT);
		}

		let Some(monitor) = ctx.monitor(&ev.monitor_id) else {
			return;
		};
		let (cursor_x, cursor_y) = ctx.cursor_position();
		let (local_x, local_y) = monitor.cursor_relative_position((cursor_x, cursor_y));
		let radius = if self.left_down { 6 } else { 10 };
		draw_cursor_circle(gl, ev.width, ev.height, local_x as _, local_y as _, radius);
	}

	fn on_mouse_down(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: MouseDownEvent,
	) {
		self.left_down = true;
	}

	fn on_mouse_up(
		&mut self,
		_ctx: &mut GlEventContext<'_, '_, Self>,
		_ev: MouseUpEvent,
	) {
		self.left_down = false;
	}
}
// This is not really the best way but it's the approach that requires the least lines of code
fn draw_cursor_circle(gl: &glow::Context, width: i32, height: i32, cx: i32, cy: i32, radius: i32) {
	if width <= 0 || height <= 0 || radius <= 0 {
		return;
	}
	unsafe {
		gl.enable(glow::SCISSOR_TEST);
		gl.clear_color(1.0, 0.0, 0.0, 1.0);
		for dy in -radius..=radius {
			let y = cy + dy;
			if y < 0 || y >= height {
				continue;
			}
			let span = ((radius * radius - dy * dy) as f64).sqrt().floor() as i32;
			let mut x0 = cx - span;
			let mut x1 = cx + span;
			if x1 < 0 || x0 >= width {
				continue;
			}
			x0 = x0.max(0);
			x1 = x1.min(width - 1);
			gl.scissor(x0, y, x1 - x0 + 1, 1);
			gl.clear(glow::COLOR_BUFFER_BIT);
		}
		gl.disable(glow::SCISSOR_TEST);
	}
}

fn main() -> anyhow::Result<()> {
	let _ = fmt()
		.with_env_filter(
			EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| EnvFilter::new("info,tab_app_framework.core=debug")),
		)
		.try_init();
	if let Err(err) = run() {
		error!(target: "tab_app_framework.example.minimal_gl", error = ?err, "example failed");
		return Err(err);
	}
	Ok(())
}

fn run() -> anyhow::Result<()> {
	let mut app = GlTabAppFramework::<App>::init(|config: &mut Config| {
		config.opengl_version(3, 3);
		config.set_render_mode(RenderMode::Eager);
	})?;
	app.run()?;
	Ok(())
}
