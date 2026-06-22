//! Tick cadence — not every creature needs to *think* every frame. The steady
//! cost of the ecosystem is dominated by per-creature surface probing (terrain
//! steering, posture, foraging), so a hundred distant animals re-deciding their
//! heading 60×/s is wasted work no one can see. This schedules coarser thinking
//! with distance and **staggers it by index** so a far-off cohort doesn't all
//! think on the same frame (which would just move the spike, not remove it).
//!
//! Pure scheduling. The bin asks [`tick_period`] for a creature's cadence, then
//! [`should_tick`]; on the frames it does run, it advances with a `dt` scaled by
//! the period so motion stays time-consistent (a creature thinking 1/4 as often
//! takes 4× the step). Near creatures always run, so nothing visible stutters.

/// How many frames between thinks for a creature at squared distance `dist2`
/// from the viewer. `near`/`far` are radii (not squared). Inside `near`: every
/// frame. Between `near` and `far`: every 4th. Beyond `far`: every 16th.
pub fn tick_period(dist2: f32, near: f32, far: f32) -> u32 {
    if dist2 <= near * near {
        1
    } else if dist2 <= far * far {
        4
    } else {
        16
    }
}

/// Does creature `index` think on global frame `tick`, given its `period`?
/// Period 1 always runs. Otherwise it runs once per `period` frames, offset by
/// `index` so members of the same cadence band are spread across the cycle
/// instead of bunching onto one frame.
pub fn should_tick(index: usize, tick: u32, period: u32) -> bool {
    if period <= 1 {
        return true;
    }
    (tick.wrapping_add(index as u32)) % period == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_grows_with_distance() {
        assert_eq!(tick_period(0.0, 32.0, 96.0), 1);
        assert_eq!(tick_period(50.0 * 50.0, 32.0, 96.0), 4);
        assert_eq!(tick_period(200.0 * 200.0, 32.0, 96.0), 16);
    }

    #[test]
    fn near_creatures_always_think() {
        for tick in 0..100u32 {
            assert!(should_tick(7, tick, 1), "period 1 runs every frame");
        }
    }

    #[test]
    fn far_creatures_think_once_per_period() {
        let fires = (0..64u32).filter(|&t| should_tick(0, t, 4)).count();
        assert_eq!(fires, 16, "period 4 over 64 frames fires 16 times");
    }

    #[test]
    fn cohort_is_staggered_across_frames() {
        // A band of 4 creatures at period 4: on any given frame exactly one of
        // them thinks — load spread evenly, no synchronized spike.
        for tick in 0..20u32 {
            let firing = (0..4usize).filter(|&i| should_tick(i, tick, 4)).count();
            assert_eq!(firing, 1, "exactly one of the cohort thinks each frame");
        }
    }
}
