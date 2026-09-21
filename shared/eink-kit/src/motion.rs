//! Motion: which parts of the screen are changing continuously.
//!
//! Colour and quality cost refresh time on these panels. A video, a
//! playhead or a level meter changes many times a second and would keep the
//! controller busy with slow colour waveforms, starving the finger. So
//! tiles are classified:
//!
//! * **motion**: changing at or above `enter_hz` for a sustained period.
//!   Updated at a capped rate, with a fast waveform, mono and ordered-
//!   dithered by default (fast waveforms are close to 1-bit, and dithering
//!   is what keeps a picture a picture instead of a threshold blot).
//! * everything else: the normal interaction/settle path.
//!
//! When motion stops (below `exit_hz`, or quiet for `quiet`), the region is
//! re-rendered undithered and settles to full colour.
//!
//! Detection is automatic: no app changes needed. Apps that know better
//! declare regions with [`hint`]. The hint protocol is plain egui memory
//! (an `Id` and a `Vec<(Rect, u8)>`), so an app or a shared UI crate can
//! emit hints WITHOUT depending on eink-kit; on the desktop they are inert.

use crate::raster::DirtyTiles;
use std::time::Duration;

/// What an app says a region is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Hint {
    /// Continuous picture (video, camera preview): motion from the first frame.
    Video = 1,
    /// Small continuously-updating element (playhead, meter, spinner).
    Animation = 2,
    /// Never treat as motion, however often it changes (e.g. a text caret).
    Static = 3,
}

/// The egui memory key hints are stored under. Stable: other crates may
/// write it directly as `Vec<(egui::Rect, u8)>` without linking eink-kit.
pub const HINTS_KEY: &str = "eink.hints";
/// Where the engine publishes the motion refresh interval in seconds
/// (`f32`), so a producer (a video decoder) can skip frames that would never
/// reach the panel.
pub const BUDGET_KEY: &str = "eink.motion_interval";

/// Declare a region for this frame. Call every frame the region exists.
pub fn hint(ctx: &egui::Context, rect: egui::Rect, h: Hint) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(egui::Rect, u8)>>(egui::Id::new(HINTS_KEY)).push((rect, h as u8));
    });
}

/// Seconds between motion refreshes the panel will actually show, or `None`
/// when not running on eink-kit. A decoder should not produce frames faster.
pub fn motion_interval(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new(BUDGET_KEY)))
}

pub(crate) fn take_hints(ctx: &egui::Context) -> Vec<(egui::Rect, Hint)> {
    let raw: Vec<(egui::Rect, u8)> =
        ctx.data_mut(|d| d.remove_temp(egui::Id::new(HINTS_KEY))).unwrap_or_default();
    raw.into_iter()
        .filter_map(|(r, h)| {
            let h = match h {
                1 => Hint::Video,
                2 => Hint::Animation,
                3 => Hint::Static,
                _ => return None,
            };
            Some((r, h))
        })
        .collect()
}

/// Per-tile change-rate tracker with hysteresis.
#[derive(Debug, Clone)]
pub struct Heat {
    pub cols: usize,
    pub rows: usize,
    last: Vec<Option<Duration>>,
    rate: Vec<f32>,
    moving: Vec<bool>,
    pub enter_hz: f32,
    pub exit_hz: f32,
    pub quiet: Duration,
}

impl Heat {
    pub fn new(cols: usize, rows: usize) -> Heat {
        Heat {
            cols,
            rows,
            last: vec![None; cols * rows],
            rate: vec![0.0; cols * rows],
            moving: vec![false; cols * rows],
            enter_hz: 3.0,
            exit_hz: 1.0,
            quiet: Duration::from_millis(700),
        }
    }

    /// Record which tiles changed in a frame at `now`.
    pub fn observe(&mut self, dirty: &DirtyTiles, now: Duration) {
        for i in 0..self.rate.len() {
            if dirty.dirty[i] {
                if let Some(t) = self.last[i] {
                    let dt = now.saturating_sub(t).as_secs_f32().max(0.001);
                    // EWMA of instantaneous rate: two or three changes at a
                    // steady cadence are enough to cross enter_hz.
                    self.rate[i] = self.rate[i] * 0.5 + (1.0 / dt) * 0.5;
                }
                self.last[i] = Some(now);
            }
        }
        self.decay(now);
    }

    /// Time passed without a frame.
    pub fn decay(&mut self, now: Duration) {
        for i in 0..self.rate.len() {
            if let Some(t) = self.last[i] {
                let idle = now.saturating_sub(t);
                // A tile that has not changed for `idle` cannot be changing
                // faster than 1/idle, whatever the average said.
                self.rate[i] = self.rate[i].min(1.0 / idle.as_secs_f32().max(0.001));
                self.moving[i] = if self.moving[i] {
                    self.rate[i] >= self.exit_hz && idle < self.quiet
                } else {
                    self.rate[i] >= self.enter_hz
                };
            }
        }
    }

