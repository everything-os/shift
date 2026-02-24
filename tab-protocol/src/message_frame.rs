use nix::errno::Errno;
use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{ErrorKind, IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, RawFd};

use crate::{HelloPayload, MessageHeader, PROTOCOL_VERSION, ProtocolError};

/// Raw framed Tab message: header line + payload line (strings) plus optional FDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMessageFrame {
	pub header: MessageHeader,
	pub payload: Option<String>,
	pub fds: Vec<RawFd>,
}
fn would_block_err() -> std::io::Error {
	std::io::Error::new(ErrorKind::WouldBlock, ProtocolError::WouldBlock)
}
#[derive(Default)]
pub struct TabMessageFrameReader {
	pending_bytes: Vec<u8>,
	pending_fds: Vec<RawFd>,
	ready_frames: VecDeque<TabMessageFrame>,
}
impl TabMessageFrameReader {
	pub fn new() -> Self {
		Self::default()
	}
	fn pop_ready(&mut self) -> Option<TabMessageFrame> {
		self.ready_frames.pop_front()
	}
	pub fn try_pop_ready_frame(&mut self) -> Option<TabMessageFrame> {
		self.pop_ready()
	}
	#[tracing::instrument(skip_all)]
	fn feed_chunk(&mut self, bytes: &[u8], mut fds: Vec<RawFd>) -> Result<(), ProtocolError> {
		if !bytes.is_empty() {
			self.pending_bytes.extend_from_slice(bytes);
		}
		if !fds.is_empty() {
			self.pending_fds.append(&mut fds);
		}
		self.process_pending()?;
		Ok(())
	}
	#[tracing::instrument(skip_all)]
	fn process_pending(&mut self) -> Result<(), ProtocolError> {
		loop {
			if self.pending_bytes.is_empty() {
				break;
			}
			let fds_for_frame = self.pending_fds.clone();
			match TabMessageFrame::parse_from_bytes(&self.pending_bytes, fds_for_frame)? {
				Some((frame, used)) => {
					self.pending_bytes.drain(..used);
					self.pending_fds.clear();
					self.ready_frames.push_back(frame);
				}
				None => break,
			}
		}
		Ok(())
	}
	#[tracing::instrument(skip_all)]
	pub fn read_framed(&mut self, stream: &impl AsRawFd) -> Result<TabMessageFrame, ProtocolError> {
		loop {
			if let Some(frame) = self.pop_ready() {
				return Ok(frame);
			}
			let (bytes, fds) = recv_into_vec(stream)?;
			self.feed_chunk(&bytes, fds)?;
		}
	}
	#[cfg(feature = "async")]
	#[tracing::instrument(skip_all)]
	pub async fn read_frame_from_async_fd<T: AsRawFd>(
		&mut self,
		fd: &tokio::io::unix::AsyncFd<T>,
	) -> Result<TabMessageFrame, ProtocolError> {
		loop {
			if let Some(frame) = self.pop_ready() {
				return Ok(frame);
			}
			let mut guard = fd.readable().await?;
			if let Ok(result) = guard.try_io(|_| match self.read_framed(fd.get_ref()) {
				Err(ProtocolError::WouldBlock) => Err(would_block_err()),
				def => Ok(def),
			}) {
				break result?;
			}
		}
	}
}
#[tracing::instrument(skip_all)]
fn recv_into_vec(stream: &impl AsRawFd) -> Result<(Vec<u8>, Vec<RawFd>), ProtocolError> {
	let mut buf = [0u8; 4096];
	let mut cmsg_space = nix::cmsg_space!([RawFd; 8]);
	let mut iov = [IoSliceMut::new(&mut buf)];
	let msg = loop {
		match recvmsg::<()>(
			stream.as_raw_fd(),
			&mut iov,
			Some(&mut cmsg_space),
			MsgFlags::empty(),
		) {
			Err(errno) if errno == Errno::EINTR => continue,
			Err(errno) if errno == Errno::EAGAIN || errno == Errno::EWOULDBLOCK => {
				break Err(ProtocolError::WouldBlock);
			}
			Err(errno) => break Err(ProtocolError::Nix(errno.into())),
			Ok(msg) => break Ok(msg),
		}
	}?;
	if msg.bytes == 0 {
		return Err(ProtocolError::UnexpectedEof);
	}
	if msg.flags.contains(MsgFlags::MSG_TRUNC) {
		return Err(ProtocolError::Truncated);
	}
	let mut fds = Vec::new();
	let mut c_iter = msg.cmsgs()?;
	while let Some(cmsg) = c_iter.next() {
		if let ControlMessageOwned::ScmRights(rights) = cmsg {
			fds.extend(rights);
		}
	}
	let bytes = msg.bytes;
	let _ = msg;
	let data = iov[0][..bytes].to_vec();
	Ok((data, fds))
}
impl TabMessageFrame {
	/// Write a framed TabMessageFrame to the provided stream using sendmsg/SCM_RIGHTS.
	pub fn encode_and_send(&self, stream: &impl AsRawFd) -> Result<(), ProtocolError> {
		let (encoded_header, encoded_payload) = self.serialize();
		let encoded_header = format!("{encoded_header}\n");
		let encoded_payload = format!("{encoded_payload}\n");
		let iov = [
			IoSlice::new(encoded_header.as_bytes()),
			IoSlice::new(encoded_payload.as_bytes()),
		];
		let cmsg = if self.fds.is_empty() {
			vec![]
		} else {
			vec![ControlMessage::ScmRights(&self.fds)]
		};
		sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)?;
		Ok(())
	}
	pub fn serialize(&self) -> (String, String) {
		let header_line = self.header.0.trim_end();
		let payload_line = self
			.payload
			.as_ref()
			.map(|p| p.trim_end_matches('\n'))
			.unwrap_or_else(|| "\0\0\0\0");

		(header_line.to_string(), payload_line.to_string())
	}

	/// Sends a message asynchronously
	#[cfg(feature = "async")]
	pub async fn send_frame_to_async_fd<T: AsRawFd>(
		&self,
		fd: &tokio::io::unix::AsyncFd<T>,
	) -> Result<(), ProtocolError> {
		let packet = loop {
			let mut guard = fd.writable().await?;
			if let Ok(result) = guard.try_io(|_| match self.encode_and_send(fd) {
				Err(ProtocolError::WouldBlock) => Err(would_block_err()),
				def => Ok(def),
			}) {
				break result?;
			} else {
				continue;
			}
		}?;
		return Ok(packet);
	}

	#[tracing::instrument(skip_all)]
	pub(crate) fn expect_payload_json<'a, T>(&'a self) -> Result<T, ProtocolError>
	where
		T: serde::Deserialize<'a>,
	{
		if let Some(payload) = &self.payload {
			let span = tracing::span!(tracing::Level::TRACE, "json_decode");
			let _enter = span.enter();
			serde_json::from_str(payload.as_str()).map_err(ProtocolError::from)
		} else {
			Err(ProtocolError::ExpectedPayload)
		}
	}
	pub fn json(header: impl Into<MessageHeader>, payload: impl Serialize) -> Self {
		Self {
			header: header.into(),
			payload: Some(serde_json::to_string(&payload).unwrap()),
			fds: Vec::new(),
		}
	}

	pub fn raw(header: impl Into<MessageHeader>, body: impl Into<String>) -> Self {
		Self {
			header: header.into(),
			payload: Some(body.into()),
			fds: Vec::new(),
		}
	}

	pub fn no_payload(header: impl Into<MessageHeader>) -> Self {
		Self {
			header: header.into(),
			payload: None,
			fds: Vec::new(),
		}
	}
	pub fn hello(server: impl Into<String>) -> Self {
		let payload = HelloPayload {
			server: server.into(),
			protocol: PROTOCOL_VERSION.to_string(),
		};
		let json = serde_json::to_value(payload).expect("HelloPayload is serializable");
		Self::json("hello", json)
	}

	pub fn expect_n_fds(&self, amount: u32) -> Result<(), ProtocolError> {
		let found = self.fds.len() as u32;
		if found == amount {
			Ok(())
		} else {
			Err(ProtocolError::ExpectedFds {
				expected: amount,
				found,
			})
		}
	}

	#[tracing::instrument(skip_all, fields(frame_size = bytes.len(), fds = fds.len()))]
	pub fn parse_from_bytes(
		bytes: &[u8],
		fds: Vec<RawFd>,
	) -> Result<Option<(Self, usize)>, ProtocolError> {
		let Some(first_nl) = bytes.iter().position(|b| *b == b'\n') else {
			return Ok(None);
		};
		let Some(second_rel) = bytes[first_nl + 1..].iter().position(|b| *b == b'\n') else {
			return Ok(None);
		};
		let second_nl = first_nl + 1 + second_rel;
		let header_bytes = &bytes[..first_nl];
		let payload_bytes = &bytes[first_nl + 1..second_nl];
		let consumed = second_nl + 1;
		let frame = Self::from_lines(header_bytes, payload_bytes, fds)?;
		Ok(Some((frame, consumed)))
	}

	fn from_lines(
		header_bytes: &[u8],
		payload_bytes: &[u8],
		fds: Vec<RawFd>,
	) -> Result<Self, ProtocolError> {
		let header = String::from_utf8(header_bytes.to_vec())?;
		let payload_str = String::from_utf8(payload_bytes.to_vec())?;
		Ok(Self {
			header: header.into(),
			payload: if payload_str == "\0\0\0\0" {
				None
			} else {
				Some(payload_str)
			},
			fds,
		})
	}
}
