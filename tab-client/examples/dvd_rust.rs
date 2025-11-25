use std::{
	env,
	error::Error,
	os::fd::AsRawFd,
	process::{Child, Command, Stdio},
	time::{Duration, Instant},
};

use tab_client::{FrameTarget, TabClient, TabClientError, TabEvent, gl};
use tab_protocol::{
	InputEventPayload, KeyState, SessionCreatePayload, SessionRole, TabMessageFrame, message_header,
};

const DVD_BYTES: &[u8] = include_bytes!("dvd.png");
const COLORS: &[[f32; 3]] = &[
	[1.0, 1.0, 1.0],
	[1.0, 0.4, 0.4],
	[0.4, 1.0, 0.5],
	[0.4, 0.7, 1.0],
	[1.0, 0.7, 0.4],
];
const KEY_ENTER: u32 = 28;

fn main() -> Result<(), Box<dyn Error>> {
	let token = env::args()
		.nth(1)
		.or_else(|| env::var("SHIFT_SESSION_TOKEN").ok())
		.expect("Provide a session token via SHIFT_SESSION_TOKEN or argv[1]");
	let session_client_bin = env::var("DVD_EXAMPLE_SESSION_CLIENT_BIN")
		.expect("Set DVD_EXAMPLE_SESSION_CLIENT_BIN to the normal session client binary");

	let mut client = TabClient::connect_default(token)?;
	println!(
		"Connected to Shift server '{}' via protocol {}",
		client.hello().server,
		client.hello().protocol
	);

	let mut monitor_id = client.monitor_ids().into_iter().next();
	let mut spawned_sessions: Vec<Child> = Vec::new();

	if monitor_id.is_none() {
		println!("Waiting for a monitor from Shift...");
		while monitor_id.is_none() {
			let events = pump_events(&mut client, true)?;
			handle_events(
				&mut client,
				&events,
				&mut monitor_id,
				&session_client_bin,
				&mut spawned_sessions,
			)?;
		}
	}

	if let Some(id) = &monitor_id {
		println!("Using monitor {id} for playback");
	}

	let gl = client.gl().clone();
	let renderer = GlRenderer::new(&gl)?;
	client.send_ready()?;
	println!("Sent session_ready; starting animation...");

	let mut logo = LogoState::new();
	let mut last_frame = Instant::now();
	let mut fps_counter = FpsCounter::new();
	loop {
		if monitor_id.is_none() {
			let events = pump_events(&mut client, true)?;
			handle_events(
				&mut client,
				&events,
				&mut monitor_id,
				&session_client_bin,
				&mut spawned_sessions,
			)?;
			continue;
		}

		let active_monitor = monitor_id.clone().unwrap();
		match client.acquire_frame(&active_monitor) {
			Ok(frame) => {
				let dt = last_frame.elapsed().as_secs_f32().max(1.0 / 480.0);
				last_frame = Instant::now();
				let logo_size = renderer.logo_size_for(frame.size());
				logo.update(dt, frame.size(), logo_size);
				renderer.draw_frame(&gl, &frame, &logo, logo_size);
				client.swap_buffers(&active_monitor)?;
				if let Some(fps) = fps_counter.tick() {
					println!("client fps: {fps:.1}");
				}
			}
			Err(TabClientError::NoFreeBuffers(_)) => {
				let events = pump_events(&mut client, true)?;
				handle_events(
					&mut client,
					&events,
					&mut monitor_id,
					&session_client_bin,
					&mut spawned_sessions,
				)?;
				continue;
			}
			Err(TabClientError::UnknownMonitor(_)) => {
				monitor_id = None;
				continue;
			}
			Err(err) => return Err(err.into()),
		}

		let events = pump_events(&mut client, false)?;
		handle_events(
			&mut client,
			&events,
			&mut monitor_id,
			&session_client_bin,
			&mut spawned_sessions,
		)?;
	}
}

fn handle_events(
	client: &mut TabClient,
	events: &[TabEvent],
	monitor_id: &mut Option<String>,
	session_client_bin: &str,
	spawned_sessions: &mut Vec<Child>,
) -> Result<(), Box<dyn Error>> {
	for event in events {
		match event {
			TabEvent::MonitorAdded(info) => {
				println!("Monitor added: {}", info.id);
				if monitor_id.is_none() {
					*monitor_id = Some(info.id.clone());
					println!("Switched to monitor {}", info.id);
				}
			}
			TabEvent::MonitorRemoved(id) => {
				println!("Monitor removed: {id}");
				if monitor_id.as_deref() == Some(id) {
					*monitor_id = None;
				}
			}
			TabEvent::SessionState(state) => {
				println!("Session state changed: {:?}", state.state);
			}
			TabEvent::Input(payload) => {
				handle_input_event(client, payload, session_client_bin, spawned_sessions)?;
			}
			TabEvent::SessionCreated(payload) => {
				if payload.session.role == SessionRole::Session {
					println!(
						"Session {} ready with token {}",
						payload.session.id, payload.token
					);
					let child = spawn_session_client(session_client_bin, &payload.token)?;
					spawned_sessions.push(child);
				}
			}
			TabEvent::FrameDone { .. } => {}
		}
	}
	Ok(())
}

