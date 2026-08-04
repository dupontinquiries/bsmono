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

/// Time and tokens are not the same split, and they diverge by more than an order of magnitude.
///
/// Five 3B seats on host cores generate concurrently. At ~4 tok/s each that is **20 tok/s aggregate**
/// against a streaming Scout's measured **1.4 tok/s** — a 14× ratio. So a schedule expressed in TIME
/// says something very different from one expressed in TOKENS:
///
/// ```text
/// thinker 20% of TIME  ->   1.7% of tokens
/// thinker 50% of TIME  ->   6.5% of tokens
/// thinker 80% of TIME  ->  21.9% of tokens
/// ```
///
/// **A 20/80 time split gives the thinker under two percent of the output.** If "20/80" was meant as
/// a share of what gets written, the time split has to be almost inverted to reach it.
///
/// Which is the right axis depends on what is scarce. The panel produces most of the tokens; the
/// thinker produces the ones that hold state across steps — Scout tracked the mode instructions where
/// a 3B did not. So the useful quantity is neither raw tokens nor raw seconds, and choosing a split
/// without saying which is being divided is how a plan silently becomes its opposite.
pub struct Throughput {
    pub thinker_tok_s: f32,
    pub seat_tok_s: f32,
    pub seats: usize,
}

impl Throughput {
    pub fn panel_tok_s(&self) -> f32 { self.seat_tok_s * self.seats as f32 }

    /// Aggregate tokens per second at a given share of TIME on the thinker.
    pub fn blended_tok_s(&self, thinker_time_pct: u32) -> f32 {
        let t = thinker_time_pct.min(100) as f32 / 100.0;
        t * self.thinker_tok_s + (1.0 - t) * self.panel_tok_s()
    }

    /// What share of the OUTPUT the thinker contributes, given its share of time.
    pub fn thinker_token_share(&self, thinker_time_pct: u32) -> f32 {
        let t = thinker_time_pct.min(100) as f32 / 100.0;
        let tt = t * self.thinker_tok_s;
        let total = self.blended_tok_s(thinker_time_pct);
        if total <= 0.0 { 0.0 } else { tt / total }
    }

    /// The share of TIME that gives the thinker a target share of TOKENS.
    ///
    /// This is the conversion the caller actually wants and cannot do in their head: at a 14× ratio,
    /// asking for a fifth of the output means giving the thinker most of the clock.
    pub fn time_for_token_share(&self, target_token_share: f32) -> u32 {
        let s = target_token_share.clamp(0.0, 1.0);
        let (a, p) = (self.thinker_tok_s, self.panel_tok_s());
        if a <= 0.0 { return 0; }
        // t*a / (t*a + (1-t)*p) = s   →   t = s*p / (a - s*a + s*p)
        let denom = a - s * a + s * p;
        if denom <= 0.0 { return 100; }
        (((s * p / denom) * 100.0).round() as i64).clamp(0, 100) as u32
    }
}

#[cfg(test)]
mod throughput_tests {
    use super::*;

    fn measured() -> Throughput {
        // Scout streaming, measured. Five 3B seats at a conservative 4 tok/s each on host cores.
        Throughput { thinker_tok_s: 1.4, seat_tok_s: 4.0, seats: 5 }
    }

    #[test]
    fn five_seats_out_produce_the_thinker_by_an_ORDER_OF_MAGNITUDE() {
        let t = measured();
        assert!(t.panel_tok_s() / t.thinker_tok_s > 10.0,
            "{} vs {}", t.panel_tok_s(), t.thinker_tok_s);
    }

    #[test]
    fn a_20_80_TIME_split_gives_the_thinker_under_two_percent_of_the_TOKENS() {
        // The number that makes the distinction worth encoding. "20/80" sounds like a fifth and buys
        // a fiftieth.
        let s = measured().thinker_token_share(20);
        assert!(s < 0.02, "thinker token share at 20% time = {s:.3}");
    }

