//! XKB keyboard composition helpers used by the GL framework.

use thiserror::Error;
use xkbcommon::xkb;

/// Effective modifier state snapshot.
#[derive(Debug, Clone, Default)]
pub struct Modifiers {
	/// Depressed modifiers bitmask.
	pub depressed: u32,
	/// Latched modifiers bitmask.
	pub latched: u32,
	/// Locked modifiers bitmask.
	pub locked: u32,
	/// Active keyboard group/layout index.
	pub group: u32,
}

/// Result of feeding one key event through XKB.
#[derive(Debug, Clone)]
pub struct KeyComposition {
	/// Optional UTF-8 text produced by the key event.
	pub text: Option<String>,
	/// Whether compose state consumed the key.
	pub consumed: bool,
	/// Resulting keysym.
	pub keysym: u32,
	/// Effective modifier state after processing.
	pub modifiers: Modifiers,
}

/// Errors from XKB initialization.
#[derive(Debug, Error)]
pub enum XkbError {
	#[error("failed to build xkb keymap")]
	KeymapCompile,
	#[error("failed to build xkb compose table")]
	ComposeTable,
}

/// Stateful XKB engine for key->text composition.
pub struct XkbEngine {
	_context: xkb::Context,
	state: xkb::State,
	compose: Option<xkb::compose::State>,
}

impl XkbEngine {
	/// Creates an XKB engine using current locale environment.
	pub fn new() -> Result<Self, XkbError> {
		let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
		let keymap =
			xkb::Keymap::new_from_names(&context, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
				.ok_or(XkbError::KeymapCompile)?;
		let state = xkb::State::new(&keymap);

		let compose = std::env::var("LC_ALL")
			.ok()
			.or_else(|| std::env::var("LC_CTYPE").ok())
			.or_else(|| std::env::var("LANG").ok())
			.and_then(|locale| {
				xkb::compose::Table::new_from_locale(
					&context,
					std::ffi::OsStr::new(&locale),
					xkb::compose::COMPILE_NO_FLAGS,
				)
				.ok()
			})
			.map(|table| xkb::compose::State::new(&table, xkb::compose::STATE_NO_FLAGS));

		Ok(Self {
			_context: context,
			state,
			compose,
		})
	}

	/// Processes a key event and returns composition output.
	///
	/// `keycode` is the Linux evdev keycode (without the XKB +8 offset).
	pub fn process_key(&mut self, keycode: u32, pressed: bool) -> KeyComposition {
		let xkb_keycode = keycode.saturating_add(8);
		let xkb_keycode = xkb_keycode.into();
		let direction = if pressed {
			xkb::KeyDirection::Down
		} else {
			xkb::KeyDirection::Up
		};
		self.state.update_key(xkb_keycode, direction);

		let keysym = self.state.key_get_one_sym(xkb_keycode);
		let mut text = if pressed {
			let utf = self.state.key_get_utf8(xkb_keycode);
			if utf.is_empty() { None } else { Some(utf) }
		} else {
			None
		};
		let mut consumed = false;

		if pressed {
			if let Some(compose) = self.compose.as_mut() {
				consumed = true;
				compose.feed(keysym);
				match compose.status() {
					xkb::compose::Status::Composed => {
						text = compose.utf8().filter(|s| !s.is_empty());
						compose.reset();
					}
					xkb::compose::Status::Cancelled => {
						text = None;
						compose.reset();
					}
					xkb::compose::Status::Composing => {
						text = None;
					}
					xkb::compose::Status::Nothing => {
						// Keep fallback `state.key_get_utf8` result.
					}
				}
			}
		}

		KeyComposition {
			text,
			consumed,
			keysym: keysym.raw(),
			modifiers: Modifiers {
				depressed: self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
				latched: self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
				locked: self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
				group: self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
			},
		}
	}
}
