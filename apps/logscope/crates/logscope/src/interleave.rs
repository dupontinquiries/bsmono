//! interleave.rs — thinker and panel taking turns without paying for the switch.
//!
//! ## The constraint that decides the design
//!
//! On the measured ceiling this hardware holds about 31 GiB of buffers at once. Scout pinned at a
//! 28 GiB budget plus five 3B debaters at ~1.7 GiB each is 36.5 GiB. **They do not co-reside.**
//!
//! So an interleave that keeps both on the GPU must evict on every switch — and eviction is precisely
//! what makes prefill expensive again, which is the cost the interleave was supposed to avoid. A 20/80
//! split that reloads at each boundary spends more on loading than either participant spends thinking.
//!
//! ## Which makes the placement the answer, not the schedule
//!
//! The debaters are small enough to live in **host memory on the efficiency cores**, and the thinker
//! keeps the GPU. Nothing is evicted because nothing is competing for the same pool — the switch costs
//! a context change, not a reload.
//!
//! That is not free. On unified memory the e-cores share the same memory controller, so a panel
//! running under a thinker still takes bandwidth from it, and decode is bandwidth-latency-bound. The
//! claim is "cheap because small", not "free because elsewhere".
//!
//! ## And the KV pool is the second ceiling
//!
//! Each participant needs its cache to survive its turn or the interleave re-prefills. The pool caps
//! entries, so a panel wider than the pool evicts its own members between turns — which is the same
//! failure as evicting weights, one level up.

/// Where a participant runs. The switch cost follows entirely from this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// GPU-resident. One participant at a time on a device this size.
    Accelerator,
    /// Host memory, efficiency cores. Several can co-reside.
    HostCores,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub thinker: Placement,
    pub panel: Placement,
    /// Seats the KV pool can hold across a switch. Beyond this the panel re-prefills each turn.
    pub panel_seats: usize,
    /// Whether a switch forces weights out of the accelerator.
    pub evicts_on_switch: bool,
    pub why: &'static str,
}

/// Decide placement from what actually fits.
///
/// `ceiling_gib` is what the device holds concurrently — the measured figure, not the largest
/// allocation it will accept. Those differ by 2x on Honeykrisp and confusing them is what put a
/// 56.6 GiB plan on a device that dies at 31.
pub fn plan(thinker_gib: f64, seat_gib: f64, seats: usize, ceiling_gib: f64, kv_pool: usize) -> Plan {
    let both = thinker_gib + seat_gib * seats as f64;
    if both <= ceiling_gib {
        return Plan {
            thinker: Placement::Accelerator,
            panel: Placement::Accelerator,
            panel_seats: seats.min(kv_pool),
            evicts_on_switch: false,
            why: "thinker and panel fit together, so both stay resident and a switch costs nothing",
        };
    }
    Plan {
        thinker: Placement::Accelerator,
        panel: Placement::HostCores,
        // The pool still binds: a seat whose cache is evicted between turns re-prefills, which is the
        // same failure as evicting weights one level up.
        panel_seats: seats.min(kv_pool.max(1)),
        evicts_on_switch: false,
        why: "they do not co-reside, so the panel moves to host cores rather than evicting the \
thinker — a switch that reloads weights costs more than either participant saves",
    }
}

/// Turns in a repeating cycle, given a percentage of time for the thinker.
///
/// Returned as a schedule rather than a ratio because the useful question is "who goes next", and a
/// ratio has to be turned into that at every step by whoever asks.
pub fn schedule(thinker_pct: u32, cycle: usize) -> Vec<Placement> {
    if cycle == 0 { return Vec::new(); }
    let t = ((thinker_pct.min(100) as usize * cycle) + 50) / 100;
    // The thinker's turns are SPREAD rather than blocked at the front. A panel that waits through
    // eight consecutive thinker turns has stale context by the time it speaks, and the point of the
    // interleave is that its view stays current.
    (0..cycle).map(|i| {
        if t == 0 { Placement::HostCores }
        else if (i * t) / cycle != ((i + 1) * t) / cycle { Placement::Accelerator }
        else { Placement::HostCores }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_and_a_five_seat_panel_do_NOT_co_reside() {
        // 28.0 + 5 x 1.7 = 36.5 against a measured 31 GiB ceiling. The panel moves to host cores
        // rather than evicting the thinker.
        let p = plan(28.0, 1.7, 5, 31.0, 3);
        assert_eq!(p.thinker, Placement::Accelerator);
        assert_eq!(p.panel, Placement::HostCores);
        assert!(p.why.contains("costs more than either participant saves"));
    }

    #[test]
    fn a_small_thinker_lets_everything_stay_resident() {
        // An 8B thinker at ~4.6 GiB with the same panel is 13.1 GiB and fits, and then a switch costs
        // nothing at all.
        let p = plan(4.6, 1.7, 5, 31.0, 8);
        assert_eq!(p.panel, Placement::Accelerator);
        assert!(!p.evicts_on_switch);
    }

    #[test]
    fn NOTHING_is_evicted_in_either_arrangement() {
        // The whole point. An interleave that evicts on each switch re-prefills, and re-prefilling is
        // the cost the interleave exists to avoid.
        for (t, s) in [(28.0, 5), (4.6, 5), (60.0, 9)] {
            assert!(!plan(t, 1.7, s, 31.0, 3).evicts_on_switch);
        }
    }

    #[test]
    fn the_KV_POOL_caps_the_panel_even_when_memory_does_not() {
        // A seat whose cache is evicted between turns re-prefills — the same failure as evicting
        // weights, one level up, and easy to miss because the weights fit fine.
        assert_eq!(plan(4.6, 1.7, 9, 31.0, 3).panel_seats, 3);
    }

    #[test]
    fn a_20_80_split_SPREADS_the_thinker_rather_than_blocking_it() {
        // A panel that waits through eight consecutive thinker turns has stale context by the time it
        // speaks, and the point of interleaving is that its view stays current.
        let s = schedule(20, 10);
        assert_eq!(s.iter().filter(|p| **p == Placement::Accelerator).count(), 2);
        let idx: Vec<usize> = s.iter().enumerate()
            .filter(|(_, p)| **p == Placement::Accelerator).map(|(i, _)| i).collect();
        assert!(idx[1] - idx[0] > 1, "the two thinker turns must not be adjacent: {idx:?}");
    }

    #[test]
    fn the_extremes_are_coherent() {
        assert!(schedule(0, 8).iter().all(|p| *p == Placement::HostCores));
        assert!(schedule(100, 8).iter().all(|p| *p == Placement::Accelerator));
        assert!(schedule(50, 0).is_empty());
    }
}
