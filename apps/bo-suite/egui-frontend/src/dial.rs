//! dial — `VolumeDial`, the hero control: a wheel that behaves like a physical
//! one.
//!
//! Four things make it feel real, and each is a rule rather than a decoration:
//!
//! * **Inertia.** Flick the ring and it keeps turning, decelerating under
//!   friction, instead of stopping dead. A weighted aluminium wheel does not
//!   stop when your finger leaves it.
//! * **Detents.** A pulse as the value crosses each 10% mark — the visual
//!   analogue of the tactile clicks the real hardware has. They are *feedback*,
//!   not snapping: the value passes through them freely and 43% stays 43%.
//! * **The rail.** Hitting 0 or 100 kills the spin outright. A wheel that keeps
//!   spinning against its own end stop is a wheel that is broken.
//! * **Two ways to grab it.** Around the ring sets by angle; vertically near
//!   the centre is a fine adjust, up for louder. Angle near the middle is
//!   meaningless — a few pixels of movement sweep a huge arc — so the centre
//!   has to mean something else or it means nothing.
//!
//! ## The geometry
//!
//! The arc starts at 135° and sweeps 270°, which leaves the bottom quarter open
//! — the gap is where the wheel is *not*, and it is what tells you at a glance
//! which way is up without reading a number.
//!
//! ## Who commits
//!
//! `value_changed` fires continuously during interaction and the PARENT decides
//! when to push to the device, debounced. A dial that wrote to the network on
//! every frame of a flick would send a hundred volume commands for one gesture.

/// Where the arc begins, in degrees clockwise from 3 o'clock.
pub const START_ANGLE: f32 = 135.0;
/// How far it sweeps.
pub const SWEEP_ANGLE: f32 = 270.0;
/// Per-tick velocity decay. Below 1 or it accelerates; too close to 1 and the
/// wheel never settles.
pub const FRICTION: f32 = 0.92;
/// Spin below this is indistinguishable from stopped, and continuing to
/// integrate it keeps a timer alive forever for no visible motion.
pub const MIN_SPIN: f32 = 0.05;
/// The inertia tick, matching the Qt timer.
pub const TICK_MS: f32 = 16.0;
/// Detents every 10%.
pub const DETENT_STEP: f32 = 10.0;
/// Inside this fraction of the radius, a drag is a fine adjust rather than an
/// angle.
pub const FINE_ZONE: f32 = 0.45;
/// Pixels of vertical travel per percent, in the fine zone.
pub const FINE_PX_PER_PERCENT: f32 = 2.0;

/// How a drag is being interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// Around the ring: value follows the angle.
    Ring,
    /// Near the centre: value follows vertical travel.
    Fine,
}

/// The dial.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeDial {
    pub value: f32,
    /// Degrees per tick, for the inertial flick.
    spin: f32,
    grab: Option<Grab>,
    last_angle: f32,
    last_y: f32,
    /// The 10% bucket last crossed, so a detent fires once per crossing.
    last_detent: i32,
}

impl Default for VolumeDial {
    fn default() -> Self {
        VolumeDial { value: 0.0, spin: 0.0, grab: None,
                     last_angle: 0.0, last_y: 0.0, last_detent: 0 }
    }
}

/// Map a value to its angle on the arc.
pub fn value_to_angle(v: f32) -> f32 {
    START_ANGLE + SWEEP_ANGLE * (v.clamp(0.0, 100.0) / 100.0)
}

/// Map an angle back to a value, clamped to the arc.
///
/// Angles in the open bottom quarter have no value — they are off the end of
/// the wheel — so they clamp to the nearer rail rather than wrapping around to
/// the opposite extreme, which would make a drag past the bottom jump the
/// volume from 100 to 0.
pub fn angle_to_value(angle_deg: f32) -> f32 {
    let mut a = (angle_deg - START_ANGLE).rem_euclid(360.0);
    if a > SWEEP_ANGLE {
        // In the dead zone: snap to whichever end is closer.
        a = if a - SWEEP_ANGLE < (360.0 - a) { SWEEP_ANGLE } else { 0.0 };
    }
    (a / SWEEP_ANGLE * 100.0).clamp(0.0, 100.0)
}

impl VolumeDial {
    pub fn new(value: f32) -> Self {
        VolumeDial { value: value.clamp(0.0, 100.0),
                     last_detent: (value / DETENT_STEP).round() as i32,
                     ..Default::default() }
    }

    /// Decide what kind of grab this is, from the distance to the centre.
    ///
    /// Angle near the middle is meaningless — a few pixels sweep a huge arc —
    /// so the centre has to mean something else or it means nothing.
    pub fn grab_kind(dist_from_centre: f32, radius: f32) -> Grab {
        if radius > 0.0 && dist_from_centre < radius * FINE_ZONE { Grab::Fine } else { Grab::Ring }
    }

