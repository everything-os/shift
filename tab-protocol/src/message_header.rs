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
		BUFFER_REQUEST,
		BUFFER_REQUEST_ACK,
		BUFFER_RELEASE,
		INPUT_EVENT,
		MONITOR_ADDED,
		MONITOR_REMOVED,
		SESSION_SWITCH,
		SESSION_CREATE,
		SESSION_CREATED,
		SESSION_READY,
		SESSION_STATE,
		SESSION_ACTIVE,
		SESSION_AWAKE,
		SESSION_SLEEP,
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
