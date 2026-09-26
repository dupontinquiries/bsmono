//! The display as presentation feedback describes it. Pure.

use std::collections::VecDeque;

/// One frame's fate, from `wp_presentation_feedback`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Presented(Presented),
    /// Replaced by a later commit before it reached the screen: a frame
    /// decoded, uploaded and composited for nothing.
    Discarded { target_ns: Option<u64> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presented {
    /// When the frame was meant to be seen, on the presentation clock.
    pub target_ns: Option<u64>,
    /// When it was (the compositor's best estimate of first light).
    pub at_ns: u64,
    /// The output's refresh period; 0 when the output has none (variable).
    pub refresh_ns: u32,
    /// The output's vblank counter; 0 when the compositor has none.
    pub seq: u64,
    /// Presentation was synchronised to the vblank (no tearing).
    pub vsync: bool,
    /// The buffer was scanned out directly, without composition: the
    /// cheapest way a frame can reach the screen.
    pub zero_copy: bool,
    /// The timestamp came from the display hardware (a real vblank), not a
    /// software timer -- headless and nested compositors have none.
    pub hw_clock: bool,
}

/// A rolling window of outcomes, and what they add up to.
#[derive(Debug)]
pub struct PresentLog {
    window: VecDeque<Outcome>,
    cap: usize,
    pub presented: u64,
    pub discarded: u64,
}

impl PresentLog {
    pub fn new(cap: usize) -> Self {
        Self { window: VecDeque::with_capacity(cap), cap: cap.max(2), presented: 0, discarded: 0 }
    }

    pub fn push(&mut self, o: Outcome) {
        match o {
            Outcome::Presented(_) => self.presented += 1,
            Outcome::Discarded { .. } => self.discarded += 1,
        }
        if self.window.len() == self.cap {
            self.window.pop_front();
        }
        self.window.push_back(o);
    }

    fn shown(&self) -> impl Iterator<Item = &Presented> {
        self.window.iter().filter_map(|o| match o {
            Outcome::Presented(p) => Some(p),
            _ => None,
        })
    }

    /// The output's fixed refresh period, if it reports one: the median of
    /// the non-zero reports (an output can change mode mid-window).
    pub fn refresh_ns(&self) -> Option<u32> {
        let mut r: Vec<u32> = self.shown().map(|p| p.refresh_ns).filter(|&r| r > 0).collect();
        if r.is_empty() {
            return None;
        }
        r.sort_unstable();
        Some(r[r.len() / 2])
    }

    /// How many refreshes each shown frame stayed on screen, from one
    /// presentation to the next: vblank counts when the compositor has them,
    /// else time over the refresh period. `None` for a gap that cannot be
    /// counted (no refresh period and no sequence).
    pub fn holds(&self) -> Vec<Option<u32>> {
        let refresh = self.refresh_ns();
        let v: Vec<&Presented> = self.shown().collect();
        v.windows(2)
            .map(|w| {
                if w[0].seq > 0 && w[1].seq > w[0].seq {
                    Some((w[1].seq - w[0].seq) as u32)
                } else {
                    let r = refresh? as f64;
                    Some(((w[1].at_ns.saturating_sub(w[0].at_ns)) as f64 / r).round() as u32)
                }
            })
            .collect()
    }

    /// Hold counts and how often each occurred, most common first.
    pub fn hold_histogram(&self) -> Vec<(u32, usize)> {
        let mut h: Vec<(u32, usize)> = Vec::new();
        for n in self.holds().into_iter().flatten() {
            match h.iter_mut().find(|e| e.0 == n) {
                Some(e) => e.1 += 1,
                None => h.push((n, 1)),
            }
        }
        h.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        h
    }

    /// Whether the output is presenting variable-rate. `None`: not enough
    /// evidence (fewer than 8 gaps, or software timestamps with no vblank
    /// counter -- a timer-driven compositor is off the grid by its own
    /// jitter, which says nothing about the display).
    ///
    /// With a vblank counter: under VRR each presentation is one refresh
    /// cycle of whatever length, so the counter says 1 where the time says
    /// several periods. Without one, hardware timestamps off the fixed grid
    /// by more than noise say the same. An output that reports no refresh
    /// at all is variable by the protocol's own definition. A content rate
    /// that divides the refresh (24 fps on 144 Hz) reads as fixed either
    /// way -- and is judder-free either way.
    pub fn variable(&self) -> Option<bool> {
        let v: Vec<&Presented> = self.shown().collect();
        if v.len() < 9 {
            return None;
        }
        let Some(r) = self.refresh_ns() else { return Some(true) };
        let r = r as f64;
        let gaps: Vec<(f64, u64)> = v.windows(2).map(|w| (w[1].at_ns.saturating_sub(w[0].at_ns) as f64, w[1].seq.wrapping_sub(w[0].seq))).collect();
        let off = if v.iter().all(|p| p.seq > 0) {
            // Time in periods against counted vblanks.
            gaps.iter().filter(|(d, n)| ((d / r).round() as i64 - *n as i64).abs() >= 1).count()
        } else if v.iter().all(|p| p.hw_clock) {
            // Off the vblank grid by more than timestamp noise: 8% of a
            // period, at least 0.25 ms. (A fraction of a period alone is too
            // loose on a fast output: 24 fps on 165 Hz is 6.88 periods,
            // only 0.12 off.)
            let tol = (0.08 * r).max(250_000.0);
            gaps.iter().filter(|(d, _)| (d - (d / r).round() * r).abs() > tol).count()
        } else {
            return None;
        };
        Some(off * 4 > gaps.len())
    }

    /// Lateness (presented minus meant) in ns: (median, 95th percentile),
    /// over frames that carried a target. Negative: shown early.
    pub fn lateness_ns(&self) -> Option<(i64, i64)> {
        let mut l: Vec<i64> = self.shown().filter_map(|p| Some(p.at_ns as i64 - p.target_ns? as i64)).collect();
        if l.is_empty() {
            return None;
        }
        l.sort_unstable();
        Some((l[l.len() / 2], l[((l.len() - 1) * 95) / 100]))
    }

    /// Frames in the window that were scanned out directly.
    pub fn zero_copy_share(&self) -> Option<f64> {
        let (n, z) = self.shown().fold((0usize, 0usize), |(n, z), p| (n + 1, z + p.zero_copy as usize));
        (n > 0).then(|| z as f64 / n as f64)
    }

    /// Judder: the share of holds that are not in the ideal pulldown for
    /// `fps` on this output. 0 = the cadence the content allows; a 24 fps
    /// film on 60 Hz shows 3,2,3,2 at best, so only other holds count.
    pub fn judder(&self, fps: f64) -> Option<f64> {
        let hz = 1e9 / self.refresh_ns()? as f64;
        let ideal = pulldown(fps, hz);
        let holds: Vec<u32> = self.holds().into_iter().flatten().collect();
        if holds.is_empty() || ideal.is_empty() {
            return None;
        }
        let bad = holds.iter().filter(|h| !ideal.contains(h)).count();
        Some(bad as f64 / holds.len() as f64)
    }

    /// One line for the stats overlay and the profile log.
    pub fn summary(&self, fps: Option<f64>) -> String {
        let mut s = match (self.refresh_ns(), self.variable()) {
            (_, Some(true)) => "display variable-rate".to_string(),
            (Some(r), _) => format!("display {:.2} Hz", 1e9 / r as f64),
            (None, _) => "display rate unknown".to_string(),
        };
        let h = self.hold_histogram();
        if !h.is_empty() {
            let parts: Vec<String> = h.iter().take(3).map(|(n, c)| format!("{n}x{c}")).collect();
            s += &format!(" · holds {}", parts.join(" "));
        }
        if let Some(j) = fps.and_then(|f| self.judder(f)) {
            s += &format!(" · judder {:.0}%", j * 100.0);
        }
        if let Some((m, p95)) = self.lateness_ns() {
            s += &format!(" · late {:.1}/{:.1} ms", m as f64 / 1e6, p95 as f64 / 1e6);
        }
        if self.discarded > 0 {
            s += &format!(" · {} discarded", self.discarded);
        }
        if let Some(z) = self.zero_copy_share() {
            if z > 0.0 {
                s += &format!(" · scanout {:.0}%", z * 100.0);
            }
        }
        s
    }
}

/// How much later than asked frames reach the screen, learned from
/// feedback, so the player can pick each frame for the moment it will
/// actually be SEEN. The audio clock already runs on what is heard (the
/// sink's latency taken off); without this, picture trails sound by the
/// compositor's whole pipeline -- 32 ms measured on headless weston.
///
/// A slow integral loop: every `batch` presented frames, move by half the
/// median residual (presented minus target, where the target already
/// includes the current compensation). Vblank quantisation makes single
/// residuals jump by a period; the median over a batch and the half gain
/// keep that from ringing. Bounded to `0..=max`.
#[derive(Debug, Clone)]
pub struct LatencyTracker {
    comp_ns: i64,
    residuals: Vec<i64>,
    batch: usize,
    max_ns: i64,
}

impl LatencyTracker {
    pub fn new(batch: usize, max_ns: i64) -> Self {
        Self { comp_ns: 0, residuals: Vec::with_capacity(batch), batch: batch.max(3), max_ns }
    }

    /// The compensation now, ns.
    pub fn comp_ns(&self) -> i64 {
        self.comp_ns
    }

    /// One outcome in; the new compensation when it changed.
    pub fn observe(&mut self, o: &Outcome) -> Option<i64> {
        let Outcome::Presented(p) = o else { return None };
        let t = p.target_ns?;
        self.residuals.push(p.at_ns as i64 - t as i64);
        if self.residuals.len() < self.batch {
            return None;
        }
        self.residuals.sort_unstable();
        let m = self.residuals[self.residuals.len() / 2];
        self.residuals.clear();
        // Deadband: within a millisecond is as good as it gets.
        if m.abs() < 1_000_000 {
            return None;
        }
        let next = (self.comp_ns + m / 2).clamp(0, self.max_ns);
        (next != self.comp_ns).then(|| {
            self.comp_ns = next;
            next
        })
    }
}

/// The hold counts a steady `fps` stream takes on a fixed `hz` output:
/// frame k shown from the first vblank at or after k/fps. 24 on 60 is
/// {2, 3}; 30 on 60 is {2}; 60 on 60 is {1}; 25 on 60 is {2, 3}. Pure.
pub fn pulldown(fps: f64, hz: f64) -> Vec<u32> {
    if !(fps > 0.0 && hz > 0.0) {
        return Vec::new();
    }
    // A thousandth of a vblank of slack: a reported period is an integer of
    // ns (16666666 is 60.0000024 Hz), and without slack 30 fps on "60 Hz"
    // lands a hair past each vblank and reads as {2, 3}.
    let vb = |k: u64| (k as f64 * hz / fps - 1e-3).ceil() as u64;
    let mut out: Vec<u32> = (0..240u64).map(|k| (vb(k + 1) - vb(k)) as u32).filter(|&n| n > 0).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(hz: f64, holds: &[u32], target_late_ns: i64) -> PresentLog {
        let r = (1e9 / hz) as u64;
        let mut log = PresentLog::new(256);
        let (mut seq, mut t) = (100u64, 1_000_000_000u64);
        for &h in holds.iter().chain(std::iter::once(&1)) {
            log.push(Outcome::Presented(Presented {
                target_ns: Some((t as i64 - target_late_ns) as u64),
                at_ns: t,
                refresh_ns: r as u32,
                seq,
                vsync: true,
                zero_copy: false,
                hw_clock: true,
            }));
            seq += h as u64;
            t += h as u64 * r;
        }
        log
    }

    #[test]
    fn pulldown_patterns() {
        assert_eq!(pulldown(24.0, 60.0), vec![2, 3]);
        assert_eq!(pulldown(30.0, 60.0), vec![2]);
        assert_eq!(pulldown(30.0, 1e9 / 16_666_666.0), vec![2], "a period reported in whole ns");
        assert_eq!(pulldown(60.0, 60.0), vec![1]);
        assert_eq!(pulldown(24000.0 / 1001.0, 60000.0 / 1001.0), vec![2, 3]);
        assert_eq!(pulldown(60.0, 120.0), vec![2]);
        assert_eq!(pulldown(30.0, 90.0), vec![3], "the Fairphone 5 panel");
        assert_eq!(pulldown(24.0, 120.0), vec![5]);
        assert!(pulldown(0.0, 60.0).is_empty());
    }

    #[test]
    fn a_clean_three_two_is_no_judder() {
        let log = fixed(60.0, &[3, 2, 3, 2, 3, 2, 3, 2, 3, 2], 0);
        assert_eq!(log.refresh_ns(), Some(16_666_666));
        assert_eq!(log.hold_histogram(), vec![(2, 5), (3, 5)]);
        assert_eq!(log.judder(24.0), Some(0.0));
        assert_eq!(log.variable(), Some(false));
    }

    #[test]
    fn a_four_is_judder_and_a_late_frame_shows() {
        let log = fixed(60.0, &[3, 2, 4, 1, 3, 2, 3, 2, 3, 2], 4_000_000);
        let j = log.judder(24.0).unwrap();
        assert!((j - 0.2).abs() < 1e-9, "{j}");
        assert_eq!(log.lateness_ns(), Some((4_000_000, 4_000_000)));
        assert!(log.summary(Some(24.0)).contains("judder 20%"), "{}", log.summary(Some(24.0)));
    }

    /// Without vblank counts the holds come from time over the period.
    #[test]
    fn holds_from_time_when_there_is_no_sequence() {
        let mut log = fixed(60.0, &[2, 2, 2, 2], 0);
        let v: Vec<Outcome> = log.window.iter().map(|o| match o {
            Outcome::Presented(p) => Outcome::Presented(Presented { seq: 0, ..*p }),
            x => *x,
        }).collect();
        log = PresentLog::new(64);
        for o in v {
            log.push(o);
        }
        assert_eq!(log.hold_histogram(), vec![(2, 4)]);
    }

    /// A VRR output presenting 24 fps at the content's own rate: gaps of
    /// 41.7 ms against a 165 Hz period are not whole refreshes. (On 144 Hz
    /// they would be -- 6 each -- and fixed and variable look the same,
    /// which is also what the viewer sees.)
    #[test]
    fn content_rate_gaps_on_a_fast_output_read_as_variable() {
        let mut log = PresentLog::new(64);
        for k in 0..12u64 {
            log.push(Outcome::Presented(Presented {
                target_ns: None,
                at_ns: 1_000_000_000 + k * 41_708_333,
                refresh_ns: 6_060_606,
                seq: 0,
                vsync: true,
                zero_copy: false,
                hw_clock: true,
            }));
        }
        assert_eq!(log.variable(), Some(true));
        let mut none = PresentLog::new(64);
        for k in 0..12u64 {
            none.push(Outcome::Presented(Presented { target_ns: None, at_ns: k * 40_000_000, refresh_ns: 0, seq: 0, vsync: true, zero_copy: false, hw_clock: true }));
        }
        assert_eq!(none.variable(), Some(true), "no refresh reported");
        assert!(none.summary(None).starts_with("display variable-rate"));
    }

    /// VRR with a vblank counter: one cycle per presentation, whatever the
    /// time says. And a software-timed compositor gives no verdict.
    #[test]
    fn the_vblank_counter_tells_vrr_and_software_clocks_tell_nothing() {
        let mut vrr = PresentLog::new(64);
        for k in 0..12u64 {
            vrr.push(Outcome::Presented(Presented { target_ns: None, at_ns: k * 41_708_333, refresh_ns: 6_944_444, seq: 10 + k, vsync: true, zero_copy: false, hw_clock: true }));
        }
        assert_eq!(vrr.variable(), Some(true), "144 Hz period, 6 periods apart, 1 vblank apart");
        let mut soft = PresentLog::new(64);
        for k in 0..12u64 {
            soft.push(Outcome::Presented(Presented { target_ns: None, at_ns: k * 33_000_000 + (k % 3) * 1_700_000, refresh_ns: 16_666_666, seq: 0, vsync: true, zero_copy: false, hw_clock: false }));
        }
        assert_eq!(soft.variable(), None);
    }

    /// A compositor that shows each commit at the first 60 Hz vblank at
    /// least 30 ms after it: the tracker learns ~30 ms plus quantisation and
    /// the residual settles within a period of zero, without ringing.
    #[test]
    fn latency_is_learned_and_settles() {
        let period = 16_666_667i64;
        let pipeline = 30_000_000i64;
        let mut tr = LatencyTracker::new(15, 80_000_000);
        let mut last = Vec::new();
        for k in 0..1200i64 {
            // Commit at wall time t (30 fps, with a little phase drift).
            let t = k * 33_333_333 + (k * 7919 % 3_000_000);
            let target = t + tr.comp_ns();
            let earliest = t + pipeline;
            let at = (earliest + period - 1) / period * period;
            tr.observe(&Outcome::Presented(Presented { target_ns: Some(target as u64), at_ns: at as u64, refresh_ns: period as u32, seq: 0, vsync: true, zero_copy: false, hw_clock: true }));
            if k >= 1000 {
                last.push(at - target);
            }
        }
        let c = tr.comp_ns();
        assert!(c > pipeline - period && c < pipeline + period, "learned {c} ns for a {pipeline} ns pipeline");
        last.sort_unstable();
        let med = last[last.len() / 2];
        assert!(med.abs() < period, "settled residual median {med} ns");
    }

    /// Frames shown EARLY (a compositor honouring commit timing too well, or
    /// a clock mismatch) never drive the compensation negative.
    #[test]
    fn latency_never_goes_negative() {
        let mut tr = LatencyTracker::new(5, 80_000_000);
        for k in 0..50u64 {
            tr.observe(&Outcome::Presented(Presented { target_ns: Some(1_000_000_000 + k), at_ns: 990_000_000, refresh_ns: 0, seq: 0, vsync: true, zero_copy: false, hw_clock: true }));
        }
        assert_eq!(tr.comp_ns(), 0);
    }

    #[test]
    fn discards_are_counted() {
        let mut log = fixed(60.0, &[1, 1], 0);
        log.push(Outcome::Discarded { target_ns: None });
        assert_eq!(log.discarded, 1);
        assert!(log.summary(None).contains("1 discarded"));
    }
}
