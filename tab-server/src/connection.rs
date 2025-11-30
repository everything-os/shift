use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::errno::Errno;
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};
use std::io::IoSliceMut;

use nix::unistd::close;

use tab_protocol::{ProtocolError, TabMessage, TabMessageFrame};

#[derive(Debug)]
pub struct TabConnection {
	stream: UnixStream,
	buffer: Vec<u8>,
	pending_fds: Vec<RawFd>,
}

impl TabConnection {
	pub fn new(stream: UnixStream) -> std::io::Result<Self> {
		stream.set_nonblocking(true)?;
		Ok(Self {
			stream,
			buffer: Vec::new(),
			pending_fds: Vec::new(),
		})
	}

	pub fn read_message(&mut self) -> Result<TabMessage, ProtocolError> {
		loop {
			if let Some(frame) = self.try_parse_buffer()? {
				return TabMessage::parse_message_frame(frame);
			}
			match self.recv_frame()? {
				Some(frame) => return TabMessage::parse_message_frame(frame),
				None => continue,
			}
		}
	}

	pub fn read_message_nonblocking(&mut self) -> Result<Option<TabMessage>, ProtocolError> {
		if let Some(frame) = self.try_parse_buffer()? {
			return Ok(Some(TabMessage::parse_message_frame(frame)?));
		}
		match self.recv_frame()? {
			Some(frame) => Ok(Some(TabMessage::parse_message_frame(frame)?)),
			None => Ok(None),
		}
	}

	pub fn send_frame(&self, frame: &TabMessageFrame) -> Result<(), ProtocolError> {
		frame.encode_and_send(&self.stream)
	}

	pub fn send_hello(&mut self, server_ident: impl Into<String>) -> Result<(), ProtocolError> {
		let frame = TabMessageFrame::hello(server_ident);
		self.send_frame(&frame)
	}
}
impl TabConnection {
	fn try_parse_buffer(&mut self) -> Result<Option<TabMessageFrame>, ProtocolError> {
		if self.buffer.is_empty() {
			return Ok(None);
		}
		let fds = std::mem::take(&mut self.pending_fds);
		let parse_result = if fds.is_empty() {
			TabMessageFrame::parse_from_bytes(&self.buffer, Vec::new())?
		} else {
			TabMessageFrame::parse_from_bytes(&self.buffer, fds.clone())?
		};
		match parse_result {
			Some((frame, consumed)) => {
				self.buffer.drain(..consumed);
				Ok(Some(frame))
			}
			None => {
				if !fds.is_empty() {
					self.pending_fds = fds;
				}
				Ok(None)
			}
		}
	}

	fn recv_frame(&mut self) -> Result<Option<TabMessageFrame>, ProtocolError> {
		let mut buf = [0u8; 4096];
		let mut cmsg_space = nix::cmsg_space!([RawFd; 8]);
		let mut iov = [IoSliceMut::new(&mut buf)];
		match recvmsg::<()>(
			self.stream.as_raw_fd(),
			&mut iov,
			Some(&mut cmsg_space),
			MsgFlags::empty(),
		) {
			Err(err) if err == Errno::EINTR => self.recv_frame(),
			Err(err) if err == Errno::EAGAIN || err == Errno::EWOULDBLOCK => Ok(None),
			Err(err) => Err(ProtocolError::Nix(err.into())),
			Ok(msg) => {
				let (bytes, fds) = {
					let bytes = msg.bytes;
					if bytes == 0 {
						return Err(ProtocolError::UnexpectedEof);
					}
					if msg.flags.contains(MsgFlags::MSG_TRUNC) {
						return Err(ProtocolError::Truncated);
					}
					let mut fds = Vec::new();
					for cmsg in msg.cmsgs()? {
						if let ControlMessageOwned::ScmRights(rights) = cmsg {
							fds.extend(rights);
						}
					}
					(bytes, fds)
				};
				if !fds.is_empty() {
					if !self.pending_fds.is_empty() {
						for fd in fds {
							let _ = close(fd);
						}
						return Err(ProtocolError::InvalidPayload(
							"received file descriptors for a frame while previous frame is incomplete"
								.into(),
						));
					}
					self.pending_fds = fds;
				}
				self.buffer.extend_from_slice(&buf[..bytes]);
				self.try_parse_buffer()
			}
		}
	}
}

impl AsRawFd for TabConnection {
	fn as_raw_fd(&self) -> RawFd {
		self.stream.as_raw_fd()
	}
}

impl Drop for TabConnection {
	fn drop(&mut self) {
		for fd in self.pending_fds.drain(..) {
			let _ = close(fd);
		}
	}
}