fn pump_events(client: &mut TabClient, blocking: bool) -> Result<Vec<TabEvent>, TabClientError> {
	let socket_fd = client.socket_fd().as_raw_fd();
	let swap_fd = client.swap_notifier_fd().as_raw_fd();
	let mut pfds = [
		libc::pollfd {
			fd: socket_fd,
			events: libc::POLLIN,
			revents: 0,
		},
		libc::pollfd {
			fd: swap_fd,
			events: libc::POLLIN,
			revents: 0,
		},
	];
	let timeout = if blocking { -1 } else { 0 };
	let ready = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, timeout) };
	if ready < 0 {
		let err = std::io::Error::last_os_error();
		if err.kind() == std::io::ErrorKind::Interrupted {
			return Ok(Vec::new());
		}
		return Err(TabClientError::Io(err));
	}
	let mut events = Vec::new();
	if ready == 0 {
		return Ok(events);
	}
	if pfds[0].revents & libc::POLLIN != 0 {
		events.extend(client.process_socket_events()?);
	}
	if pfds[1].revents & libc::POLLIN != 0 {
		client.process_ready_swaps()?;
	}
	Ok(events)
}

fn handle_input_event(
	client: &mut TabClient,
	payload: &InputEventPayload,
	_session_client_bin: &str,
	_spawned_sessions: &mut Vec<Child>,
) -> Result<(), Box<dyn Error>> {
	match payload {
		InputEventPayload::Key { key, state, .. } => {
			println!("Key event: code={key} state={state:?}");
			if *key == KEY_ENTER && *state == KeyState::Pressed {
				println!("Enter pressed – requesting a Penger session");
				request_session(client, "Penger Session")?;
			}
		}
		InputEventPayload::PointerMotion {
			device,
			time_usec,
			x,
			y,
			dx,
			dy,
		} => {
			println!(
				"Pointer motion: dev={device} time={time_usec} x={x:.2} y={y:.2} dx={dx:.3} dy={dy:.3}"
			);
		}
		InputEventPayload::PointerMotionAbsolute {
			device,
			time_usec,
			x,
			y,
			x_transformed,
			y_transformed,
		} => {
			println!(
				"Pointer absolute: dev={device} time={time_usec} raw=({x:.2}, {y:.2}) transformed=({x_transformed:.2}, {y_transformed:.2})"
			);
		}
		_ => {}
	}
	Ok(())
}

fn request_session(client: &mut TabClient, display_name: &str) -> Result<(), Box<dyn Error>> {
	let payload = SessionCreatePayload {
		role: SessionRole::Session,
		display_name: Some(display_name.to_string()),
	};
	let frame = TabMessageFrame::json(message_header::SESSION_CREATE, payload);
	client.send(&frame)?;
	Ok(())
}

fn spawn_session_client(bin: &str, token: &str) -> Result<Child, Box<dyn Error>> {
	let mut cmd = Command::new(bin);
	cmd.env("SHIFT_SESSION_TOKEN", token);
	cmd.stdout(Stdio::inherit());
	cmd.stderr(Stdio::inherit());
	let child = cmd.spawn()?;
	Ok(child)
}

struct LogoState {
	pos: [f32; 2],
	vel: [f32; 2],
	color: usize,
}

impl LogoState {
	fn new() -> Self {
		Self {
			pos: [120.0, 90.0],
			vel: [260.0, 190.0],
			color: 0,
		}
	}

	fn update(&mut self, dt: f32, fb_size: (i32, i32), logo_size: (f32, f32)) {
		let (width, height) = (fb_size.0 as f32, fb_size.1 as f32);
		let (logo_w, logo_h) = logo_size;
		let max_x = (width - logo_w).max(0.0);
		let max_y = (height - logo_h).max(0.0);

		self.pos[0] = (self.pos[0] + self.vel[0] * dt).clamp(0.0, max_x);
		self.pos[1] = (self.pos[1] + self.vel[1] * dt).clamp(0.0, max_y);

		let mut bounced = false;
		if self.pos[0] <= 0.0 || self.pos[0] >= max_x {
			self.vel[0] = -self.vel[0];
			bounced = true;
		}
		if self.pos[1] <= 0.0 || self.pos[1] >= max_y {
			self.vel[1] = -self.vel[1];
			bounced = true;
		}
		if bounced {
			self.color = (self.color + 1) % COLORS.len();
		}
	}

