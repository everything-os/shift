use easydrm::gl;
use thiserror::Error;

use crate::dma_buf_importer::ExternalTexture;
use crate::opengl::{Buffer, BufferGroup, Shader, TextureBindGuard};

#[derive(Debug, Error)]
pub enum RendererError {
	#[error("failed to compile shader: {0}")]
	Shader(String),
	#[error("failed to link program: {0}")]
	Program(String),
	#[error("failed to allocate GL resource")]
	Allocation,
	#[error("buffer length {len} is not divisible by dimensions {dimensions}")]
	InvalidDimensions { len: usize, dimensions: usize },
	#[error("invalid buffer index {0}")]
	InvalidBufferIndex(usize),
	#[error("buffer stores {expected} data but {actual} was provided")]
	TypeMismatch {
		expected: &'static str,
		actual: &'static str,
	},
}

const QUAD_POSITIONS: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
const QUAD_TEX_COORDS: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
const MAX_BLUR_PASSES: usize = 5;
const MIN_KAWASE_RADIUS: f32 = 2.0;
const MAX_KAWASE_RADIUS: f32 = 20.0;
const DEFAULT_VIBRANCY: f32 = 0.17;
const DEFAULT_VIBRANCY_DARKNESS: f32 = 0.0;
const FINISH_NOISE: f32 = 0.012;
const FINISH_BRIGHTNESS: f32 = 0.98;

const VERT_SHADER: &str = r#"
#version 330 core
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_tex_coord;
uniform vec2 u_scale;
uniform vec2 u_translate;
out vec2 v_tex_coord;

void main() {
	vec2 scaled = a_position * u_scale;
	gl_Position = vec4(scaled + u_translate, 0.0, 1.0);
	v_tex_coord = a_tex_coord;
}
"#;

const FRAG_SHADER: &str = r#"
#version 330 core
in vec2 v_tex_coord;
uniform sampler2D u_texture;
uniform sampler2D u_texture2;
uniform float u_tween;
uniform float u_blur;
out vec4 frag_color;


void main() {
	vec2 flipped_uv = vec2(v_tex_coord.x, 1.0 - v_tex_coord.y);
	vec4 base = texture(u_texture, flipped_uv);
	if (u_tween == 0.0) {
		frag_color = base;
		return;
	}
	vec4 base2 = texture(u_texture2, flipped_uv);
	vec4 tweened = mix(base, base2, u_tween);
	frag_color = tweened;
}
"#;

const KAWASE_DOWN_FRAG: &str = r#"
#version 330 core
in vec2 v_tex_coord;
uniform sampler2D u_texture;
uniform vec2 u_half_pixel;
uniform float u_radius;
uniform float u_passes;
uniform float u_vibrancy;
uniform float u_vibrancy_darkness;
out vec4 frag_color;

const float Pr = 0.299;
const float Pg = 0.587;
const float Pb = 0.114;
const float A_CONST = 0.93;
const float B_CONST = 0.11;
const float C_CONST = 0.66;

float doubleCircleSigmoid(float x, float a) {
    a = clamp(a, 0.0, 1.0);
    float y = 0.0;
    if (x <= a) {
        y = a - sqrt(a * a - x * x);
    } else {
        y = a + sqrt(pow(1.0 - a, 2.0) - pow(x - 1.0, 2.0));
    }
    return y;
}

vec3 rgb2hsl(vec3 col) {
    float minc  = min(col.r, min(col.g, col.b));
    float maxc  = max(col.r, max(col.g, col.b));
    float delta = maxc - minc;
    float lum = (minc + maxc) * 0.5;
    float sat = 0.0;
    float hue = 0.0;
    if (lum > 0.0 && lum < 1.0) {
        float mul = lum < 0.5 ? lum : (1.0 - lum);
        sat = delta / (mul * 2.0);
    }
    if (delta > 0.0) {
        vec3 maxcVec = vec3(maxc);
        vec3 masks = vec3(equal(maxcVec, col)) * vec3(notEqual(maxcVec, vec3(col.g, col.b, col.r)));
        vec3 adds = vec3(0.0, 2.0, 4.0) + vec3(col.g - col.b, col.b - col.r, col.r - col.g) / delta;
        hue += dot(adds, masks);
        hue /= 6.0;
        if (hue < 0.0)
            hue += 1.0;
    }
    return vec3(hue, sat, lum);
}

