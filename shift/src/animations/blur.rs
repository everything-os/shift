use crate::animations::animation::{BasicAnimation, DelayAnimation};
use crate::animations::animation::{FromToAnimation, easing};
use crate::animations::{AnimationStateTracker, Transition, TransitionFrame};
use crate::dma_buf_importer::ExternalTexture;
use crate::renderer::{AnimationCanvas, Transform2D};
use crate::{all, seq};

pub struct BlurFade;

impl Transition for BlurFade {
	fn timeline(&self) -> AnimationStateTracker {
		let fade_time = 1. / 2.;
		let fade_delay = (1. - fade_time) / 2.;
		AnimationStateTracker::from(all!(
			seq!(
				BasicAnimation::new("blur", 1. / 2., easing::ease_out_cubic),
				FromToAnimation::new("blur", 1. / 2., 1., 0., easing::ease_in_cubic)
			),
			DelayAnimation::new(
				fade_delay,
				BasicAnimation::new("fade", fade_time, easing::ease_out_cubic)
			)
		))
	}

	fn render(
		&self,
		canvas: &mut AnimationCanvas<'_>,
		primary: &ExternalTexture,
		secondary: Option<&ExternalTexture>,
		frame: TransitionFrame<'_>,
	) {
		let secondary = secondary.unwrap_or(primary);
		let blur = frame.value("blur");
		let fade = frame.value("fade");
		let outgoing = canvas.draw_tweening_with_blur(primary, secondary, fade, blur);
		canvas.draw_texture(&outgoing, Transform2D::identity());
	}
}
