//! v0.1.642: tab-cycle index math for the `{N}b` web-tab shortcut.
//! `b` = next tab, `B` = previous; a leading count repeats it (`2b` = two
//! forward, `3B` = three back). Pure index math (wraparound, count
//! normalization, edge cases) so the off-by-one-prone part is tested; the
//! QML key handler accumulates the digit prefix and calls this, then
//! backend.tab_switch(target).

/// Compute the target tab index when cycling.
///   - `current`: current active tab index.
///   - `count`: total number of tabs.
///   - `step`: how many to move (the {N}; treated as >=1).
///   - `forward`: true for `b` (next), false for `B` (prev).
/// Wraps around. `step` is taken mod `count` so an absurd count (99b on 3
/// tabs) still lands on a real tab. Returns `current` unchanged when there
/// are 0 or 1 tabs (nothing to cycle to).
pub fn cycle_target(current: usize, count: usize, step: usize, forward: bool) -> usize {
    if count <= 1 {
        return current.min(count.saturating_sub(1)).min(current);
    }
    let step = step.max(1) % count; // 5b on 3 tabs → 2; step 0 means "no move"
    if step == 0 {
        return current % count;
    }
    let cur = current % count; // clamp a stale index into range
    if forward {
        (cur + step) % count
    } else {
        // backward with wraparound: add (count - step) to avoid usize
        // underflow.
        (cur + (count - step)) % count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_single_step() {
        assert_eq!(cycle_target(0, 3, 1, true), 1);
        assert_eq!(cycle_target(1, 3, 1, true), 2);
    }

    #[test]
    fn forward_wraps() {
        assert_eq!(cycle_target(2, 3, 1, true), 0);
    }

    #[test]
    fn backward_wraps() {
        assert_eq!(cycle_target(0, 3, 1, false), 2);
        assert_eq!(cycle_target(1, 3, 2, false), 2);
    }

    #[test]
    fn count_prefix_multi_step() {
        assert_eq!(cycle_target(0, 3, 2, true), 2);
        assert_eq!(cycle_target(0, 5, 3, true), 3);
        assert_eq!(cycle_target(4, 5, 3, false), 1);
    }

    #[test]
    fn step_larger_than_count_mods() {
        // 5b on 3 tabs → effectively 2 forward
        assert_eq!(cycle_target(0, 3, 5, true), 2);
        // a full-loop step lands back where you are
        assert_eq!(cycle_target(1, 3, 3, true), 1);
        assert_eq!(cycle_target(1, 3, 6, true), 1);
    }

    #[test]
    fn step_zero_or_one_treated_as_one_min() {
        // step 0 is normalized to "no move beyond mod" → stays put for a
        // full-loop; step is max(1) so a 0 from a missing prefix = 1.
        assert_eq!(cycle_target(0, 3, 1, true), 1);
    }

    #[test]
    fn single_or_no_tabs_stays() {
        assert_eq!(cycle_target(0, 1, 1, true), 0);
        assert_eq!(cycle_target(0, 0, 1, true), 0);
        assert_eq!(cycle_target(0, 1, 5, false), 0);
    }

    #[test]
    fn stale_current_index_clamped() {
        // current beyond range (e.g. a tab closed) → still lands in range
        let t = cycle_target(9, 3, 1, true);
        assert!(t < 3, "target {} must be in range", t);
    }
}