vec3 hsl2rgb(vec3 col) {
    const float onethird = 1.0 / 3.0;
    const float twothird = 2.0 / 3.0;
    const float rcpsixth = 6.0;
    float hue = col.x;
    float sat = col.y;
    float lum = col.z;
    vec3 xt = vec3(0.0);
    if (hue < onethird) {
        xt.r = rcpsixth * (onethird - hue);
        xt.g = rcpsixth * hue;
    } else if (hue < twothird) {
        xt.g = rcpsixth * (twothird - hue);
        xt.b = rcpsixth * (hue - onethird);
    } else {
        xt.r = rcpsixth * (hue - twothird);
        xt.b = rcpsixth * (1.0 - hue);
    }
    xt = min(xt, 1.0);
    float sat2 = 2.0 * sat;
    float satinv = 1.0 - sat;
    float luminv = 1.0 - lum;
    float lum2m1 = (2.0 * lum) - 1.0;
    vec3 ct = (sat2 * xt) + satinv;
    vec3 rgb;
    if (lum >= 0.5)
        rgb = (luminv * ct) + lum2m1;
    else
        rgb = lum * ct;
    return rgb;
}

void main() {
    vec2 offset = u_half_pixel * u_radius;
    vec4 sum = texture(u_texture, v_tex_coord) * 4.0;
    sum += texture(u_texture, v_tex_coord + vec2(offset.x, 0.0));
    sum += texture(u_texture, v_tex_coord - vec2(offset.x, 0.0));
    sum += texture(u_texture, v_tex_coord + vec2(0.0, offset.y));
    sum += texture(u_texture, v_tex_coord - vec2(0.0, offset.y));
    sum += texture(u_texture, v_tex_coord + offset);
    sum += texture(u_texture, v_tex_coord - offset);
    sum += texture(u_texture, v_tex_coord + vec2(offset.x, -offset.y));
    sum += texture(u_texture, v_tex_coord + vec2(-offset.x, offset.y));
    vec4 color = sum / 12.0;
    if (u_vibrancy <= 0.0) {
        frag_color = color;
        return;
    }
    float vibrancy_darkness1 = 1.0 - u_vibrancy_darkness;
    vec3 hsl = rgb2hsl(color.rgb);
    float perceived = doubleCircleSigmoid(sqrt(color.r * color.r * Pr + color.g * color.g * Pg + color.b * color.b * Pb), 0.8 * vibrancy_darkness1);
    float boostBase = hsl.y > 0.0 ? smoothstep((B_CONST * vibrancy_darkness1) - C_CONST * 0.5, (B_CONST * vibrancy_darkness1) + C_CONST * 0.5,
        1.0 - (pow(1.0 - hsl.y * cos(A_CONST), 2.0) + pow(1.0 - perceived * sin(A_CONST), 2.0))) : 0.0;
    float saturation = clamp(hsl.y + (boostBase * u_vibrancy) / max(u_passes, 1.0), 0.0, 1.0);
    vec3 newColor = hsl2rgb(vec3(hsl.x, saturation, hsl.z));
    frag_color = vec4(newColor, color.a);
}
"#;

const KAWASE_UP_FRAG: &str = r#"
#version 330 core
in vec2 v_tex_coord;
uniform sampler2D u_texture;
uniform vec2 u_half_pixel;
uniform float u_radius;
out vec4 frag_color;

void main() {
    vec2 offset = u_half_pixel * u_radius;
    vec4 sum = texture(u_texture, v_tex_coord);
    sum += texture(u_texture, v_tex_coord + vec2(offset.x, offset.y));
    sum += texture(u_texture, v_tex_coord + vec2(offset.x, -offset.y));
    sum += texture(u_texture, v_tex_coord - vec2(offset.x, offset.y));
    sum += texture(u_texture, v_tex_coord - vec2(offset.x, -offset.y));
    sum += texture(u_texture, v_tex_coord + vec2(offset.x, 0.0));
    sum += texture(u_texture, v_tex_coord - vec2(offset.x, 0.0));
    sum += texture(u_texture, v_tex_coord + vec2(0.0, offset.y));
    sum += texture(u_texture, v_tex_coord - vec2(0.0, offset.y));
    frag_color = sum / 9.0;
}
"#;

