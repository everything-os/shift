use const_str::convert_ascii_case;

macro_rules! define_headers {
    ($( $name:ident ),* $(,)?) => {
        $(
            pub const $name: &str = {
                const RAW: &str = stringify!($name);
                const LOWER: &str = convert_ascii_case!(lower, RAW);
                LOWER
            };
        )*
    };
}

define_headers! {
		HELLO,
		AUTH,
		AUTH_OK,
		AUTH_ERROR,
		FRAMEBUFFER_LINK,
		SWAP_BUFFERS,
		FRAME_DONE,
		INPUT_EVENT,
		MONITOR_ADDED,
		MONITOR_REMOVED,
		SESSION_SWITCH,
		SESSION_CREATE,
		SESSION_CREATED,
		SESSION_READY,
		SESSION_STATE,
		SESSION_ACTIVE,
		REMOVE_CURSOR,
		SET_CURSOR_FB,
		SET_CURSOR_POS,
		ERROR,
		PING,
		PONG,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct MessageHeader(pub String);
impl<S: Into<String>> From<S> for MessageHeader {
	fn from(value: S) -> Self {
		Self(value.into())
	}
}