    pub fn press(&mut self, angle_deg: f32, y: f32, dist_from_centre: f32, radius: f32) {
        self.grab = Some(Self::grab_kind(dist_from_centre, radius));
        self.last_angle = angle_deg;
        self.last_y = y;
        // A press stops a spin. Grabbing a moving wheel stops it; that is what
        // grabbing a wheel does.
        self.spin = 0.0;
    }

    /// Drag. Returns the detent that was crossed, if any.
    pub fn drag(&mut self, angle_deg: f32, y: f32) -> Option<i32> {
        let Some(grab) = self.grab else { return None };
        let before = self.value;
        match grab {
            Grab::Ring => {
                // Shortest-way-round, so a drag across the 0/360 seam does not
                // read as a full turn backwards.
                let mut d = angle_deg - self.last_angle;
                if d > 180.0 { d -= 360.0; }
                if d < -180.0 { d += 360.0; }
                self.spin = d;
                self.value = (self.value + d / SWEEP_ANGLE * 100.0).clamp(0.0, 100.0);
            }
            Grab::Fine => {
                // Up is louder: screen y grows downward, so the delta inverts.
                let dy = self.last_y - y;
                self.value = (self.value + dy / FINE_PX_PER_PERCENT).clamp(0.0, 100.0);
                self.spin = 0.0;
            }
        }
        self.last_angle = angle_deg;
        self.last_y = y;
        if self.value != before { self.check_detent() } else { None }
    }

    /// Release. The ring keeps its spin; a fine adjust does not fling.
    pub fn release(&mut self) {
        if self.grab == Some(Grab::Fine) { self.spin = 0.0; }
        self.grab = None;
    }

    pub fn dragging(&self) -> bool { self.grab.is_some() }
    pub fn spinning(&self) -> bool { self.spin.abs() >= MIN_SPIN }

    /// One inertia tick. Returns the detent crossed, if any.
    pub fn tick(&mut self) -> Option<i32> {
        if self.grab.is_some() || !self.spinning() {
            if !self.spinning() { self.spin = 0.0; }
            return None;
        }
        let before = self.value;
        self.value = (self.value + self.spin / SWEEP_ANGLE * 100.0).clamp(0.0, 100.0);
        self.spin *= FRICTION;
        // The rail: a wheel that keeps spinning against its own end stop is a
        // wheel that is broken.
        if self.value <= 0.0 || self.value >= 100.0 { self.spin = 0.0; }
        if self.value != before { self.check_detent() } else { None }
    }

    /// Set from outside — the device reported a new volume. Kills any spin,
    /// because the wheel is no longer describing what is happening.
    pub fn set_external(&mut self, v: f32) {
        self.value = v.clamp(0.0, 100.0);
        self.spin = 0.0;
        self.last_detent = (self.value / DETENT_STEP).round() as i32;
    }

    /// Did we just cross a 10% mark?
    ///
    /// A detent is FEEDBACK, not snapping: the value passes through freely and
    /// 43% stays 43%. Snapping to tens would make a dial that cannot express
    /// most of its own range.
    fn check_detent(&mut self) -> Option<i32> {
        let bucket = (self.value / DETENT_STEP).round() as i32;
        if bucket != self.last_detent {
            self.last_detent = bucket;
            return Some(bucket);
        }
        None
    }

    pub fn angle(&self) -> f32 { value_to_angle(self.value) }

    /// The idle breathing glow, 0..=1 — only at rest, so the "alive" signal is
    /// not competing with actual motion.
    pub fn idle_glow(&self, phase: f32) -> f32 {
        if self.dragging() || self.spinning() { return 0.0; }
        0.5 + 0.5 * phase.sin()
    }
}

