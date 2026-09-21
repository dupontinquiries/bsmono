//! Press-and-hold on the bottom edge opens a small arc of mode buttons above
//! the finger: slide onto one and lift to toggle it, lift anywhere else to
//! cancel. One continuous gesture, no second tap, and nothing happens if you
//! change your mind — the same contract as a long-press context menu.
//!
//! Options are laid out on an arc of radius `radius` centred on the press
//! point, clamped so the arc stays on screen when the press is near a corner.

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
