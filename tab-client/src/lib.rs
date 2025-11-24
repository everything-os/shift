//! Tab client-side helper crate.
//! - Rust API for connecting to Shift via Tab v1
//! - C ABI surface for C/C++ consumers (cdylib/staticlib)
//! FD passing is not yet abstracted; consumers can access the raw UnixStream.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use gbm::AsRaw;
use khronos_egl::{self as kegl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::unistd::close;
use nix::{
	errno::Errno,
	fcntl::OFlag,
	unistd::{pipe2, read, write},
};
use tab_protocol::{
	AuthOkPayload, AuthPayload, DEFAULT_SOCKET_PATH, FrameDonePayload, FramebufferLinkPayload,
	HelloPayload, MonitorAddedPayload, MonitorInfo, MonitorRemovedPayload, PROTOCOL_VERSION,
	ProtocolError, SessionInfo, SessionReadyPayload, TabMessage, TabMessageFrame, message_header,
};

mod egl;
pub mod gl;
use crate::egl::{self as egl_sys, types::EGLTime};
pub use gl::Gles2;

/// Client-side error wrapper.
#[derive(Debug, thiserror::Error)]
pub enum TabClientError {
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("protocol error: {0}")]
	Protocol(#[from] ProtocolError),
	#[error("utf-8 error: {0}")]
	Utf8(#[from] std::str::Utf8Error),
	#[error("unexpected message: {0} (expected hello)")]
	UnexpectedHeader(String),
	#[error("serde error: {0}")]
	Serde(#[from] serde_json::Error),
	#[error("unsupported protocol: {0}")]
	UnsupportedProtocol(String),
	#[error("authentication rejected: {0}")]
	AuthRejected(String),
	#[error("unexpected message during auth: {0:?}")]
	UnexpectedMessage(TabMessage),
	#[error("client is not authenticated yet")]
	NotAuthenticated,
	#[error("egl error: {0}")]
	Egl(String),
	#[error("gl error: {0}")]
	Gl(String),
	#[error("no free buffers for monitor {0}")]
	NoFreeBuffers(String),
	#[error("monitor {0} not found")]
	UnknownMonitor(String),
}

const EGL_PLATFORM_GBM_KHR: egl_sys::types::EGLenum = 0x31D7;

const DEFAULT_RENDER_NODES: &[&str] = &[
	"/dev/dri/renderD128",
	"/dev/dri/renderD129",
	"/dev/dri/renderD130",
	"/dev/dri/renderD131",
	"/dev/dri/renderD132",
	"/dev/dri/renderD133",
	"/dev/dri/renderD134",
	"/dev/dri/renderD135",
];

#[derive(Debug, Clone)]
pub struct FrameTarget {
	framebuffer: u32,
	texture: u32,
	width: i32,
	height: i32,
}

impl FrameTarget {
	pub fn framebuffer(&self) -> u32 {
		self.framebuffer
	}

	pub fn texture(&self) -> u32 {
		self.texture
	}

	pub fn size(&self) -> (i32, i32) {
		(self.width, self.height)
	}
}

#[derive(Debug, Clone)]
pub enum TabEvent {
	FrameDone { monitor_id: String },
	MonitorAdded(MonitorInfo),
	MonitorRemoved(String),
	SessionState(SessionInfo),
}

#[derive(Clone, Copy)]
struct DisplayHandle(isize);

unsafe impl Send for DisplayHandle {}
unsafe impl Sync for DisplayHandle {}

impl DisplayHandle {
	fn from_ptr(ptr: egl_sys::types::EGLDisplay) -> Self {
		Self(ptr as isize)
	}

	fn as_ptr(self) -> egl_sys::types::EGLDisplay {
		self.0 as _
	}
}

#[derive(Clone, Copy)]
struct SyncHandle(egl_sys::types::EGLSyncKHR);

unsafe impl Send for SyncHandle {}
unsafe impl Sync for SyncHandle {}

impl SyncHandle {
	fn from_ptr(ptr: egl_sys::types::EGLSyncKHR) -> Self {
		Self(ptr)
	}

	fn as_ptr(self) -> egl_sys::types::EGLSyncKHR {
		self.0
	}
}

struct PendingSwap {
	monitor_id: String,
	buffer_index: usize,
	sync: SyncHandle,
}

struct CompletedSwap {
	monitor_id: String,
	buffer_index: usize,
}

struct SwapDispatcher {
	cmd_tx: Sender<SwapCommand>,
	ready_rx: Receiver<CompletedSwap>,
	notify_read: OwnedFd,
	worker: Option<std::thread::JoinHandle<()>>,
}

enum SwapCommand {
	Submit(PendingSwap),
	Shutdown,
}

impl SwapDispatcher {
	fn new(display: DisplayHandle, egl_ext: egl_sys::Egl) -> Result<Self, TabClientError> {
		let (cmd_tx, cmd_rx) = mpsc::channel();
		let (ready_tx, ready_rx) = mpsc::channel();
		let (notify_read, notify_write) =
			pipe2(OFlag::O_NONBLOCK | OFlag::O_CLOEXEC).map_err(|err| TabClientError::Io(err.into()))?;
		let worker_notify = notify_write
			.try_clone()
			.map_err(|err| TabClientError::Io(std::io::Error::from(err)))?;
		drop(notify_write);
		let worker_egl = egl_ext.clone();
		let worker_display = display;
		let worker = thread::spawn(move || {
			while let Ok(cmd) = cmd_rx.recv() {
				match cmd {
					SwapCommand::Submit(pending) => unsafe {
						let sync_ptr = pending.sync.as_ptr();
						let wait_result = worker_egl.ClientWaitSync(
							worker_display.as_ptr(),
							sync_ptr,
							egl_sys::SYNC_FLUSH_COMMANDS_BIT as egl_sys::EGLint,
							egl_sys::FOREVER as EGLTime,
						);
						let _ = worker_egl.DestroySync(worker_display.as_ptr(), sync_ptr);
						if wait_result == egl_sys::CONDITION_SATISFIED as egl_sys::EGLint {
							let _ = ready_tx.send(CompletedSwap {
								monitor_id: pending.monitor_id,
								buffer_index: pending.buffer_index,
							});
							let _ = write(&worker_notify, &[1]);
						}
					},
					SwapCommand::Shutdown => break,
				}
			}
		});
		Ok(Self {
			cmd_tx,
			ready_rx,
			notify_read,
			worker: Some(worker),
		})
	}

	fn submit(&self, swap: PendingSwap) {
		let _ = self.cmd_tx.send(SwapCommand::Submit(swap));
	}

	fn drain_ready(&self) -> Vec<CompletedSwap> {
		let mut buf = [0u8; 64];
		loop {
			match read(self.notify_read.as_raw_fd(), &mut buf) {
				Ok(0) => break,
				Ok(_) => continue,
				Err(Errno::EAGAIN) => break,
				Err(Errno::EINTR) => continue,
				Err(_) => break,
			}
		}
		let mut completions = Vec::new();
		while let Ok(item) = self.ready_rx.try_recv() {
			completions.push(item);
		}
		completions
	}

	fn notify_fd_raw(&self) -> RawFd {
		self.notify_read.as_raw_fd()
	}

	fn shutdown(&mut self) {
		let _ = self.cmd_tx.send(SwapCommand::Shutdown);
		if let Some(handle) = self.worker.take() {
			let _ = handle.join();
		}
	}
}

/// Rust-oriented Tab client.
pub struct TabClient {
	stream: UnixStream,
	read_buffer: Vec<u8>,
	last_error: Option<String>,
	hello: HelloPayload,
	session: Option<SessionInfo>,
	gfx: GraphicsContext,
	outputs: HashMap<String, Output>,
	swap_dispatcher: SwapDispatcher,
}

impl TabClient {
	/// Connect to a Tab socket at an explicit path.
	pub fn connect<P: AsRef<Path>, S: Into<String>>(
		path: P,
		token: S,
	) -> Result<Self, TabClientError> {
		let gfx = GraphicsContext::new()?;
		let stream = UnixStream::connect(path)?;
		let hello_msg = TabMessageFrame::read_framed(&stream)?;
		let parsed = TabMessage::parse_message_frame(hello_msg)?;
		let hello = match parsed {
			TabMessage::Hello(p) => p,
			other => return Err(TabClientError::UnexpectedHeader(format!("{:?}", other))),
		};

		if hello.protocol != PROTOCOL_VERSION {
			return Err(TabClientError::UnsupportedProtocol(hello.protocol));
		}

		let dispatcher = SwapDispatcher::new(
			DisplayHandle::from_ptr(gfx.display.as_ptr()),
			gfx.egl_ext.clone(),
		)?;
		let mut this = Self {
			stream,
			read_buffer: Vec::new(),
			last_error: None,
			hello,
			session: None,
			gfx,
			outputs: HashMap::new(),
			swap_dispatcher: dispatcher,
		};
		let auth_payload = this.authenticate(token)?;
		this.initialize_outputs(&auth_payload.monitors)?;
		Ok(this)
	}

	/// Connect to the default `/tmp/shift.sock` socket.
	pub fn connect_default(token: impl Into<String>) -> Result<Self, TabClientError> {
		Self::connect(DEFAULT_SOCKET_PATH, token)
	}

	/// Send a framed Tab message.
	pub fn send(&mut self, msg: &TabMessageFrame) -> Result<(), TabClientError> {
		msg.encode_and_send(&self.stream)?;
		Ok(())
	}

	/// Receive a parsed Tab message (blocking).
	pub fn receive(&mut self) -> Result<TabMessage, TabClientError> {
		let frame = self.read_frame_blocking()?;
		Ok(TabMessage::parse_message_frame(frame)?)
	}

	/// Borrow the underlying socket (for FD passing or poll integration).
	pub fn stream(&self) -> &UnixStream {
		&self.stream
	}

	/// Borrow the underlying socket mutably.
	pub fn stream_mut(&mut self) -> &mut UnixStream {
		&mut self.stream
	}

	pub fn socket_fd(&self) -> BorrowedFd<'_> {
		unsafe { BorrowedFd::borrow_raw(self.stream.as_raw_fd()) }
	}

	pub fn swap_notifier_fd(&self) -> BorrowedFd<'_> {
		unsafe { BorrowedFd::borrow_raw(self.swap_dispatcher.notify_fd_raw()) }
	}

	/// Access the received `hello` payload.
	pub fn hello(&self) -> &HelloPayload {
		&self.hello
	}

	pub fn gl(&self) -> &Gles2 {
		&self.gfx.gl
	}

	pub fn monitor_ids(&self) -> Vec<String> {
		let mut ids: Vec<_> = self.outputs.keys().cloned().collect();
		ids.sort();
		ids
	}

	pub fn authenticate(
		&mut self,
		token: impl Into<String>,
	) -> Result<AuthOkPayload, TabClientError> {
		let token = token.into();
		let frame = TabMessageFrame::json(message_header::AUTH, AuthPayload { token });
		self.send(&frame)?;
		loop {
			match self.receive()? {
				TabMessage::AuthOk(payload) => {
					self.session = Some(payload.session.clone());
					return Ok(payload);
				}
				TabMessage::AuthError(payload) => {
					return Err(TabClientError::AuthRejected(payload.error));
				}
				TabMessage::Error(payload) => {
					let msg = payload.message.unwrap_or_else(|| payload.code);
					return Err(TabClientError::AuthRejected(msg));
				}
				other => return Err(TabClientError::UnexpectedMessage(other)),
			}
		}
	}

	pub fn session(&self) -> Option<&SessionInfo> {
		self.session.as_ref()
	}

	pub fn send_ready(&mut self) -> Result<(), TabClientError> {
		let session = self
			.session
			.as_ref()
			.ok_or(TabClientError::NotAuthenticated)?;
		let payload = SessionReadyPayload {
			session_id: session.id.clone(),
		};
		let frame = TabMessageFrame::json(message_header::SESSION_READY, payload);
		self.send(&frame)
	}

	pub(crate) fn record_error(&mut self, err: impl ToString) {
		self.last_error = Some(err.to_string());
	}

	fn read_frame_blocking(&mut self) -> Result<TabMessageFrame, ProtocolError> {
		loop {
			if let Some(frame) = self.try_parse_buffered_frame()? {
				return Ok(frame);
			}
			self.read_more()?;
		}
	}

	fn try_parse_buffered_frame(&mut self) -> Result<Option<TabMessageFrame>, ProtocolError> {
		if self.read_buffer.is_empty() {
			return Ok(None);
		}
		match TabMessageFrame::parse_from_bytes(&self.read_buffer, Vec::new())? {
			Some((frame, consumed)) => {
				self.read_buffer.drain(..consumed);
				Ok(Some(frame))
			}
			None => Ok(None),
		}
	}

	fn read_more(&mut self) -> Result<(), ProtocolError> {
		let mut buf = [0u8; 4096];
		let bytes = self.stream.read(&mut buf)?;
		if bytes == 0 {
			return Err(ProtocolError::UnexpectedEof);
		}
		self.read_buffer.extend_from_slice(&buf[..bytes]);
		Ok(())
	}

	fn initialize_outputs(&mut self, monitors: &[MonitorInfo]) -> Result<(), TabClientError> {
		for monitor in monitors {
			self.create_output(monitor.clone())?;
		}
		Ok(())
	}

	fn create_output(&mut self, info: MonitorInfo) -> Result<(), TabClientError> {
		let mut output = Output::new(info.clone(), &self.gfx)?;
		self.send_framebuffer_link(&info, &mut output)?;
		self.outputs.insert(info.id.clone(), output);
		Ok(())
	}

	fn send_framebuffer_link(
		&mut self,
		info: &MonitorInfo,
		output: &mut Output,
	) -> Result<(), TabClientError> {
		let descriptors = output.export_dmabufs()?;
		let payload = FramebufferLinkPayload {
			monitor_id: info.id.clone(),
			width: info.width,
			height: info.height,
			stride: descriptors[0].stride,
			offset: descriptors[0].offset,
			fourcc: descriptors[0].fourcc,
		};
		let payload_json = serde_json::to_string(&payload)?;
		let mut frame = TabMessageFrame::raw(message_header::FRAMEBUFFER_LINK, payload_json);
		frame.fds = descriptors.iter().map(|desc| desc.fd).collect();
		self.send(&frame)?;
		drop(descriptors);
		Ok(())
	}

	pub fn acquire_frame(&mut self, monitor_id: &str) -> Result<FrameTarget, TabClientError> {
		let output = self
			.outputs
			.get_mut(monitor_id)
			.ok_or_else(|| TabClientError::UnknownMonitor(monitor_id.into()))?;
		output.acquire_frame()
	}

	pub fn swap_buffers(&mut self, monitor_id: &str) -> Result<(), TabClientError> {
		let output = self
			.outputs
			.get_mut(monitor_id)
			.ok_or_else(|| TabClientError::UnknownMonitor(monitor_id.into()))?;
		let buffer_index = output
			.begin_swap()
			.ok_or_else(|| TabClientError::NoFreeBuffers(monitor_id.into()))?;
		let sync = self.gfx.create_fence()?;
		self.swap_dispatcher.submit(PendingSwap {
			monitor_id: monitor_id.into(),
			buffer_index,
			sync,
		});
		Ok(())
	}

	pub fn poll_events(&mut self) -> Result<Vec<TabEvent>, TabClientError> {
		let mut events = Vec::new();
		let (ready, revents) = {
			let socket_fd = unsafe { BorrowedFd::borrow_raw(self.stream.as_raw_fd()) };
			let notify_fd = unsafe { BorrowedFd::borrow_raw(self.swap_dispatcher.notify_fd_raw()) };
			let mut pfds = [
				PollFd::new(socket_fd, PollFlags::POLLIN),
				PollFd::new(notify_fd, PollFlags::POLLIN),
			];
			let ready = poll(&mut pfds, PollTimeout::ZERO)
				.map_err(|err| TabClientError::Io(std::io::Error::from(err)))?;
			(ready, [pfds[0].revents(), pfds[1].revents()])
		};
		if ready > 0 {
			if revents[0]
				.unwrap_or(PollFlags::empty())
				.contains(PollFlags::POLLIN)
			{
				events.extend(self.process_socket_events()?);
			}
			if revents[1]
				.unwrap_or(PollFlags::empty())
				.contains(PollFlags::POLLIN)
			{
				self.process_ready_swaps()?;
			}
		}
		Ok(events)
	}

	pub fn process_socket_events(&mut self) -> Result<Vec<TabEvent>, TabClientError> {
		let mut events = Vec::new();
		self.read_more().map_err(TabClientError::from)?;
		while let Some(frame) = self.try_parse_buffered_frame()? {
			let msg = TabMessage::parse_message_frame(frame)?;
			events.extend(self.handle_event_message(msg)?);
		}
		Ok(events)
	}

	pub fn process_ready_swaps(&mut self) -> Result<(), TabClientError> {
		let ready = self.swap_dispatcher.drain_ready();
		for completed in ready {
			let payload = format!("{} {}", completed.monitor_id, completed.buffer_index);
			let frame = TabMessageFrame::raw(message_header::SWAP_BUFFERS, payload);
			self.send(&frame)?;
		}
		Ok(())
	}

	fn handle_event_message(&mut self, msg: TabMessage) -> Result<Vec<TabEvent>, TabClientError> {
		let mut events = Vec::new();
		match msg {
			TabMessage::FrameDone(FrameDonePayload { monitor_id }) => {
				if let Some(output) = self.outputs.get_mut(&monitor_id) {
					if output.complete_frame() {
						events.push(TabEvent::FrameDone { monitor_id });
					}
				}
			}
			TabMessage::MonitorAdded(MonitorAddedPayload { monitor }) => {
				self.create_output(monitor.clone())?;
				events.push(TabEvent::MonitorAdded(monitor));
			}
			TabMessage::MonitorRemoved(MonitorRemovedPayload { monitor_id, .. }) => {
				self.outputs.remove(&monitor_id);
				events.push(TabEvent::MonitorRemoved(monitor_id));
			}
			TabMessage::SessionState(payload) => {
				events.push(TabEvent::SessionState(payload.session));
			}
			other => {
				if let TabMessage::Error(payload) = &other {
					self.record_error(payload.message.clone().unwrap_or(payload.code.clone()));
				}
			}
		}
		Ok(events)
	}
}

impl Drop for TabClient {
	fn drop(&mut self) {
		self.swap_dispatcher.shutdown();
	}
}

mod c_bindings;

struct GraphicsContext {
	egl: kegl::DynamicInstance<kegl::EGL1_5>,
	display: kegl::Display,
	context: kegl::Context,
	gl: Gles2,
	egl_ext: egl_sys::Egl,
	_gbm_device: gbm::Device<File>,
}

impl GraphicsContext {
	fn new() -> Result<Self, TabClientError> {
		let lib = unsafe { libloading::Library::new("libEGL.so.1") }
			.map_err(|err| TabClientError::Egl(format!("Failed to load libEGL.so.1: {err}")))?;
		let egl =
			unsafe { khronos_egl::DynamicInstance::<khronos_egl::EGL1_5>::load_required_from(lib) }
				.map_err(|err| {
					TabClientError::Egl(format!(
						"Failed to load libEGL.so.1 (load_required_from): {err}"
					))
				})?;
		let egl_ext = egl_sys::Egl::load_with(|symbol| {
			egl
				.get_proc_address(symbol)
				.map_or(std::ptr::null(), |p| p as _)
		});

		let (display, context, gl, gbm_device) = Self::initialize_with_gbm(&egl, &egl_ext)?;
		Ok(Self {
			egl,
			display,
			context,
			gl,
			egl_ext,
			_gbm_device: gbm_device,
		})
	}

	fn initialize_on_display(
		egl: &kegl::DynamicInstance<kegl::EGL1_5>,
		display: kegl::Display,
	) -> Result<(kegl::Context, Gles2), TabClientError> {
		egl
			.initialize(display)
			.map_err(|err| TabClientError::Egl(err.to_string()))?;
		let attribs = [
			kegl::RED_SIZE,
			8,
			kegl::GREEN_SIZE,
			8,
			kegl::BLUE_SIZE,
			8,
			kegl::ALPHA_SIZE,
			8,
			kegl::RENDERABLE_TYPE,
			kegl::OPENGL_ES2_BIT,
			kegl::NONE,
		];
		let config = egl
			.choose_first_config(display, &attribs)
			.map_err(|err| TabClientError::Egl(err.to_string()))?
			.ok_or_else(|| TabClientError::Egl("No EGL config".into()))?;
		egl
			.bind_api(kegl::OPENGL_ES_API)
			.map_err(|err| TabClientError::Egl(err.to_string()))?;
		let context = egl
			.create_context(
				display,
				config,
				None,
				&[kegl::CONTEXT_CLIENT_VERSION, 3, kegl::NONE],
			)
			.map_err(|err| TabClientError::Egl(err.to_string()))?;
		egl
			.make_current(display, None, None, Some(context))
			.map_err(|err| TabClientError::Egl(err.to_string()))?;
		let gl = Gles2::load_with(|symbol| {
			egl
				.get_proc_address(symbol)
				.map_or(std::ptr::null(), |p| p as _)
		});
		Ok((context, gl))
	}

	fn initialize_with_gbm(
		egl: &kegl::DynamicInstance<kegl::EGL1_5>,
		egl_ext: &egl_sys::Egl,
	) -> Result<(kegl::Display, kegl::Context, Gles2, gbm::Device<File>), TabClientError> {
		let gbm_device = Self::create_gbm_device()?;
		let native_display = gbm_device.as_raw() as *mut std::ffi::c_void;
		let raw_display = unsafe {
			egl_ext.GetPlatformDisplayEXT(EGL_PLATFORM_GBM_KHR, native_display, std::ptr::null())
		};
		if raw_display == egl_sys::NO_DISPLAY {
			return Err(TabClientError::Egl(
				"eglGetPlatformDisplayEXT(EGL_PLATFORM_GBM_KHR) failed".into(),
			));
		}
		let display = unsafe { kegl::Display::from_ptr(raw_display as *mut _) };
		let (context, gl) = Self::initialize_on_display(egl, display)?;
		Ok((display, context, gl, gbm_device))
	}

	fn create_gbm_device() -> Result<gbm::Device<File>, TabClientError> {
		let mut last_error = None;
		for candidate in Self::render_node_candidates() {
			match OpenOptions::new().read(true).write(true).open(&candidate) {
				Ok(file) => match gbm::Device::new(file) {
					Ok(device) => return Ok(device),
					Err(err) => {
						last_error = Some(format!("{} (gbm: {err})", candidate.display()));
					}
				},
				Err(err) => {
					last_error = Some(format!("{} ({err})", candidate.display()));
				}
			}
		}
		Err(TabClientError::Egl(match last_error {
			Some(msg) => format!("Failed to initialize GBM device: {msg}"),
			None => "No render nodes available for GBM initialization".into(),
		}))
	}

	fn render_node_candidates() -> Vec<PathBuf> {
		if let Ok(path) = std::env::var("TAB_CLIENT_RENDER_NODE") {
			vec![PathBuf::from(path)]
		} else {
			DEFAULT_RENDER_NODES
				.iter()
				.map(|p| PathBuf::from(p))
				.collect()
		}
	}

	fn create_fence(&self) -> Result<SyncHandle, TabClientError> {
		let attributes = [egl_sys::NONE as egl_sys::types::EGLAttrib];
		let sync = unsafe {
			self.egl_ext.CreateSync(
				self.display.as_ptr(),
				egl_sys::SYNC_FENCE,
				attributes.as_ptr(),
			)
		};
		if sync == egl_sys::NO_SYNC {
			return Err(TabClientError::Egl("eglCreateSyncKHR failed".into()));
		}
		unsafe {
			self.gl.Flush();
		}
		Ok(SyncHandle::from_ptr(sync))
	}
}

impl Drop for GraphicsContext {
	fn drop(&mut self) {
		let _ = self.egl.make_current(self.display, None, None, None);
		let _ = self.egl.destroy_context(self.display, self.context);
		let _ = self.egl.terminate(self.display);
	}
}

struct Output {
	info: MonitorInfo,
	buffers: [OutputBuffer; 2],
	available: VecDeque<usize>,
	in_flight: VecDeque<usize>,
	drawing: Option<usize>,
}

impl Output {
	fn new(info: MonitorInfo, gfx: &GraphicsContext) -> Result<Self, TabClientError> {
		let buffers = [
			OutputBuffer::new(&info, gfx)?,
			OutputBuffer::new(&info, gfx)?,
		];
		let mut available = VecDeque::new();
		available.push_back(0);
		available.push_back(1);
		Ok(Self {
			info,
			buffers,
			available,
			in_flight: VecDeque::new(),
			drawing: None,
		})
	}

	fn acquire_frame(&mut self) -> Result<FrameTarget, TabClientError> {
		let Some(index) = self.available.pop_front() else {
			return Err(TabClientError::NoFreeBuffers(self.info.id.clone()));
		};
		self.drawing = Some(index);
		let buf = &self.buffers[index];
		Ok(FrameTarget {
			framebuffer: buf.framebuffer,
			texture: buf.texture,
			width: self.info.width,
			height: self.info.height,
		})
	}

	fn begin_swap(&mut self) -> Option<usize> {
		let idx = self.drawing.take()?;
		self.in_flight.push_back(idx);
		Some(idx)
	}

	fn complete_frame(&mut self) -> bool {
		if let Some(idx) = self.in_flight.pop_front() {
			self.available.push_back(idx);
			true
		} else {
			false
		}
	}

	fn export_dmabufs(&self) -> Result<[Dmabuf; 2], TabClientError> {
		let mut descs = Vec::new();
		for buf in &self.buffers {
			descs.push(buf.export_dmabuf()?);
		}
		Ok([descs.remove(0), descs.remove(0)])
	}
}

struct OutputBuffer {
	texture: u32,
	framebuffer: u32,
	image: egl_sys::types::EGLImageKHR,
	gl: Gles2,
	egl_ext: egl_sys::Egl,
	display: kegl::Display,
}

impl OutputBuffer {
	fn new(info: &MonitorInfo, gfx: &GraphicsContext) -> Result<Self, TabClientError> {
		let mut texture = 0;
		let mut framebuffer = 0;
		let gl = gfx.gl.clone();
		unsafe {
			gl.GenTextures(1, &mut texture);
			gl.BindTexture(gl::TEXTURE_2D, texture);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
			gl.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
			gl.TexImage2D(
				gl::TEXTURE_2D,
				0,
				gl::RGBA as i32,
				info.width,
				info.height,
				0,
				gl::RGBA,
				gl::UNSIGNED_BYTE,
				std::ptr::null(),
			);
			gl.GenFramebuffers(1, &mut framebuffer);
			gl.BindFramebuffer(gl::FRAMEBUFFER, framebuffer);
			gl.FramebufferTexture2D(
				gl::FRAMEBUFFER,
				gl::COLOR_ATTACHMENT0,
				gl::TEXTURE_2D,
				texture,
				0,
			);
		}
		let egl_ext = gfx.egl_ext.clone();
		let image = unsafe {
			egl_ext.CreateImageKHR(
				gfx.display.as_ptr(),
				gfx.context.as_ptr(),
				egl_sys::GL_TEXTURE_2D,
				texture as _,
				std::ptr::null(),
			)
		};
		if image == egl_sys::NO_IMAGE_KHR {
			return Err(TabClientError::Egl("eglCreateImageKHR failed".into()));
		}
		Ok(Self {
			texture,
			framebuffer,
			image,
			gl,
			egl_ext,
			display: gfx.display,
		})
	}

	fn export_dmabuf(&self) -> Result<Dmabuf, TabClientError> {
		let mut fourcc = 0;
		let mut num_planes = 0;
		let query = unsafe {
			self.egl_ext.ExportDMABUFImageQueryMESA(
				self.display.as_ptr(),
				self.image,
				&mut fourcc,
				&mut num_planes,
				std::ptr::null_mut(),
			)
		};
		if query == 0 {
			return Err(TabClientError::Egl(
				"eglExportDMABUFImageQueryMESA failed".into(),
			));
		}
		let mut fd = 0;
		let mut stride = 0;
		let mut offset = 0;
		let exported = unsafe {
			self.egl_ext.ExportDMABUFImageMESA(
				self.display.as_ptr(),
				self.image,
				&mut fd,
				&mut stride,
				&mut offset,
			)
		};
		if exported == 0 {
			return Err(TabClientError::Egl(
				"eglExportDMABUFImageMESA failed".into(),
			));
		}
		Ok(Dmabuf {
			fd,
			stride,
			offset,
			fourcc,
		})
	}
}

impl Drop for OutputBuffer {
	fn drop(&mut self) {
		unsafe {
			if self.image != egl_sys::NO_IMAGE_KHR {
				let _ = self
					.egl_ext
					.DestroyImageKHR(self.display.as_ptr(), self.image);
			}
			if self.framebuffer != 0 {
				self.gl.DeleteFramebuffers(1, &self.framebuffer);
			}
			if self.texture != 0 {
				self.gl.DeleteTextures(1, &self.texture);
			}
		}
	}
}

struct Dmabuf {
	fd: RawFd,
	stride: i32,
	offset: i32,
	fourcc: i32,
}

impl Drop for Dmabuf {
	fn drop(&mut self) {
		let _ = close(self.fd);
	}
}
