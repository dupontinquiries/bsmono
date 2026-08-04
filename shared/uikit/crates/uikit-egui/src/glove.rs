//! glove — cold-weather glove mode: a cursor you steer by jostling the phone.
//!
//! Gloves do not work on capacitive glass. In cold weather the choice is
//! normally between bare fingers and not using the phone, and the second is
//! what people actually pick. This is the third option: a ball on the screen
//! that rolls with the device's tilt, snaps to whatever is under it, and is
//! driven the rest of the way by voice.
//!
//! ## What is here and what is the host's
//!
//! The physics, the snapping, the vocabulary and the gate are here and pure.
//! Three things are seams the host fills, because each is a daemon that already
//! exists and none of them belongs in a view:
//!
//! * **Temperature** — `ambient-learner`'s `EnvSnapshot::aux_temp_c`, a real
//!   ambient reading from a BME280-style sensor rather than a CPU thermal zone.
//! * **Tilt** — and this one needs saying plainly: `iio-sensor-proxy` exposes
//!   orientation only as STRINGS ("normal", "left-up", "bottom-up"), not as a
//!   vector. A ball cannot be rolled with four labels. The raw counts live in
//!   sysfs, which is the path snapclone's `IioSensorProxySource` already polls
//!   for its motion bit, so the vector is reachable — but it is not what the
//!   D-Bus daemon hands you, and anything built on the proxy alone would move
//!   in four directions at one speed.
//! * **Speech** — the recogniser. `Vocab` maps a transcript to an action; it
//!   does not listen.
//!
//! ## Why the cursor snaps rather than clicking where it is
//!
//! A tilt-driven ball is not precise, and it is least precise exactly when it
//! is needed — a gloved hand on a cold morning is shaking. Requiring the ball
//! to be *inside* a target would make small controls unreachable. So the
//! nearest target within a generous radius wins, the ball visibly latches to
//! it, and "select" acts on the latched target rather than on the ball's
//! position. The user aims roughly and the screen does the last inch.
//!
//! ## The keyboard deliberately does not open
//!
//! Snapping to a text field arms VOICE input, not the on-screen keyboard.
//! rusty-board raising itself here would cover the screen with keys that cannot
//! be pressed by the hand that needed this mode — which is the whole problem
//! restated. `FieldArm::Voice` is what the host forwards, and rusty-board's
//! cue is its absence: no key event is ever synthesised.

/// Below this ambient temperature, glove mode is offered.
///
/// 8°C is roughly where gloves come out. It is a default rather than a truth —
/// people differ, and the setting is the user's.
pub const COLD_C: f32 = 8.0;

/// How far from the ball a target may be and still capture it.
///
/// Generous on purpose: a gloved hand on a cold morning is shaking, and a
/// strict radius makes small controls unreachable exactly when precision is
/// worst.
pub const SNAP_RADIUS_PX: f32 = 72.0;

/// Tilt below this is the phone resting, not steering. Without it the ball
/// creeps across the screen whenever the device is not perfectly level, which
/// it never is.
pub const TILT_DEADZONE: f32 = 0.06;

/// Acceleration per unit of tilt, px/s².
pub const TILT_GAIN: f32 = 2_400.0;

/// Velocity retained per second — rolling friction. Without it the ball
/// oscillates around a target forever and never settles enough to snap.
pub const FRICTION_PER_SEC: f32 = 0.12;

/// Speed below which the ball is considered parked, px/s.
pub const PARK_SPEED: f32 = 8.0;

/// What kind of thing the cursor can land on. The action vocabulary differs per
/// kind, because "louder" means nothing to a checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Button,
    /// A text field. Snapping here arms voice, never the keyboard.
    Field,
    /// A row in a list.
    ListItem,
    Slider,
    Toggle,
}

/// Something the cursor can snap to.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub kind: TargetKind,
    /// Centre, in screen coordinates.
    pub at: (f32, f32),
    /// Spoken label, for "select the volume slider" and for the readout.
    pub label: String,
}

/// What the mode asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum GloveAction {
    Activate(String),
    /// Arm voice entry for a field. NOT a request to raise a keyboard.
    ArmField(String),
    Toggle(String),
    /// Nudge a slider by a fraction of its range, positive or negative.
    Nudge { id: String, delta: f32 },
    /// Move the snap to the next/previous target — for lists, where rolling to
    /// each row is slower than saying "next".
    Step(i32),
    Dismiss,
}

/// Maps a transcript to an action against the current target.
///
/// Deliberately small and literal. A big grammar fails in the cold, where the
/// user is speaking through a scarf into a phone at arm's length, and a
/// recogniser that needs a clean sentence is a recogniser that needs bare
/// hands.
pub struct Vocab;

