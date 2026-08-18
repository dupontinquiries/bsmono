//! probe.rs — which layer, if any, carries a direction that predicts executable success.
//!
//! `steer.rs` can add a vector to the residual stream after any block. It does not know WHICH block,
//! and the honest answer is that nobody does: the layer at which eventual correctness becomes
//! linearly accessible is an empirical property of a checkpoint and a task distribution, not
//! something to reason out. This module measures it.
//!
//! ## The constraint that dominates everything here
//!
//! A round produces 16 or 32 labelled activations. The residual is 4096-wide on an 8B. So the
//! regime is `n = 32, d = 4096` — a hundred and twenty-eight times more dimensions than samples.
//!
//! In that regime a difference-of-means direction fits the noise almost perfectly, and any
//! IN-SAMPLE separation statistic measured along it is meaningless. That includes the `sep` gate in
//! `steer.rs`, which this module supersedes: `sep` is computed along the direction that was chosen
//! to maximise it, so it reports a large effect on pure noise. `examples/probe_null.rs` measures how
//! large.
//!
//! Everything below is therefore built around one rule: **the direction is fitted on one set of
//! branches and scored on another, and a permuted-label null says whether the held-out score is
//! anything at all.** There is no version of this that skips that step and stays honest.
//!
//! It also rules out most of the mathematics one would want. A value field `V(z)`, its gradient, a
//! local metric, Jacobian structure, information gain — each needs orders of magnitude more
//! trajectories than a generational round produces. What 32 branches buys is ONE direction per
//! layer, cross-validated, accumulated across rounds until the null clears. Start there.
//!
//! ## And prediction is not control
//!
//! A probe that separates future-successful from future-failed states has found a correlate. It has
//! not shown that pushing along it moves the outcome — the direction may be reading a consequence of
//! the branch's situation rather than a cause of it. `trial_arms_needed` prices the paired
//! intervention experiment that would settle it, and the price is the point: detecting a 22% → 48%
//! correction-rate shift takes about 55 branches per arm, so a 32-wide round split two ways needs
//! roughly four rounds before the answer means anything.
//!
//! Pure: no wgpu, no model. Probes are fitted and scored on the CPU from captured rows.

/// One branch's residual at one layer, with the outcome that branch eventually reached.
///
/// `ok` is the EVENTUAL outcome of the trajectory this state was on, not the immediate one. A state
/// three tokens into a branch that compiles forty tokens later is a positive. Labelling by immediate
/// outcome measures something much less useful and much easier.
#[derive(Clone, Debug)]
pub struct Row {
    /// Which fork this branch descended from. The unit of independence — see `effective_n`.
    pub fork: usize,
    pub h: Vec<f32>,
    pub ok: bool,
}

/// Held-out performance of a linear probe at one layer.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub layer: usize,
    /// Area under the ROC curve over pooled out-of-fold scores. 0.5 is chance.
    pub cv_auc: f32,
    /// In-sample AUC along the direction fitted to all the data. Reported only so the gap against
    /// `cv_auc` is visible — it is not evidence of anything and must never be gated on.
    pub in_sample_auc: f32,
    /// Fraction of label permutations whose cross-validated AUC matched or beat `cv_auc`. This is
    /// the p-value, and at n=32 it is the only number in this struct that carries weight.
    pub null_p: f32,
    /// Direction fitted on all rows, unit norm. Only meaningful when `deployable()`.
    pub dir: Vec<f32>,
    /// Mean RMS of the rows at this layer — `steer::Steer::base_rms` wants this, because gain has to
    /// be expressed relative to the residual norm at the layer being steered.
    pub base_rms: f32,
    pub n_ok: usize,
    pub n_bad: usize,
    /// Effective sample size after correlation between rows.
    ///
    /// The captures are NOT 32 independent draws. Branches forked from a shared prefix run the same
    /// tokens through the same weights until they diverge, so their residuals at a capture position
    /// shortly after the fork are near-copies of each other. Standard design effect:
    /// `n_eff = n / (1 + (n-1) * rho_bar)`. At `rho_bar = 0.9` a 32-branch round is worth about one
    /// sample, and every statistic above is computed as though it had thirty-two.
    pub eff_n: f32,
    /// Mean pairwise correlation between centred rows. What drives `eff_n`.
    pub rho_bar: f32,
}

