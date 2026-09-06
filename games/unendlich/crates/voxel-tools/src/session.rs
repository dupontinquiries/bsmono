//! A build that takes minutes: dozens of rounds, and a marker that
//! says what is happening.
//!
//! Structures at the scale of a thousand blocks are not one reply. A
//! map-builder will happily leave a village running for fifteen
//! minutes; what they will not accept is fifteen minutes of nothing
//! on screen, and a failure discovered at the end.
//!
//! THE SESSION
//!
//! `Run` is one long build: a request, then rounds. Each round is a
//! model turn (plan, or review of what stands) followed by a dry run.
//! It records what changed, what it measures, and how far it is from
//! its spec, and it STOPS when it should:
//!
//!   * the model said `done`
//!   * `stall_rounds` in a row changed nothing
//!   * the distance to the spec stopped improving for `patience` rounds
//!   * the round limit, or the wall-clock limit
//!   * a round failed and `fail_stop` failures have accumulated
//!
//! Convergence matters more the longer the loop: forty rounds of a
//! model that has stopped having ideas is forty minutes of GPU for
//! nothing.
//!
//! THE MARKER
//!
//! `status()` is one line for the waypoint in the world -- "round 7/30:
//! 812 blocks, closing (0.42 -> 0.31)" or "round 12: FAILED - unknown
//! form 'zigurat'" -- and `blob()` is the shape so far as a coarse box
//! list the renderer can draw before a single block is committed. The
//! player sees the thing take form at the marker while the model works.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase { Planning, Reviewing, Converged, Failed, Stopped }

#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub n: usize,
    /// What the model said, in a word or two.
    pub verdict: String,
    pub blocks: usize,
    pub ops: usize,
    /// Distance to the spec, when there is one.
    pub distance: Option<f32>,
    pub changed: bool,
    pub error: Option<String>,
    pub took: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub max_rounds: usize,
    pub wall: Option<Duration>,
    /// Rounds with no change before stopping.
    pub stall_rounds: usize,
    /// Rounds without improving the distance before stopping.
    pub patience: usize,
    /// Failures before stopping.
    pub fail_stop: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_rounds: 24, wall: Some(Duration::from_secs(900)), stall_rounds: 2, patience: 6, fail_stop: 3 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub prose: String,
    pub limits: Limits,
    pub rounds: Vec<Round>,
    pub phase: Phase,
    /// Best distance seen, and the round that saw it.
    pub best: Option<(f32, usize)>,
    pub started: Instant,
    /// The build's box, as it stands.
    pub bounds: Option<((i32, i32, i32), (i32, i32, i32))>,
    /// A coarse occupancy of the current build for the marker: boxes
    /// in world coordinates, at most a few dozen.
    pub blob: Vec<((i32, i32, i32), (i32, i32, i32))>,
    failures: usize,
}

impl Run {
    pub fn new(prose: &str, limits: Limits) -> Self {
        Self { prose: prose.to_string(), limits, rounds: Vec::new(), phase: Phase::Planning,
               best: None, started: Instant::now(), bounds: None, blob: Vec::new(), failures: 0 }
    }

    pub fn round_no(&self) -> usize { self.rounds.len() }
    pub fn is_running(&self) -> bool { matches!(self.phase, Phase::Planning | Phase::Reviewing) }

    /// Record a round and decide whether to continue.
    pub fn record(&mut self, r: Round) -> Phase {
        let failed = r.error.is_some();
        if failed { self.failures += 1; } else { self.failures = 0; }
        if let Some(d) = r.distance {
            if self.best.map(|(b, _)| d < b - 1e-3).unwrap_or(true) { self.best = Some((d, r.n)); }
        }
        let changed = r.changed;
        let verdict_done = r.verdict.contains("done");
        self.rounds.push(r);
        self.phase = if verdict_done {
            Phase::Converged
        } else if self.failures >= self.limits.fail_stop {
            Phase::Failed
        } else if self.rounds.len() >= self.limits.max_rounds {
            Phase::Stopped
        } else if self.limits.wall.map(|w| self.started.elapsed() >= w).unwrap_or(false) {
            Phase::Stopped
        } else if !changed && self.trailing_unchanged() >= self.limits.stall_rounds {
            Phase::Converged
        } else if self.rounds_since_best() >= self.limits.patience {
            Phase::Converged
        } else {
            Phase::Reviewing
        };
        self.phase
    }

