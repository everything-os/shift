#[macro_export]
macro_rules! define_id_type {
	(
        $name:ident,
        $prefix:literal
    ) => {
		paste::paste! {

				#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
				pub struct [<$name Id>](u64);

				impl [<$name Id>] {
						#[inline]
						pub fn rand() -> Self {
								Self(rand::random::<u64>())
						}

						#[inline]
						pub fn raw(self) -> u64 {
								self.0
						}
				}

				impl std::fmt::Display for [<$name Id>] {
						fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
								f.write_fmt(format_args!(concat!($prefix, "{:x}"), self.0))
						}
				}

				#[derive(thiserror::Error, Debug)]
				pub enum [<$name IdParseError>] {
						#[error("invalid id: {0}")]
						InvalidHex(std::num::ParseIntError),
						#[error("expected prefix '{expected}' but found {found:?}")]
						InvalidPrefix {
								expected: &'static str,
								found: Option<String>
						},
				}

				impl std::str::FromStr for [<$name Id>] {
						type Err = [<$name IdParseError>];

						fn from_str(s: &str) -> Result<Self, Self::Err> {
								if !s.starts_with($prefix) {
										return Err(Self::Err::InvalidPrefix {
												expected: $prefix,
												found: s.split_once("_").map(|(prefix, _)| format!("{prefix}_"))
										});
								}

								let s = &s[$prefix.len()..];
								u64::from_str_radix(s, 16)
										.map(Self)
										.map_err(Self::Err::InvalidHex)
						}
				}
		}
	};
}
