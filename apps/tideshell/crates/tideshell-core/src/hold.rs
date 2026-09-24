//! Press-and-hold on the bottom edge opens a small arc of mode buttons above
//! the finger: slide onto one and lift to toggle it, lift anywhere else to
//! cancel. One continuous gesture, no second tap, and nothing happens if you
//! change your mind — the same contract as a long-press context menu.
//!
//! Options are laid out on an arc of radius `radius` centred on the press
//! point, clamped so the arc stays on screen when the press is near a corner.
//!
//! ## Three buttons, and a fourth thing that is not a button
//!
//! Three fits the arc a thumb sweeps without regripping. More than that and
//! the targets shrink, the sweep gets wider than the joint allows, and the
//! menu stops being glanceable — so everything else lives behind SEARCH,
//! which is reached by travel rather than by aim.
//!
//! Distance from the press point decides, in bands
//! ([`Reach::at`]):
//!
//! | distance | meaning |
//! |---|---|
//! | under `DEAD_FRAC` | nothing. Holding still is the null gesture |
//! | around the arc | the option under the thumb, by angle |
//! | past `SEARCH_FRAC` | search, which needs no aim at all |
//! | past `DISMISS_FRAC` | abandoning: the menu fades with the travel |
//!
//! **Stillness does nothing on purpose.** People hesitate mid-menu, and a
//! gesture that fires when the thumb stops would go off every time someone
//! pauses to decide or shifts their grip. Continued travel is the only
//! unambiguous "none of these".
//!
//! **Search occupies the whole beyond**, not a fourth circle: it is the
//! thing you reach for when the three on offer are wrong, which is exactly
//! when you do not want to aim at a small target.
//!
//! **Dismissal is further still**, and fades in proportion to the travel, so
//! the menu visibly gets out of the way as the thumb leaves rather than
//! vanishing at a threshold. Releasing fades it out from wherever it is.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldOption {
    Carousel,
    Keyboard,
    Trackpad,
}

impl HoldOption {
    pub const ALL: [HoldOption; 3] = [HoldOption::Carousel, HoldOption::Keyboard, HoldOption::Trackpad];

    pub fn label(self) -> &'static str {
        match self {
            HoldOption::Carousel => "Carousel",
            HoldOption::Keyboard => "Keyboard",
            HoldOption::Trackpad => "Trackpad",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoldMenu {
    /// Arc centre after clamping (not necessarily the raw press point).
    pub origin: (f32, f32),
    pub radius: f32,
    /// Hit radius of each option button.
    pub hit: f32,
    /// Button centres, parallel to `HoldOption::ALL`.
    pub centers: [(f32, f32); 3],
    /// Option currently under the finger.
    pub hover: Option<HoldOption>,
}

impl HoldMenu {
    pub fn open(press: (f32, f32), screen: (f32, f32)) -> HoldMenu {
        let radius = 110.0_f32;
        let hit = 46.0_f32;
        let margin = hit + 6.0;
        // The arc spans ±60° around straight up, so its horizontal half-width
        // is r·sin60 and it rises a full r above the origin.
        let half_w = radius * 60f32.to_radians().sin();
        let ox = press.0.clamp(margin + half_w, (screen.0 - margin - half_w).max(margin + half_w));
        let oy = press.1.max(margin + radius);
        let centers = [-150f32, -90.0, -30.0].map(|deg| {
            let a = deg.to_radians();
            (ox + radius * a.cos(), oy + radius * a.sin())
        });
        HoldMenu { origin: (ox, oy), radius, hit, centers, hover: None }
    }

    pub fn option_at(&self, x: f32, y: f32) -> Option<HoldOption> {
        HoldOption::ALL
            .into_iter()
            .zip(self.centers)
            .map(|(o, (cx, cy))| (o, (x - cx).hypot(y - cy)))
            .filter(|(_, d)| *d <= self.hit)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, _)| o)
    }

