use std::{env, fs::File, path::PathBuf};

use gl_generator::{Api, Fallbacks, Profile, Registry};

fn main() {
	let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

	let egl_path = out_dir.join("egl_bindings.rs");
	let mut egl_file = File::create(&egl_path).expect("failed to create egl bindings file");

	Registry::new(
		Api::Egl,
		(1, 5),
		Profile::Core,
		Fallbacks::All,
		&[
			"EGL_EXT_platform_base",
			"EGL_KHR_platform_gbm",
			"EGL_MESA_platform_gbm",
			"EGL_KHR_surfaceless_context",
			"EGL_KHR_create_context",
			"EGL_KHR_fence_sync",
			"EGL_KHR_image_base",
			"EGL_EXT_image_dma_buf_import",
			"EGL_ANDROID_native_fence_sync",
		],
	)
	.write_bindings(gl_generator::StructGenerator, &mut egl_file)
	.expect("failed to generate EGL bindings");

	println!("cargo:rerun-if-changed=build.rs");
}