    #[test]
    fn reaching_a_fifth_of_the_TOKENS_needs_most_of_the_CLOCK() {
        // The conversion a caller cannot do in their head, and the reason a split must say which axis
        // it divides.
        let pct = measured().time_for_token_share(0.20);
        assert!(pct > 70, "needs {pct}% of time to get 20% of tokens");
        // And it round-trips.
        let back = measured().thinker_token_share(pct);
        assert!((back - 0.20).abs() < 0.03, "{back:.3}");
    }

    #[test]
    fn more_TIME_on_the_thinker_lowers_aggregate_throughput() {
        // Worth stating plainly: every second given to the thinker costs about fourteen tokens. That
        // is the price of the tokens that hold state, not an inefficiency to tune away.
        let t = measured();
        assert!(t.blended_tok_s(80) < t.blended_tok_s(20));
    }

    #[test]
    fn a_SINGLE_seat_panel_barely_moves_the_split() {
        // The divergence is a property of the panel's WIDTH, not of small models. One 3B against
        // Scout is under 3x, and time and tokens stay comparable.
        let one = Throughput { thinker_tok_s: 1.4, seat_tok_s: 4.0, seats: 1 };
        assert!(one.thinker_token_share(50) > 0.20, "{}", one.thinker_token_share(50));
    }

    #[test]
    fn a_zero_rate_thinker_does_not_divide_by_zero() {
        let t = Throughput { thinker_tok_s: 0.0, seat_tok_s: 4.0, seats: 5 };
        assert_eq!(t.time_for_token_share(0.5), 0);
        assert_eq!(t.thinker_token_share(50), 0.0);
    }
}

/// The fraction of TIME the thinker needs to hold a given share of TOKENS.
///
/// The two are not close, and conflating them is the trap this function exists to remove. With Scout
/// at 1.4 tok/s and five 3B seats aggregating ~20 tok/s:
///
/// ```text
/// 10% of tokens  needs  61% of time   ->  8.6 tok/s total, 6.1x thinker-only
/// 20% of tokens  needs  78% of time   ->  5.5 tok/s total, 3.9x
/// 30% of tokens  needs  86% of time   ->  4.0 tok/s total, 2.9x
/// ```
///
/// A "20/80 split" invites the reading that the thinker writes a fifth of the output. At a 20% time
/// share it writes **1.7%** of it. Operators think in token shares; schedulers run on time; this is
/// the conversion, and taking the floor as the input is what makes the knob mean what it says.
///
/// ## Why it takes measured rates rather than constants
///
/// Over a long run the rates move — a model swap, a warm cache, a kernel lever finally landing. Left
/// as a fixed split, an overnight job holds a time ratio chosen for numbers that stopped being true
/// hours ago. Re-solving from current rates keeps the FLOOR fixed instead, which is the thing worth
/// holding.
pub fn time_for_token_floor(floor: f64, thinker_tok_s: f64, panel_tok_s: f64) -> f64 {
    let floor = floor.clamp(0.0, 1.0);
    if thinker_tok_s <= 0.0 { return 0.0; }
    // A panel no faster than the thinker cannot dilute it, so any time share already clears the
    // floor and the interleave is about diversity rather than throughput.
    if panel_tok_s <= thinker_tok_s { return floor; }
    let denom = thinker_tok_s * (1.0 - floor) + floor * panel_tok_s;
    if denom <= 0.0 { return 1.0 }
    (floor * panel_tok_s / denom).clamp(0.0, 1.0)
}

/// Aggregate throughput at a given time split, for comparing against thinker-only.
pub fn blended_tok_s(time_on_thinker: f64, thinker_tok_s: f64, panel_tok_s: f64) -> f64 {
    let t = time_on_thinker.clamp(0.0, 1.0);
    t * thinker_tok_s + (1.0 - t) * panel_tok_s
}

/// How good a panel token must be, relative to a thinker token, for the mix to beat thinker-only.
///
/// **This model only applies when panel output is SEPARABLE** — its own drafts, summaries or
/// expansions, where a bad one is discarded and costs its own time. Inline in a shared artifact a
/// wrong token costs the rework rather than nothing, and no ratio makes that back.
///
/// Which is why `sketchexpand`'s rule matters more than it looks: the thinker sets the spec and the
/// panel expands within it. That bound, not the panel's quality, is what keeps its tokens above the
/// break-even.
pub fn breakeven_token_value(time_on_thinker: f64, thinker_tok_s: f64, panel_tok_s: f64) -> f64 {
    let t = time_on_thinker.clamp(0.0, 1.0);
    let panel_share = (1.0 - t) * panel_tok_s;
    if panel_share <= 0.0 { return f64::INFINITY; }
    ((thinker_tok_s - t * thinker_tok_s) / panel_share).max(0.0)
}

