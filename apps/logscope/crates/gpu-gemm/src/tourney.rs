//! tourney.rs — successive halving over layers, and the thing it cannot do.
//!
//! `probe.rs` evaluates every layer at once. That is the wrong allocation: a flat sweep spends the
//! same captures on layer 3 (which nothing expects to carry the signal) as on layer 22, and it runs
//! 32 simultaneous tests at a 5% false-positive rate each, so about 1.6 layers come back
//! "deployable" on data with no signal in it. Halving fixes the allocation. It does not fix the
//! inference, and conflating the two is the trap this module is shaped around.
//!
//! ## What halving buys
//!
//! Arms that lose get no more captures. Over a few rounds the budget concentrates on the region
//! that keeps winning, which is the correct thing to do with a fixed number of branches per round.
//!
//! ## What halving cannot buy
//!
//! **A winner.** If every arm is noise, one of them still wins every cut, and after five halvings
//! from 32 arms exactly one is left holding a confident-looking record of five straight victories.
//! Its `null_survival` is 1/32, which is not a p-value for "this layer carries signal" — it is the
//! probability that THIS PARTICULAR arm was the one the noise favoured. Something always wins.
//! `examples/tourney_null.rs` runs it on pure noise and the tournament produces a champion every
//! single time.
//!
//! So the tournament is an allocation strategy and `conclude` refuses to name a layer until a
//! permutation null has been run on FRESH captures from the survivor. Halving picks where to look;
//! the null says whether there is anything there.
//!
//! ## Fresh data per round is a correctness boundary, not a nicety
//!
//! Re-scoring survivors on the captures that selected them is fitting to the test set with extra
//! steps: the survivors are, by construction, the arms that looked best on those rows. `settle`
//! therefore takes scores for exactly the currently-alive arms and refuses anything else, so
//! resubmitting a stale round is an error rather than an optimistic result.
//!
//! ## What pruning layers actually frees
//!
//! Not forward passes — those run whether you capture one layer or all of them. It frees
//! **readbacks** during capture (a `download` per alive layer per branch, on a path where a stray
//! readback used to cost the whole frame) and, once steering is on, **dispatches**: `SteerBank`
//! issues one `add_bias_t` per steered layer per token, so 32 branches steered at 4 layers is 128
//! extra dispatches a step against 1024 at all 32. That lands in the currency `PLATFORM_VALIDATION`
//! already tracks. It does not free KV bytes, and KV bytes are what set the branch count.
//!
//! Pure: no wgpu, no model.

/// One layer under test.
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    pub layer: usize,
    pub alive: bool,
    /// Cuts this arm has survived. Feeds `null_survival`.
    pub rounds_survived: usize,
    /// Score from the most recent round it took part in.
    pub last: f32,
}

/// The outcome of one cut.
#[derive(Clone, Debug)]
pub struct Cut {
    pub round: usize,
    pub kept: Vec<usize>,
    pub dropped: Vec<usize>,
    /// Lowest score that survived.
    pub threshold: f32,
    /// Are the survivors a contiguous run of layers?
    ///
    /// Usually yes, and that is worth surfacing rather than celebrating. The residual stream is
    /// additive — `h[l+1] = h[l] + F(h[l])` — so adjacent layers are strongly correlated and the
    /// arms are nothing like independent. Four contiguous survivors are one finding about a region,
    /// not four findings, which cuts both ways: fewer effective tests than 32, and no reason to
    /// expect the survivors to be complementary steering sites.
    pub contiguous: bool,
}

pub struct Tournament {
    arms: Vec<Arm>,
    keep: f32,
    min_arms: usize,
    round: usize,
    cuts: Vec<Cut>,
}

impl Tournament {
    /// `keep` is the fraction of live arms carried into the next round (0.5 is halving). `min_arms`
    /// is the floor the tournament stops at.
    pub fn new(layers: impl IntoIterator<Item = usize>, keep: f32, min_arms: usize) -> Self {
        let arms = layers.into_iter()
            .map(|layer| Arm { layer, alive: true, rounds_survived: 0, last: f32::NAN })
            .collect();
        Tournament { arms, keep: keep.clamp(0.05, 0.95), min_arms: min_arms.max(1), round: 0, cuts: Vec::new() }
    }

