//! glide — what the camera does after the finger leaves the glass.
//!
//! Today a lift stops the look dead. That is the RIGID behaviour and it is a
//! legitimate preference, but it makes one specific thing awkward: flicking to
//! swing the view, lifting, and then putting the same finger down on an
//! ability button. With a hard stop the view has to be dragged all the way and
//! the finger is still on the wrong side of the screen when the cast wants to
//! happen.
//!
//! With glide, the flick keeps the view moving while the finger travels to the
//! button — the two motions overlap instead of queueing.
//!
//! ## Three modes, because this is taste and not a default
//!
//! Some players read glide as the camera drifting when they told it to stop,
//! which is exactly as annoying as a camera that will not coast. The three
//! settings are not a quality slider — they are three different opinions about
//! whether a lift means "stop" or "let go".
//!
//! ## Why exponential decay and not a linear ramp
//!
//! A linear ramp-down ends abruptly: the velocity is still meaningful on the
//! last frame and then it is zero, which reads as a small jolt at the end of
//! every flick. Exponential decay approaches zero smoothly, and a
//! per-second half-life makes the setting frame-rate independent — the same
//! flick travels the same distance at 60fps and at 90.

/// What a lift means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Glide {
    /// The view keeps moving and eases to a stop. For flick-then-tap.
    Smooth,
    /// A short carry — enough that the flick does not end mid-gesture, gone
    /// before the finger lands anywhere else.
    #[default]
    Light,
    /// The view stops the instant the finger leaves. Today's behaviour, and
    /// the right default for anyone who aims by dragging rather than flicking.
    Rigid,
}

impl Glide {
    pub const ALL: [Glide; 3] = [Glide::Smooth, Glide::Light, Glide::Rigid];

    pub fn label(self) -> &'static str {
        match self {
            Glide::Smooth => "Smooth (glides)",
            Glide::Light => "Light",
            Glide::Rigid => "Rigid (stops dead)",
        }
    }

    /// Seconds for the carried velocity to halve.
    ///
    /// `0.0` means no carry at all. A half-life rather than a duration because
    /// it is what makes the decay frame-rate independent: the same flick
    /// travels the same distance at 60fps and at 90, which a per-frame
    /// multiplier does not give.
    pub fn half_life(self) -> f32 {
        match self {
            Glide::Smooth => 0.22,
            Glide::Light => 0.09,
            Glide::Rigid => 0.0,
        }
    }

    /// Below this speed the glide is over.
    ///
    /// Without a floor the velocity decays forever and the view creeps by
    /// fractions of a pixel indefinitely — which shows up as a camera that
    /// never quite settles, and as a `look` value that is never zero for
    /// anything downstream that tests it.
    pub fn stop_below(self) -> f32 {
        // 40, not 1. MEASURED: with a floor of 1 a Smooth flick ran for TWO
        // SECONDS, long after it had stopped being useful -- the thumb reaches
        // an ability button in roughly 0.3s, and everything after that is the
        // camera drifting on its own, which is the exact complaint the Rigid
        // mode exists to avoid.
        //
        // At 40 the same flick carries about 0.5s: past the thumb's travel,
        // and over before it turns into drift. The tail it cuts is the part
        // moving slower than a slow drag, which nobody is steering by.
        40.0
    }
}

/// The carried look velocity after a lift.
#[derive(Debug, Clone, Copy, Default)]
pub struct Coast {
    vel: [f32; 2],
}

impl Coast {
    pub fn new() -> Coast {
        Coast::default()
    }

    /// Start a glide from the velocity the finger had when it left.
    ///
    /// `Rigid` starts nothing, so every downstream check sees exactly the
    /// behaviour that exists today rather than a very short glide.
    pub fn launch(&mut self, vel: (f32, f32), mode: Glide) {
        if mode == Glide::Rigid {
            self.vel = [0.0, 0.0];
            return;
        }
        self.vel = [vel.0, vel.1];
        if self.speed() < mode.stop_below() {
            // A slow lift is a placed finger, not a flick. Carrying it would
            // make a careful aim drift at the moment of release, which is the
            // worst possible time.
            self.vel = [0.0, 0.0];
        }
    }

