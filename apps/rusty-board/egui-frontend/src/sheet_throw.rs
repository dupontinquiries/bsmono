//! sheet_throw — a new board is thrown onto the keyboard like a sheet.
//!
//! Switching boards used to swap every key in one frame, which reads as a
//! glitch more than a change. Now the new board's keys slide in from the
//! layout rail's side and land in place with a small overshoot, each key a
//! little after the one before (row by row, then left to right), so the
//! sheet visibly SPREADS onto the board rather than appearing.
//!
//! Pure: an offset per key for a progress `t` in [0, 1]. The renderer adds it
//! to the key's x; nothing here knows about egui.

/// Whole animation, first key leaving to last key settled.
pub const THROW_MS: f32 = 340.0;
/// Delay per row and per column, as fractions of the animation.
const ROW_DELAY: f32 = 0.07;
const COL_DELAY: f32 = 0.02;
/// Each key's own flight, as a fraction of the animation. Delays top out
/// around 0.5 for a 5-row, 12-column board; `KEY_SPAN` is what is left.
const KEY_SPAN: f32 = 0.5;
/// ease-out-back's overshoot: how far past its place a key lands before
/// settling. Small, or it reads as a bounce instead of a throw.
const OVERSHOOT: f32 = 1.3;

fn ease_out_back(u: f32) -> f32 {
    let c3 = OVERSHOOT + 1.0;
    let v = u - 1.0;
    1.0 + c3 * v * v * v + OVERSHOOT * v * v
}

/// How far LEFT of its resting place a key is drawn at progress `t`, for a
/// throw that starts `distance` points away. Negative briefly (the
/// overshoot), exactly zero once the key has landed.
pub fn key_offset(t: f32, row: usize, col: usize, distance: f32) -> f32 {
    if t >= 1.0 {
        return 0.0;
    }
    let delay = (row as f32 * ROW_DELAY + col as f32 * COL_DELAY).min(1.0 - KEY_SPAN);
    let u = ((t - delay) / KEY_SPAN).clamp(0.0, 1.0);
    (1.0 - ease_out_back(u)) * distance
}

/// Progress for `elapsed_ms` since the switch.
pub fn progress(elapsed_ms: f32) -> f32 {
    (elapsed_ms / THROW_MS).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_starts_off_the_board_and_lands_exactly_in_place() {
        for (r, c) in [(0, 0), (0, 11), (4, 0), (4, 11)] {
            assert_eq!(key_offset(0.0, r, c, 300.0), 300.0, "({r},{c}) starts a full throw away");
            assert_eq!(key_offset(1.0, r, c, 300.0), 0.0, "({r},{c}) lands");
            assert!(key_offset(0.999, r, c, 300.0).abs() < 1.0, "({r},{c}) has settled by the end");
        }
    }

    #[test]
    fn the_sheet_spreads_row_by_row_then_left_to_right() {
        let t = 0.3;
        assert!(key_offset(t, 0, 0, 300.0) < key_offset(t, 0, 5, 300.0), "left of a row lands first");
        assert!(key_offset(t, 0, 0, 300.0) < key_offset(t, 2, 0, 300.0), "the top row lands first");
    }

    #[test]
    fn it_overshoots_a_little_and_no_more() {
        let min = (0..=100).map(|i| key_offset(i as f32 / 100.0, 0, 0, 300.0)).fold(f32::MAX, f32::min);
        assert!(min < 0.0, "a throw lands past its place before settling");
        assert!(min > -0.15 * 300.0, "a throw, not a bounce: {min}");
    }
}