    pub fn round(&self) -> usize { self.round }
    pub fn cuts(&self) -> &[Cut] { &self.cuts }
    /// An arm's record, including the score from the last round it ran in.
    pub fn arm(&self, layer: usize) -> Option<Arm> {
        self.arms.iter().find(|a| a.layer == layer).copied()
    }
    /// The live arm with the best most-recent score. Where to spend the fresh-data null.
    pub fn leader(&self) -> Option<usize> {
        self.arms.iter().filter(|a| a.alive && a.last.is_finite())
            .max_by(|a, b| a.last.partial_cmp(&b.last).unwrap())
            .map(|a| a.layer)
    }
    pub fn alive(&self) -> Vec<usize> {
        self.arms.iter().filter(|a| a.alive).map(|a| a.layer).collect()
    }
    pub fn finished(&self) -> bool { self.alive().len() <= self.min_arms }

    /// Readbacks this round costs, at `branches` captures per layer. Forward passes are unaffected;
    /// this is the number pruning actually reduces.
    pub fn downloads(&self, branches: usize) -> usize { self.alive().len() * branches }

    /// Total readbacks to run the tournament out, against probing every layer for the same number of
    /// rounds. The ratio is the honest statement of what pruning saves.
    pub fn downloads_projected(&self, branches: usize) -> (usize, usize) {
        let (mut tourney, mut live) = (0usize, self.alive().len());
        let mut rounds = 0usize;
        while live > self.min_arms {
            tourney += live * branches;
            live = self.next_size(live);
            rounds += 1;
        }
        tourney += live * branches;
        (tourney, self.arms.len() * branches * (rounds + 1))
    }

    fn next_size(&self, live: usize) -> usize {
        let want = ((live as f32) * self.keep).ceil() as usize;
        want.max(self.min_arms).min(live.saturating_sub(1).max(self.min_arms))
    }

    /// Apply one round's scores and cut. `scores` must name exactly the live arms — no extras, no
    /// omissions, no duplicates.
    ///
    /// Higher is better. Feed held-out AUC from `probe::LayerSweep::evaluate`, never the in-sample
    /// number: a tournament ranked on in-sample fit ranks on how well each layer's direction
    /// memorised its own captures, which at n=32 in 4096 dimensions is identical for every layer.
    pub fn settle(&mut self, scores: &[(usize, f32)]) -> Result<Cut, String> {
        if self.finished() {
            return Err(format!(
                "tournament is down to {} arm(s) at the floor of {}; run the fresh-data null and \
                 conclude rather than cutting further",
                self.alive().len(), self.min_arms));
        }
        let live = self.alive();
        let mut seen: Vec<usize> = scores.iter().map(|(l, _)| *l).collect();
        seen.sort_unstable();
        let mut want = live.clone();
        want.sort_unstable();
        if seen != want {
            return Err(format!(
                "round {} expects scores for exactly the {} live arms {:?}, got {:?} — a stale or \
                 partial round would let survivors be re-scored on the captures that selected them",
                self.round, live.len(), want, seen));
        }
        if scores.iter().any(|(_, s)| !s.is_finite()) {
            return Err(format!("round {}: a non-finite score cannot be ranked", self.round));
        }

        let mut ranked = scores.to_vec();
        // Ties break toward the lower layer index: arbitrary, but fixed, so a tie does not make the
        // cut depend on hash order.
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
        let keep_n = self.next_size(live.len());
        let kept: Vec<usize> = ranked[..keep_n].iter().map(|(l, _)| *l).collect();
        let threshold = ranked[keep_n - 1].1;
        let dropped: Vec<usize> = ranked[keep_n..].iter().map(|(l, _)| *l).collect();

        for a in self.arms.iter_mut() {
            if let Some((_, s)) = scores.iter().find(|(l, _)| *l == a.layer) { a.last = *s; }
            if a.alive && !kept.contains(&a.layer) { a.alive = false; }
            else if a.alive { a.rounds_survived += 1; }
        }

        let mut sorted = kept.clone();
        sorted.sort_unstable();
        let contiguous = sorted.windows(2).all(|w| w[1] == w[0] + 1);

        let cut = Cut { round: self.round, kept, dropped, threshold, contiguous };
        self.round += 1;
        self.cuts.push(cut.clone());
        Ok(cut)
    }