    fn trailing_unchanged(&self) -> usize {
        self.rounds.iter().rev().take_while(|r| !r.changed && r.error.is_none()).count()
    }

    fn rounds_since_best(&self) -> usize {
        match self.best {
            Some((_, at)) => self.rounds.len().saturating_sub(at),
            // no spec: patience never fires, stalling does
            None => 0,
        }
    }

    /// The line for the waypoint marker.
    pub fn status(&self) -> String {
        let last = self.rounds.last();
        if let Some(r) = last {
            if let Some(e) = &r.error {
                return format!("round {}: FAILED - {}", r.n, short(e, 60));
            }
        }
        let blocks = last.map(|r| r.blocks).unwrap_or(0);
        let secs = self.started.elapsed().as_secs();
        match self.phase {
            Phase::Planning => format!("planning ... {secs}s"),
            Phase::Reviewing => {
                let trend = match (self.best, last.and_then(|r| r.distance)) {
                    (Some((b, _)), Some(d)) if d <= b + 1e-3 => format!(", closing ({d:.2})"),
                    (Some((b, _)), Some(d)) => format!(", drifting ({d:.2} vs best {b:.2})"),
                    _ => String::new(),
                };
                format!("round {}/{}: {} blocks{} - {}s", self.round_no(), self.limits.max_rounds,
                        blocks, trend, secs)
            }
            Phase::Converged => format!("done after {} round(s): {} blocks, {}s - /commit",
                                        self.round_no(), blocks, secs),
            Phase::Failed => format!("stopped after {} failure(s) in {} round(s) - /plan to see",
                                     self.failures, self.round_no()),
            Phase::Stopped => format!("stopped at the limit: {} round(s), {} blocks - /commit",
                                      self.round_no(), blocks),
        }
    }

    /// The recent history for the chat log, newest last.
    pub fn log_lines(&self, n: usize) -> Vec<String> {
        self.rounds.iter().rev().take(n).rev().map(|r| {
            let d = r.distance.map(|d| format!(", spec {d:.2}")).unwrap_or_default();
            match &r.error {
                Some(e) => format!("  {}. FAILED: {}", r.n, short(e, 70)),
                None => format!("  {}. {} -> {} ops, {} blocks{}{}", r.n, short(&r.verdict, 40), r.ops,
                                r.blocks, d, if r.changed { "" } else { " (no change)" }),
            }
        }).collect()
    }
}

fn short(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n { s } else { format!("{}...", s.chars().take(n - 3).collect::<String>()) }
}

