use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::errno::Errno;
use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
use serde::Serialize;
use tracing::trace;

use crate::{HelloPayload, MessageHeader, PROTOCOL_VERSION, ProtocolError};

/// Raw framed Tab message: header line + payload line (strings) plus optional FDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMessageFrame {
	pub header: MessageHeader,
	pub payload: Option<String>,
	pub fds: Vec<RawFd>,
}

impl TabMessageFrame {
	/// Write a framed TabMessageFrame to the provided UnixStream using sendmsg/SCM_RIGHTS.
	pub fn encode_and_send(&self, stream: &UnixStream) -> Result<(), ProtocolError> {
		let header_line = format!("{}\n", self.header.0.trim_end());
		let payload_line = self
			.payload
			.as_ref()
			.map(|p| format!("{}\n", p.trim_end_matches('\n')))
			.unwrap_or_else(|| "\0\0\0\0\n".to_string());

		let iov = [
			IoSlice::new(header_line.as_bytes()),
			IoSlice::new(payload_line.as_bytes()),
		];

		let cmsg_vec: Vec<ControlMessage> = if self.fds.is_empty() {
			Vec::new()
		} else {
			vec![ControlMessage::ScmRights(&self.fds)]
		};

		sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsg_vec, MsgFlags::empty(), None)?;
		Ok(())
	}

	/// Read one Tab message frame using recvmsg/SCM_RIGHTS.
	pub fn read_framed(stream: &UnixStream) -> Result<Self, ProtocolError> {
		// Enough for two short lines.
		let mut buf = [0u8; 4096];
		// Allow up to 8 incoming FDs per message; Tab v1 uses far fewer.
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
		let bytes_read = msg.bytes;

		let mut fds = Vec::new();
		let mut c_iter = msg.cmsgs()?;
		while let Some(cmsg) = c_iter.next() {
			if let ControlMessageOwned::ScmRights(rights) = cmsg {
				fds.extend(rights);
			}
		}
		let _ = msg; // release borrow on iov/buf

		let data = &iov[0][..bytes_read];

		let Some((frame, used)) = Self::parse_from_bytes(data, fds)? else {
			return Err(ProtocolError::UnexpectedEof);
		};
		if used < data.len() {
			if data[used..].iter().any(|b| *b != 0) {
				return Err(ProtocolError::TrailingData);
			}
		}
		Ok(frame)
	}

	pub(crate) fn expect_payload_json<'a, T>(&'a self) -> Result<T, ProtocolError>
	where
		T: serde::Deserialize<'a>,
	{
		if let Some(payload) = &self.payload {
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

	pub fn parse_from_bytes(
		bytes: &[u8],
		fds: Vec<RawFd>,
	) -> Result<Option<(Self, usize)>, ProtocolError> {
		let Some(first_nl) = bytes.iter().position(|b| *b == b'\n') else {
			trace!(total = bytes.len(), "awaiting header newline");
			return Ok(None);
		};
		let Some(second_rel) = bytes[first_nl + 1..].iter().position(|b| *b == b'\n') else {
			trace!(total = bytes.len(), first_nl, "awaiting payload newline");
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
		if payload_bytes.len() > 1024 {
			trace!(
				header = header.as_str(),
				payload_len = payload_bytes.len(),
				payload_preview = %hex_preview(payload_bytes),
				"parsing large payload"
			);
		}
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

fn hex_preview(bytes: &[u8]) -> String {
	const MAX: usize = 16;
	bytes
		.iter()
		.take(MAX)
		.map(|b| format!("{:02x}", b))
		.collect::<Vec<_>>()
		.join(" ")
}