    /// Name a layer, but only on fresh evidence.
    ///
    /// `fresh` must be a `ProbeResult` computed from captures taken AFTER the last cut, at a layer
    /// that survived. Survival record is not evidence and is not consulted here beyond membership —
    /// that is the whole point.
    pub fn conclude(&self, fresh: &crate::probe::ProbeResult) -> Result<usize, String> {
        let live = self.alive();
        if !live.contains(&fresh.layer) {
            return Err(format!("layer {} did not survive the tournament (live: {live:?})", fresh.layer));
        }
        if let Some(why) = fresh.refusal() {
            return Err(format!(
                "layer {} won {} cut(s), which a signal-free arm does with probability {:.3} — and the \
                 fresh null says: {why}",
                fresh.layer,
                self.arms.iter().find(|a| a.layer == fresh.layer).map_or(0, |a| a.rounds_survived),
                null_survival(self.keep, self.cuts.len())));
        }
        Ok(fresh.layer)
    }
}

/// Probability that an arm carrying no signal at all survives `rounds` cuts that each keep `keep` of
/// the field. Small numbers here are not evidence: with 32 arms all of them null, the expected
/// number surviving is `32 * keep^rounds`, which is 1 by construction at the end of a halving.
pub fn null_survival(keep: f32, rounds: usize) -> f32 { keep.powi(rounds as i32) }

