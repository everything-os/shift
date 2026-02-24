use skia_safe::{
	Canvas, FilterMode, Image, MipmapMode, Paint, Rect, SamplingOptions, TileMode, image_filters,
};

pub trait Animation: Send + Sync {
	fn draw(
		&self,
		canvas: &Canvas,
		old_image: &Image,
		new_image: &Image,
		progress: f64,
		width: f32,
		height: f32,
	);
}

#[derive(Default)]
pub struct AnimationRegistry {
	animations: std::collections::HashMap<String, Box<dyn Animation>>,
}

impl AnimationRegistry {
	pub fn new() -> Self {
		let mut this = Self::default();
		this.register("slide_left", Box::<SlideLeftAnimation>::default());
		this.register("blur", Box::<BlurBlendAnimation>::default());
		this
	}

	pub fn register(&mut self, name: impl Into<String>, animation: Box<dyn Animation>) {
		self.animations.insert(name.into(), animation);
	}

	pub fn get(&self, name: &str) -> Option<&dyn Animation> {
		self.animations.get(name).map(|v| v.as_ref())
	}
}

#[derive(Default)]
struct SlideLeftAnimation;

impl Animation for SlideLeftAnimation {
	fn draw(
		&self,
		canvas: &Canvas,
		old_image: &Image,
		new_image: &Image,
		progress: f64,
		width: f32,
		height: f32,
	) {
		let t = progress.clamp(0.0, 1.0) as f32;
		let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
		let mut paint = Paint::default();
		paint.set_argb(255, 255, 255, 255);

		let old_left = -width * t;
		let new_left = width * (1.0 - t);
		let old_rect = Rect::from_xywh(old_left, 0.0, width, height);
		let new_rect = Rect::from_xywh(new_left, 0.0, width, height);
		canvas.draw_image_rect_with_sampling_options(old_image, None, old_rect, sampling, &paint);
		canvas.draw_image_rect_with_sampling_options(new_image, None, new_rect, sampling, &paint);
	}
}

#[derive(Default)]
struct BlurBlendAnimation;

impl Animation for BlurBlendAnimation {
	fn draw(
		&self,
		canvas: &Canvas,
		old_image: &Image,
		new_image: &Image,
		progress: f64,
		width: f32,
		height: f32,
	) {
		let t = progress.clamp(0.0, 1.0) as f32;
		let phase = if t < 0.5 { 0 } else { 1 };
		let local_t = if phase == 0 { t * 2.0 } else { (t - 0.5) * 2.0 };

		if phase == 0 {
			// Blur the old frame out.
			draw_blurred_image(canvas, old_image, width, height, 60.0 * local_t, 1.0);
		} else {
			// Bring in the new frame blurred, then sharpen it.
			draw_blurred_image(
				canvas,
				new_image,
				width,
				height,
				60.0 * (1.0 - local_t),
				1.0,
			);
		}
	}
}

fn draw_blurred_image(
	canvas: &Canvas,
	image: &Image,
	width: f32,
	height: f32,
	radius: f32,
	alpha: f32,
) {
	let rect = Rect::from_wh(width, height);
	let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear);
	let clamped_alpha = alpha.clamp(0.0, 1.0);
	let mut paint = Paint::default();
	paint.set_argb((255.0 * clamped_alpha) as u8, 255, 255, 255);
	if radius > 0.001
		&& let Some(filter) = image_filters::blur((radius, radius), TileMode::Clamp, None, None)
	{
		paint.set_image_filter(filter);
	}
	canvas.draw_image_rect_with_sampling_options(image, None, rect, sampling, &paint);
}