    pub fn hover_to(&mut self, x: f32, y: f32) {
        self.hover = self.option_at(x, y);
    }

    /// Lift: the option under the finger, if any.
    pub fn release(&self, x: f32, y: f32) -> Option<HoldOption> {
        self.option_at(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (408.0, 900.0);

    #[test]
    fn arc_sits_above_the_finger_left_to_right() {
        let m = HoldMenu::open((204.0, 890.0), SCREEN);
        let [a, b, c] = m.centers;
        assert!(a.0 < b.0 && b.0 < c.0, "left→right: {:?}", m.centers);
        assert!(b.1 < a.1 && b.1 < c.1, "middle option highest");
        assert!(m.centers.iter().all(|p| p.1 < 890.0));
    }

    #[test]
    fn each_option_selectable_at_its_center_and_nothing_at_origin() {
        let m = HoldMenu::open((204.0, 890.0), SCREEN);
        for (o, (x, y)) in HoldOption::ALL.into_iter().zip(m.centers) {
            assert_eq!(m.release(x, y), Some(o));
            assert_eq!(m.release(x + 20.0, y - 20.0), Some(o), "within hit radius");
        }
        assert_eq!(m.release(m.origin.0, m.origin.1), None, "lifting in place cancels");
    }

    #[test]
    fn corner_press_is_clamped_on_screen() {
        for press in [(2.0, 898.0), (406.0, 898.0)] {
            let m = HoldMenu::open(press, SCREEN);
            for (x, y) in m.centers {
                assert!(x - m.hit >= 0.0 && x + m.hit <= SCREEN.0, "x {x} off screen for press {press:?}");
                assert!(y - m.hit >= 0.0);
            }
        }
    }

    #[test]
    fn hover_tracks_finger() {
        let mut m = HoldMenu::open((204.0, 890.0), SCREEN);
        let (x, y) = m.centers[2];
        m.hover_to(x, y);
        assert_eq!(m.hover, Some(HoldOption::Trackpad));
        m.hover_to(0.0, 0.0);
        assert_eq!(m.hover, None);
    }
}

/// Distance bands, as multiples of the arc radius.
pub const DEAD_FRAC: f32 = 0.45;
pub const SEARCH_FRAC: f32 = 1.55;
pub const DISMISS_FRAC: f32 = 2.0;
/// Fully faded by this far out.
pub const GONE_FRAC: f32 = 3.0;

/// What the thumb's position means right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reach {
    /// Too close to the centre to mean anything.
    Idle,
    /// On the arc: this option.
    Option(HoldOption),
    /// Past the arc: open search.
    Search,
    /// Leaving. `0.0` at the dismiss threshold, `1.0` when gone.
    Dismissing(f32),
}

impl HoldMenu {
    /// Where the thumb is, in bands. Angle only matters in the arc band.
    pub fn reach(&self, x: f32, y: f32) -> Reach {
        let d = ((x - self.origin.0).powi(2) + (y - self.origin.1).powi(2)).sqrt();
        let r = self.radius.max(1.0);
        if d < r * DEAD_FRAC {
            return Reach::Idle;
        }
        if d >= r * DISMISS_FRAC {
            let t = ((d - r * DISMISS_FRAC) / (r * (GONE_FRAC - DISMISS_FRAC))).clamp(0.0, 1.0);
            return Reach::Dismissing(t);
        }
        if d >= r * SEARCH_FRAC {
            return Reach::Search;
        }
        match self.option_at(x, y) {
            Some(o) => Reach::Option(o),
            // Between the buttons but within the arc: still choosing.
            None => Reach::Idle,
        }
    }

    /// How opaque the menu should be drawn, from the same travel.
    pub fn alpha(&self, x: f32, y: f32) -> f32 {
        match self.reach(x, y) {
            Reach::Dismissing(t) => 1.0 - t,
            _ => 1.0,
        }
    }