    /// Advance the glide and return this frame's look contribution.
    ///
    /// Returns the DISTANCE moved this frame, not the velocity, so a caller
    /// adds it to the look the same way it adds a drag delta.
    pub fn tick(&mut self, dt: f32, mode: Glide) -> (f32, f32) {
        if self.vel == [0.0, 0.0] || dt <= 0.0 {
            return (0.0, 0.0);
        }
        let hl = mode.half_life();
        if hl <= 0.0 {
            self.vel = [0.0, 0.0];
            return (0.0, 0.0);
        }
        let step = (self.vel[0] * dt, self.vel[1] * dt);
        // 0.5^(dt/half_life): the frame-rate-independent form. A fixed
        // per-frame factor would decay twice as fast at 120fps as at 60.
        let k = 0.5f32.powf(dt / hl);
        self.vel[0] *= k;
        self.vel[1] *= k;
        if self.speed() < mode.stop_below() {
            self.vel = [0.0, 0.0];
        }
        step
    }

    /// Cancel the glide — a new touch, a cast, a menu.
    ///
    /// Any new contact stops the coast. A finger arriving while the view is
    /// still moving means the player wants to take over, and continuing to
    /// glide under them fights the new input.
    pub fn stop(&mut self) {
        self.vel = [0.0, 0.0];
    }

    pub fn is_gliding(&self) -> bool {
        self.vel != [0.0, 0.0]
    }

    fn speed(&self) -> f32 {
        (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1]).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glide_distance(v: (f32, f32), mode: Glide, dt: f32) -> f32 {
        let mut c = Coast::new();
        c.launch(v, mode);
        let mut total = 0.0;
        for _ in 0..600 {
            let (dx, dy) = c.tick(dt, mode);
            total += (dx * dx + dy * dy).sqrt();
            if !c.is_gliding() {
                break;
            }
        }
        total
    }

    /// RIGID IS TODAY'S BEHAVIOUR EXACTLY. Not a very short glide -- a player
    /// who picks it is saying a lift means stop.
    #[test]
    fn rigid_does_not_move_at_all_after_a_lift() {
        let mut c = Coast::new();
        c.launch((500.0, 500.0), Glide::Rigid);
        assert!(!c.is_gliding());
        assert_eq!(c.tick(1.0 / 60.0, Glide::Rigid), (0.0, 0.0));
        assert_eq!(glide_distance((500.0, 0.0), Glide::Rigid, 1.0 / 60.0), 0.0);
    }

    /// The three modes are ordered, and Smooth actually carries far enough to
    /// be worth having -- the whole point is that the view is still moving
    /// while the finger travels to a button.
    #[test]
    fn the_modes_are_ordered_and_smooth_carries_meaningfully() {
        let dt = 1.0 / 60.0;
        let v = (600.0, 0.0);
        let (s, l, r) = (
            glide_distance(v, Glide::Smooth, dt),
            glide_distance(v, Glide::Light, dt),
            glide_distance(v, Glide::Rigid, dt),
        );
        assert!(s > l && l > r, "{s} / {l} / {r}");
        assert!(s > 100.0, "Smooth must travel a useful distance: {s}");
    }

    /// FRAME-RATE INDEPENDENCE. The same flick must travel the same distance
    /// at 60 and at 90 -- a fixed per-frame decay factor would make the view
    /// coast half as far on a faster phone, which reads as the setting not
    /// working.
    #[test]
    fn the_same_flick_travels_the_same_distance_at_any_frame_rate() {
        let v = (600.0, 0.0);
        for mode in [Glide::Smooth, Glide::Light] {
            let a = glide_distance(v, mode, 1.0 / 60.0);
            let b = glide_distance(v, mode, 1.0 / 90.0);
            let c = glide_distance(v, mode, 1.0 / 144.0);
            let spread = (a.max(b).max(c) - a.min(b).min(c)) / a;
            assert!(spread < 0.1, "{mode:?}: {a} / {b} / {c}");
        }
    }