/// Held-out AUC below this is not worth acting on even with a clean p-value: a real but tiny
/// separation cannot survive being turned into a steering gain.
pub const MIN_CV_AUC: f32 = 0.65;
/// Permutation p-value ceiling.
pub const MAX_NULL_P: f32 = 0.05;
/// Fewest rows on the smaller side. Below this, cross-validation folds stop containing both classes.
pub const MIN_PER_CLASS: usize = 6;
/// Fewest EFFECTIVE samples. Independent of how many rows were captured — this is the one that
/// cannot be satisfied by capturing more branches from the same fork, only by capturing further
/// past it or from different forks.
pub const MIN_EFFECTIVE_N: f32 = 8.0;

impl ProbeResult {
    pub fn deployable(&self) -> bool { self.refusal().is_none() }

    /// Why this layer's probe should not be turned into a steering direction, in the operator's
    /// words. Ordered so the most fundamental objection is reported first — telling someone their
    /// AUC is low when they only have four positives sends them to tune the wrong thing.
    pub fn refusal(&self) -> Option<String> {
        let smaller = self.n_ok.min(self.n_bad);
        if smaller < MIN_PER_CLASS {
            return Some(format!(
                "layer {}: {} / {} rows — the smaller class needs {MIN_PER_CLASS} before a fold can \
                 contain both, so accumulate more rounds before ranking layers",
                self.layer, self.n_ok, self.n_bad
            ));
        }
        if self.eff_n < MIN_EFFECTIVE_N {
            return Some(format!(
                "layer {}: {} rows but only {:.1} effective (mean pairwise correlation {:.2}) — these \
                 branches have not diverged enough to be separate observations. Capture further past \
                 the fork, or across forks; capturing more siblings from this one will not help.",
                self.layer, self.n_ok + self.n_bad, self.eff_n, self.rho_bar));
        }
        if self.null_p > MAX_NULL_P {
            return Some(format!(
                "layer {}: held-out AUC {:.3} but permuted labels reach it {:.0}% of the time — this \
                 is what a fitted direction looks like on noise (in-sample AUC was {:.3})",
                self.layer, self.cv_auc, self.null_p * 100.0, self.in_sample_auc
            ));
        }
        if self.cv_auc < MIN_CV_AUC {
            return Some(format!(
                "layer {}: held-out AUC {:.3} clears the null but is under {MIN_CV_AUC:.2} — real and \
                 too small to steer on",
                self.layer, self.cv_auc
            ));
        }
        None
    }
}

/// How the direction is fitted from the two class means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Raw difference of means. The discriminant under isotropic covariance — which a residual
    /// stream is not.
    MeanDiff,
    /// Difference of means divided by per-dimension pooled within-class VARIANCE: diagonal LDA.
    ///
    /// Residual streams have outlier features — a handful of coordinates carrying an order of
    /// magnitude more variance than the rest. `MeanDiff` points wherever those coordinates happened
    /// to fall this round, because a 50x-scale dimension contributes 2500x the squared difference
    /// even when its difference is pure noise. Dividing by spread makes every coordinate contribute
    /// in units of its own noise, which is what "difference" was supposed to mean.
    ///
    /// Full LDA is not available at n=32 in 4096 dimensions — the covariance is singular. The
    /// diagonal is estimable, and shrinking each per-dimension spread toward the median spread
    /// (Tibshirani's shrunken-centroid correction) keeps a coordinate that happened to draw a tiny
    /// sample spread from exploding.
    DiagLda,
}

/// Labelled activations at one layer, accumulated across selection rounds.
pub struct LayerSweep {
    layer: usize,
    d: usize,
    rows: Vec<Row>,
    fit: Fit,
}