const KAWASE_FINISH_FRAG: &str = r#"
#version 330 core
in vec2 v_tex_coord;
uniform sampler2D u_texture;
uniform float u_noise;
uniform float u_brightness;
out vec4 frag_color;

float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 1689.1984);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

void main() {
    vec4 color = texture(u_texture, v_tex_coord);
    float noiseVal = hash(v_tex_coord) - 0.5;
    color.rgb += noiseVal * u_noise;
    color.rgb *= u_brightness;
    frag_color = color;
}
"#;

const PASS_VERT_SHADER: &str = r#"
#version 330 core
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_tex_coord;
out vec2 v_tex_coord;

void main() {
	gl_Position = vec4(a_position, 0.0, 1.0);
	v_tex_coord = a_tex_coord;
}
"#;

fn fullscreen_geometry(gl: &gl::Gles2) -> Result<BufferGroup, RendererError> {
	let mut geometry = BufferGroup::new(gl)?;
	let position_buffer = Buffer::new_f32(gl, &QUAD_POSITIONS, 2)?;
	let tex_coord_buffer = Buffer::new_f32(gl, &QUAD_TEX_COORDS, 2)?;
	let position_attr = 0;
	let tex_attr = 1;
	geometry.add_buffer(position_buffer, position_attr);
	geometry.add_buffer(tex_coord_buffer, tex_attr);
	Ok(geometry)
}

pub struct Transform2D {
	pub translate: [f32; 2],
	pub scale: [f32; 2],
}

impl Transform2D {
	pub fn identity() -> Self {
		Self {
			translate: [0.0, 0.0],
			scale: [1.0, 1.0],
		}
	}
}

pub trait TextureRef {
	fn bind_texture(&self, slot: u32) -> TextureBindGuard;
	fn width(&self) -> i32;
	fn height(&self) -> i32;
}

impl TextureRef for ExternalTexture {
	fn bind_texture(&self, slot: u32) -> TextureBindGuard {
		self.bind(slot)
	}

	fn width(&self) -> i32 {
		self.width
	}

	fn height(&self) -> i32 {
		self.height
	}
}

pub struct QuadRenderer {
	gl: gl::Gles2,
	shader: Shader,
	geometry: BufferGroup,
	u_scale: i32,
	u_translate: i32,
	u_texture: i32,
	u_texture2: i32,
	u_tween: i32,
}

impl QuadRenderer {
	pub fn new(gl_ctx: &gl::Gles2) -> Result<Self, RendererError> {
		let shader = Shader::new(gl_ctx, VERT_SHADER, FRAG_SHADER)?;
		let geometry = fullscreen_geometry(gl_ctx)?;
		let u_scale = shader.uniform_location("u_scale");
		let u_translate = shader.uniform_location("u_translate");
		let u_texture = shader.uniform_location("u_texture");
		let u_texture2 = shader.uniform_location("u_texture2");
		let u_tween = shader.uniform_location("u_tween");
		gl!(gl_ctx, Enable(gl::BLEND));
		gl!(gl_ctx, BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA));
		Ok(Self {
			gl: gl_ctx.clone(),
			shader,
			geometry,
			u_scale,
			u_translate,
			u_texture,
			u_texture2,
			u_tween,
		})
	}

	pub fn draw<T: TextureRef>(&self, texture: &T, transform: Transform2D) {
		let _program = self.shader.bind();
		let _vao = self.geometry.bind();
		let _tex = texture.bind_texture(0);
		gl!(
			&self.gl,
			Uniform2f(self.u_scale, transform.scale[0], transform.scale[1])
		);
		gl!(
			&self.gl,
			Uniform2f(
				self.u_translate,
				transform.translate[0],
				transform.translate[1]
			)
		);
		gl!(&self.gl, Uniform1i(self.u_texture, 0));
		gl!(&self.gl, Uniform1f(self.u_tween, 0.0));
		gl!(&self.gl, DrawArrays(gl::TRIANGLE_STRIP, 0, 4));
	}

	pub fn draw_tween(
		&self,
		texture1: &impl TextureRef,
		texture2: &impl TextureRef,
		transform: Transform2D,
		tween: f32,
	) {
		let _program = self.shader.bind();
		let _vao = self.geometry.bind();
		let _tex1 = texture1.bind_texture(0);
		let _tex2 = texture2.bind_texture(1);
		gl!(
			&self.gl,
			Uniform2f(self.u_scale, transform.scale[0], transform.scale[1])
		);
		gl!(
			&self.gl,
			Uniform2f(
				self.u_translate,
				transform.translate[0],
				transform.translate[1]
			)
		);
		gl!(&self.gl, Uniform1i(self.u_texture, 0));
		gl!(&self.gl, Uniform1i(self.u_texture2, 1));
		gl!(&self.gl, Uniform1f(self.u_tween, tween.clamp(0.0, 1.0)));
		gl!(&self.gl, DrawArrays(gl::TRIANGLE_STRIP, 0, 4));
	}

	#[allow(dead_code)]
	pub fn gl(&self) -> &gl::Gles2 {
		&self.gl
	}
}