    /// What a lift here does. `None` means the menu simply closes.
    pub fn lift(&self, x: f32, y: f32) -> Option<Reach> {
        match self.reach(x, y) {
            Reach::Option(o) => Some(Reach::Option(o)),
            Reach::Search => Some(Reach::Search),
            _ => None,
        }
    }
}

#[cfg(test)]
mod reach_tests {
    use super::*;

    fn menu() -> HoldMenu {
        HoldMenu::open((400.0, 1200.0), (800.0, 1600.0))
    }

    fn at(m: &HoldMenu, dist: f32, up: bool) -> (f32, f32) {
        let d = if up { -dist } else { dist };
        (m.origin.0, m.origin.1 + d)
    }

    #[test]
    fn holding_still_does_nothing_at_all() {
        // The whole reason the thumb resting in the centre is inert: people
        // pause to decide, and a gesture that fires on stillness fires on
        // every hesitation.
        let m = menu();
        assert_eq!(m.reach(m.origin.0, m.origin.1), Reach::Idle);
        let (x, y) = at(&m, m.radius * 0.3, true);
        assert_eq!(m.reach(x, y), Reach::Idle);
        assert_eq!(m.lift(x, y), None, "and lifting there just closes the menu");
    }

    #[test]
    fn past_the_arc_is_search_and_needs_no_aim() {
        let m = menu();
        // Every direction works: search is the whole beyond, not a target.
        for deg in [0.0f32, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
            let t = deg.to_radians();
            let d = m.radius * 1.7;
            let (x, y) = (m.origin.0 + d * t.cos(), m.origin.1 + d * t.sin());
            assert_eq!(m.reach(x, y), Reach::Search, "at {deg} degrees");
        }
        let (x, y) = at(&m, m.radius * 1.7, true);
        assert_eq!(m.lift(x, y), Some(Reach::Search));
    }

    #[test]
    fn going_further_abandons_it_and_the_fade_follows_the_thumb() {
        let m = menu();
        let (x, y) = at(&m, m.radius * DISMISS_FRAC, true);
        assert_eq!(m.reach(x, y), Reach::Dismissing(0.0), "the fade starts, nothing is gone yet");
        assert_eq!(m.alpha(x, y), 1.0);
        // Halfway between dismiss and gone.
        let mid = m.radius * (DISMISS_FRAC + (GONE_FRAC - DISMISS_FRAC) / 2.0);
        let (x, y) = at(&m, mid, true);
        assert!((m.alpha(x, y) - 0.5).abs() < 0.02, "alpha {}", m.alpha(x, y));
        // Well past: gone, and a lift does nothing.
        let (x, y) = at(&m, m.radius * 5.0, true);
        assert_eq!(m.alpha(x, y), 0.0);
        assert_eq!(m.lift(x, y), None);
    }

    #[test]
    fn the_bands_are_ordered_and_do_not_overlap() {
        // Walking straight out must pass Idle -> (option or idle) -> Search
        // -> Dismissing, and never go back.
        let m = menu();
        let mut seen_search = false;
        let mut seen_dismiss = false;
        for i in 0..=120 {
            let d = m.radius * (i as f32 / 40.0);
            let (x, y) = at(&m, d, true);
            match m.reach(x, y) {
                Reach::Dismissing(_) => seen_dismiss = true,
                Reach::Search => {
                    assert!(!seen_dismiss, "search after dismissing at {d}");
                    seen_search = true;
                }
                _ => assert!(!seen_search && !seen_dismiss, "went back to the arc at {d}"),
            }
        }
        assert!(seen_search && seen_dismiss);
    }

    #[test]
    fn an_option_still_wins_inside_the_arc() {
        // The bands must not have swallowed the thing the menu is for.
        let m = menu();
        let c = m.centers[1];
        assert_eq!(m.reach(c.0, c.1), Reach::Option(HoldOption::ALL[1]));
        assert_eq!(m.lift(c.0, c.1), Some(Reach::Option(HoldOption::ALL[1])));
        assert_eq!(m.alpha(c.0, c.1), 1.0);
    }
}
