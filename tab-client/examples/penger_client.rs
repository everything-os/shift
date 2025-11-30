use std::{env, error::Error, os::fd::AsRawFd, time::Instant};

use tab_client::{FrameTarget, TabClient, TabClientError, TabEvent, gl};

const PENGER_BYTES: &[u8] = include_bytes!("penger.png");

fn main() -> Result<(), Box<dyn Error>> {
	let token = env::args()
		.nth(1)
		.or_else(|| env::var("SHIFT_SESSION_TOKEN").ok())
		.expect("Provide a session token via SHIFT_SESSION_TOKEN or argv[1]");

	let mut client = TabClient::connect_default(token)?;
	println!(
		"Connected to Shift server '{}' via protocol {}",
		client.hello().server,
		client.hello().protocol
	);

	let mut monitor_id = client.monitor_ids().into_iter().next();
	if monitor_id.is_none() {
		println!("Waiting for monitor...");
		while monitor_id.is_none() {
			let events = pump_events(&mut client, true)?;
			handle_events(&events, &mut monitor_id);
		}
	}
	let gl = client.gl().clone();
	let renderer = PengerRenderer::new(&gl)?;
	client.send_ready()?;
	let mut spinner = Spinner::default();
	let mut last_frame = Instant::now();

	loop {
		if monitor_id.is_none() {
			let events = pump_events(&mut client, true)?;
			handle_events(&events, &mut monitor_id);
			continue;
		}
		let active = monitor_id.clone().unwrap();
		match client.acquire_frame(&active) {
			Ok(frame) => {
				let dt = last_frame.elapsed().as_secs_f32().max(1.0 / 240.0);
				last_frame = Instant::now();
				spinner.update(dt);
				renderer.draw_frame(&gl, &frame, spinner.scale());
				client.swap_buffers(&active)?;
			}
			Err(TabClientError::NoFreeBuffers(_)) => {
				let events = pump_events(&mut client, true)?;
				handle_events(&events, &mut monitor_id);
				continue;
			}
			Err(TabClientError::UnknownMonitor(_)) => {
				monitor_id = None;
				continue;
			}
			Err(err) => return Err(err.into()),
		}

		let events = pump_events(&mut client, false)?;
		handle_events(&events, &mut monitor_id);
	}
}

