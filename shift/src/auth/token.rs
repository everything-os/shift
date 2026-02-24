use core::fmt;
use std::str::FromStr;

use base64::Engine;

/// A cryptographically-random token (opaque bytes) with convenient encoding/decoding.
///
/// - Generated using the OS CSPRNG (via `getrandom`)
/// - Encoded as URL-safe base64 (no padding)
/// - Constant-time equality to avoid timing leaks in comparisons
#[derive(Clone, Hash)]
pub struct Token<const N: usize = 20> {
	bytes: [u8; N],
}

impl<const N: usize> Token<N> {
	/// Generate a new cryptographically-random token.
	pub fn generate() -> Result<Self, Error> {
		let mut bytes = [0u8; N];
		getrandom::fill(&mut bytes).map_err(Error::GetRandom)?;
		Ok(Self { bytes })
	}

	/// Get the raw token bytes (opaque). Prefer `to_string()` for transport/storage.
	pub fn as_bytes(&self) -> &[u8; N] {
		&self.bytes
	}

	/// Encode the token as URL-safe base64 without padding.
	pub fn to_base64url(&self) -> String {
		base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.bytes)
	}

	/// Decode a URL-safe base64 token (no padding) into a Token.
	pub fn from_base64url(s: &str) -> Result<Self, Error> {
		let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
			.decode(s)
			.map_err(Error::Base64)?;

		if decoded.len() != N {
			return Err(Error::InvalidLength {
				expected: N,
				got: decoded.len(),
			});
		}

		let mut bytes = [0u8; N];
		bytes.copy_from_slice(&decoded);
		Ok(Self { bytes })
	}

	/// Constant-time token comparison.
	pub fn ct_eq(&self, other: &Self) -> bool {
		subtle::ConstantTimeEq::ct_eq(self.bytes.as_slice(), other.bytes.as_slice()).into()
	}
}

impl<const N: usize> fmt::Debug for Token<N> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let redact_token = !std::env::var("NO_REDACT").is_ok_and(|s| s == "y");
		if redact_token {
			f.write_str("Token(REDACTED)")
		} else {
			fmt::Display::fmt(self, f)
		}
	}
}

impl<const N: usize> fmt::Display for Token<N> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Display as base64url for easy transport.
		f.write_str(&self.to_base64url())
	}
}
impl<const N: usize> FromStr for Token<N> {
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Self::from_base64url(s)
	}
	type Err = Error;
}

impl<const N: usize> PartialEq for Token<N> {
	fn eq(&self, other: &Self) -> bool {
		self.ct_eq(other)
	}
}
impl<const N: usize> Eq for Token<N> {}

impl<const N: usize> Drop for Token<N> {
	fn drop(&mut self) {
		// Best-effort zeroization to reduce accidental leakage in memory dumps.
		self.bytes.fill(0);
	}
}

#[derive(Debug)]
pub enum Error {
	GetRandom(getrandom::Error),
	Base64(base64::DecodeError),
	InvalidLength { expected: usize, got: usize },
}