impl LayerSweep {
    pub fn new(layer: usize, d: usize) -> Self {
        LayerSweep { layer, d, rows: Vec::new(), fit: Fit::DiagLda }
    }
    /// Override the estimator. `MeanDiff` exists so `examples/outlier_features.rs` can show what it
    /// costs; there is no reason to select it otherwise.
    pub fn with_fit(mut self, fit: Fit) -> Self { self.fit = fit; self }
    pub fn fit_kind(&self) -> Fit { self.fit }

    pub fn layer(&self) -> usize { self.layer }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }
    pub fn counts(&self) -> (usize, usize) {
        let ok = self.rows.iter().filter(|r| r.ok).count();
        (ok, self.rows.len() - ok)
    }

    /// Returns false and drops the row on a width mismatch, for the same reason `steer::Contrast`
    /// does: this is called mid-round and killing the round loses every other branch's work too.
    pub fn push(&mut self, fork: usize, h: &[f32], ok: bool) -> bool {
        if h.len() != self.d { return false; }
        self.rows.push(Row { fork, h: h.to_vec(), ok });
        true
    }

    /// Distinct forks represented, sorted.
    pub fn forks(&self) -> Vec<usize> {
        let mut f: Vec<usize> = self.rows.iter().map(|r| r.fork).collect();
        f.sort_unstable(); f.dedup(); f
    }

    /// Fit and evaluate. `folds` is clamped to the smaller class count; `perms` is the number of
    /// label shufflings behind `null_p` (200 is enough to resolve 0.05, and is cheap: each
    /// permutation is `folds` mean-difference fits over `n` rows).
    ///
    /// `seed` makes the permutation test reproducible. A p-value that moves between runs of the same
    /// data is not a p-value.
    pub fn evaluate(&self, folds: usize, perms: usize, seed: u64) -> Option<ProbeResult> {
        let (n_ok, n_bad) = self.counts();
        if n_ok == 0 || n_bad == 0 { return None; }
        let labels: Vec<bool> = self.rows.iter().map(|r| r.ok).collect();
        let k = folds.max(2).min(n_ok.min(n_bad)).min(self.rows.len());

        let cv_auc = self.cv_auc(&labels, k);
        let dir = fit_dir(&self.rows, &labels, self.fit)?;
        let in_sample_auc = auc(&self.rows.iter().map(|r| dot(&r.h, &dir)).collect::<Vec<_>>(), &labels);

        // Permutation null: reshuffle labels, refit and rescore end to end. Refitting inside the
        // permutation is the whole point — permuting only the scoring would leave the direction
        // fitted to the true labels and the null would be trivially beaten.
        let mut rng = Lcg::new(seed);
        let mut beat = 0usize;
        for _ in 0..perms {
            let mut perm = labels.clone();
            shuffle(&mut perm, &mut rng);
            if perm.iter().all(|b| *b) || perm.iter().all(|b| !*b) { beat += 1; continue; }
            if self.cv_auc(&perm, k) >= cv_auc { beat += 1; }
        }
        let null_p = if perms == 0 { 1.0 } else { beat as f32 / perms as f32 };

        let base_rms = (self.rows.iter().map(|r| rms(&r.h) as f64).sum::<f64>() / self.rows.len() as f64) as f32;
        let (eff_n, rho_bar) = self.effective_n();

        Some(ProbeResult { layer: self.layer, cv_auc, in_sample_auc, null_p, dir, base_rms, n_ok, n_bad, eff_n, rho_bar })
    }

    /// Effective sample size, and the mean pairwise correlation that is NOT it.
    ///
    /// The first version of this estimated a design effect from the observed correlation between
    /// centred rows, and it reported exactly zero correlation at every level of simulated
    /// fork-sharing. That was not a bug in the estimator, it was the estimator being impossible:
    /// when every row in the round comes from ONE fork, the shared component IS the grand mean, and
    /// centring annihilates precisely the thing it was meant to measure. Within-fork correlation is
    /// unmeasurable from within the fork.
    ///
    /// Worse, cross-validating over rows from a single fork does not merely fail to notice — it
    /// actively rewards the correlation. The same simulation, with a planted effect, gave a held-out
    /// AUC of 0.681 at rho=0 and 1.000 at rho=0.9: the more alike the branches, the better the probe
    /// looked, because every fold was scored on siblings of its own training rows. A direction that
    /// scores 1.000 that way has learned this fork and says nothing about the next one.
    ///
    /// So the unit of independence is the FORK, not the row, and `eff_n` is the fork count. Rows
    /// within a fork do carry some information — branches genuinely diverge — so the truth is
    /// somewhere between fork count and row count, and the fork count is the conservative end.
    /// `rho_bar` is still reported, and is still near zero for single-fork rounds; treat it as
    /// diagnostic, never as a gate.
    pub fn effective_n(&self) -> (f32, f32) {
        let n = self.rows.len();
        if n < 2 { return (self.forks().len() as f32, 0.0); }
        let d = self.d;
        let mut mean = vec![0f64; d];
        for r in &self.rows { for (a, x) in mean.iter_mut().zip(&r.h) { *a += *x as f64; } }
        for a in mean.iter_mut() { *a /= n as f64; }
        let cent: Vec<Vec<f64>> = self.rows.iter()
            .map(|r| r.h.iter().zip(&mean).map(|(x, m)| *x as f64 - m).collect())
            .collect();
        let norms: Vec<f64> = cent.iter().map(|c| c.iter().map(|x| x * x).sum::<f64>().sqrt()).collect();
        let (mut acc, mut pairs) = (0f64, 0usize);
        for i in 0..n {
            for j in (i + 1)..n {
                if norms[i] <= 0.0 || norms[j] <= 0.0 { continue; }
                let dp: f64 = cent[i].iter().zip(&cent[j]).map(|(a, b)| a * b).sum();
                acc += dp / (norms[i] * norms[j]);
                pairs += 1;
            }
        }
        let rho = if pairs == 0 { 0.0 } else { (acc / pairs as f64).max(0.0) };
        (self.forks().len() as f32, rho as f32)
    }

    /// Mean per-fold AUC over STRATIFIED folds.
    ///
    /// Two details here were wrong in the first version and both inflated the score:
    ///
    /// * **Folds must be stratified.** Striding `i % k` over the raw row order aliases against any
    ///   regularity in capture order — with survivors and failures recorded alternately and `k=4`,
    ///   every test fold came out single-class. Assigning fold index within each class separately
    ///   removes that, and it also keeps the permutation null MATCHED: permuted labels have to see
    ///   the same fold structure as real ones or the null is measuring a different procedure.
    /// * **AUC is computed per fold and averaged, not pooled.** Each fold fits its own direction, so
    ///   scores from different folds are on different scales and pooling them compares numbers that
    ///   were never commensurable.
    fn cv_auc(&self, labels: &[bool], k: usize) -> f32 {
        let n = self.rows.len();
        // Folds are assigned by FORK, so a fold's training rows never include a sibling of one of
        // its test rows. This is the difference between measuring "can the direction separate these
        // branches" and "will it separate the next fork's branches", and only the second is worth
        // anything. Within each fork the two classes are still striped across folds so that a fold
        // is not left single-class.
        let forks = self.forks();
        if forks.len() < 2 { return 0.5; }
        let k = k.min(forks.len());
        // Stratified GROUP k-fold. Striping forks by raw index aliases against label order — with
        // one branch per fork and classes alternating, fold `i % k` collects every fourth fork and
        // every fold comes out single-class. So forks are split by their own majority label first
        // and each list is striped separately.
        let mut pos_forks: Vec<usize> = Vec::new();
        let mut neg_forks: Vec<usize> = Vec::new();
        for f in &forks {
            let (t, n_) = (0..n).filter(|i| self.rows[*i].fork == *f)
                .fold((0usize, 0usize), |(t, n_), i| if labels[i] { (t + 1, n_) } else { (t, n_ + 1) });
            if t >= n_ { pos_forks.push(*f) } else { neg_forks.push(*f) }
        }
        let mut fold_of_fork: std::collections::BTreeMap<usize, usize> = Default::default();
        for (i, f) in pos_forks.iter().enumerate() { fold_of_fork.insert(*f, i % k); }
        for (i, f) in neg_forks.iter().enumerate() { fold_of_fork.insert(*f, i % k); }
        let mut fold = vec![usize::MAX; n];
        for i in 0..n { fold[i] = fold_of_fork[&self.rows[i].fork]; }
        let (mut acc, mut wsum) = (0f64, 0f64);
        for f in 0..k {
            let test: Vec<usize> = (0..n).filter(|i| fold[*i] == f).collect();
            let train: Vec<usize> = (0..n).filter(|i| fold[*i] != f).collect();
            let tl: Vec<bool> = train.iter().map(|i| labels[*i]).collect();
            let el: Vec<bool> = test.iter().map(|i| labels[*i]).collect();
            if tl.iter().all(|b| *b) || tl.iter().all(|b| !*b) { continue; }
            if el.iter().all(|b| *b) || el.iter().all(|b| !*b) { continue; }
            let tr: Vec<Row> = train.iter().map(|i| self.rows[*i].clone()).collect();
            let Some(dir) = fit_dir(&tr, &tl, self.fit) else { continue };
            let sc: Vec<f32> = test.iter().map(|i| dot(&self.rows[*i].h, &dir)).collect();
            let w = test.len() as f64;
            acc += auc(&sc, &el) as f64 * w;
            wsum += w;
        }
        if wsum == 0.0 { return 0.5; }
        (acc / wsum) as f32
    }
}