/// Expected number of signal-free arms still standing after `rounds` cuts, from `start` arms. When
/// this is at or above 1, a champion proves nothing.
pub fn expected_null_survivors(start: usize, keep: f32, rounds: usize) -> f32 {
    start as f32 * null_survival(keep, rounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t32() -> Tournament { Tournament::new(0..32, 0.5, 2) }

    #[test]
    fn halving_cuts_the_field_and_keeps_the_top_scores() {
        let mut t = t32();
        let scores: Vec<(usize, f32)> = (0..32).map(|l| (l, l as f32 / 32.0)).collect();
        let c = t.settle(&scores).unwrap();
        assert_eq!(c.kept.len(), 16);
        assert_eq!(c.dropped.len(), 16);
        assert!(c.kept.contains(&31) && c.kept.contains(&16));
        assert!(!c.kept.contains(&15));
        assert_eq!(t.alive().len(), 16);
        assert!(c.contiguous, "16..=31 is a contiguous run");
    }

    #[test]
    fn a_stale_or_partial_round_is_refused() {
        let mut t = t32();
        t.settle(&(0..32).map(|l| (l, l as f32)).collect::<Vec<_>>()).unwrap();
        // Resubmitting the full field after a cut: the dropped arms are no longer live.
        let err = t.settle(&(0..32).map(|l| (l, l as f32)).collect::<Vec<_>>()).unwrap_err();
        assert!(err.contains("exactly the 16 live arms"), "{err}");
        // Omitting a live arm is refused too.
        let live = t.alive();
        let partial: Vec<(usize, f32)> = live[1..].iter().map(|l| (*l, 1.0)).collect();
        assert!(t.settle(&partial).is_err());
    }

    #[test]
    fn the_tournament_stops_at_the_floor_instead_of_naming_a_single_winner() {
        let mut t = Tournament::new(0..8, 0.5, 2);
        let mut r = 0;
        while !t.finished() {
            let s: Vec<(usize, f32)> = t.alive().iter().map(|l| (*l, *l as f32)).collect();
            t.settle(&s).unwrap();
            r += 1;
            assert!(r < 10, "should converge");
        }
        assert_eq!(t.alive().len(), 2);
        let s: Vec<(usize, f32)> = t.alive().iter().map(|l| (*l, *l as f32)).collect();
        assert!(t.settle(&s).unwrap_err().contains("floor"));
    }

    #[test]
    fn non_contiguous_survivors_are_reported_as_such() {
        let mut t = Tournament::new([2usize, 3, 20, 21], 0.5, 2);
        let c = t.settle(&[(2, 0.9), (3, 0.1), (20, 0.8), (21, 0.2)]).unwrap();
        assert_eq!(c.kept.len(), 2);
        assert!(!c.contiguous, "layers 2 and 20 are not adjacent");
    }

    #[test]
    fn ties_break_deterministically_toward_the_lower_layer() {
        let mut a = Tournament::new(0..4, 0.5, 2);
        let mut b = Tournament::new(0..4, 0.5, 2);
        let flat = [(0usize, 0.5f32), (1, 0.5), (2, 0.5), (3, 0.5)];
        let ca = a.settle(&flat).unwrap();
        let cb = b.settle(&[(3, 0.5), (1, 0.5), (0, 0.5), (2, 0.5)]).unwrap();
        assert_eq!(ca.kept, cb.kept);
        assert_eq!(ca.kept, vec![0, 1]);
    }

    #[test]
    fn a_non_finite_score_is_refused_rather_than_sorted() {
        let mut t = Tournament::new(0..4, 0.5, 2);
        let err = t.settle(&[(0, 0.5), (1, f32::NAN), (2, 0.7), (3, 0.1)]).unwrap_err();
        assert!(err.contains("non-finite"));
    }

    #[test]
    fn survival_alone_is_never_a_conclusion() {
        // A champion with a perfect record, and a fresh probe that fails its null. `conclude` must
        // report the survival probability alongside the refusal, because "won five straight" is
        // exactly what one signal-free arm out of 32 does.
        let mut t = t32();
        while !t.finished() {
            let s: Vec<(usize, f32)> = t.alive().iter().map(|l| (*l, *l as f32)).collect();
            t.settle(&s).unwrap();
        }
        let champ = *t.alive().last().unwrap();
        let bad = crate::probe::ProbeResult {
            layer: champ, cv_auc: 0.52, in_sample_auc: 1.0, null_p: 0.61,
            dir: vec![1.0], base_rms: 1.0, n_ok: 16, n_bad: 16, eff_n: 32.0, rho_bar: 0.0,
        };
        let err = t.conclude(&bad).unwrap_err();
        assert!(err.contains("probability"), "{err}");

        let good = crate::probe::ProbeResult { null_p: 0.005, cv_auc: 0.81, ..bad };
        assert_eq!(t.conclude(&good).unwrap(), champ);
    }

    #[test]
    fn a_layer_that_lost_cannot_be_concluded_even_with_a_clean_null() {
        let mut t = t32();
        t.settle(&(0..32).map(|l| (l, l as f32)).collect::<Vec<_>>()).unwrap();
        let loser = crate::probe::ProbeResult {
            layer: 0, cv_auc: 0.95, in_sample_auc: 1.0, null_p: 0.001,
            dir: vec![1.0], base_rms: 1.0, n_ok: 16, n_bad: 16, eff_n: 32.0, rho_bar: 0.0,
        };
        assert!(t.conclude(&loser).unwrap_err().contains("did not survive"));
    }

    #[test]
    fn one_null_arm_is_expected_to_survive_a_full_halving_from_32() {
        // The arithmetic behind "something always wins".
        assert!((expected_null_survivors(32, 0.5, 5) - 1.0).abs() < 1e-4);
        assert!(expected_null_survivors(32, 0.5, 3) > 1.0);
    }

    #[test]
    fn pruning_saves_readbacks_and_the_saving_is_reported() {
        let t = t32();
        let (tourney, flat) = t.downloads_projected(32);
        assert!(tourney < flat, "{tourney} vs {flat}");
        // Halving 32 -> 16 -> 8 -> 4 -> 2 is four cuts, so five rounds in all. A flat sweep pays
        // 32 layers every round; the tournament pays 32+16+8+4+2 = 62.
        assert_eq!(flat, 32 * 32 * 5);
        assert_eq!(tourney, 62 * 32);
        assert_eq!(t.downloads(32), 32 * 32, "first round probes every layer either way");
    }
}
