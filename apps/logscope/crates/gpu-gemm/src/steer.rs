//! steer.rs — the only "modify the tensors" pathway a forward-only engine can actually take.
//!
//! The premise being tested is that text feedback is a weak control channel and something closer to
//! the weights would be stronger. On this stack almost none of that is reachable: a learned feedback
//! encoder, a correction operator, a dynamic LoRA and a recurrent belief state all need gradients,
//! and this crate has no autograd — `burn-probe` is a correctness oracle, not a training stack.
//!
//! What IS reachable is one line of arithmetic in the residual stream:
//!
//! ```text
//! x = x + gain * v            // per layer, between blocks
//! ```
//!
//! and the engine already has every piece of it. `decode_step` threads the residual `x` through
//! `for li in 0..m.n_layers` in the open, and `add_bias_t(a, bias)` is `o[i] = a[i] + bias[i % cols]`
//! — a broadcast of a `[d]` vector over `[m, d]` rows, already in the validated kernel set. The
//! injection is an insert in that loop; nothing new goes on the GPU.
//!
//! ## Where `v` comes from, given that we cannot train
//!
//! Difference of means. Capture the residual at one layer from branches that survived selection and
//! from branches that died, and take `mean(survived) - mean(died)`, normalised. That needs forward
//! passes and nothing else.
//!
//! The reason this is worth trying HERE specifically, rather than being a generic steering
//! experiment, is that the speculative population is already generating exactly the paired,
//! same-prefix, same-position captures the difference of means wants — and generating them as a
//! by-product of a search it was going to run anyway. The batching runtime is the data collection
//! apparatus. Nothing else in this design gets labelled pairs that cheaply.
//!
//! ## What it will and will not move
//!
//! Stated up front so the experiment is not read as more than it is. A single direction added to the
//! residual stream reliably moves DISPOSITIONS — how readily the model rewrites versus patches, how
//! much it hedges, whether it reaches for a trait impl or a signature change. It does not encode
//! "the bound on line 40 is wrong". Target a disposition or the result will be noise, and report
//! `sep` (below) so a noise direction is visible as one rather than being deployed.
//!
//! ## Two constraints that come from elsewhere in the engine
//!
//! **Never steer a shared-prefix position.** Under the block-table KV the prefix rows are physically
//! shared with every sibling branch. A steered forward pass writes steered K/V, so steering during
//! shared prefill does not steer one branch — it silently rewrites the cache all of them read.
//! `Schedule::gain` returns 0 at or before `fork_pos` and there is no way to ask it not to.
//!
//! **Gain must be a function of absolute position only.** `spec.rs` requires that at temperature 0
//! speculative decode emits exactly what single-token decode would. That holds iff the verify pass
//! sees the same gain at each position the single-token path would. Position-only gain gives that
//! for free with the PromptLookup drafter (which runs no model at all). An MTP or small-model
//! drafter must be given the same schedule or speculation has to be off while steering is on.
//!
//! Pure: no wgpu, no model. The direction is solved and the schedule evaluated on the CPU, so both
//! are testable without a GPU — which is the only reason the awkward cases below are cheap to pin.

use std::collections::BTreeMap;

/// Which side of the selection boundary a capture came from. This is the label, and it is supplied
/// by the evaluator (compiler/tests), never by a model judging its own output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The branch was in the top-K at the round boundary.
    Survived,
    /// The branch was dropped.
    Died,
}

