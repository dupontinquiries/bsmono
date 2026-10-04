//! The day as a graph, for the thin bar, where a clock face or "12:34" does not fit a 32 px column.
//!
//! Time runs DOWN the widget, midnight at the top. Two bars side by side on a grid:
//!
//! * the left bar fills from the top to now, so how much of the day is gone reads at a glance;
//! * the right bar is how bright the shell runs through the day: full by day, dimmed through the night window
//!   (`NightWindow`, fades included), so when the screen will dim is visible before it does.
//!
//! A horizontal tick every three hours, the six-hour ones stronger, forms the grid; a marker line crosses both bars at now.
//! Pure geometry in fractions of the widget's height; the frontend paints it.

use crate::ambient::NightWindow;

/// The right bar is this many slices tall (half an hour each).
pub const SLICES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Fraction of the height, 0 (midnight) to 1 (the next midnight).
    pub at: f32,
    /// A six-hour mark (06, 12, 18) rather than a three-hour one.
    pub major: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayGraph {
    /// Where now is, 0..1 down the widget.
    pub now: f32,
    /// How bright each half hour of the day is run: 1 at full day, 0 at full night.
    pub level: [f32; SLICES],
    pub ticks: Vec<Tick>,
}

pub fn layout(minute_of_day: u32, night: &NightWindow) -> DayGraph {
    let m = minute_of_day % 1440;
    let span = (1.0 - night.dim).max(1e-3);
    let mut level = [1.0f32; SLICES];
    // a window that does not dim (dim at 1) dims nothing: the whole day is bright, not "all night"
    for (i, l) in level.iter_mut().enumerate().filter(|_| night.dim < 0.999) {
        // sample the middle of each slice
        let minute = (i as u32 * 30 + 15) % 1440;
        *l = ((night.factor(minute) - night.dim) / span).clamp(0.0, 1.0);
    }
    let ticks = (1..8).map(|k| Tick { at: k as f32 / 8.0, major: k % 2 == 0 }).collect();
    DayGraph { now: m as f32 / 1440.0, level, ticks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_the_fraction_of_the_day_gone_and_midnight_wraps_to_the_top() {
        let n = NightWindow::default();
        assert_eq!(layout(0, &n).now, 0.0);
        assert_eq!(layout(720, &n).now, 0.5);
        assert!((layout(18 * 60, &n).now - 0.75).abs() < 1e-6);
        assert_eq!(layout(1440, &n).now, 0.0, "24:00 is the next day's top");
        assert!(layout(1439, &n).now < 1.0);
    }

    #[test]
    fn the_night_window_shows_as_dark_slices_across_midnight_with_its_fades() {
        let g = layout(600, &NightWindow::default()); // 22:00 to 07:00, a 60 minute fade each side
        let at = |h: f32| g.level[(h * 2.0) as usize];
        assert_eq!(at(2.0), 0.0, "02:00 is deep night");
        assert_eq!(at(12.0), 1.0, "noon is full day");
        assert_eq!(at(23.5), 0.0, "inside the window before midnight");
        assert!(at(21.5) > 0.0 && at(21.5) < 1.0, "the fade before 22:00 is partial: {}", at(21.5));
        assert!(at(7.5) > 0.0 && at(7.5) < 1.0, "and so is the one after 07:00: {}", at(7.5));
        let dark = g.level.iter().filter(|&&l| l == 0.0).count();
        assert!((16..=19).contains(&dark), "about nine hours of full night: {dark} half-hours");
    }

    #[test]
    fn a_day_with_no_dimming_is_all_bright_and_a_daytime_window_is_in_the_middle() {
        let flat = NightWindow { dim: 1.0, ..NightWindow::default() };
        assert!(layout(0, &flat).level.iter().all(|&l| l == 1.0));
        let nap = NightWindow { start: 13 * 60, end: 15 * 60, fade: 0, dim: 0.5 };
        let g = layout(0, &nap);
        assert_eq!(g.level[2 * 14], 0.0, "14:00 is inside the window");
        assert_eq!(g.level[2 * 10], 1.0);
        assert_eq!(g.level[0], 1.0, "midnight is bright when the window is in the afternoon");
    }

    #[test]
    fn the_grid_has_a_tick_every_three_hours_and_the_six_hour_ones_are_major() {
        let g = layout(0, &NightWindow::default());
        assert_eq!(g.ticks.len(), 7, "03 06 09 12 15 18 21");
        let hours: Vec<f32> = g.ticks.iter().map(|t| t.at * 24.0).collect();
        assert_eq!(hours, vec![3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0]);
        let major: Vec<f32> = g.ticks.iter().filter(|t| t.major).map(|t| t.at * 24.0).collect();
        assert_eq!(major, vec![6.0, 12.0, 18.0]);
    }
}
