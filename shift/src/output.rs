use easydrm::MonitorContextCreationRequest;

use crate::egl::Egl;
use crate::renderer::MonitorRenderer;
use tab_server::MonitorIdStorage;

pub struct OutputContext {
	monitor_id: Option<String>,
	pub egl: Egl,
	pub renderer: MonitorRenderer,
}

impl OutputContext {
	pub fn new(request: &MonitorContextCreationRequest<'_>) -> Self {
		let egl = Egl::load_with(request.get_proc_address);
		let renderer = MonitorRenderer::new(request.gl).expect("failed to initialize renderer");
		Self {
			monitor_id: None,
			egl,
			renderer,
		}
	}
	pub fn monitor_id(&self) -> Option<&str> {
		self.monitor_id.as_deref()
	}
}

impl MonitorIdStorage for OutputContext {
	fn monitor_id(&self) -> Option<&str> {
		self.monitor_id()
	}

	fn set_monitor_id(&mut self, id: String) {
		self.monitor_id = Some(id);
	}
}