    /// Changed within `quiet` of `now`.
    pub fn recently_changed(&self, i: usize, now: Duration) -> bool {
        self.last[i].is_some_and(|t| now.saturating_sub(t) < self.quiet)
    }

    pub fn is_moving(&self, i: usize) -> bool {
        self.moving[i]
    }
    pub fn any_moving(&self) -> bool {
        self.moving.iter().any(|m| *m)
    }
    pub fn rate(&self, i: usize) -> f32 {
        self.rate[i]
    }
}

/// 4×4 Bayer thresholds, 0..255.
const BAYER4: [[u8; 4]; 4] = [[8, 136, 40, 168], [200, 72, 232, 104], [56, 184, 24, 152], [248, 120, 216, 88]];

/// Ordered dither to black/white. Position-stable: the same image gives the
/// same pixels every frame, so a paused dithered video produces no damage.
///
/// Near-white and near-black snap to pure paper and ink first. Without
/// that, the UI's off-white background (≈247) dithers to a sparse dot
/// texture across every motion tile, and anti-aliased text gets specks.
#[inline]
pub fn dither_bw(luma: u8, x: usize, y: usize) -> u8 {
    if luma >= 232 {
        255
    } else if luma <= 23 {
        0
    } else if luma > BAYER4[y & 3][x & 3] {
        255
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::TILE;

    fn tiles(cols: usize, rows: usize, dirty: &[usize]) -> DirtyTiles {
        let mut d = DirtyTiles {
            cols, rows, dirty: vec![false; cols * rows], chroma: vec![false; cols * rows],
            width: (cols * TILE) as u32, height: (rows * TILE) as u32,
        };
        for &i in dirty { d.dirty[i] = true; }
        d
    }
    fn ms(v: u64) -> Duration { Duration::from_millis(v) }

    #[test]
    fn steady_ten_hz_becomes_motion_and_stops_when_it_stops() {
        let mut h = Heat::new(4, 1);
        for f in 0..5 {
            h.observe(&tiles(4, 1, &[1]), ms(f * 100));
        }
        assert!(h.is_moving(1), "rate {}", h.rate(1));
        assert!(!h.is_moving(0));
        h.decay(ms(400 + 800));
        assert!(!h.is_moving(1), "quiet for 800ms should end motion");
    }

    #[test]
    fn a_single_change_or_slow_clicks_are_not_motion() {
        let mut h = Heat::new(2, 1);
        for f in 0..5 {
            h.observe(&tiles(2, 1, &[0]), ms(f * 1500)); // a click every 1.5 s
        }
        assert!(!h.is_moving(0), "rate {}", h.rate(0));
    }

    #[test]
    fn hysteresis_keeps_a_stuttering_video_classified() {
        let mut h = Heat::new(1, 1);
        for f in 0..6 {
            h.observe(&tiles(1, 1, &[0]), ms(f * 100));
        }
        assert!(h.is_moving(0));
        // one late frame (400 ms gap) must not flip it back
        h.observe(&tiles(1, 1, &[0]), ms(500 + 400));
        assert!(h.is_moving(0), "rate {}", h.rate(0));
    }

    #[test]
    fn dither_is_black_or_white_and_tracks_luma() {
        let mut ones = 0;
        for y in 0..4 {
            for x in 0..4 {
                let v = dither_bw(128, x, y);
                assert!(v == 0 || v == 255);
                ones += (v == 255) as u32;
            }
        }
        assert_eq!(ones, 8, "mid grey is half white");
        assert!((0..16).all(|i| dither_bw(255, i % 4, i / 4) == 255));
        assert!((0..16).all(|i| dither_bw(0, i % 4, i / 4) == 0));
    }

    #[test]
    fn paper_stays_paper_and_ink_stays_ink() {
        for l in [232u8, 240, 247, 255] {
            assert!((0..16).all(|i| dither_bw(l, i % 4, i / 4) == 255), "luma {l} speckled");
        }
        for l in [0u8, 10, 23] {
            assert!((0..16).all(|i| dither_bw(l, i % 4, i / 4) == 0), "luma {l} speckled");
        }
    }

    #[test]
    fn hints_round_trip_through_plain_egui_memory() {
        let ctx = egui::Context::default();
        // written the way a crate WITHOUT an eink-kit dependency would
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(HINTS_KEY), vec![(egui::Rect::EVERYTHING, 1u8), (egui::Rect::NOTHING, 9u8)]));
        hint(&ctx, egui::Rect::NOTHING, Hint::Static);
        let h = take_hints(&ctx);
        assert_eq!(h.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Hint::Video, Hint::Static]);
        assert!(take_hints(&ctx).is_empty(), "consumed per frame");
    }
}