pub struct BlurPipeline {
	gl: gl::Gles2,
	geometry: BufferGroup,
	down_shader: Shader,
	down_tex: i32,
	down_half_pixel: i32,
	down_radius: i32,
	down_passes: i32,
	down_vibrancy: i32,
	down_vibrancy_darkness: i32,
	up_shader: Shader,
	up_tex: i32,
	up_half_pixel: i32,
	up_radius: i32,
	finish_shader: Shader,
	finish_tex: i32,
	finish_noise: i32,
	finish_brightness: i32,
}

impl BlurPipeline {
	pub fn new(gl_ctx: &gl::Gles2) -> Result<Self, RendererError> {
		let geometry = fullscreen_geometry(gl_ctx)?;
		let down_shader = Shader::new(gl_ctx, PASS_VERT_SHADER, KAWASE_DOWN_FRAG)?;
		let up_shader = Shader::new(gl_ctx, PASS_VERT_SHADER, KAWASE_UP_FRAG)?;
		let finish_shader = Shader::new(gl_ctx, PASS_VERT_SHADER, KAWASE_FINISH_FRAG)?;
		Ok(Self {
			gl: gl_ctx.clone(),
			geometry,
			down_tex: down_shader.uniform_location("u_texture"),
			down_half_pixel: down_shader.uniform_location("u_half_pixel"),
			down_radius: down_shader.uniform_location("u_radius"),
			down_passes: down_shader.uniform_location("u_passes"),
			down_vibrancy: down_shader.uniform_location("u_vibrancy"),
			down_vibrancy_darkness: down_shader.uniform_location("u_vibrancy_darkness"),
			down_shader,
			up_tex: up_shader.uniform_location("u_texture"),
			up_half_pixel: up_shader.uniform_location("u_half_pixel"),
			up_radius: up_shader.uniform_location("u_radius"),
			up_shader,
			finish_tex: finish_shader.uniform_location("u_texture"),
			finish_noise: finish_shader.uniform_location("u_noise"),
			finish_brightness: finish_shader.uniform_location("u_brightness"),
			finish_shader,
		})
	}

	pub fn gl(&self) -> &gl::Gles2 {
		&self.gl
	}

	pub fn dual_kawase_blur(
		&self,
		ping_pong: &mut PingPongBuffers,
		base: ScratchTexture,
		amount: f32,
	) -> ScratchTexture {
		if amount <= f32::EPSILON {
			return base;
		}
		let passes = (amount * MAX_BLUR_PASSES as f32).ceil() as usize;
		let passes = passes.clamp(1, MAX_BLUR_PASSES);
		let radius = MIN_KAWASE_RADIUS + amount * (MAX_KAWASE_RADIUS - MIN_KAWASE_RADIUS);
		let vibrancy = DEFAULT_VIBRANCY * amount;
		let mut textures = Vec::with_capacity(passes + 1);
		textures.push(base);
		for level in 0..passes {
			let source_size = (
				textures[level].width().max(1),
				textures[level].height().max(1),
			);
			let target_w = (source_size.0 / 2).max(1);
			let target_h = (source_size.1 / 2).max(1);
			let target = ScratchTexture::new(&self.gl, target_w, target_h);
			ping_pong.bind_external(target.texture_id());
			gl!(&self.gl, Viewport(0, 0, target_w, target_h));
			self.draw_down(
				&textures[level],
				radius,
				passes as f32,
				(0.5 / source_size.0 as f32, 0.5 / source_size.1 as f32),
				vibrancy,
			);
			ping_pong.unbind();
			textures.push(target);
		}
		for level in (1..=passes).rev() {
			let (left, right) = textures.split_at_mut(level);
			let dest = &mut left[level - 1];
			let src = &right[0];
			ping_pong.bind_external(dest.texture_id());
			gl!(&self.gl, Viewport(0, 0, dest.width(), dest.height()));
			self.draw_up(
				src,
				radius,
				(
					0.5 / dest.width().max(1) as f32,
					0.5 / dest.height().max(1) as f32,
				),
			);
			ping_pong.unbind();
		}
		let final_base = textures.remove(0);
		let final_texture = ScratchTexture::new(&self.gl, final_base.width(), final_base.height());
		ping_pong.bind_external(final_texture.texture_id());
		gl!(
			&self.gl,
			Viewport(0, 0, final_texture.width(), final_texture.height())
		);
		self.draw_finish(&final_base);
		ping_pong.unbind();
		final_texture
	}