/// A schedule that holds a TOKEN floor, re-solved from the rates in force right now.
pub fn schedule_for_floor(floor: f64, thinker_tok_s: f64, panel_tok_s: f64, cycle: usize)
    -> Vec<Placement>
{
    let t = time_for_token_floor(floor, thinker_tok_s, panel_tok_s);
    schedule((t * 100.0).round() as u32, cycle)
}

#[cfg(test)]
mod floor_tests {
    use super::*;

    const SCOUT: f64 = 1.40;
    const PANEL: f64 = 20.0;

    #[test]
    fn a_10pct_TOKEN_floor_needs_61pct_of_TIME() {
        // The number that makes the whole distinction worth encoding: "10%" of output requires the
        // majority of the clock.
        let t = time_for_token_floor(0.10, SCOUT, PANEL);
        assert!((t - 0.613).abs() < 0.01, "got {t:.3}");
    }

    #[test]
    fn holding_the_floor_still_beats_thinker_only_by_SIX_times() {
        // The answer to "is it worth it": 6.1x throughput while the thinker still writes a tenth of
        // the tokens.
        let t = time_for_token_floor(0.10, SCOUT, PANEL);
        let blended = blended_tok_s(t, SCOUT, PANEL);
        assert!(blended / SCOUT > 6.0, "{:.1}x", blended / SCOUT);
    }

    #[test]
    fn the_solved_split_ACTUALLY_delivers_the_floor() {
        // Checked by recomputing the token share rather than trusting the algebra.
        for f in [0.05, 0.10, 0.25, 0.50] {
            let t = time_for_token_floor(f, SCOUT, PANEL);
            let share = (t * SCOUT) / blended_tok_s(t, SCOUT, PANEL);
            assert!((share - f).abs() < 0.01, "floor {f}: got {share:.3}");
        }
    }

    #[test]
    fn a_panel_no_faster_than_the_thinker_cannot_dilute_it() {
        // Then the interleave is about diversity rather than throughput, and any time share already
        // clears the floor — returning a huge time demand would be wrong.
        assert_eq!(time_for_token_floor(0.10, 5.0, 5.0), 0.10);
        assert_eq!(time_for_token_floor(0.10, 5.0, 2.0), 0.10);
    }

    #[test]
    fn a_panel_token_need_only_be_worth_SEVEN_PERCENT() {
        // Low bar — and it only applies when panel output is separable. Inline in a shared artifact a
        // wrong token costs the rework rather than nothing.
        let t = time_for_token_floor(0.10, SCOUT, PANEL);
        let q = breakeven_token_value(t, SCOUT, PANEL);
        assert!(q > 0.05 && q < 0.10, "got {q:.3}");
    }

    #[test]
    fn RE_SOLVING_moves_the_time_share_when_the_thinker_gets_faster() {
        // The long-horizon case. If a kernel lever lands and Scout triples, holding a fixed TIME split
        // would quietly raise the thinker's token share past what was asked for; re-solving keeps the
        // floor and gives the time back to the panel.
        let slow = time_for_token_floor(0.10, SCOUT, PANEL);
        let fast = time_for_token_floor(0.10, SCOUT * 3.0, PANEL);
        assert!(fast < slow, "faster thinker needs LESS time to hold the same share: {fast} vs {slow}");
    }

    #[test]
    fn a_floor_of_zero_or_one_is_coherent() {
        assert_eq!(time_for_token_floor(0.0, SCOUT, PANEL), 0.0);
        assert_eq!(time_for_token_floor(1.0, SCOUT, PANEL), 1.0);
        assert!(schedule_for_floor(0.10, SCOUT, PANEL, 10).len() == 10);
    }
}