	fn tint(&self) -> [f32; 3] {
		COLORS[self.color]
	}
}

struct GlRenderer {
	program: u32,
	texture: u32,
	uni_resolution: i32,
	uni_position: i32,
	uni_size: i32,
	uni_tint: i32,
	texture_dims: (u32, u32),
}

struct FpsCounter {
	last: Instant,
	frames: u32,
}

impl FpsCounter {
	fn new() -> Self {
		Self {
			last: Instant::now(),
			frames: 0,
		}
	}

	fn tick(&mut self) -> Option<f32> {
		self.frames += 1;
		let elapsed = self.last.elapsed();
		if elapsed >= Duration::from_secs(1) {
			let fps = self.frames as f32 / elapsed.as_secs_f32();
			self.frames = 0;
			self.last = Instant::now();
			Some(fps)
		} else {
			None
		}
	}
}

impl GlRenderer {
	fn new(gl: &gl::Gles2) -> Result<Self, Box<dyn Error>> {
		unsafe {
			gl.Enable(gl::BLEND);
			gl.BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
		}

		let vert = compile_shader(
			gl,
			gl::VERTEX_SHADER,
			r#"
attribute vec2 aPos;
attribute vec2 aUv;
varying vec2 vUv;
uniform vec2 uResolution;
uniform vec2 uPosition;
uniform vec2 uSize;
void main() {
    vec2 scaled = uPosition + aPos * uSize;
    vec2 clip = vec2(
        (scaled.x / uResolution.x) * 2.0 - 1.0,
        1.0 - (scaled.y / uResolution.y) * 2.0
    );
    gl_Position = vec4(clip, 0.0, 1.0);
    vUv = aUv;
}
"#,
		)?;
		let frag = compile_shader(
			gl,
			gl::FRAGMENT_SHADER,
			r#"
precision mediump float;
varying vec2 vUv;
uniform sampler2D uTexture;
uniform vec3 uTint;
void main() {
    vec4 tex = texture2D(uTexture, vUv);
    gl_FragColor = vec4((vec3(1.0) - tex.rgb) * uTint, tex.a);
}
"#,
		)?;
		let program = link_program(gl, vert, frag)?;

		let attr_pos = unsafe { gl.GetAttribLocation(program, b"aPos\0".as_ptr() as _) };
		let attr_uv = unsafe { gl.GetAttribLocation(program, b"aUv\0".as_ptr() as _) };
		let uni_resolution = unsafe { gl.GetUniformLocation(program, b"uResolution\0".as_ptr() as _) };
		let uni_position = unsafe { gl.GetUniformLocation(program, b"uPosition\0".as_ptr() as _) };
		let uni_size = unsafe { gl.GetUniformLocation(program, b"uSize\0".as_ptr() as _) };
		let uni_tint = unsafe { gl.GetUniformLocation(program, b"uTint\0".as_ptr() as _) };
		let uni_tex = unsafe { gl.GetUniformLocation(program, b"uTexture\0".as_ptr() as _) };

		let mut vbo = 0;
		unsafe { gl.GenBuffers(1, &mut vbo) };
		const VERTICES: [f32; 16] = [
			0.0, 0.0, 0.0, 0.0, //
			1.0, 0.0, 1.0, 0.0, //
			0.0, 1.0, 0.0, 1.0, //
			1.0, 1.0, 1.0, 1.0, //
		];
		unsafe {
			gl.BindBuffer(gl::ARRAY_BUFFER, vbo);
			gl.BufferData(
				gl::ARRAY_BUFFER,
				(VERTICES.len() * std::mem::size_of::<f32>()) as isize,
				VERTICES.as_ptr() as _,
				gl::STATIC_DRAW,
			);
			let stride = (4 * std::mem::size_of::<f32>()) as i32;
			gl.EnableVertexAttribArray(attr_pos as u32);
			gl.VertexAttribPointer(
				attr_pos as u32,
				2,
				gl::FLOAT,
				gl::FALSE,
				stride,
				std::ptr::null(),
			);
			gl.EnableVertexAttribArray(attr_uv as u32);
			gl.VertexAttribPointer(
				attr_uv as u32,
				2,
				gl::FLOAT,
				gl::FALSE,
				stride,
				(2 * std::mem::size_of::<f32>()) as *const _,
			);
		}

		let image = image::load_from_memory(DVD_BYTES)?.to_rgba8();
		let texture_dims = image.dimensions();
		let mut texture = 0;
		unsafe {
			gl.GenTextures(1, &mut texture);
			gl.ActiveTexture(gl::TEXTURE0);
			gl.BindTexture(gl::TEXTURE_2D, texture);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
			gl.TexImage2D(
				gl::TEXTURE_2D,
				0,
				gl::RGBA as i32,
				texture_dims.0 as i32,
				texture_dims.1 as i32,
				0,
				gl::RGBA,
				gl::UNSIGNED_BYTE,
				image.as_ptr() as _,
			);
			gl.UseProgram(program);
			gl.Uniform1i(uni_tex, 0);
		}

		Ok(Self {
			program,
			texture,
			uni_resolution,
			uni_position,
			uni_size,
			uni_tint,
			texture_dims,
		})
	}