    /// A SLOW LIFT IS A PLACED FINGER, not a flick. Carrying it would make a
    /// careful aim drift at the exact moment of release.
    #[test]
    fn a_slow_release_does_not_glide() {
        let mut c = Coast::new();
        c.launch((0.4, 0.3), Glide::Smooth);
        assert!(!c.is_gliding(), "a crawl must not coast");
        // And a deliberate slow drag-and-release, well under the 40 floor.
        let mut c2 = Coast::new();
        c2.launch((25.0, 10.0), Glide::Smooth);
        assert!(!c2.is_gliding(), "a careful aim must not drift on release");
    }

    /// The glide must END. Without a floor the velocity decays forever and the
    /// view creeps by fractions of a pixel, so `look` is never zero for
    /// anything downstream that tests it.
    #[test]
    fn the_glide_terminates() {
        let mut c = Coast::new();
        c.launch((900.0, 900.0), Glide::Smooth);
        let mut frames = 0;
        while c.is_gliding() && frames < 10_000 {
            c.tick(1.0 / 60.0, Glide::Smooth);
            frames += 1;
        }
        assert!(!c.is_gliding(), "still gliding after {frames} frames");
        assert!(frames < 240, "and it should not take 4 seconds: {frames}");
    }

    /// A NEW FINGER TAKES OVER. Continuing to glide under a fresh contact
    /// fights the input the player just gave.
    #[test]
    fn a_new_touch_cancels_the_glide() {
        let mut c = Coast::new();
        c.launch((600.0, 0.0), Glide::Smooth);
        assert!(c.is_gliding());
        c.stop();
        assert!(!c.is_gliding());
        assert_eq!(c.tick(1.0 / 60.0, Glide::Smooth), (0.0, 0.0));
    }

    /// Direction is preserved: a flick up-left glides up-left.
    #[test]
    fn the_glide_keeps_the_flicks_direction() {
        let mut c = Coast::new();
        c.launch((-300.0, 400.0), Glide::Smooth);
        let (dx, dy) = c.tick(1.0 / 60.0, Glide::Smooth);
        assert!(dx < 0.0 && dy > 0.0, "{dx},{dy}");
        // The ratio survives the decay.
        assert!((dy / dx + 400.0 / 300.0).abs() < 1e-3, "{dx},{dy}");
    }

    /// THE GLIDE MUST OUTLAST A THUMB AND NOT MUCH MORE. A thumb reaches an
    /// ability button in roughly 0.3s; everything after that is the camera
    /// drifting on its own, which is what Rigid exists to avoid.
    ///
    /// Measured with a floor of 1.0 first: Smooth ran for TWO SECONDS. The
    /// distances looked fine and the duration was the problem, which is not
    /// visible from the half-life alone.
    #[test]
    fn a_smooth_flick_lasts_about_as_long_as_reaching_a_button() {
        let dt = 1.0 / 60.0;
        for v in [300.0f32, 600.0, 1200.0] {
            let mut c = Coast::new();
            c.launch((v, 0.0), Glide::Smooth);
            let mut frames = 0;
            while c.is_gliding() && frames < 1000 {
                c.tick(dt, Glide::Smooth);
                frames += 1;
            }
            let secs = frames as f32 * dt;
            assert!(secs > 0.3, "{v}: {secs}s is gone before the thumb lands");
            assert!(secs < 1.5, "{v}: {secs}s is drifting, not gliding");
        }
    }

    /// A zero or negative dt must not move anything or divide by zero -- a
    /// paused frame is a real thing.
    #[test]
    fn a_stalled_frame_is_harmless() {
        let mut c = Coast::new();
        c.launch((600.0, 0.0), Glide::Smooth);
        assert_eq!(c.tick(0.0, Glide::Smooth), (0.0, 0.0));
        assert_eq!(c.tick(-1.0, Glide::Smooth), (0.0, 0.0));
        assert!(c.is_gliding(), "and the glide survives to the next real frame");
    }
}
