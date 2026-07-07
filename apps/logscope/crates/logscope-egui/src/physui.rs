//! physui.rs — the behaviour of in-scene PHYSICAL CONTROLS: buttons and toggles that live in the 3D world
//! instead of a 2D toolbar (Overview / Tour / Freecam / Prune become objects you press).
//!
//! [`crate::picking`] answers "which object is under the pointer"; this answers "what is that object and how
//! does it respond". It's a real button FSM — a press only actuates if the pointer is RELEASED over the same
//! control it went down on (drag off to cancel), which is what makes a 3D object feel like a physical button
//! rather than a hover-trigger. Pure + testable: pointer events + the picked id go in, actuation events and a
//! per-control visual state (hover / pressed / active) come out — no egui, no GPU.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlKind {
    Momentary,      // fires on each press (Tour, Overview)
    Toggle,         // flips on each press (Freecam on/off)
    Selector(usize), // cycles through N options on press (the theme / model dropdown, made physical)
    Dial,           // a continuous value you DRAG (scene quality, time-of-day, exposure) — min/max on Control
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pointer { Move, Down, Up }

/// Emitted only when a control actually actuates. (Not `Eq` — `Adjusted` carries an f32.)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ControlEvent {
    Fired(usize),
    Toggled(usize, bool),
    Selected(usize, usize), // (control id, new option index)
    Adjusted(usize, f32),   // (control id, new dial value)
}

#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub id: usize,
    pub kind: ControlKind,
    pub toggled: bool, // meaningful for Toggle; the lit/active state
    pub index: usize,  // meaningful for Selector; the current option
    pub value: f32,    // meaningful for Dial; the current value
    pub min: f32,      // Dial range
    pub max: f32,
}

/// How a control should read this frame: the scene draws `hovered` as a glow, `pressed` as pushed-in, and
/// `active` as lit. A pressed control is also hovered.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Visual {
    pub hovered: bool,
    pub pressed: bool,
    pub active: bool,
}

/// Per-control animation state — what gives a physical control its tactile FEEL. The renderer offsets the
/// button inward by `press` (pushed-in), brightens it by `hover` (glow), and lights it by `active`. Eased,
/// so a press snaps in and springs back while hover/active fade — a button that just hard-cuts between states
/// reads as flat 2D chrome, not something in the world. Pure: feed the frame's [`Visual`] + dt, read the
/// eased 0..1 values.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlAnim {
    pub press: f32,  // 0 = out, 1 = fully depressed (springs, can dip slightly negative on release)
    pub hover: f32,  // 0..1 hover glow
    pub active: f32, // 0..1 lit (toggle on / current selector)
    pub flash: f32,  // 0..1 actuation pulse — kicked to 1 the instant a control fires, then decays
    vel: f32,        // press spring velocity (internal)
}

impl ControlAnim {
    fn approach(cur: f32, target: f32, k: f32) -> f32 {
        cur + (target - cur) * k.clamp(0.0, 1.0)
    }

    /// Kick the actuation flash — call the moment a control fires/toggles so the press visibly *does* something.
    pub fn kick_flash(&mut self) { self.flash = 1.0; }

    /// Advance the eases toward this frame's visual state. `dt` is seconds since last frame.
    pub fn tick(&mut self, v: Visual, dt: f32) {
        let dt = dt.clamp(0.0, 0.05); // guard against frame hitches so the spring can't blow up
        let bin = |b: bool| if b { 1.0 } else { 0.0 };
        // press is a damped spring toward the target: crisp IN, with a slight pop-OUT overshoot on release —
        // the difference between a cheap fade and a tactile click. Sub-stepped semi-implicit Euler for stability.
        let target = bin(v.pressed);
        let (k, c) = (300.0_f32, 22.0_f32); // ζ≈0.63 → ~10% overshoot
        let steps = 4;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let a = (target - self.press) * k - self.vel * c;
            self.vel += a * h;
            self.press += self.vel * h;
            // a button bottoms out at fully-pressed (no over-depression) but may pop slightly proud on release
            if self.press > 1.0 { self.press = 1.0; self.vel = self.vel.min(0.0); }
            if self.press < -0.15 { self.press = -0.15; self.vel = self.vel.max(0.0); }
        }
        self.hover = Self::approach(self.hover, bin(v.hovered), dt * 11.0).clamp(0.0, 1.0);
        self.active = Self::approach(self.active, bin(v.active), dt * 8.0).clamp(0.0, 1.0);
        self.flash = (self.flash - dt * 4.5).max(0.0); // decays over ~0.22 s
    }
}