fn handle_events(events: &[TabEvent], monitor_id: &mut Option<String>) {
	for event in events {
		match event {
			TabEvent::MonitorAdded(info) => {
				if monitor_id.is_none() {
					*monitor_id = Some(info.id.clone());
				}
			}
			TabEvent::MonitorRemoved(id) => {
				if monitor_id.as_deref() == Some(id) {
					*monitor_id = None;
				}
			}
			TabEvent::SessionState(state) => {
				println!("Session {} transitioned to {:?}", state.id, state.state);
			}
			TabEvent::Input(_) => {}
			TabEvent::SessionCreated(_) => {}
			TabEvent::FrameDone { .. } => {}
			TabEvent::Error(err) => {
				eprintln!(
					"[Shift error] code={} message={}",
					err.code,
					err.message.clone().unwrap_or_else(|| "unknown".into())
				);
			}
		}
	}
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

#[derive(Default)]
struct Spinner {
	phase: f32,
}

impl Spinner {
	fn update(&mut self, dt: f32) {
		self.phase += dt * 1.5;
	}

	fn scale(&self) -> f32 {
		self.phase.sin()
	}
}

struct PengerRenderer {
	program: u32,
	texture: u32,
	uni_resolution: i32,
	uni_center: i32,
	uni_size: i32,
	uni_scale: i32,
	texture_dims: (u32, u32),
}

impl PengerRenderer {
	fn new(gl: &gl::Gles2) -> Result<Self, Box<dyn Error>> {
		let vert_src = r#"
attribute vec2 aPos;
attribute vec2 aUv;
varying vec2 vUv;
uniform vec2 uResolution;
uniform vec2 uCenter;
uniform vec2 uSize;
uniform float uScaleX;
void main() {
    vec2 halfSize = uSize * 0.5;
    vec2 scaled = vec2(aPos.x * halfSize.x * uScaleX, aPos.y * halfSize.y);
    vec2 pixel = uCenter + scaled;
    vec2 clip = vec2(
        (pixel.x / uResolution.x) * 2.0 - 1.0,
        1.0 - (pixel.y / uResolution.y) * 2.0
    );
    gl_Position = vec4(clip, 0.0, 1.0);
    vUv = (aPos + 1.0) * 0.5;
}
"#;
		let frag_src = r#"
precision mediump float;
varying vec2 vUv;
uniform sampler2D uTexture;
void main() {
    gl_FragColor = texture2D(uTexture, vUv);
}
"#;
		let vert = compile_shader(gl, gl::VERTEX_SHADER, vert_src)?;
		let frag = compile_shader(gl, gl::FRAGMENT_SHADER, frag_src)?;
		let program = link_program(gl, vert, frag)?;

		let attr_pos = unsafe { gl.GetAttribLocation(program, b"aPos\0".as_ptr() as _) };
		let attr_uv = unsafe { gl.GetAttribLocation(program, b"aUv\0".as_ptr() as _) };
		let uni_resolution = unsafe { gl.GetUniformLocation(program, b"uResolution\0".as_ptr() as _) };
		let uni_center = unsafe { gl.GetUniformLocation(program, b"uCenter\0".as_ptr() as _) };
		let uni_size = unsafe { gl.GetUniformLocation(program, b"uSize\0".as_ptr() as _) };
		let uni_scale = unsafe { gl.GetUniformLocation(program, b"uScaleX\0".as_ptr() as _) };
		let uni_tex = unsafe { gl.GetUniformLocation(program, b"uTexture\0".as_ptr() as _) };

		let mut vbo = 0;
		unsafe { gl.GenBuffers(1, &mut vbo) };
		const VERTICES: [f32; 16] = [
			-1.0, -1.0, 0.0, 0.0, //
			1.0, -1.0, 1.0, 0.0, //
			-1.0, 1.0, 0.0, 1.0, //
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

		let image = image::load_from_memory(PENGER_BYTES)?.to_rgba8();
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

		unsafe {
			gl.Enable(gl::BLEND);
			gl.BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
		}

		Ok(Self {
			program,
			texture,
			uni_resolution,
			uni_center,
			uni_size,
			uni_scale,
			texture_dims,
		})
	}

	fn draw_frame(&self, gl: &gl::Gles2, target: &FrameTarget, scale: f32) {
		let (w, h) = target.size();
		let size = self.target_size(w as f32, h as f32);
		unsafe {
			gl.BindFramebuffer(gl::FRAMEBUFFER, target.framebuffer());
			gl.Viewport(0, 0, w, h);
			gl.ClearColor(
				0xff as f32 / 255.,
				0xc0 as f32 / 255.,
				0xCB as f32 / 255.,
				1.0,
			); // FFC0CB
			gl.Clear(gl::COLOR_BUFFER_BIT);
			gl.UseProgram(self.program);
			gl.ActiveTexture(gl::TEXTURE0);
			gl.BindTexture(gl::TEXTURE_2D, self.texture);
			gl.Uniform2f(self.uni_resolution, w as f32, h as f32);
			gl.Uniform2f(self.uni_center, w as f32 * 0.5, h as f32 * 0.5);
			gl.Uniform2f(self.uni_size, size.0, size.1);
			gl.Uniform1f(self.uni_scale, scale);
			gl.DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
		}
	}

	fn target_size(&self, fb_w: f32, fb_h: f32) -> (f32, f32) {
		let aspect = self.texture_dims.0 as f32 / self.texture_dims.1 as f32;
		let mut width = fb_w * 0.5;
		let mut height = width / aspect;
		if height > fb_h * 0.6 {
			height = fb_h * 0.6;
			width = height * aspect;
		}
		(width, height)
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
			let mut buf = vec![0u8; len as usize];
			gl.GetShaderInfoLog(shader, len, std::ptr::null_mut(), buf.as_mut_ptr() as _);
			let msg = String::from_utf8_lossy(&buf);
			return Err(format!("Shader compile failed: {msg}").into());
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
			let mut buf = vec![0u8; len as usize];
			gl.GetProgramInfoLog(program, len, std::ptr::null_mut(), buf.as_mut_ptr() as _);
			let msg = String::from_utf8_lossy(&buf);
			return Err(format!("Program link failed: {msg}").into());
		}
		gl.DeleteShader(vert);
		gl.DeleteShader(frag);
	}
	Ok(program)
}
