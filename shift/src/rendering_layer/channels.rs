use crate::comms::{
	render2server::{RenderEvtRx, RenderEvtTx},
	server2render::{RenderCmdRx, RenderCmdTx},
};

const DEFAULT_CHANNEL_CAPACITY: usize = 5000;

#[derive(Debug)]
pub struct ServerEnd {
	render_events: RenderEvtRx,
	render_commands: RenderCmdTx,
}

impl ServerEnd {
	pub fn new(render_events: RenderEvtRx, render_commands: RenderCmdTx) -> Self {
		Self {
			render_events,
			render_commands,
		}
	}

	pub fn into_parts(self) -> (RenderEvtRx, RenderCmdTx) {
		(self.render_events, self.render_commands)
	}

	pub fn command_tx(&self) -> RenderCmdTx {
		self.render_commands.clone()
	}

	pub fn events(&mut self) -> &mut RenderEvtRx {
		&mut self.render_events
	}
}

#[derive(Debug)]
pub struct RenderingEnd {
	commands: RenderCmdRx,
	events: RenderEvtTx,
}

impl RenderingEnd {
	pub fn new(commands: RenderCmdRx, events: RenderEvtTx) -> Self {
		Self { commands, events }
	}

	pub fn into_parts(self) -> (RenderCmdRx, RenderEvtTx) {
		(self.commands, self.events)
	}

	pub fn commands(&mut self) -> &mut RenderCmdRx {
		&mut self.commands
	}

	pub fn events(&self) -> &RenderEvtTx {
		&self.events
	}
}

pub struct Channels {
	server_end: ServerEnd,
	rendering_end: RenderingEnd,
}

impl Channels {
	pub fn new() -> Self {
		Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
	}

	pub fn with_capacity(capacity: usize) -> Self {
		let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(capacity);
		let (evt_tx, evt_rx) = tokio::sync::mpsc::channel(capacity);

		Self {
			server_end: ServerEnd::new(evt_rx, cmd_tx),
			rendering_end: RenderingEnd::new(cmd_rx, evt_tx),
		}
	}

	pub fn split(self) -> (ServerEnd, RenderingEnd) {
		(self.server_end, self.rendering_end)
	}
}

impl Default for Channels {
	fn default() -> Self {
		Self::new()
	}
}