	fn logo_size_for(&self, framebuffer: (i32, i32)) -> (f32, f32) {
		let (w, h) = (framebuffer.0 as f32, framebuffer.1 as f32);
		let aspect = self.texture_dims.0 as f32 / self.texture_dims.1 as f32;
		let mut desired_w = (w * 0.25).clamp(80.0, w * 0.9);
		let mut desired_h = desired_w / aspect;
		if desired_h > h * 0.5 {
			desired_h = h * 0.5;
			desired_w = desired_h * aspect;
		}
		(desired_w, desired_h)
	}

	fn draw_frame(
		&self,
		gl: &gl::Gles2,
		target: &FrameTarget,
		logo: &LogoState,
		logo_size: (f32, f32),
	) {
		let (w, h) = target.size();
		unsafe {
			gl.BindFramebuffer(gl::FRAMEBUFFER, target.framebuffer());
			gl.Viewport(0, 0, w, h);
			gl.ClearColor(0.2, 0.2, 0.2, 1.0);
			gl.Clear(gl::COLOR_BUFFER_BIT);
			gl.UseProgram(self.program);
			gl.ActiveTexture(gl::TEXTURE0);
			gl.BindTexture(gl::TEXTURE_2D, self.texture);
			gl.Uniform2f(self.uni_resolution, w as f32, h as f32);
			gl.Uniform2f(self.uni_position, logo.pos[0], logo.pos[1]);
			gl.Uniform2f(self.uni_size, logo_size.0, logo_size.1);
			let tint = logo.tint();
			gl.Uniform3f(self.uni_tint, tint[0], tint[1], tint[2]);
			gl.DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
		}
	}
}

fn compile_shader(gl: &gl::Gles2, ty: u32, source: &str) -> Result<u32, Box<dyn Error>> {
	let shader = unsafe { gl.CreateShader(ty) };
	let c_source = std::ffi::CString::new(source)?;
	unsafe {
		gl.ShaderSource(shader, 1, &c_source.as_ptr(), std::ptr::null());
		gl.CompileShader(shader);
		let mut status = 0;
		gl.GetShaderiv(shader, gl::COMPILE_STATUS, &mut status);
		if status == 0 {
			let mut len = 0;
			gl.GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len);
			let mut log = vec![0u8; len as usize];
			gl.GetShaderInfoLog(shader, len, std::ptr::null_mut(), log.as_mut_ptr() as _);
			return Err(
				format!(
					"Shader compilation failed: {}",
					String::from_utf8_lossy(&log)
				)
				.into(),
			);
		}
	}
	Ok(shader)
}

fn link_program(gl: &gl::Gles2, vert: u32, frag: u32) -> Result<u32, Box<dyn Error>> {
	let program = unsafe { gl.CreateProgram() };
	unsafe {
		gl.AttachShader(program, vert);
		gl.AttachShader(program, frag);
		gl.LinkProgram(program);
		let mut status = 0;
		gl.GetProgramiv(program, gl::LINK_STATUS, &mut status);
		if status == 0 {
			let mut len = 0;
			gl.GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len);
			let mut log = vec![0u8; len as usize];
			gl.GetProgramInfoLog(program, len, std::ptr::null_mut(), log.as_mut_ptr() as _);
			return Err(format!("Program link failed: {}", String::from_utf8_lossy(&log)).into());
		}
		gl.DetachShader(program, vert);
		gl.DetachShader(program, frag);
		gl.DeleteShader(vert);
		gl.DeleteShader(frag);
	}
	Ok(program)
}
