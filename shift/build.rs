use std::{env, fs::File, path::PathBuf};

use gl_generator::{Api, Fallbacks, Profile, Registry};

fn main() {
	let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

	// EGL bindings
	let egl_path = out_dir.join("egl_bindings.rs");
	let mut egl_file = File::create(&egl_path).unwrap();
	Registry::new(
		Api::Egl,
		(1, 5),
		Profile::Core,
		Fallbacks::All,
		&[
			"EGL_KHR_image_base",
			"EGL_KHR_fence_sync",
			"EGL_KHR_reusable_sync",
			"EGL_EXT_image_dma_buf_import",
			"EGL_EXT_image_dma_buf_import_modifiers",
			"EGL_MESA_image_dma_buf_export",
			"EGL_KHR_surfaceless_context",
			"EGL_ANDROID_native_fence_sync",
		],
	)
	.write_bindings(gl_generator::StructGenerator, &mut egl_file)
	.unwrap();

	println!("cargo:rerun-if-changed=build.rs");
}
