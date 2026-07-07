//! sim::player_rank — Resonance Rank progression from lifetime chi.
//!
//! From `character_panel_reference §2/§7`: the header shows **Rank** (Resonance
//! Rank — aggregate bending mastery across all disciplines) and an XP bar toward
//! the next rank, both derived from **lifetime chi expenditure** (the ⚔ value —
//! "a measure of bending experience depth"). This is the pure mapping between
//! total chi spent and rank + progress. It's separate from the ♦ Resonance
//! Rating (gear score — see `sim::resonance`) and from Season (world-time index —
//! see `season`); Rank is earned by *doing* bending, so it's a monotone function
//! of cumulative chi. Deterministic, std-only.
//!
//! Curve: advancing from rank `r` to `r+1` costs `RANK_BASE + r*RANK_STEP` chi,
//! so ranks get gradually more expensive. (The doc's exact figures — rank 164 at
//! 18,051 chi — are illustrative; these constants are tunable.)

/// Chi to advance from rank 0 to rank 1.
pub const RANK_BASE: u64 = 100;
/// Extra chi each successive rank costs over the previous.
pub const RANK_STEP: u64 = 1;

/// Chi cost to advance *from* `rank` to `rank + 1`.
pub fn chi_to_advance_from(rank: u32) -> u64 {
    RANK_BASE + rank as u64 * RANK_STEP
}

/// Total lifetime chi required to *reach* `rank` from zero.
/// `Σ_{r=0}^{rank-1} (RANK_BASE + r*RANK_STEP)`.
pub fn cumulative_chi_for_rank(rank: u32) -> u64 {
    let r = rank as u64;
    RANK_BASE * r + RANK_STEP * (r.saturating_sub(1)) * r / 2
}

/// The rank a player with `total_chi` lifetime chi has reached.
pub fn rank_for_chi(total_chi: u64) -> u32 {
    // Invert the quadratic cumulative(r) = (STEP/2)r² + (BASE - STEP/2)r.
    // Estimate with f64, then correct exactly against the integer cumulative so
    // rounding can never place us in the wrong rank.
    let step = RANK_STEP as f64;
    let base = RANK_BASE as f64;
    let c = total_chi as f64;
    let a = step / 2.0;
    let b = base - step / 2.0;
    let mut r = if a > 0.0 {
        ((-b + (b * b + 4.0 * a * c).sqrt()) / (2.0 * a)).floor() as i64
    } else {
        (c / base).floor() as i64
    };
    if r < 0 {
        r = 0;
    }
    let mut rank = r as u32;
    // Correct downward if the estimate overshot, upward if it undershot.
    while rank > 0 && cumulative_chi_for_rank(rank) > total_chi {
        rank -= 1;
    }
    while cumulative_chi_for_rank(rank + 1) <= total_chi {
        rank += 1;
    }
    rank
}

/// Chi still needed to reach the next rank.
pub fn chi_to_next(total_chi: u64) -> u64 {
    let rank = rank_for_chi(total_chi);
    cumulative_chi_for_rank(rank + 1).saturating_sub(total_chi)
}

/// XP-bar fill toward the next rank, in `[0, 1)`.
pub fn progress_to_next(total_chi: u64) -> f32 {
    let rank = rank_for_chi(total_chi);
    let floor = cumulative_chi_for_rank(rank);
    let span = chi_to_advance_from(rank);
    if span == 0 {
        return 0.0;
    }
    ((total_chi - floor) as f32 / span as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_zero_at_no_chi_and_grows_monotonically() {
        assert_eq!(rank_for_chi(0), 0);
        let mut prev = 0;
        for chi in (0..50_000).step_by(777) {
            let r = rank_for_chi(chi);
            assert!(r >= prev, "rank must not decrease as chi rises");
            prev = r;
        }
    }

    #[test]
    fn cumulative_and_rank_are_inverse() {
        for rank in [0u32, 1, 5, 50, 164, 1000] {
            let floor = cumulative_chi_for_rank(rank);
            // exactly at the threshold ⇒ that rank
            assert_eq!(rank_for_chi(floor), rank, "at floor of rank {rank}");
            // one below the *next* threshold ⇒ still this rank
            let next = cumulative_chi_for_rank(rank + 1);
            assert_eq!(rank_for_chi(next - 1), rank, "just under rank {}", rank + 1);
        }
    }

    #[test]
    fn first_rank_costs_the_base() {
        assert_eq!(cumulative_chi_for_rank(0), 0);
        assert_eq!(cumulative_chi_for_rank(1), RANK_BASE);
        assert_eq!(rank_for_chi(RANK_BASE - 1), 0);
        assert_eq!(rank_for_chi(RANK_BASE), 1);
    }

    #[test]
    fn ranks_get_more_expensive() {
        assert!(chi_to_advance_from(200) > chi_to_advance_from(0));
    }

    #[test]
    fn progress_bar_spans_zero_to_one() {
        // Just after hitting a rank, the bar is near empty.
        let floor = cumulative_chi_for_rank(10);
        assert!(progress_to_next(floor) < 0.02);
        // Near the next threshold, near full.
        let span = chi_to_advance_from(10);
        assert!(progress_to_next(floor + span - 1) > 0.9);
        // Always in range.
        for chi in (0..20_000).step_by(333) {
            let p = progress_to_next(chi);
            assert!((0.0..=1.0).contains(&p));
        }
    }

    #[test]
    fn chi_to_next_reaches_the_threshold() {
        let chi = 5_000;
        let rank = rank_for_chi(chi);
        assert_eq!(chi + chi_to_next(chi), cumulative_chi_for_rank(rank + 1));
    }

    #[test]
    fn deterministic() {
        assert_eq!(rank_for_chi(18_051), rank_for_chi(18_051));
        assert_eq!(progress_to_next(18_051), progress_to_next(18_051));
    }
}