/// Draw the dial. Returns the value whenever it changes, for the parent to
/// debounce and push.
pub fn dial_ui(ui: &mut egui::Ui, d: &mut VolumeDial, size: f32,
               c: &uikit_egui::PaletteColors) -> Option<f32> {
    let before = d.value;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let centre = rect.center();
    let radius = size * 0.5 - 6.0;

    let angle_of = |p: egui::Pos2| -> f32 {
        (p.y - centre.y).atan2(p.x - centre.x).to_degrees()
    };
    if let Some(p) = resp.interact_pointer_pos() {
        let dist = (p - centre).length();
        if resp.drag_started() { d.press(angle_of(p), p.y, dist, radius); }
        else if resp.dragged() { d.drag(angle_of(p), p.y); }
    }
    if resp.drag_stopped() { d.release(); }
    d.tick();

    let p = ui.painter();
    p.circle_stroke(centre, radius, egui::Stroke::new(2.0, c.border));
    // Tick marks every 10%, which is where the detents are — the marks and the
    // feedback have to agree or the clicks land between the lines.
    for i in 0..=10 {
        let a = (START_ANGLE + SWEEP_ANGLE * i as f32 / 10.0).to_radians();
        let dir = egui::vec2(a.cos(), a.sin());
        p.line_segment([centre + dir * (radius - 4.0), centre + dir * radius],
                       egui::Stroke::new(1.0, c.text_weak));
    }
    // The filled arc, drawn as a run of short segments.
    let steps = ((d.value / 100.0) * 48.0) as usize;
    for i in 0..steps {
        let a0 = (START_ANGLE + SWEEP_ANGLE * i as f32 / 48.0).to_radians();
        let a1 = (START_ANGLE + SWEEP_ANGLE * (i + 1) as f32 / 48.0).to_radians();
        p.line_segment([centre + egui::vec2(a0.cos(), a0.sin()) * radius,
                        centre + egui::vec2(a1.cos(), a1.sin()) * radius],
                       egui::Stroke::new(3.0, c.accent));
    }
    p.text(centre, egui::Align2::CENTER_CENTER, format!("{:.0}", d.value),
           egui::FontId::proportional(size * 0.22), c.text);

    if d.value != before { Some(d.value) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_leaves_the_bottom_quarter_open() {
        // The gap is where the wheel is NOT, and it is what tells you which way
        // is up without reading a number.
        assert_eq!(SWEEP_ANGLE, 270.0);
        assert_eq!(value_to_angle(0.0), 135.0);
        assert_eq!(value_to_angle(100.0), 405.0);
        assert_eq!(value_to_angle(50.0), 270.0);
    }

    #[test]
    fn angle_and_value_round_trip() {
        for v in [0.0, 12.5, 37.0, 50.0, 99.0, 100.0] {
            let back = angle_to_value(value_to_angle(v));
            assert!((back - v).abs() < 0.01, "{v} -> {back}");
        }
    }

    #[test]
    fn an_angle_in_the_dead_zone_clamps_to_the_nearer_rail() {
        // Wrapping would make a drag past the bottom jump the volume from 100
        // to 0 — the single worst thing a volume control can do.
        let just_past_max = angle_to_value(START_ANGLE + SWEEP_ANGLE + 10.0);
        assert_eq!(just_past_max, 100.0);
        let just_before_min = angle_to_value(START_ANGLE - 10.0);
        assert_eq!(just_before_min, 0.0);
    }

    #[test]
    fn a_grab_near_the_centre_is_a_fine_adjust() {
        // Angle near the middle is meaningless: a few pixels sweep a huge arc.
        assert_eq!(VolumeDial::grab_kind(5.0, 100.0), Grab::Fine);
        assert_eq!(VolumeDial::grab_kind(80.0, 100.0), Grab::Ring);
        assert_eq!(VolumeDial::grab_kind(100.0 * FINE_ZONE, 100.0), Grab::Ring, "the boundary");
    }

    #[test]
    fn fine_adjust_treats_up_as_louder() {
        // Screen y grows downward, so the delta has to invert. Getting this
        // backwards makes the dial fight the user on the one gesture that is
        // supposed to be precise.
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        d.drag(0.0, 180.0);          // moved UP 20px
        assert!(d.value > 50.0, "up should be louder, got {}", d.value);
        d.drag(0.0, 220.0);          // back down past the start
        assert!(d.value < 50.0);
    }

    #[test]
    fn a_ring_drag_across_the_seam_is_not_read_as_a_full_turn_backwards() {
        // 359 -> 1 degrees is two degrees forward, not 358 back.
        let mut d = VolumeDial::new(50.0);
        d.press(359.0, 0.0, 90.0, 100.0);
        d.drag(1.0, 0.0);
        assert!(d.value > 50.0 && d.value < 51.0, "got {}", d.value);
    }

    #[test]
    fn a_flick_keeps_turning_after_release_and_decelerates() {
        // A weighted wheel does not stop when your finger leaves it.
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 0.0, 90.0, 100.0);
        d.drag(10.0, 0.0);
        d.release();
        assert!(d.spinning());
        let mut last_step = f32::INFINITY;
        let mut v = d.value;
        // A 10 deg/tick flick decays below the stop threshold in about 64 ticks
        // — roughly a second of coast at 16ms, which is what a real weighted
        // wheel does. An earlier version of this test allowed 20 and failed;
        // the wheel was right and the test was impatient.
        let mut ticks = 0;
        while d.spinning() && ticks < 500 {
            d.tick();
            ticks += 1;
            let step = d.value - v;
            if step > 0.0 { assert!(step <= last_step + 0.001, "should decelerate"); last_step = step; }
            v = d.value;
        }
        assert!(!d.spinning(), "and eventually settles");
        assert!((40..=120).contains(&ticks), "coasted for {ticks} ticks");
    }

    #[test]
    fn a_spin_stops_dead_at_the_rail() {
        // A wheel that keeps spinning against its own end stop is broken.
        let mut d = VolumeDial::new(95.0);
        d.press(0.0, 0.0, 90.0, 100.0);
        d.drag(60.0, 0.0);
        d.release();
        for _ in 0..50 { d.tick(); }
        assert_eq!(d.value, 100.0);
        assert!(!d.spinning());
    }

    #[test]
    fn a_fine_adjust_does_not_fling() {
        // Precision gestures should not have momentum; you let go where you
        // meant to stop.
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        d.drag(0.0, 100.0);
        d.release();
        assert!(!d.spinning());
    }

    #[test]
    fn grabbing_a_spinning_wheel_stops_it() {
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 0.0, 90.0, 100.0);
        d.drag(10.0, 0.0);
        d.release();
        assert!(d.spinning());
        d.press(0.0, 0.0, 90.0, 100.0);
        assert!(!d.spinning(), "that is what grabbing a wheel does");
    }

    #[test]
    fn a_detent_fires_once_per_ten_percent_crossed() {
        let mut d = VolumeDial::new(0.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        let mut detents = 0;
        // Drag up through the whole range in small steps.
        for i in 1..=200 {
            if d.drag(0.0, 200.0 - i as f32).is_some() { detents += 1; }
        }
        assert_eq!(d.value, 100.0);
        assert_eq!(detents, 10, "one per 10% mark, got {detents}");
    }

    #[test]
    fn a_detent_is_feedback_not_snapping() {
        // Snapping to tens would make a dial that cannot express most of its
        // own range.
        let mut d = VolumeDial::new(40.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        d.drag(0.0, 194.0);   // +3%
        assert!((d.value - 43.0).abs() < 0.01, "43% stays 43%, got {}", d.value);
    }

    #[test]
    fn holding_still_inside_a_bucket_does_not_re_fire_a_detent() {
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        assert!(d.drag(0.0, 199.0).is_none(), "half a percent crosses nothing");
        assert!(d.drag(0.0, 198.0).is_none());
    }

    #[test]
    fn an_external_change_kills_the_spin() {
        // The wheel is no longer describing what is happening; continuing to
        // integrate would fight the device that just told us the truth.
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 0.0, 90.0, 100.0);
        d.drag(20.0, 0.0);
        d.release();
        assert!(d.spinning());
        d.set_external(20.0);
        assert_eq!(d.value, 20.0);
        assert!(!d.spinning());
    }

    #[test]
    fn an_external_change_does_not_fire_a_detent_afterwards() {
        // Otherwise arriving at 30 from the device clicks the moment the user
        // next nudges it, as if they had crossed a mark they never touched.
        let mut d = VolumeDial::new(0.0);
        d.set_external(30.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        assert!(d.drag(0.0, 199.5).is_none());
    }

    #[test]
    fn the_value_never_leaves_its_range() {
        let mut d = VolumeDial::new(50.0);
        d.press(0.0, 200.0, 1.0, 100.0);
        d.drag(0.0, -10_000.0);
        assert_eq!(d.value, 100.0);
        d.drag(0.0, 10_000.0);
        assert_eq!(d.value, 0.0);
    }

    #[test]
    fn the_idle_glow_only_breathes_at_rest() {
        // An "alive" signal competing with actual motion is noise.
        let mut d = VolumeDial::new(50.0);
        assert!(d.idle_glow(1.0) > 0.0);
        d.press(0.0, 0.0, 90.0, 100.0);
        assert_eq!(d.idle_glow(1.0), 0.0, "not while dragging");
        d.drag(10.0, 0.0);
        d.release();
        assert_eq!(d.idle_glow(1.0), 0.0, "nor while spinning");
    }

    #[test]
    fn friction_is_between_zero_and_one() {
        // Above 1 the wheel accelerates forever; at 1 it never stops.
        assert!(FRICTION > 0.0 && FRICTION < 1.0);
    }

    #[test]
    fn a_drag_with_no_press_does_nothing() {
        let mut d = VolumeDial::new(50.0);
        assert!(d.drag(90.0, 0.0).is_none());
        assert_eq!(d.value, 50.0);
    }
}