impl Vocab {
    /// Words that act on whatever is snapped, whatever its kind.
    pub fn universal(phrase: &str, target: &Target) -> Option<GloveAction> {
        match phrase {
            "select" | "tap" | "press" | "go" | "okay" | "ok" =>
                Some(match target.kind {
                    // Selecting a FIELD arms voice; it does not "press" it.
                    TargetKind::Field => GloveAction::ArmField(target.id.clone()),
                    TargetKind::Toggle => GloveAction::Toggle(target.id.clone()),
                    _ => GloveAction::Activate(target.id.clone()),
                }),
            "next" | "down" => Some(GloveAction::Step(1)),
            "back" | "previous" | "up" => Some(GloveAction::Step(-1)),
            "cancel" | "dismiss" | "stop" | "never mind" => Some(GloveAction::Dismiss),
            _ => None,
        }
    }

    /// Words that only mean something for a particular kind.
    pub fn resolve(phrase: &str, target: &Target) -> Option<GloveAction> {
        let p = phrase.trim().to_lowercase();
        if let Some(a) = Self::universal(&p, target) { return Some(a); }
        match target.kind {
            TargetKind::Slider => match p.as_str() {
                // Coarse and fine, because "a bit" is how people talk about
                // volume and "set it to 47" is not.
                "up" | "louder" | "more" | "higher" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 0.10 }),
                "down" | "quieter" | "less" | "lower" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -0.10 }),
                "a bit more" | "nudge up" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 0.02 }),
                "a bit less" | "nudge down" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -0.02 }),
                "max" | "maximum" | "all the way up" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 1.0 }),
                "off" | "mute" | "all the way down" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -1.0 }),
                _ => None,
            },
            TargetKind::Toggle => match p.as_str() {
                "on" | "enable" | "turn on" | "off" | "disable" | "turn off" =>
                    Some(GloveAction::Toggle(target.id.clone())),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The rolling cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct GloveCursor {
    pub enabled: bool,
    /// Last ambient temperature the host reported.
    pub ambient_c: Option<f32>,
    /// Threshold, from settings.
    pub cold_c: f32,
    pos: (f32, f32),
    vel: (f32, f32),
    snapped: Option<String>,
    bounds: (f32, f32),
}

impl Default for GloveCursor {
    fn default() -> Self {
        GloveCursor {
            enabled: false, ambient_c: None, cold_c: COLD_C,
            pos: (0.0, 0.0), vel: (0.0, 0.0), snapped: None, bounds: (0.0, 0.0),
        }
    }
}

impl GloveCursor {
    /// Is it cold enough to offer this?
    ///
    /// Unknown temperature is NOT cold. A phone with no ambient sensor would
    /// otherwise show a rolling ball indoors in July, and an accessibility mode
    /// that appears uninvited is one people switch off permanently.
    pub fn is_cold(&self) -> bool {
        self.ambient_c.map(|c| c <= self.cold_c).unwrap_or(false)
    }

    /// Both conditions: the user asked for it AND the world agrees.
    pub fn active(&self) -> bool { self.enabled && self.is_cold() }

    pub fn set_bounds(&mut self, w: f32, h: f32) {
        self.bounds = (w, h);
        if self.pos == (0.0, 0.0) { self.pos = (w * 0.5, h * 0.5); }
    }

    pub fn pos(&self) -> (f32, f32) { self.pos }
    pub fn velocity(&self) -> (f32, f32) { self.vel }
    pub fn snapped(&self) -> Option<&str> { self.snapped.as_deref() }
    pub fn parked(&self) -> bool { self.vel.0.hypot(self.vel.1) < PARK_SPEED }

    /// Roll the ball. `tilt` is a unit-ish gravity vector from the host's
    /// accelerometer — see the module docs on why the D-Bus proxy cannot supply
    /// this.
    ///
    /// Returns the id the cursor is snapped to after moving, if any.
    pub fn tick(&mut self, tilt: (f32, f32), dt_s: f32, targets: &[Target]) -> Option<String> {
        if !self.active() { return None; }

        // Deadzone, or the ball creeps whenever the phone is not perfectly
        // level — which it never is, least of all in a gloved hand.
        let (tx, ty) = tilt;
        let mag = tx.hypot(ty);
        let (ax, ay) = if mag < TILT_DEADZONE { (0.0, 0.0) }
                       else { (tx * TILT_GAIN, ty * TILT_GAIN) };

        self.vel.0 += ax * dt_s;
        self.vel.1 += ay * dt_s;
        // Exponential friction, framerate-independent: a fixed per-frame
        // multiplier would make the ball heavier on a slow device, which is the
        // opposite of helpful.
        let keep = FRICTION_PER_SEC.powf(dt_s);
        self.vel.0 *= keep;
        self.vel.1 *= keep;

        self.pos.0 += self.vel.0 * dt_s;
        self.pos.1 += self.vel.1 * dt_s;

        // Walls absorb rather than bounce. A ball that bounces off the edge
        // travels back across the screen and you have to chase it.
        if self.pos.0 < 0.0 { self.pos.0 = 0.0; self.vel.0 = 0.0; }
        if self.pos.1 < 0.0 { self.pos.1 = 0.0; self.vel.1 = 0.0; }
        if self.pos.0 > self.bounds.0 { self.pos.0 = self.bounds.0; self.vel.0 = 0.0; }
        if self.pos.1 > self.bounds.1 { self.pos.1 = self.bounds.1; self.vel.1 = 0.0; }

        self.snapped = self.nearest(targets).map(|t| t.id.clone());
        self.snapped.clone()
    }

    /// The nearest target within the snap radius.
    pub fn nearest<'a>(&self, targets: &'a [Target]) -> Option<&'a Target> {
        targets.iter()
            .map(|t| (t, (t.at.0 - self.pos.0).hypot(t.at.1 - self.pos.1)))
            .filter(|(_, d)| *d <= SNAP_RADIUS_PX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(t, _)| t)
    }

    /// Move the snap by hand — "next" and "back", which beat rolling to each
    /// row of a list.
    pub fn step(&mut self, targets: &[Target], delta: i32) -> Option<String> {
        if targets.is_empty() { return None; }
        let cur = self.snapped.as_ref()
            .and_then(|id| targets.iter().position(|t| &t.id == id));
        let n = targets.len() as i32;
        let next = match cur {
            Some(i) => (i as i32 + delta).rem_euclid(n),
            // With nothing snapped, "next" means the first — not a no-op the
            // user has to guess at.
            None => if delta >= 0 { 0 } else { n - 1 },
        } as usize;
        // Stepping moves the BALL too, so the cursor stays where the attention
        // is rather than leaving a latched highlight somewhere off-ball.
        self.pos = targets[next].at;
        self.vel = (0.0, 0.0);
        self.snapped = Some(targets[next].id.clone());
        self.snapped.clone()
    }

    /// Resolve a spoken phrase against whatever is snapped.
    pub fn speak(&mut self, phrase: &str, targets: &[Target]) -> Option<GloveAction> {
        let target = targets.iter().find(|t| Some(t.id.as_str()) == self.snapped())?;
        let action = Vocab::resolve(phrase, target)?;
        if let GloveAction::Step(d) = action {
            self.step(targets, d);
        }
        Some(action)
    }

    /// What the readout says, so the user can hear-check the snap before
    /// speaking. Without it, "select" on a shaking screen is a guess.
    pub fn readout(&self, targets: &[Target]) -> String {
        match targets.iter().find(|t| Some(t.id.as_str()) == self.snapped()) {
            Some(t) => t.label.clone(),
            None => "nothing selected".into(),
        }
    }
}