	fn draw_down<T: TextureRef>(
		&self,
		texture: &T,
		radius: f32,
		passes: f32,
		half_pixel: (f32, f32),
		vibrancy: f32,
	) {
		let _program = self.down_shader.bind();
		let _vao = self.geometry.bind();
		let _tex = texture.bind_texture(0);
		gl!(&self.gl, Uniform1i(self.down_tex, 0));
		gl!(
			&self.gl,
			Uniform2f(self.down_half_pixel, half_pixel.0, half_pixel.1)
		);
		gl!(&self.gl, Uniform1f(self.down_radius, radius));
		gl!(&self.gl, Uniform1f(self.down_passes, passes));
		gl!(&self.gl, Uniform1f(self.down_vibrancy, vibrancy));
		gl!(
			&self.gl,
			Uniform1f(self.down_vibrancy_darkness, DEFAULT_VIBRANCY_DARKNESS)
		);
		gl!(&self.gl, DrawArrays(gl::TRIANGLE_STRIP, 0, 4));
	}

	fn draw_up<T: TextureRef>(&self, texture: &T, radius: f32, half_pixel: (f32, f32)) {
		let _program = self.up_shader.bind();
		let _vao = self.geometry.bind();
		let _tex = texture.bind_texture(0);
		gl!(&self.gl, Uniform1i(self.up_tex, 0));
		gl!(
			&self.gl,
			Uniform2f(self.up_half_pixel, half_pixel.0, half_pixel.1)
		);
		gl!(&self.gl, Uniform1f(self.up_radius, radius));
		gl!(&self.gl, DrawArrays(gl::TRIANGLE_STRIP, 0, 4));
	}

	fn draw_finish<T: TextureRef>(&self, texture: &T) {
		let _program = self.finish_shader.bind();
		let _vao = self.geometry.bind();
		let _tex = texture.bind_texture(0);
		gl!(&self.gl, Uniform1i(self.finish_tex, 0));
		gl!(&self.gl, Uniform1f(self.finish_noise, FINISH_NOISE));
		gl!(
			&self.gl,
			Uniform1f(self.finish_brightness, FINISH_BRIGHTNESS)
		);
		gl!(&self.gl, DrawArrays(gl::TRIANGLE_STRIP, 0, 4));
	}
}

pub struct AnimationCanvas<'a> {
	renderer: &'a QuadRenderer,
	blur_pipeline: &'a BlurPipeline,
	ping_pong: &'a mut PingPongBuffers,
	viewport: (i32, i32),
}

impl<'a> AnimationCanvas<'a> {
	pub fn new(
		renderer: &'a QuadRenderer,
		blur_pipeline: &'a BlurPipeline,
		ping_pong: &'a mut PingPongBuffers,
		viewport: (i32, i32),
	) -> Self {
		Self {
			renderer,
			blur_pipeline,
			ping_pong,
			viewport,
		}
	}

	pub fn draw_texture<T: TextureRef>(&mut self, texture: &T, transform: Transform2D) {
		self.renderer.draw(texture, transform);
	}
	pub fn draw_texture_tweening(
		&mut self,
		texture: &impl TextureRef,
		texture2: &impl TextureRef,
		mix: f32,
		transform: Transform2D,
	) {
		self.renderer.draw_tween(texture, texture2, transform, mix);
	}