/// Rank layers by held-out AUC, best first.
///
/// `ambiguous_top` reports whether the winner is distinguishable from the runner-up. At n=32 the
/// standard error on an AUC is roughly 0.1, so two layers within ~0.1 of each other are not ranked,
/// they are tied — and picking the nominal winner from a tie is how a sweep over 32 layers finds a
/// "best layer" in pure noise every single time.
pub fn rank(mut results: Vec<ProbeResult>) -> (Vec<ProbeResult>, bool) {
    results.sort_by(|a, b| b.cv_auc.partial_cmp(&a.cv_auc).unwrap_or(std::cmp::Ordering::Equal));
    let ambiguous = results.len() >= 2 && (results[0].cv_auc - results[1].cv_auc) < 0.1;
    (results, ambiguous)
}

/// Branches needed PER ARM to detect a shift from `p0` to `p1` in a paired steered/unsteered trial.
///
/// Two-proportion normal approximation. This is the number that decides whether the causal test is
/// affordable, and it is usually larger than people expect — which is the useful part.
///
/// `z_alpha` and `z_beta` are passed rather than derived so the assumptions are visible at the call
/// site: `Z_95`/`Z_80` for the conventional two-sided 5% test at 80% power.
pub const Z_95: f32 = 1.959_964;
pub const Z_90: f32 = 1.644_854;
pub const Z_80: f32 = 0.841_621;
pub const Z_POWER_90: f32 = 1.281_552;

