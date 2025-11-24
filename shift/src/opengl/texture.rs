use easydrm::gl;

use crate::opengl::binding::read_binding;

pub struct TextureBindGuard {
	gl: gl::Gles2,
	target: u32,
	old_slot: u32,
	old_id: i32,
	changed: bool,
}

impl TextureBindGuard {
	pub fn bind(gl: &gl::Gles2, target: u32, texture: u32, slot: u32) -> Self {
		let raw_slot = read_binding(gl, gl::ACTIVE_TEXTURE);
		let old_slot = raw_slot.saturating_sub(gl::TEXTURE0);
		let binding_enum = binding_param(target);
		let mut old_id = 0;
		gl!(gl, GetIntegerv(binding_enum, &mut old_id));
		let changed = old_slot != slot || old_id as u32 != texture;
		if changed {
			gl!(gl, ActiveTexture(gl::TEXTURE0 + slot));
			gl!(gl, BindTexture(target, texture));
		}
		Self {
			gl: gl.clone(),
			target,
			old_slot,
			old_id,
			changed,
		}
	}
}

impl Drop for TextureBindGuard {
	fn drop(&mut self) {
		if self.changed {
			gl!(&self.gl, ActiveTexture(gl::TEXTURE0 + self.old_slot));
			gl!(&self.gl, BindTexture(self.target, self.old_id as u32));
		}
	}
}

fn binding_param(target: u32) -> u32 {
	match target {
		gl::TEXTURE_2D => gl::TEXTURE_BINDING_2D,
		_ => panic!("unsupported texture target {target}"),
	}
}