	pub fn draw_tweening_with_blur(
		&mut self,
		texture: &impl TextureRef,
		texture2: &impl TextureRef,
		mix: f32,
		amount: f32,
	) -> ScratchTexture {
		let normalized = amount.clamp(0.0, 1.0);
		let mix_texture = self.render_mix_texture(texture, Some(texture2), mix);
		self
			.blur_pipeline
			.dual_kawase_blur(self.ping_pong, mix_texture, normalized)
	}

	fn render_mix_texture(
		&mut self,
		primary: &impl TextureRef,
		secondary: Option<&impl TextureRef>,
		mix: f32,
	) -> ScratchTexture {
		let (width, height) = self.viewport;
		let target = ScratchTexture::new(self.blur_pipeline.gl(), width.max(1), height.max(1));
		self.ping_pong.bind_external(target.texture_id());
		gl!(
			self.blur_pipeline.gl(),
			Viewport(0, 0, width.max(1), height.max(1))
		);
		match secondary {
			Some(sec) => self
				.renderer
				.draw_tween(primary, sec, Transform2D::identity(), mix),
			None => self.renderer.draw(primary, Transform2D::identity()),
		}
		self.ping_pong.unbind();
		target
	}
}

pub struct ScratchTexture {
	gl: gl::Gles2,
	texture: u32,
	width: i32,
	height: i32,
}

impl ScratchTexture {
	fn new(gl: &gl::Gles2, width: i32, height: i32) -> Self {
		let mut texture = 0;
		gl!(gl, GenTextures(1, &mut texture));
		gl!(gl, BindTexture(gl::TEXTURE_2D, texture));
		gl!(
			gl,
			TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32)
		);
		gl!(
			gl,
			TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32)
		);
		gl!(
			gl,
			TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32)
		);
		gl!(
			gl,
			TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32)
		);
		gl!(
			gl,
			TexImage2D(
				gl::TEXTURE_2D,
				0,
				gl::RGBA as i32,
				width.max(1),
				height.max(1),
				0,
				gl::RGBA,
				gl::UNSIGNED_BYTE,
				std::ptr::null()
			)
		);
		gl!(gl, BindTexture(gl::TEXTURE_2D, 0));
		Self {
			gl: gl.clone(),
			texture,
			width,
			height,
		}
	}
}

impl Drop for ScratchTexture {
	fn drop(&mut self) {
		if self.texture != 0 {
			gl!(&self.gl, DeleteTextures(1, &self.texture));
		}
	}
}

impl ScratchTexture {
	fn bind(&self, slot: u32) -> TextureBindGuard {
		TextureBindGuard::bind(&self.gl, gl::TEXTURE_2D, self.texture, slot)
	}

	pub(crate) fn texture_id(&self) -> u32 {
		self.texture
	}
}

impl TextureRef for ScratchTexture {
	fn bind_texture(&self, slot: u32) -> TextureBindGuard {
		self.bind(slot)
	}

	fn width(&self) -> i32 {
		self.width
	}

	fn height(&self) -> i32 {
		self.height
	}
}

pub struct PingPongBuffers {
	gl: gl::Gles2,
	framebuffer: u32,
}

impl PingPongBuffers {
	pub fn new(gl: &gl::Gles2) -> Result<Self, RendererError> {
		let mut fbo = 0;
		gl!(gl, GenFramebuffers(1, &mut fbo));
		if fbo == 0 {
			return Err(RendererError::Allocation);
		}
		Ok(Self {
			gl: gl.clone(),
			framebuffer: fbo,
		})
	}

	pub fn unbind(&self) {
		gl!(&self.gl, BindFramebuffer(gl::FRAMEBUFFER, 0));
	}

	pub fn bind_external(&self, texture: u32) {
		gl!(&self.gl, BindFramebuffer(gl::FRAMEBUFFER, self.framebuffer));
		gl!(
			&self.gl,
			FramebufferTexture2D(
				gl::FRAMEBUFFER,
				gl::COLOR_ATTACHMENT0,
				gl::TEXTURE_2D,
				texture,
				0
			)
		);
	}
}

impl Drop for PingPongBuffers {
	fn drop(&mut self) {
		if self.framebuffer != 0 {
			gl!(&self.gl, DeleteFramebuffers(1, &self.framebuffer));
		}
	}
}