#[derive(Default)]
pub struct Panel {
    controls: Vec<Control>,
    hovered: Option<usize>,
    pressed: Option<usize>,
}

impl Panel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a control. `initial` is the starting toggled/active state (ignored for momentary/selector).
    pub fn add(&mut self, id: usize, kind: ControlKind, initial: bool) {
        self.controls.push(Control { id, kind, toggled: kind == ControlKind::Toggle && initial, index: 0, value: 0.0, min: 0.0, max: 1.0 });
    }

    /// Register a selector cycling through `count` options, starting at `initial` (the theme/model dropdown).
    pub fn add_selector(&mut self, id: usize, count: usize, initial: usize) {
        let count = count.max(1);
        self.controls.push(Control { id, kind: ControlKind::Selector(count), toggled: false, index: initial % count, value: 0.0, min: 0.0, max: 1.0 });
    }

    /// Register a DIAL — a continuous value you drag over `[min, max]`, starting at `initial` (a scene
    /// parameter made physical: render quality, hour-of-day, exposure).
    pub fn add_dial(&mut self, id: usize, min: f32, max: f32, initial: f32) {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        self.controls.push(Control { id, kind: ControlKind::Dial, toggled: false, index: 0, value: initial.clamp(lo, hi), min: lo, max: hi });
    }

    /// Drag a dial by a fraction of its range (`delta_frac` = pointer travel ÷ dial length; +right/up).
    /// Clamps to `[min,max]` and returns the new value. No-op for non-dials.
    pub fn drag(&mut self, id: usize, delta_frac: f32) -> Option<ControlEvent> {
        let c = self.get_mut(id)?;
        if c.kind == ControlKind::Dial {
            let span = c.max - c.min;
            c.value = (c.value + delta_frac * span).clamp(c.min, c.max);
            return Some(ControlEvent::Adjusted(id, c.value));
        }
        None
    }

    /// Sync a dial's value from outside (the scene param changed by something other than a drag).
    pub fn set_value(&mut self, id: usize, v: f32) {
        if let Some(c) = self.get_mut(id) {
            if c.kind == ControlKind::Dial { c.value = v.clamp(c.min, c.max); }
        }
    }

    /// The current value of a dial (0 for non-dials).
    pub fn value_of(&self, id: usize) -> f32 {
        self.get(id).filter(|c| c.kind == ControlKind::Dial).map(|c| c.value).unwrap_or(0.0)
    }

    fn get(&self, id: usize) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }
    fn get_mut(&mut self, id: usize) -> Option<&mut Control> {
        self.controls.iter_mut().find(|c| c.id == id)
    }
    fn known(&self, id: usize) -> bool {
        self.controls.iter().any(|c| c.id == id)
    }

    /// Sync a toggle's lit state from the outside world (e.g. freecam turned on by a voice command, not a
    /// press) so the physical control stays consistent with app state.
    pub fn set_active(&mut self, id: usize, on: bool) {
        if let Some(c) = self.get_mut(id) {
            if c.kind == ControlKind::Toggle {
                c.toggled = on;
            }
        }
    }

    pub fn is_active(&self, id: usize) -> bool {
        self.get(id).map(|c| c.toggled).unwrap_or(false)
    }

    /// Feed one pointer event. `over` is the control id currently under the pointer (from `picking::pick`),
    /// or `None` for empty space. Returns a [`ControlEvent`] iff a control actuated on THIS event.
    pub fn on_pointer(&mut self, ev: Pointer, over: Option<usize>) -> Option<ControlEvent> {
        let over = over.filter(|&id| self.known(id));
        match ev {
            Pointer::Move => {
                self.hovered = over;
                None
            }
            Pointer::Down => {
                self.pressed = over;
                self.hovered = over;
                None
            }
            Pointer::Up => {
                let fired = match (self.pressed, over) {
                    // released over the SAME control it was pressed on → actuate
                    (Some(p), Some(o)) if p == o => self.actuate(p),
                    // released elsewhere (dragged off) or on nothing → cancel, no actuation
                    _ => None,
                };
                self.pressed = None;
                self.hovered = over;
                fired
            }
        }
    }

    fn actuate(&mut self, id: usize) -> Option<ControlEvent> {
        let c = self.get_mut(id)?;
        match c.kind {
            ControlKind::Momentary => Some(ControlEvent::Fired(id)),
            ControlKind::Toggle => {
                c.toggled = !c.toggled;
                Some(ControlEvent::Toggled(id, c.toggled))
            }
            ControlKind::Selector(n) => {
                c.index = (c.index + 1) % n.max(1); // press advances to the next option, wrapping
                Some(ControlEvent::Selected(id, c.index))
            }
            ControlKind::Dial => None, // a dial responds to drag(), not a press
        }
    }

    /// Step a selector by `delta` (wheel / drag-to-spin — negative goes back), wrapping. Returns the event.
    pub fn nudge_selector(&mut self, id: usize, delta: i32) -> Option<ControlEvent> {
        let c = self.get_mut(id)?;
        if let ControlKind::Selector(n) = c.kind {
            let n = n.max(1) as i32;
            c.index = (((c.index as i32 + delta) % n + n) % n) as usize; // wrap both directions
            return Some(ControlEvent::Selected(id, c.index));
        }
        None
    }

    /// Sync a selector's option from outside (e.g. theme changed by a command, not a press).
    pub fn set_index(&mut self, id: usize, index: usize) {
        if let Some(c) = self.get_mut(id) {
            if let ControlKind::Selector(n) = c.kind {
                c.index = index % n.max(1);
            }
        }
    }

    /// The current option of a selector (0 for non-selectors).
    pub fn index_of(&self, id: usize) -> usize {
        self.get(id).map(|c| c.index).unwrap_or(0)
    }

    /// The render state for a control this frame.
    pub fn visual(&self, id: usize) -> Visual {
        Visual {
            hovered: self.hovered == Some(id) || self.pressed == Some(id),
            pressed: self.pressed == Some(id),
            active: self.is_active(id),
        }
    }
}