pub fn trial_arms_needed(p0: f32, p1: f32, z_alpha: f32, z_beta: f32) -> usize {
    let delta = (p1 - p0).abs();
    if delta <= 0.0 { return usize::MAX; }
    let pbar = (p0 + p1) / 2.0;
    let a = z_alpha * (2.0 * pbar * (1.0 - pbar)).sqrt();
    let b = z_beta * (p0 * (1.0 - p0) + p1 * (1.0 - p1)).sqrt();
    (((a + b) * (a + b)) / (delta * delta)).ceil() as usize
}

/// How many rounds of `width` branches, split into two arms, the trial above takes.
pub fn rounds_needed(arms: usize, width: usize) -> usize {
    if width < 2 { return usize::MAX; }
    let per_round = width / 2;
    (arms + per_round - 1) / per_round
}

// ---- primitives ----

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (*x as f64) * (*y as f64)).sum::<f64>() as f32
}

fn rms(v: &[f32]) -> f32 {
    if v.is_empty() { return 0.0; }
    ((v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / v.len() as f64).sqrt()) as f32
}

/// Unit-norm difference of class means.
///
/// Deliberately not a full linear discriminant. LDA needs an inverse covariance, and at n=32 in 4096
/// dimensions the sample covariance is singular and any regularised inverse is dominated by the
/// regulariser. Difference of means IS the discriminant under isotropic covariance, which is the
/// only assumption the data can support.
fn fit_dir(rows: &[Row], labels: &[bool], fit: Fit) -> Option<Vec<f32>> {
    let d = rows.first()?.h.len();
    let (mut mo, mut mb) = (vec![0f64; d], vec![0f64; d]);
    let (mut no, mut nb) = (0usize, 0usize);
    for (r, ok) in rows.iter().zip(labels) {
        let acc = if *ok { &mut mo } else { &mut mb };
        for (a, x) in acc.iter_mut().zip(&r.h) { *a += *x as f64; }
        if *ok { no += 1 } else { nb += 1 }
    }
    if no == 0 || nb == 0 { return None; }
    for a in mo.iter_mut() { *a /= no as f64; }
    for a in mb.iter_mut() { *a /= nb as f64; }
    let mut raw: Vec<f64> = (0..d).map(|i| mo[i] - mb[i]).collect();

    if fit == Fit::DiagLda {
        // Pooled within-class variance per dimension, then shrink toward the median so a dimension
        // that drew a near-zero spread cannot dominate the direction by dividing by almost nothing.
        let mut ss = vec![0f64; d];
        for (r, ok) in rows.iter().zip(labels) {
            let mu = if *ok { &mo } else { &mb };
            for i in 0..d { let e = r.h[i] as f64 - mu[i]; ss[i] += e * e; }
        }
        let dof = (no + nb).saturating_sub(2).max(1) as f64;
        let mut sd: Vec<f64> = ss.iter().map(|x| (x / dof).sqrt()).collect();
        let mut sorted = sd.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let s0 = sorted[d / 2].max(1e-12);
        // Divide by VARIANCE, not spread. The diagonal discriminant is `Sigma^-1 (mu1 - mu0)`, and
        // Sigma holds variances. Dividing by `s` once looks like standardisation and is not: it
        // reweights the direction but leaves each coordinate's contribution to the SCORE scaled by
        // `s` again, so a 100x dimension still injects 100x the noise into every projection. The
        // first version of this did exactly that and moved held-out AUC from 0.518 to 0.519.
        for (r, s) in raw.iter_mut().zip(sd.iter_mut()) { let v = *s + s0; *r /= v * v; }
    }

    let norm = raw.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm <= 0.0 || !norm.is_finite() { return None; }
    Some(raw.into_iter().map(|x| (x / norm) as f32).collect())
}

