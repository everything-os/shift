use crate::define_id_type;
use tab_protocol::MonitorInfo as ProtocolMonitorInfo;

define_id_type!(Monitor, "mon_");
#[derive(Debug, Clone)]
pub struct Monitor {
	pub id: MonitorId,
	pub width: i32,
	pub height: i32,
	pub refresh_rate: u32,
	pub name: String,
}

impl Monitor {
	pub fn to_protocol_info(&self) -> ProtocolMonitorInfo {
		ProtocolMonitorInfo {
			id: self.id.to_string(),
			width: self.width,
			height: self.height,
			refresh_rate: self.refresh_rate as i32,
			name: self.name.clone(),
		}
	}
}