/// Lay out `n` physical controls along a horizontal ARC facing the user — a control dock beside the pearl.
/// Returns each control's world position, evenly spaced across `spread` radians (centred on straight-ahead),
/// on a circle of `radius` around `center` in the plane spanned by `right` and `fwd`. `n == 1` sits dead
/// centre. These positions feed the renderer (draw a button there) and `picking` (project them to hit-test).
pub fn dock_arc(n: usize, center: [f32; 3], right: [f32; 3], fwd: [f32; 3], radius: f32, spread: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // angle across [-spread/2, +spread/2]; single control → 0 (centre)
        let t = if n <= 1 { 0.5 } else { i as f32 / (n - 1) as f32 };
        let ang = (t - 0.5) * spread;
        let (s, c) = ang.sin_cos();
        // position = center + radius*(fwd*cos + right*sin) — an arc bulging toward the user
        out.push([
            center[0] + radius * (fwd[0] * c + right[0] * s),
            center[1] + radius * (fwd[1] * c + right[1] * s),
            center[2] + radius * (fwd[2] * c + right[2] * s),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOUR: usize = 1;
    const FREECAM: usize = 2;

    fn panel() -> Panel {
        let mut p = Panel::new();
        p.add(TOUR, ControlKind::Momentary, false);
        p.add(FREECAM, ControlKind::Toggle, false);
        p
    }

    #[test]
    fn momentary_fires_on_press_release_same_control() {
        let mut p = panel();
        assert_eq!(p.on_pointer(Pointer::Down, Some(TOUR)), None); // press alone doesn't fire
        assert_eq!(p.on_pointer(Pointer::Up, Some(TOUR)), Some(ControlEvent::Fired(TOUR)));
    }

    #[test]
    fn drag_off_cancels_the_press() {
        let mut p = panel();
        p.on_pointer(Pointer::Down, Some(TOUR));
        p.on_pointer(Pointer::Move, None); // slid off the button
        assert_eq!(p.on_pointer(Pointer::Up, None), None, "released off-target → no fire");
        assert!(!p.visual(TOUR).pressed, "press state cleared");
    }

    #[test]
    fn release_on_a_different_control_does_not_fire_either() {
        let mut p = panel();
        p.on_pointer(Pointer::Down, Some(TOUR));
        assert_eq!(p.on_pointer(Pointer::Up, Some(FREECAM)), None, "pressed TOUR, released over FREECAM → nothing");
        assert!(!p.is_active(FREECAM), "and FREECAM did not toggle");
    }

    #[test]
    fn toggle_flips_and_reports_state() {
        let mut p = panel();
        p.on_pointer(Pointer::Down, Some(FREECAM));
        assert_eq!(p.on_pointer(Pointer::Up, Some(FREECAM)), Some(ControlEvent::Toggled(FREECAM, true)));
        assert!(p.is_active(FREECAM));
        // press again → off
        p.on_pointer(Pointer::Down, Some(FREECAM));
        assert_eq!(p.on_pointer(Pointer::Up, Some(FREECAM)), Some(ControlEvent::Toggled(FREECAM, false)));
        assert!(!p.is_active(FREECAM));
    }

    #[test]
    fn external_state_syncs_the_toggle() {
        let mut p = panel();
        p.set_active(FREECAM, true); // freecam enabled by a voice command, not a press
        assert!(p.is_active(FREECAM));
        assert!(p.visual(FREECAM).active);
        // a momentary control ignores set_active
        p.set_active(TOUR, true);
        assert!(!p.is_active(TOUR));
    }

    #[test]
    fn hover_and_press_visuals_track_the_pointer() {
        let mut p = panel();
        p.on_pointer(Pointer::Move, Some(TOUR));
        assert!(p.visual(TOUR).hovered && !p.visual(TOUR).pressed);
        p.on_pointer(Pointer::Down, Some(TOUR));
        let v = p.visual(TOUR);
        assert!(v.hovered && v.pressed, "a pressed control is also hovered");
        p.on_pointer(Pointer::Up, Some(TOUR));
        assert!(!p.visual(TOUR).pressed);
    }

    #[test]
    fn unknown_ids_are_ignored() {
        let mut p = panel();
        assert_eq!(p.on_pointer(Pointer::Down, Some(999)), None);
        assert_eq!(p.on_pointer(Pointer::Up, Some(999)), None);
    }

    #[test]
    fn control_anim_eases_toward_visual_state() {
        let mut a = ControlAnim::default();
        let pressed = Visual { hovered: true, pressed: true, active: false };
        // a single tick moves partway (not a hard cut), in the right direction
        a.tick(pressed, 0.016);
        assert!(a.press > 0.0 && a.press < 1.0, "press eases in, doesn't snap: {}", a.press);
        assert!(a.hover > 0.0);
        // many ticks converge to fully pressed + hovered
        for _ in 0..120 { a.tick(pressed, 0.016); }
        assert!(a.press > 0.98 && a.hover > 0.98, "converges to 1");
        // release: press springs back toward 0
        let released = Visual { hovered: false, pressed: false, active: false };
        for _ in 0..120 { a.tick(released, 0.016); }
        assert!(a.press < 0.02 && a.hover < 0.02, "springs back to 0");
        // stays clamped in [0,1] even with a huge dt
        a.tick(Visual { hovered: true, pressed: true, active: true }, 10.0);
        assert!(a.press <= 1.0 && a.active <= 1.0 && a.hover <= 1.0);
    }

    #[test]
    fn press_release_pops_out_before_settling() {
        // hold to fully pressed, then release: the spring should dip PROUD (press < 0) before settling —
        // the tactile "pop" — not just fade monotonically to rest.
        let mut a = ControlAnim::default();
        for _ in 0..120 { a.tick(Visual { hovered: true, pressed: true, active: false }, 0.016); }
        assert!(a.press > 0.98);
        let released = Visual { hovered: false, pressed: false, active: false };
        let mut min_seen = 1.0_f32;
        for _ in 0..40 { a.tick(released, 0.016); min_seen = min_seen.min(a.press); }
        assert!(min_seen < -0.01, "release overshoots proud of rest (got min {min_seen})");
        for _ in 0..120 { a.tick(released, 0.016); }
        assert!(a.press.abs() < 0.02, "then settles to rest");
    }

    #[test]
    fn actuation_flash_kicks_and_decays() {
        let mut a = ControlAnim::default();
        assert_eq!(a.flash, 0.0);
        a.kick_flash();
        assert_eq!(a.flash, 1.0, "kick lights the flash");
        for _ in 0..8 { a.tick(Visual::default(), 0.016); } // ~0.13 s
        assert!(a.flash > 0.0 && a.flash < 1.0, "flash decays gradually: {}", a.flash);
        for _ in 0..40 { a.tick(Visual::default(), 0.016); }
        assert_eq!(a.flash, 0.0, "flash fully decays");
    }

    #[test]
    fn selector_cycles_through_options() {
        const THEME: usize = 5;
        let mut p = Panel::new();
        p.add_selector(THEME, 3, 0); // 3 themes, starting at 0
        // a press advances to the next option, wrapping
        p.on_pointer(Pointer::Down, Some(THEME));
        assert_eq!(p.on_pointer(Pointer::Up, Some(THEME)), Some(ControlEvent::Selected(THEME, 1)));
        assert_eq!(p.index_of(THEME), 1);
        p.on_pointer(Pointer::Down, Some(THEME));
        p.on_pointer(Pointer::Up, Some(THEME));
        p.on_pointer(Pointer::Down, Some(THEME));
        assert_eq!(p.on_pointer(Pointer::Up, Some(THEME)), Some(ControlEvent::Selected(THEME, 0)), "wraps 2→0");
        // wheel/drag-to-spin can go backwards
        assert_eq!(p.nudge_selector(THEME, -1), Some(ControlEvent::Selected(THEME, 2)), "−1 wraps to the last");
        // external sync (theme changed by a command)
        p.set_index(THEME, 1);
        assert_eq!(p.index_of(THEME), 1);
        // a momentary control has no selector behaviour
        assert_eq!(p.nudge_selector(999, 1), None);
    }

    #[test]
    fn dock_arc_lays_controls_on_an_arc() {
        let center = [0.0, 0.0, 0.0];
        let (right, fwd) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let ps = dock_arc(3, center, right, fwd, 2.0, std::f32::consts::FRAC_PI_2);
        assert_eq!(ps.len(), 3);
        // all on the circle of the given radius
        for p in &ps {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 2.0).abs() < 1e-4, "each control sits on the dock radius");
        }
        // symmetric about straight-ahead: middle control is dead centre (+fwd), ends mirror on x
        assert!((ps[1][0]).abs() < 1e-4 && (ps[1][2] - 2.0).abs() < 1e-4);
        assert!((ps[0][0] + ps[2][0]).abs() < 1e-4, "left/right ends mirror across centre");
        // a single control sits dead centre
        let one = dock_arc(1, center, right, fwd, 2.0, 1.0);
        assert!((one[0][0]).abs() < 1e-4 && (one[0][2] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn dial_drags_within_range() {
        const QUALITY: usize = 7;
        let mut p = Panel::new();
        p.add_dial(QUALITY, 0.5, 1.5, 1.0); // render quality, starts at 1.0
        assert_eq!(p.drag(QUALITY, 0.3), Some(ControlEvent::Adjusted(QUALITY, 1.3))); // +30% of range
        assert!((p.value_of(QUALITY) - 1.3).abs() < 1e-6);
        p.drag(QUALITY, 1.0);
        assert!((p.value_of(QUALITY) - 1.5).abs() < 1e-6, "clamps at max");
        p.drag(QUALITY, -5.0);
        assert!((p.value_of(QUALITY) - 0.5).abs() < 1e-6, "clamps at min");
        p.set_value(QUALITY, 1.1);
        assert!((p.value_of(QUALITY) - 1.1).abs() < 1e-6, "external sync");
        p.add(1, ControlKind::Momentary, false);
        assert_eq!(p.drag(1, 0.5), None, "non-dials don't drag");
        assert_eq!(p.value_of(1), 0.0);
    }
}