/// Draw the ball and its snap ring.
pub fn glove_ui(ui: &mut egui::Ui, g: &GloveCursor, targets: &[Target],
                c: &crate::PaletteColors) {
    if !g.active() { return; }
    let p = ui.painter();
    // The snap ring is drawn on the TARGET, not the ball: the user needs to
    // know what will be acted on, and the ball is the thing they are already
    // watching.
    if let Some(t) = targets.iter().find(|t| Some(t.id.as_str()) == g.snapped()) {
        p.circle_stroke(egui::pos2(t.at.0, t.at.1), 26.0,
                        egui::Stroke::new(2.0, c.accent));
    }
    let at = egui::pos2(g.pos().0, g.pos().1);
    p.circle_filled(at, 14.0, c.accent);
    p.circle_stroke(at, 14.0, egui::Stroke::new(2.0, c.bg_base));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, kind: TargetKind, x: f32, y: f32) -> Target {
        Target { id: id.into(), kind, at: (x, y), label: id.into() }
    }

    fn cold_cursor() -> GloveCursor {
        let mut g = GloveCursor { enabled: true, ambient_c: Some(2.0), ..Default::default() };
        g.set_bounds(400.0, 800.0);
        g
    }

    #[test]
    fn an_unknown_temperature_is_not_cold() {
        // A phone with no ambient sensor would otherwise roll a ball across the
        // screen indoors in July, and a mode that appears uninvited is one
        // people switch off permanently.
        let mut g = GloveCursor { enabled: true, ..Default::default() };
        assert!(!g.is_cold());
        assert!(!g.active());
        g.ambient_c = Some(20.0);
        assert!(!g.active(), "and a warm reading is not cold either");
    }

    #[test]
    fn both_the_user_and_the_world_have_to_agree() {
        let mut g = GloveCursor { enabled: false, ambient_c: Some(-5.0), ..Default::default() };
        assert!(g.is_cold());
        assert!(!g.active(), "cold alone does not switch it on");
        g.enabled = true;
        assert!(g.active());
    }

    #[test]
    fn an_inactive_cursor_does_not_move() {
        let mut g = GloveCursor { enabled: false, ambient_c: Some(0.0), ..Default::default() };
        g.set_bounds(400.0, 800.0);
        let before = g.pos();
        g.tick((1.0, 1.0), 0.5, &[]);
        assert_eq!(g.pos(), before);
    }

    #[test]
    fn tilt_rolls_the_ball_in_the_direction_of_the_tilt() {
        let mut g = cold_cursor();
        let before = g.pos();
        g.tick((0.5, 0.0), 0.1, &[]);
        assert!(g.pos().0 > before.0, "rolled right");
        assert!((g.pos().1 - before.1).abs() < 0.01, "and not vertically");
    }

    #[test]
    fn a_resting_phone_does_not_creep() {
        // The deadzone. A phone is never perfectly level, least of all in a
        // gloved hand, and a ball that drifts is a ball you fight.
        let mut g = cold_cursor();
        let before = g.pos();
        for _ in 0..100 { g.tick((TILT_DEADZONE * 0.5, TILT_DEADZONE * 0.5), 0.016, &[]); }
        assert_eq!(g.pos(), before);
    }

    #[test]
    fn friction_brings_the_ball_to_rest() {
        // Without it the ball oscillates around a target forever and never
        // settles enough to snap.
        let mut g = cold_cursor();
        g.tick((1.0, 0.0), 0.1, &[]);
        assert!(!g.parked());
        for _ in 0..200 { g.tick((0.0, 0.0), 0.016, &[]); }
        assert!(g.parked(), "still moving at {:?}", g.velocity());
    }

    #[test]
    fn friction_is_framerate_independent() {
        // A fixed per-frame multiplier would make the ball heavier on a slow
        // device, which is the opposite of helpful.
        let mut fast = cold_cursor();
        let mut slow = cold_cursor();
        fast.tick((1.0, 0.0), 0.1, &[]);
        slow.tick((1.0, 0.0), 0.1, &[]);
        for _ in 0..60 { fast.tick((0.0, 0.0), 1.0 / 60.0, &[]); }
        for _ in 0..15 { slow.tick((0.0, 0.0), 1.0 / 15.0, &[]); }
        let (fv, sv) = (fast.velocity().0, slow.velocity().0);
        assert!((fv - sv).abs() < fv.abs().max(1.0) * 0.2, "{fv} vs {sv}");
    }

    #[test]
    fn walls_absorb_rather_than_bounce() {
        // A ball that bounces off the edge travels back across the screen and
        // you have to chase it.
        let mut g = cold_cursor();
        for _ in 0..200 { g.tick((1.0, 0.0), 0.016, &[]); }
        assert_eq!(g.pos().0, 400.0);
        assert_eq!(g.velocity().0, 0.0, "the wall took the energy");
    }

    #[test]
    fn the_nearest_target_within_the_radius_wins() {
        let mut g = cold_cursor();
        let ts = vec![target("far", TargetKind::Button, 200.0, 500.0),
                      target("near", TargetKind::Button, 200.0, 410.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.snapped(), Some("near"));
    }

    #[test]
    fn nothing_outside_the_radius_captures_the_ball() {
        let mut g = cold_cursor();
        let ts = vec![target("miles", TargetKind::Button, 20.0, 20.0)];
        assert_eq!(g.tick((0.0, 0.0), 0.016, &ts), None);
        assert_eq!(g.readout(&ts), "nothing selected");
    }

    #[test]
    fn the_snap_radius_is_generous_because_a_cold_hand_shakes() {
        // A strict radius makes small controls unreachable exactly when
        // precision is worst.
        assert!(SNAP_RADIUS_PX >= 60.0);
    }

    #[test]
    fn selecting_a_field_arms_voice_and_never_a_keyboard() {
        // rusty-board raising itself here would cover the screen with keys that
        // cannot be pressed by the hand that needed this mode.
        let mut g = cold_cursor();
        let ts = vec![target("name", TargetKind::Field, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("select", &ts), Some(GloveAction::ArmField("name".into())));
        // Nothing in the action vocabulary can ask for a keyboard.
        let dbg = format!("{:?}", g.speak("press", &ts));
        assert!(!dbg.to_lowercase().contains("keyboard"));
    }

    #[test]
    fn selecting_a_button_activates_it() {
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("go", &ts), Some(GloveAction::Activate("send".into())));
    }

    #[test]
    fn a_toggle_flips_on_select_and_on_its_own_words() {
        let mut g = cold_cursor();
        let ts = vec![target("wifi", TargetKind::Toggle, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("select", &ts), Some(GloveAction::Toggle("wifi".into())));
        assert_eq!(g.speak("turn off", &ts), Some(GloveAction::Toggle("wifi".into())));
    }

    #[test]
    fn a_slider_takes_coarse_and_fine_nudges_because_that_is_how_people_talk() {
        // "a bit more" is a real thing to say about volume; "set it to 47" is
        // not.
        let mut g = cold_cursor();
        let ts = vec![target("vol", TargetKind::Slider, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("louder", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: 0.10 }));
        assert_eq!(g.speak("a bit less", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: -0.02 }));
        assert_eq!(g.speak("mute", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: -1.0 }));
    }

    #[test]
    fn slider_words_mean_nothing_to_a_button() {
        // "louder" against a Send button should fail rather than doing
        // something arbitrary.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("louder", &ts), None);
    }

    #[test]
    fn stepping_beats_rolling_to_each_row_of_a_list() {
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 200.0, 400.0),
                      target("b", TargetKind::ListItem, 200.0, 460.0),
                      target("c", TargetKind::ListItem, 200.0, 520.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.snapped(), Some("a"));
        g.speak("next", &ts);
        assert_eq!(g.snapped(), Some("b"));
        g.speak("back", &ts);
        assert_eq!(g.snapped(), Some("a"));
    }

    #[test]
    fn stepping_moves_the_ball_to_the_new_target() {
        // Otherwise the highlight is somewhere the ball is not, and the next
        // roll snaps back to where the ball actually sits.
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 200.0, 400.0),
                      target("b", TargetKind::ListItem, 200.0, 460.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        g.speak("next", &ts);
        assert_eq!(g.pos(), (200.0, 460.0));
        assert_eq!(g.velocity(), (0.0, 0.0), "and it arrives stopped");
    }

    #[test]
    fn stepping_wraps_and_starts_sensibly_from_nothing() {
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 10.0, 10.0),
                      target("b", TargetKind::ListItem, 10.0, 70.0)];
        // Nothing snapped: "next" means the first, not a no-op to guess at.
        assert_eq!(g.step(&ts, 1).as_deref(), Some("a"));
        g.step(&ts, 1);
        assert_eq!(g.step(&ts, 1).as_deref(), Some("a"), "wrapped");
    }

    #[test]
    fn speaking_with_nothing_snapped_does_nothing() {
        let mut g = cold_cursor();
        assert_eq!(g.speak("select", &[]), None);
    }

    #[test]
    fn an_unknown_phrase_is_refused_rather_than_guessed() {
        // Acting on a misheard word is worse than doing nothing: the user is
        // wearing gloves and cannot easily undo.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.speak("banana", &ts), None);
    }

    #[test]
    fn the_vocabulary_is_case_and_whitespace_forgiving() {
        // Someone is speaking through a scarf; the transcript will be messy.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert!(g.speak("  SELECT ", &ts).is_some());
    }

    #[test]
    fn the_readout_names_what_will_be_acted_on() {
        // Without it, "select" on a shaking screen is a guess.
        let mut g = cold_cursor();
        let mut t = target("vol", TargetKind::Slider, 200.0, 400.0);
        t.label = "Volume".into();
        let ts = vec![t];
        g.tick((0.0, 0.0), 0.016, &ts);
        assert_eq!(g.readout(&ts), "Volume");
    }

    #[test]
    fn cancel_works_from_any_kind() {
        let mut g = cold_cursor();
        for kind in [TargetKind::Button, TargetKind::Field, TargetKind::Slider,
                     TargetKind::Toggle, TargetKind::ListItem] {
            let ts = vec![target("x", kind, 200.0, 400.0)];
            g.tick((0.0, 0.0), 0.016, &ts);
            assert_eq!(g.speak("cancel", &ts), Some(GloveAction::Dismiss), "{kind:?}");
        }
    }
}