/// Mann-Whitney AUC with midpoint credit for ties.
fn auc(scores: &[f32], labels: &[bool]) -> f32 {
    let pos: Vec<f32> = scores.iter().zip(labels).filter(|(_, l)| **l).map(|(s, _)| *s).collect();
    let neg: Vec<f32> = scores.iter().zip(labels).filter(|(_, l)| !**l).map(|(s, _)| *s).collect();
    if pos.is_empty() || neg.is_empty() { return 0.5; }
    let mut acc = 0f64;
    for p in &pos {
        for n in &neg {
            acc += if p > n { 1.0 } else if p < n { 0.0 } else { 0.5 };
        }
    }
    (acc / (pos.len() as f64 * neg.len() as f64)) as f32
}

/// Small deterministic LCG. Matches the `rng: u64` the engine's sessions already carry; a
/// permutation test does not need a good generator, it needs a reproducible one.
pub struct Lcg(u64);
impl Lcg {
    pub fn new(seed: u64) -> Self { Lcg(seed.wrapping_mul(6364136223846793005).wrapping_add(1)) }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x = self.0;
        (x >> 33) ^ x
    }
    pub fn below(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize } }
    /// Standard normal via Box-Muller. Only used by the null study.
    pub fn normal(&mut self) -> f32 {
        let u1 = ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64).max(1e-12);
        let u2 = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
    }
}