/// A coarse occupancy of a cell set: the world divided into `grid`
/// cubes, each cube that holds anything emitted as a box. Cheap for
/// the renderer and enough to read a shape from across a valley.
pub fn blob(cells: &[(i32, i32, i32)], grid: i32, max_boxes: usize) -> Vec<((i32, i32, i32), (i32, i32, i32))> {
    if cells.is_empty() { return Vec::new(); }
    let g = grid.max(1);
    let mut seen: std::collections::BTreeSet<(i32, i32, i32)> = Default::default();
    for c in cells { seen.insert((c.0.div_euclid(g), c.1.div_euclid(g), c.2.div_euclid(g))); }
    // too many: coarsen until it fits, rather than truncate -- half a
    // shape is worse than a blocky whole one
    let mut g = g;
    while seen.len() > max_boxes {
        g *= 2;
        let coarse: std::collections::BTreeSet<(i32, i32, i32)> =
            cells.iter().map(|c| (c.0.div_euclid(g), c.1.div_euclid(g), c.2.div_euclid(g))).collect();
        seen = coarse;
    }
    seen.into_iter().map(|(x, y, z)| ((x * g, y * g, z * g), (x * g + g - 1, y * g + g - 1, z * g + g - 1))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(n: usize, blocks: usize, distance: Option<f32>, changed: bool) -> Round {
        Round { n, verdict: "drop 2; add 1".into(), blocks, ops: 4, distance, changed, error: None,
                took: Duration::from_secs(20) }
    }

    #[test]
    fn a_run_keeps_going_while_it_improves_and_converges_when_it_stops() {
        let mut r = Run::new("a village", Limits { max_rounds: 30, patience: 3, ..Default::default() });
        for (i, d) in [0.9, 0.7, 0.5, 0.4].iter().enumerate() {
            assert_eq!(r.record(round(i + 1, 400 + i * 100, Some(*d), true)), Phase::Reviewing);
        }
        assert_eq!(r.best, Some((0.4, 4)));
        // three rounds with no improvement: patience runs out
        for i in 5..7 { assert_eq!(r.record(round(i, 800, Some(0.45), true)), Phase::Reviewing); }
        assert_eq!(r.record(round(7, 800, Some(0.44), true)), Phase::Converged);
        assert!(r.status().starts_with("done after 7 round(s)"), "{}", r.status());
    }

    #[test]
    fn done_stalling_failures_and_limits_each_stop_it() {
        let mut r = Run::new("x", Limits { max_rounds: 30, stall_rounds: 2, patience: 99, fail_stop: 3, wall: None });
        r.record(round(1, 100, None, true));
        let mut done = round(2, 100, None, false); done.verdict = "done".into();
        assert_eq!(r.record(done), Phase::Converged);

        let mut r = Run::new("x", Limits { max_rounds: 30, stall_rounds: 2, patience: 99, fail_stop: 3, wall: None });
        r.record(round(1, 100, None, true));
        assert_eq!(r.record(round(2, 100, None, false)), Phase::Reviewing);
        assert_eq!(r.record(round(3, 100, None, false)), Phase::Converged, "two unchanged in a row");

        let mut r = Run::new("x", Limits { max_rounds: 30, stall_rounds: 9, patience: 99, fail_stop: 2, wall: None });
        let mut f = round(1, 0, None, false); f.error = Some("unknown form 'zigurat'".into());
        r.record(f.clone());
        f.n = 2;
        assert_eq!(r.record(f), Phase::Failed);
        assert!(r.status().contains("FAILED - unknown form 'zigurat'"), "{}", r.status());

        let mut r = Run::new("x", Limits { max_rounds: 2, stall_rounds: 9, patience: 99, fail_stop: 9, wall: None });
        r.record(round(1, 100, None, true));
        assert_eq!(r.record(round(2, 200, None, true)), Phase::Stopped);
        assert!(r.status().contains("stopped at the limit"));
    }

    #[test]
    fn a_failure_recovers_when_the_next_round_works() {
        let mut r = Run::new("x", Limits { fail_stop: 2, stall_rounds: 9, patience: 99, wall: None, max_rounds: 30 });
        let mut f = round(1, 0, None, false); f.error = Some("bad line".into());
        r.record(f);
        assert_eq!(r.record(round(2, 300, None, true)), Phase::Reviewing, "one good round clears the count");
        let mut f = round(3, 0, None, false); f.error = Some("bad line".into());
        assert_eq!(r.record(f), Phase::Reviewing, "the counter restarted");
    }

    #[test]
    fn the_status_line_reads_as_progress_and_the_log_as_history() {
        let mut r = Run::new("a village", Limits::default());
        assert!(r.status().starts_with("planning ..."));
        r.record(round(1, 400, Some(0.9), true));
        let s = r.status();
        assert!(s.starts_with("round 1/24: 400 blocks, closing (0.90)"), "{s}");
        r.record(round(2, 500, Some(1.2), true));
        assert!(r.status().contains("drifting (1.20 vs best 0.90)"), "{}", r.status());
        let lines = r.log_lines(5);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("1. drop 2; add 1 -> 4 ops, 400 blocks, spec 0.90"), "{:?}", lines[0]);
    }

    #[test]
    fn the_blob_is_coarse_and_never_truncated() {
        let mut cells = Vec::new();
        for x in 0..40 { for y in 0..20 { for z in 0..40 { if x % 7 == 0 || y == 0 { cells.push((x, y, z)); } } } }
        let b = blob(&cells, 4, 64);
        assert!(!b.is_empty() && b.len() <= 64, "{} boxes", b.len());
        // every cell is inside some box: coarsened, not dropped
        for c in cells.iter().step_by(97) {
            assert!(b.iter().any(|(lo, hi)| c.0 >= lo.0 && c.0 <= hi.0 && c.1 >= lo.1 && c.1 <= hi.1 && c.2 >= lo.2 && c.2 <= hi.2),
                    "{c:?} is in no box");
        }
        assert!(blob(&[], 4, 64).is_empty());
        // a tight budget coarsens rather than truncating
        let tiny = blob(&cells, 4, 8);
        assert!(tiny.len() <= 8);
        let (lo, hi) = (tiny.iter().map(|(l, _)| l.0).min().unwrap(), tiny.iter().map(|(_, h)| h.0).max().unwrap());
        assert!(lo <= 0 && hi >= 39, "still spans the build: {lo}..{hi}");
    }
}