/// A direction in the residual stream at one layer, plus the evidence that it is a direction and not
/// noise.
#[derive(Clone, Debug)]
pub struct Steer {
    /// Which block's output this was captured after. Injection must use the same index.
    pub layer: usize,
    /// Unit-norm difference of means. Length `d`.
    pub dir: Vec<f32>,
    /// Cohen's d of the two groups projected onto `dir`, IN SAMPLE.
    ///
    /// Retained for reporting and worthless as a gate. `examples/probe_null.rs` measures it at a
    /// mean of 23.5 on pure Gaussian noise at n=32, d=4096, clearing the old 0.8 threshold in
    /// 200 runs out of 200 — because it is measured along the direction that was chosen to maximise
    /// it. Print it next to a held-out score to show the gap; never decide on it.
    pub sep: f32,
    /// Mean RMS of the captured residuals at this layer. Gains are expressed as multiples of this,
    /// because the residual norm grows with depth — a fixed absolute alpha is a no-op at layer 4 and
    /// a lobotomy at layer 28, and that difference has nothing to do with the direction.
    pub base_rms: f32,
    /// How many captures went into each side. A direction from three branches is not a direction.
    pub n_survived: usize,
    pub n_died: usize,
    /// Has this direction survived held-out scoring against a permuted-label null? Only
    /// `from_probe` sets it. A direction straight out of `Contrast::solve` has not been validated
    /// and cannot be installed.
    pub validated: bool,
}

/// The smallest population per side worth solving from. Sixteen branches split 4/12 is the smallest
/// realistic round, so four is the floor.
pub const MIN_PER_SIDE: usize = 4;

impl Steer {
    /// Promote a validated probe into a steering direction. This is the only constructor that
    /// produces something `Plan::install` will accept.
    pub fn from_probe(p: &crate::probe::ProbeResult) -> Result<Steer, String> {
        if let Some(why) = p.refusal() { return Err(why); }
        Ok(Steer {
            layer: p.layer,
            dir: p.dir.clone(),
            sep: 0.0,
            base_rms: p.base_rms,
            n_survived: p.n_ok,
            n_died: p.n_bad,
            validated: true,
        })
    }

    /// Is this direction supported well enough to inject?
    pub fn deployable(&self) -> bool { self.refusal().is_none() }

    /// Why not, in the operator's words. `None` when it is deployable.
    pub fn refusal(&self) -> Option<String> {
        if !self.validated {
            return Some(format!(
                "layer {}: direction is unvalidated — a difference of means at n={} in {} dimensions \
                 fits the noise (in-sample sep {:.1} means nothing here). Run it through \
                 `probe::LayerSweep::evaluate` and build the Steer with `from_probe`.",
                self.layer, self.n_survived + self.n_died, self.dir.len(), self.sep
            ));
        }
        if self.n_survived < MIN_PER_SIDE || self.n_died < MIN_PER_SIDE {
            return Some(format!(
                "{} survived / {} died at layer {}: need {MIN_PER_SIDE} a side before a fold can hold \
                 both classes",
                self.n_survived, self.n_died, self.layer
            ));
        }
        None
    }
}

/// Paired residual captures for one layer, accumulated across a selection round.
///
/// Captures are kept rather than reduced to running sums because the separation statistic needs a
/// second pass over the rows once the direction is known, and the memory is nothing: 32 branches at
/// d=4096 is 512 KiB per layer per round.
pub struct Contrast {
    d: usize,
    layer: usize,
    rows: Vec<(Outcome, Vec<f32>)>,
}

impl Contrast {
    pub fn new(layer: usize, d: usize) -> Self {
        Contrast { d, layer, rows: Vec::new() }
    }

