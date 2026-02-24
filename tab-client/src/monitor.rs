use tab_protocol::MonitorInfo;

pub type MonitorId = String;

#[derive(Debug, Clone)]
pub struct MonitorState {
	pub info: MonitorInfo,
}

impl MonitorState {
	pub fn new(info: MonitorInfo) -> Self {
		Self { info }
	}
}