fn shuffle<T>(v: &mut [T], rng: &mut Lcg) {
    for i in (1..v.len()).rev() { let j = rng.below(i + 1); v.swap(i, j); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planted(d: usize, n: usize, effect: f32, seed: u64) -> LayerSweep {
        // Half positive, half negative, differing by `effect` along dimension 0 and by unit noise
        // everywhere. effect=0 is the null.
        let mut rng = Lcg::new(seed);
        let mut s = LayerSweep::new(20, d);
        for i in 0..n {
            let ok = i % 2 == 0;
            let mut h: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
            h[0] += if ok { effect } else { -effect };
            s.push(i, &h, ok);
        }
        s
    }

    #[test]
    fn a_strongly_planted_direction_is_found_and_clears_the_null() {
        let s = planted(256, 40, 3.0, 7);
        let r = s.evaluate(4, 200, 1).unwrap();
        assert!(r.cv_auc > 0.8, "cv_auc {}", r.cv_auc);
        assert!(r.null_p <= MAX_NULL_P, "null_p {}", r.null_p);
        assert!(r.deployable(), "{:?}", r.refusal());
        assert!(r.dir[0].abs() > 0.5, "direction should load on dim 0, got {}", r.dir[0]);
    }

    #[test]
    fn pure_noise_is_refused_even_though_the_in_sample_fit_looks_excellent() {
        // The case this module exists for. d >> n, no signal at all.
        let s = planted(2048, 32, 0.0, 11);
        let r = s.evaluate(4, 200, 2).unwrap();
        assert!(r.in_sample_auc > 0.9, "in-sample fit on noise should look great, got {}", r.in_sample_auc);
        assert!(!r.deployable(), "cv_auc {} null_p {}", r.cv_auc, r.null_p);
    }

    fn heavy_tailed(d: usize, n_out: usize, scale: f32, effect: f32, signal_dim: usize,
                    fit: Fit, seed: u64) -> LayerSweep {
        let mut rng = Lcg::new(seed);
        let mut s = LayerSweep::new(20, d).with_fit(fit);
        for i in 0..32 {
            let ok = i % 2 == 0;
            let mut h: Vec<f32> = (0..d)
                .map(|j| (if j < n_out { scale } else { 1.0 }) * rng.normal()).collect();
            h[signal_dim] += if ok { effect } else { -effect };
            s.push(i / 4, &h, ok);
        }
        s
    }

    /// Mean held-out AUC over `seeds` independent rounds.
    ///
    /// One round is not enough to compare estimators. The per-round spread of a cross-validated AUC
    /// at n=32 runs from about 0.25 to 0.77 on data with no signal at all, so a single seed can show
    /// either estimator ahead by a wide margin for no reason. The first version of the test below
    /// used one seed and failed on noise.
    fn mean_auc(seeds: u64, fit: Fit, d: usize, n_out: usize, scale: f32,
                effect: f32, signal_dim: usize) -> f32 {
        let mut acc = 0f64;
        for s in 0..seeds {
            acc += heavy_tailed(d, n_out, scale, effect, signal_dim, fit, 1000 + s * 7919)
                .evaluate(4, 0, 1).unwrap().cv_auc as f64;
        }
        (acc / seeds as f64) as f32
    }

    #[test]
    fn outlier_dimensions_blind_a_raw_difference_of_means() {
        // Signal sits in an ordinary coordinate; eight coordinates carry 100x the scale and nothing
        // else. MeanDiff points at the loud ones and stops seeing the signal.
        let md = mean_auc(24, Fit::MeanDiff, 512, 8, 100.0, 1.5, 256);
        let dl = mean_auc(24, Fit::DiagLda, 512, 8, 100.0, 1.5, 256);
        assert!(md < 0.60, "MeanDiff should be near chance, got {md}");
        assert!(dl > md + 0.08, "DiagLda {dl} should recover over MeanDiff {md}");
    }

    #[test]
    fn with_no_outliers_the_two_estimators_agree() {
        // DiagLda must not cost anything when the scales are already uniform, or it is a fix that
        // trades one failure for another.
        let md = mean_auc(24, Fit::MeanDiff, 512, 0, 1.0, 1.5, 256);
        let dl = mean_auc(24, Fit::DiagLda, 512, 0, 1.0, 1.5, 256);
        assert!((md - dl).abs() < 0.05, "{md} vs {dl}");
    }

    #[test]
    fn standardising_costs_a_little_when_the_signal_is_in_the_loud_dimension() {
        // The honest trade. If the label really is written in an outlier coordinate, downweighting
        // it loses some detection — DiagLda is not free, it is the right default.
        let md = mean_auc(24, Fit::MeanDiff, 512, 8, 100.0, 150.0, 0);
        let dl = mean_auc(24, Fit::DiagLda, 512, 8, 100.0, 150.0, 0);
        assert!(md > 0.9 && dl > 0.8, "md {md} dl {dl}");
        assert!(dl <= md, "DiagLda {dl} should not beat MeanDiff {md} on its home ground");
    }

    #[test]
    fn diag_lda_is_the_default() {
        assert_eq!(LayerSweep::new(0, 8).fit_kind(), Fit::DiagLda);
    }

    #[test]
    fn too_few_positives_is_reported_before_anything_about_auc() {
        let mut s = LayerSweep::new(3, 8);
        for i in 0..3 { s.push(i, &[1.0; 8], true); }
        for i in 0..20 { s.push(100 + i, &[0.0; 8], false); }
        let r = s.evaluate(4, 20, 3).unwrap();
        let why = r.refusal().unwrap();
        assert!(why.contains("smaller class"), "{why}");
    }

    #[test]
    fn a_single_class_has_nothing_to_evaluate() {
        let mut s = LayerSweep::new(0, 4);
        for i in 0..10 { s.push(i, &[1.0; 4], true); }
        assert!(s.evaluate(4, 10, 4).is_none());
    }

    #[test]
    fn width_mismatch_is_dropped_not_panicked() {
        let mut s = LayerSweep::new(1, 8);
        assert!(s.push(0, &[0.0; 8], true));
        assert!(!s.push(1, &[0.0; 4], false));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn evaluation_is_reproducible_for_a_given_seed() {
        let s = planted(128, 24, 1.0, 5);
        let a = s.evaluate(4, 100, 99).unwrap();
        let b = s.evaluate(4, 100, 99).unwrap();
        assert_eq!(a.null_p, b.null_p);
        assert_eq!(a.cv_auc, b.cv_auc);
    }

    #[test]
    fn a_near_tie_at_the_top_is_reported_as_ambiguous() {
        let mk = |layer, auc| ProbeResult {
            layer, cv_auc: auc, in_sample_auc: 0.99, null_p: 0.01,
            dir: vec![1.0], base_rms: 1.0, n_ok: 16, n_bad: 16, eff_n: 32.0, rho_bar: 0.0,
        };
        let (ranked, amb) = rank(vec![mk(4, 0.70), mk(20, 0.74), mk(28, 0.55)]);
        assert_eq!(ranked[0].layer, 20);
        assert!(amb, "0.74 vs 0.70 is inside the noise at n=32");
        let (_, amb2) = rank(vec![mk(20, 0.88), mk(4, 0.55)]);
        assert!(!amb2);
    }

    #[test]
    fn auc_is_half_for_a_constant_score_and_one_for_perfect_separation() {
        assert!((auc(&[1.0, 1.0, 1.0, 1.0], &[true, false, true, false]) - 0.5).abs() < 1e-6);
        assert!((auc(&[2.0, 1.0, 3.0, 0.0], &[true, false, true, false]) - 1.0).abs() < 1e-6);
        assert!((auc(&[0.0, 1.0, 0.5, 3.0], &[true, false, true, false]) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn the_causal_trial_is_priced_before_it_is_run() {
        // The 22% -> 48% correction-efficiency shift from the feedback discussion.
        let arms = trial_arms_needed(0.22, 0.48, Z_95, Z_80);
        assert!((50..=62).contains(&arms), "arms {arms}");
        assert_eq!(rounds_needed(arms, 32), 4, "32-wide round, two arms of 16");
        assert_eq!(rounds_needed(arms, 16), 7);
        // A subtler effect is far more expensive, which is the thing worth knowing up front.
        assert!(trial_arms_needed(0.22, 0.30, Z_95, Z_80) > 400);
    }

    #[test]
    fn an_effect_of_zero_can_never_be_detected() {
        assert_eq!(trial_arms_needed(0.4, 0.4, Z_95, Z_80), usize::MAX);
    }
}