    pub fn layer(&self) -> usize { self.layer }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }

    /// Record one branch's residual row at this layer.
    ///
    /// The caller must capture at the SAME token position class across branches — the last prompt
    /// token, or the token at a fixed offset past the fork. Mixing positions mixes "what survived"
    /// with "where in the sequence we looked", and the difference of means will happily return the
    /// second one.
    ///
    /// Returns false and ignores the row on a width mismatch rather than panicking, because this is
    /// called from a capture pass in the middle of a search round and killing the round over a
    /// bookkeeping error loses every other branch's work too.
    pub fn push(&mut self, outcome: Outcome, h: &[f32]) -> bool {
        if h.len() != self.d { return false; }
        self.rows.push((outcome, h.to_vec()));
        true
    }

    fn mean(&self, want: Outcome) -> Option<(Vec<f32>, usize)> {
        let mut acc = vec![0f64; self.d];
        let mut n = 0usize;
        for (o, h) in &self.rows {
            if *o != want { continue; }
            for (a, x) in acc.iter_mut().zip(h) { *a += *x as f64; }
            n += 1;
        }
        if n == 0 { return None; }
        for a in acc.iter_mut() { *a /= n as f64; }
        Some((acc.into_iter().map(|x| x as f32).collect(), n))
    }

    /// Solve for the direction. `None` only when a side is empty or the two means coincide exactly
    /// (which in practice means every capture was identical — a bug in the capture pass, not a
    /// property of the population).
    pub fn solve(&self) -> Option<Steer> {
        let (mu_s, n_s) = self.mean(Outcome::Survived)?;
        let (mu_d, n_d) = self.mean(Outcome::Died)?;

        let raw: Vec<f32> = mu_s.iter().zip(&mu_d).map(|(a, b)| a - b).collect();
        let norm = (raw.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>()).sqrt();
        if norm <= 0.0 || !norm.is_finite() { return None; }
        let dir: Vec<f32> = raw.iter().map(|x| (*x as f64 / norm) as f32).collect();

        // Second pass: project onto the direction and measure how far apart the groups sit in units
        // of their own spread. A large gap between means says nothing if the within-group spread is
        // larger still.
        let mut proj_s: Vec<f64> = Vec::with_capacity(n_s);
        let mut proj_d: Vec<f64> = Vec::with_capacity(n_d);
        let mut rms_sum = 0f64;
        for (o, h) in &self.rows {
            let p: f64 = h.iter().zip(&dir).map(|(a, b)| (*a as f64) * (*b as f64)).sum();
            match o { Outcome::Survived => proj_s.push(p), Outcome::Died => proj_d.push(p) }
            let ms: f64 = h.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / self.d as f64;
            rms_sum += ms.sqrt();
        }
        let sep = cohens_d(&proj_s, &proj_d) as f32;
        let base_rms = (rms_sum / self.rows.len() as f64) as f32;

        Some(Steer { layer: self.layer, dir, sep, base_rms, n_survived: n_s, n_died: n_d, validated: false })
    }
}

/// Pooled-sd standardised mean difference. Returns 0 when either group has under two members or the
/// pooled spread is zero — both mean "no evidence of separation", which is the honest answer and the
/// one that fails `deployable()`.
fn cohens_d(a: &[f64], b: &[f64]) -> f64 {
    if a.len() < 2 || b.len() < 2 { return 0.0; }
    let m = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (m(a), m(b));
    let ss = |v: &[f64], mu: f64| v.iter().map(|x| (x - mu) * (x - mu)).sum::<f64>();
    let dof = (a.len() + b.len() - 2) as f64;
    let pooled = ((ss(a, ma) + ss(b, mb)) / dof).sqrt();
    if pooled <= 0.0 || !pooled.is_finite() { return 0.0; }
    (ma - mb) / pooled
}

/// When and how hard to inject, as a function of absolute stream position.
///
/// Position-only is not a simplification, it is the property that keeps speculative decode
/// bit-exact: draft and verify agree on the gain because they agree on the position.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    /// Gain at full strength, in multiples of the layer's `base_rms`. A useful starting sweep is
    /// 0.05 to 0.5; above ~1.0 the injected component dominates the residual and output degrades in
    /// a way that has nothing to do with whether the direction was right.
    pub alpha: f32,
    /// Absolute position of the branch's fork point. Positions at or before this write KV rows that
    /// siblings share, so gain there is forced to zero.
    pub fork_pos: usize,
    /// Tokens after `fork_pos` over which gain rises linearly from 0 to `alpha`. A step change at
    /// the fork is visible as a discontinuity in the generation; 0 disables the ramp.
    pub ramp: usize,
    /// Tokens after `fork_pos` past which gain returns to 0. Steering the whole tail turns a nudge
    /// into a persona. 0 means never decay.
    pub hold: usize,
}

impl Schedule {
    /// A schedule that never injects. The baseline arm of the experiment, and the correct thing to
    /// fall back to when `Steer::deployable()` is false.
    pub fn off(fork_pos: usize) -> Self {
        Schedule { alpha: 0.0, fork_pos, ramp: 0, hold: 0 }
    }

    /// Would a forward pass at this position write a KV row that sibling branches read?
    pub fn writes_shared_kv(&self, pos: usize) -> bool { pos <= self.fork_pos }

    /// Multiplier on `Steer::base_rms` at this absolute position. Zero means the caller should skip
    /// the `add_bias_t` dispatch entirely rather than add a zero vector.
    pub fn gain(&self, pos: usize) -> f32 {
        if self.alpha == 0.0 || self.writes_shared_kv(pos) { return 0.0; }
        let since = pos - self.fork_pos;
        if self.hold > 0 && since > self.hold { return 0.0; }
        if self.ramp > 0 && since < self.ramp {
            return self.alpha * (since as f32 / self.ramp as f32);
        }
        self.alpha
    }

    /// The vector to hand `add_bias_t` at this position, or `None` to skip the dispatch.
    ///
    /// This is the whole integration surface. In `decode_step`'s layer loop:
    ///
    /// ```text
    /// x = match (..) { .. };
    /// if let Some(b) = plan.bias_at(li, pos) { x = self.add_bias_t(&x, &self.upload(&b, 1, m.d)); }
    /// ```
    ///
    /// Upload the bias once per round, not per token — the vector is constant within a round and the
    /// upload is the only part of this that is not free.
    pub fn scaled(&self, s: &Steer, pos: usize) -> Option<Vec<f32>> {
        let g = self.gain(pos) * s.base_rms;
        if g == 0.0 { return None; }
        Some(s.dir.iter().map(|x| x * g).collect())
    }
}

/// A branch's full injection plan: at most one direction per layer, plus one schedule.
///
/// One direction per layer rather than a direction per (layer, position) because the population
/// supplies one labelled pair per branch per round, and fitting a per-position family from that
/// would be fitting noise with extra steps.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    by_layer: BTreeMap<usize, Steer>,
    schedule: Option<Schedule>,
}

impl Plan {
    pub fn new(schedule: Schedule) -> Self {
        Plan { by_layer: BTreeMap::new(), schedule: Some(schedule) }
    }

    /// Add a solved direction. Refuses non-deployable ones and returns the refusal so the caller can
    /// print it — a silently dropped direction looks identical to one that had no effect, and those
    /// are the two outcomes the experiment most needs to tell apart.
    pub fn install(&mut self, s: Steer) -> Result<(), String> {
        match s.refusal() {
            Some(why) => Err(why),
            None => { self.by_layer.insert(s.layer, s); Ok(()) }
        }
    }

    pub fn layers(&self) -> impl Iterator<Item = usize> + '_ { self.by_layer.keys().copied() }
    pub fn is_empty(&self) -> bool { self.by_layer.is_empty() }

    /// Gain multiplier at this absolute position, independent of layer. The GPU-side bank keys its
    /// uploads on this: with `ramp == 0` a whole round has exactly one non-zero level, so one upload
    /// per steered layer covers every token.
    pub fn gain_at(&self, pos: usize) -> f32 {
        self.schedule.as_ref().map_or(0.0, |s| s.gain(pos))
    }

    /// The bias to add after block `layer` at absolute position `pos`, or `None`.
    pub fn bias_at(&self, layer: usize, pos: usize) -> Option<Vec<f32>> {
        let sch = self.schedule.as_ref()?;
        let s = self.by_layer.get(&layer)?;
        sch.scaled(s, pos)
    }

    /// Does this plan touch any position whose KV is shared with siblings? Must be false before a
    /// steered branch runs under the block-table cache. Cheap belt to the `gain()` suspender.
    pub fn would_poison_shared_kv(&self, first_pos: usize, last_pos: usize) -> bool {
        let Some(sch) = self.schedule.as_ref() else { return false };
        (first_pos..=last_pos).any(|p| sch.writes_shared_kv(p) && sch.gain(p) != 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(d: usize, base: f32, along: f32) -> Vec<f32> {
        // A fixed background plus a signed component on dimension 0.
        let mut v = vec![base; d];
        v[0] = base + along;
        v
    }

    #[test]
    fn separated_populations_yield_a_deployable_direction() {
        let d = 64;
        let mut c = Contrast::new(12, d);
        for i in 0..8 { c.push(Outcome::Survived, &row(d, 0.1, 1.0 + i as f32 * 0.01)); }
        for i in 0..8 { c.push(Outcome::Died, &row(d, 0.1, -1.0 - i as f32 * 0.01)); }
        let s = c.solve().expect("two non-empty sides");
        assert_eq!(s.layer, 12);
        assert!(s.dir[0] > 0.99, "direction should lie on dim 0, got {}", s.dir[0]);
        assert!(s.sep > 1.0, "in-sample sep {} (reporting only)", s.sep);
        // Even here — a genuinely planted, obvious signal — the raw contrast is not deployable.
        // Held-out validation is not a formality that clean data lets you skip.
        assert!(!s.deployable());
        assert!(s.refusal().unwrap().contains("unvalidated"));
    }

    #[test]
    fn a_probe_validated_direction_is_the_only_deployable_one() {
        use crate::probe::{LayerSweep, Lcg};
        let (d, n) = (128usize, 40usize);
        let mut rng = Lcg::new(4);
        let mut sw = LayerSweep::new(9, d);
        for i in 0..n {
            let ok = i % 2 == 0;
            let mut h: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
            h[0] += if ok { 3.0 } else { -3.0 };
            sw.push(i, &h, ok);
        }
        let r = sw.evaluate(4, 200, 1).unwrap();
        assert!(r.deployable(), "{:?}", r.refusal());
        let st = Steer::from_probe(&r).unwrap();
        assert!(st.validated && st.deployable());
        assert_eq!(st.layer, 9);

        let mut p = Plan::new(Schedule { alpha: 0.2, fork_pos: 0, ramp: 0, hold: 0 });
        p.install(st).unwrap();
        assert!(p.bias_at(9, 1).is_some());
    }

    #[test]
    fn from_probe_refuses_to_promote_a_probe_that_failed_its_own_null() {
        use crate::probe::{LayerSweep, Lcg};
        let mut rng = Lcg::new(21);
        let mut sw = LayerSweep::new(9, 512);
        for i in 0..32 {
            let h: Vec<f32> = (0..512).map(|_| rng.normal()).collect();
            sw.push(i, &h, i % 2 == 0);
        }
        let r = sw.evaluate(4, 200, 5).unwrap();
        if r.deployable() { return; } // ~5% of seeds legitimately clear a 5% gate
        assert!(Steer::from_probe(&r).is_err());
    }

    #[test]
    fn overlapping_populations_are_refused_rather_than_deployed() {
        // Means differ slightly; within-group spread is much larger. This is the case that must not
        // silently become a steering vector.
        let d = 32;
        let mut c = Contrast::new(4, d);
        let spread = [-9.0, -3.0, 3.0, 9.0, -6.0, 6.0, 0.0, 1.0];
        for (i, s) in spread.iter().enumerate() { c.push(Outcome::Survived, &row(d, 0.0, s + 0.05 * i as f32)); }
        for s in spread.iter() { c.push(Outcome::Died, &row(d, 0.0, *s)); }
        let s = c.solve().unwrap();
        assert!(!s.deployable());
        assert!(s.refusal().unwrap().contains("unvalidated"));
    }

    #[test]
    fn a_side_with_too_few_members_is_refused_even_when_perfectly_separated() {
        let d = 16;
        let mut c = Contrast::new(0, d);
        for _ in 0..2 { c.push(Outcome::Survived, &row(d, 0.0, 5.0)); }
        for _ in 0..9 { c.push(Outcome::Died, &row(d, 0.0, -5.0)); }
        let s = c.solve().unwrap();
        assert!(!s.deployable());
    }

    #[test]
    fn one_sided_captures_solve_to_nothing() {
        let d = 8;
        let mut c = Contrast::new(1, d);
        for _ in 0..5 { c.push(Outcome::Survived, &row(d, 0.0, 1.0)); }
        assert!(c.solve().is_none());
    }

    #[test]
    fn width_mismatch_is_dropped_not_panicked() {
        let mut c = Contrast::new(3, 8);
        assert!(c.push(Outcome::Survived, &[0.0; 8]));
        assert!(!c.push(Outcome::Died, &[0.0; 7]));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn gain_is_zero_on_and_before_the_fork() {
        let s = Schedule { alpha: 0.3, fork_pos: 100, ramp: 0, hold: 0 };
        assert_eq!(s.gain(0), 0.0);
        assert_eq!(s.gain(99), 0.0);
        assert_eq!(s.gain(100), 0.0, "the fork position itself is shared");
        assert_eq!(s.gain(101), 0.3);
        assert!(s.writes_shared_kv(100));
        assert!(!s.writes_shared_kv(101));
    }

    #[test]
    fn ramp_rises_linearly_and_hold_returns_to_zero() {
        let s = Schedule { alpha: 0.4, fork_pos: 10, ramp: 4, hold: 20 };
        assert_eq!(s.gain(10), 0.0);
        assert!((s.gain(11) - 0.1).abs() < 1e-6);
        assert!((s.gain(12) - 0.2).abs() < 1e-6);
        assert!((s.gain(14) - 0.4).abs() < 1e-6, "ramp complete at since==ramp");
        assert!((s.gain(30) - 0.4).abs() < 1e-6, "since==hold still holds");
        assert_eq!(s.gain(31), 0.0, "past hold");
    }

    #[test]
    fn gain_depends_only_on_position_so_draft_and_verify_agree() {
        // The property spec.rs's temperature-0 invariant rests on: the same position gives the same
        // gain no matter how many times or in what order it is asked.
        let s = Schedule { alpha: 0.25, fork_pos: 5, ramp: 3, hold: 0 };
        let forward: Vec<f32> = (0..20).map(|p| s.gain(p)).collect();
        let backward: Vec<f32> = (0..20).rev().map(|p| s.gain(p)).collect();
        let mut b = backward; b.reverse();
        assert_eq!(forward, b);
    }

    #[test]
    fn off_never_injects() {
        let s = Schedule::off(0);
        assert!((0..50).all(|p| s.gain(p) == 0.0));
    }

    #[test]
    fn plan_refuses_a_weak_direction_and_reports_why() {
        let mut p = Plan::new(Schedule { alpha: 0.2, fork_pos: 0, ramp: 0, hold: 0 });
        let weak = Steer { layer: 7, dir: vec![1.0, 0.0], sep: 0.1, base_rms: 1.0, n_survived: 8, n_died: 8, validated: false };
        let err = p.install(weak).unwrap_err();
        assert!(err.contains("unvalidated"));
        assert!(p.is_empty());
    }

    #[test]
    fn scaled_bias_is_alpha_times_base_rms_along_the_direction() {
        let s = Steer { layer: 2, dir: vec![1.0, 0.0, 0.0], sep: 2.0, base_rms: 8.0, n_survived: 8, n_died: 8, validated: true };
        let mut p = Plan::new(Schedule { alpha: 0.25, fork_pos: 0, ramp: 0, hold: 0 });
        p.install(s).unwrap();
        let b = p.bias_at(2, 1).expect("layer 2 is installed and pos 1 is past the fork");
        assert!((b[0] - 2.0).abs() < 1e-6, "0.25 * 8.0 = 2.0, got {}", b[0]);
        assert_eq!(b[1], 0.0);
        assert!(p.bias_at(3, 1).is_none(), "no direction installed at layer 3");
        assert!(p.bias_at(2, 0).is_none(), "pos 0 is the fork");
    }

    #[test]
    fn a_plan_forked_at_zero_never_reports_poisoning() {
        let mut p = Plan::new(Schedule { alpha: 0.5, fork_pos: 40, ramp: 0, hold: 0 });
        p.install(Steer { layer: 1, dir: vec![1.0], sep: 3.0, base_rms: 1.0, n_survived: 8, n_died: 8, validated: true }).unwrap();
        assert!(!p.would_poison_shared_kv(0, 200));
    }
}
