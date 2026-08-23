//! TRAINWATCH — seeing what the network can make, while it is still learning.
//!
//! `bench.rs` consumes a checkpoint and nothing watches the run that produces
//! one. This is that window, and it is deliberately NOT a second copy of
//! `beatspace`'s trainpage: that already models the loss curve, the trend, the
//! stall check and the log ring, and those are domain-free. What it cannot
//! have is the half that matters here.
//!
//! ## Loss tells you the run is broken; it does not tell you it is boring
//!
//! A GAN can sit at a perfectly healthy-looking loss while emitting the same
//! frame every time. Mode collapse is invisible in `d_loss` and `g_loss` —
//! **it is only visible in the samples** — and for voxels there is a way to
//! see it that does not need eyes: `voxel::CLUSTERS` is fourteen part classes
//! measured off the real library, so a healthy generator should be producing
//! parts that land across several of them, and a collapsed one lands in one.
//!
//! Cluster coverage is the readout. It answers the question a person actually
//! has while a run is going — *what can it make yet* — and it answers it in a
//! number.
//!
//! ## The discriminator failure has a numeric signature, already written down
//!
//! `beatspace/src/model.rs` records what a frozen discriminator looks like,
//! from having had one:
//!
//! > d_loss pinned at 2ln2, g_loss at ln2, flat for forty points, after a
//! > vertical fall. A frozen input-independent D scores exactly there.
//!
//! That is detectable, and detecting it is worth more than plotting it: those
//! two constants are where a discriminator that has stopped depending on its
//! input lands, because it is outputting the same logit for everything.
//! `Health::FrozenDiscriminator` is that check.

/// What a discriminator that has stopped looking at its input scores.
pub const D_FROZEN: f32 = 2.0 * std::f32::consts::LN_2;
/// And its generator.
pub const G_FROZEN: f32 = std::f32::consts::LN_2;
/// How close counts as pinned.
pub const PIN_EPS: f32 = 0.02;
/// Points of history a flatness check needs, from beatspace's own stall rule.
pub const FLAT_WINDOW: usize = 40;
/// Slope below which a curve counts as flat.
pub const FLAT_EPS: f32 = 0.001;

/// One epoch's readout.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub epoch: u32,
    pub d_loss: f32,
    pub g_loss: f32,
    /// how many of the fourteen part classes the batch landed in
    pub clusters_hit: u8,
    /// mean critic score over the batch
    pub critic: f32,
}

/// What the run is doing, in the order a person wants to be told.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Health {
    /// too early to say
    Warming,
    /// the signature from `beatspace/src/model.rs`: D has stopped depending
    /// on its input and both losses are pinned at their fixed point
    FrozenDiscriminator,
    /// losses look fine and the generator makes one thing
    ModeCollapse { clusters: u8 },
    /// nothing is moving, and not at the frozen point either
    Stalled,
    Learning,
}

impl Health {
    /// What to tell the operator. Losses being healthy is exactly what makes
    /// mode collapse worth calling out loudly.
    pub fn message(&self) -> String {
        match self {
            Health::Warming => "warming up".into(),
            Health::FrozenDiscriminator => format!(
                "FROZEN DISCRIMINATOR — d at {D_FROZEN:.3}, g at {G_FROZEN:.3}, flat. \
                 It has stopped depending on its input; the gate has no gradient below \
                 threshold. Restart from a checkpoint before the fall."
            ),
            Health::ModeCollapse { clusters } => format!(
                "MODE COLLAPSE — the losses look fine and it is making {clusters} kind(s) \
                 of part out of {}. Loss cannot see this.",
                crate::voxel::CLUSTERS.len()
            ),
            Health::Stalled => "stalled — flat, but not at the frozen point".into(),
            Health::Learning => "learning".into(),
        }
    }
}

/// Below this many distinct clusters, a run is making one thing.
pub const COLLAPSE_AT: u8 = 3;

pub struct Watch {
    pub curve: Vec<Point>,
    /// how much history to keep; a long run prints continuously
    pub cap: usize,
}

impl Default for Watch {
    fn default() -> Self {
        Watch { curve: Vec::new(), cap: 4000 }
    }
}

impl Watch {
    pub fn push(&mut self, p: Point) {
        self.curve.push(p);
        if self.curve.len() > self.cap {
            let drop = self.curve.len() - self.cap;
            self.curve.drain(..drop);
        }
    }

    pub fn latest(&self) -> Option<&Point> {
        self.curve.last()
    }

    /// Slope of the last `n` points of a series, per epoch. Positive falling.
    fn trend(&self, n: usize, f: impl Fn(&Point) -> f32) -> Option<f32> {
        if self.curve.len() < n {
            return None;
        }
        let w = &self.curve[self.curve.len() - n..];
        let first = f(&w[0]);
        let last = f(&w[w.len() - 1]);
        Some((first - last) / n as f32)
    }

    fn flat(&self, n: usize, f: impl Fn(&Point) -> f32) -> bool {
        matches!(self.trend(n, f), Some(t) if t.abs() < FLAT_EPS)
    }

    /// The verdict.
    ///
    /// Order matters. A frozen discriminator ALSO looks like mode collapse
    /// and like a stall, and telling somebody "mode collapse" when the real
    /// problem is a dead gate sends them to tune the wrong thing.
    pub fn health(&self) -> Health {
        if self.curve.len() < FLAT_WINDOW {
            return Health::Warming;
        }
        let Some(last) = self.latest() else { return Health::Warming };

        let pinned = (last.d_loss - D_FROZEN).abs() < PIN_EPS
            && (last.g_loss - G_FROZEN).abs() < PIN_EPS;
        if pinned && self.flat(FLAT_WINDOW, |p| p.d_loss) {
            return Health::FrozenDiscriminator;
        }
        if last.clusters_hit < COLLAPSE_AT {
            return Health::ModeCollapse { clusters: last.clusters_hit };
        }
        if self.flat(FLAT_WINDOW, |p| p.d_loss) && self.flat(FLAT_WINDOW, |p| p.critic) {
            return Health::Stalled;
        }
        Health::Learning
    }

    /// Is the range of things it makes still growing?
    ///
    /// The question "what can it make yet", as a number. Rising coverage is
    /// the only signal here that says the run is going somewhere INTERESTING
    /// rather than merely somewhere.
    pub fn coverage_trend(&self) -> Option<f32> {
        self.trend(FLAT_WINDOW.min(self.curve.len()), |p| -(p.clusters_hit as f32))
    }

    pub fn best_coverage(&self) -> u8 {
        self.curve.iter().map(|p| p.clusters_hit).max().unwrap_or(0)
    }

    /// A line an operator can paste into a message.
    pub fn summary(&self) -> String {
        let Some(l) = self.latest() else { return "no data".into() };
        format!(
            "epoch {} · d {:.3} g {:.3} · critic {:.2} · {}/{} classes (best {}) · {}",
            l.epoch,
            l.d_loss,
            l.g_loss,
            l.critic,
            l.clusters_hit,
            crate::voxel::CLUSTERS.len(),
            self.best_coverage(),
            self.health().message().split(" —").next().unwrap_or("")
        )
    }
}

/// Read a batch of generated volumes into a `Point`.
///
/// This is the bridge: the same critics that shape the reward are the readout
/// for the run. Nothing else has to be instrumented.
pub fn observe(
    epoch: u32,
    d_loss: f32,
    g_loss: f32,
    batch: &[crate::voxel::Volume],
) -> Point {
    use std::collections::BTreeSet;
    let mut hit: BTreeSet<u8> = BTreeSet::new();
    let mut critic = 0.0;
    for v in batch {
        let cs = crate::voxel::components(v, 6);
        if cs.is_empty() {
            continue;
        }
        critic += crate::voxel::reward(0.5, &cs);
        for c in cs.iter().take(4) {
            hit.insert(crate::voxel::cluster_match(c).1.id);
        }
    }
    Point {
        epoch,
        d_loss,
        g_loss,
        clusters_hit: hit.len() as u8,
        critic: critic / batch.len().max(1) as f32,
    }
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("TRAINWATCH — what the network can make, while it learns\n");
    println!("  beatspace's trainpage already models the loss curve, the trend, the");
    println!("  stall check and the log ring, and those are domain-free. This is the");
    println!("  half it cannot have.\n");

    println!("  1. LOSS CANNOT SEE MODE COLLAPSE\n");
    let mut healthy = Watch::default();
    let mut collapsed = Watch::default();
    for e in 0..60u32 {
        let d = 1.0 - (e as f32) * 0.004;
        let g = 0.8 + (e as f32) * 0.002;
        // identical losses; the only difference is what comes out
        healthy.push(Point { epoch: e, d_loss: d, g_loss: g, clusters_hit: 6, critic: 0.6 });
        collapsed.push(Point { epoch: e, d_loss: d, g_loss: g, clusters_hit: 1, critic: 0.6 });
    }
    println!("    same losses, both runs:");
    println!("      {:<12} {}", "healthy", healthy.health().message());
    println!("      {:<12} {}", "collapsed", collapsed.health().message());

    println!("\n  2. THE FROZEN DISCRIMINATOR HAS A NUMBER\n");
    println!("    beatspace/src/model.rs, from having had one:");
    println!("      \"d_loss pinned at 2ln2, g_loss at ln2, flat for forty points,");
    println!("       after a vertical fall. A frozen input-independent D scores");
    println!("       exactly there.\"");
    let mut frozen = Watch::default();
    for e in 0..60u32 {
        frozen.push(Point {
            epoch: e,
            d_loss: D_FROZEN,
            g_loss: G_FROZEN,
            clusters_hit: 1,
            critic: 0.5,
        });
    }
    println!("\n    d={D_FROZEN:.4} g={G_FROZEN:.4} flat ->");
    println!("      {}", frozen.health().message());
    println!("\n    reported BEFORE mode collapse on purpose: a dead gate also looks");
    println!("    like a boring generator, and telling somebody to tune diversity");
    println!("    when the discriminator is frozen sends them at the wrong thing.");

    println!("\n  3. WHAT IT CAN MAKE YET\n");
    let mut run = Watch::default();
    let mut s = 12345u64;
    for e in 0..200u32 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let jit = ((s >> 33) as f32 / (u32::MAX as f32 / 2.0) - 0.5) * 0.05;
        let progress = (e as f32 / 200.0).min(1.0);
        run.push(Point {
            epoch: e,
            d_loss: 1.35 - progress * 0.5 + jit,
            g_loss: 0.70 + progress * 0.3 + jit,
            clusters_hit: (1.0 + progress * 8.0) as u8,
            critic: 0.45 + progress * 0.3 + jit,
        });
        if e % 50 == 0 || e == 199 {
            println!("    {}", run.summary());
        }
    }
    println!(
        "      coverage trend {:+.3}/epoch",
        run.coverage_trend().unwrap_or(0.0)
    );

    // The bridge, on real volumes rather than synthetic points — this is what
    // a trainer actually calls once per epoch, and nothing else needs
    // instrumenting.
    println!("\n  4. THE BRIDGE — a real batch through the same critics\n");
    for (label, n) in [("dust (early)", 200usize), ("forming", 500), ("solid", 900)] {
        let batch: Vec<crate::voxel::Volume> =
            (0..6u64).map(|s| crate::voxel::noise_volume(s, n)).collect();
        let p = observe(0, 1.2, 0.8, &batch);
        println!(
            "    {:<14} {:>4} voxels/sample  ->  critic {:.2}, {}/{} classes",
            label,
            n,
            p.critic,
            p.clusters_hit,
            crate::voxel::CLUSTERS.len()
        );
    }
    println!("\n    dust reads as zero on both, and that is a diagnosis rather than a");
    println!("    hole: a generator emitting specks forms nothing big enough to judge,");
    println!("    which is what an early run looks like.");

    println!(
        "\n    coverage went 1 -> {} of {}. That rising number is the only signal",
        run.best_coverage(),
        crate::voxel::CLUSTERS.len()
    );
    println!("    here that says the run is going somewhere INTERESTING rather than");
    println!("    merely somewhere — and it comes free, because the critics that");
    println!("    shape the reward are the readout.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatline(d: f32, g: f32, clusters: u8, n: u32) -> Watch {
        let mut w = Watch::default();
        for e in 0..n {
            w.push(Point { epoch: e, d_loss: d, g_loss: g, clusters_hit: clusters, critic: 0.5 });
        }
        w
    }

    #[test]
    fn a_short_run_says_it_does_not_know_yet() {
        // a verdict from four points is a guess wearing a diagnosis
        let w = flatline(1.0, 0.7, 6, (FLAT_WINDOW as u32) - 1);
        assert_eq!(w.health(), Health::Warming);
        assert_eq!(Watch::default().health(), Health::Warming);
        assert_eq!(Watch::default().summary(), "no data");
    }

    #[test]
    fn the_frozen_discriminator_is_caught_by_its_own_numbers() {
        // the signature beatspace wrote down from having had one
        let w = flatline(D_FROZEN, G_FROZEN, 1, 60);
        assert_eq!(w.health(), Health::FrozenDiscriminator);
        // and it is not just "flat": a flat run somewhere else is a stall
        let elsewhere = flatline(0.4, 0.9, 6, 60);
        assert_eq!(elsewhere.health(), Health::Stalled);
    }

    #[test]
    fn frozen_is_reported_before_collapse_because_it_explains_it() {
        // a dead gate ALSO looks like a boring generator. Reporting mode
        // collapse there sends somebody to tune diversity when the real
        // problem is that no unit below threshold can receive a gradient.
        let w = flatline(D_FROZEN, G_FROZEN, 1, 60);
        assert_eq!(w.health(), Health::FrozenDiscriminator, "the diagnosis was shadowed");
        assert!(w.health().message().contains("gradient"));
    }

    #[test]
    fn loss_cannot_see_mode_collapse_and_this_can() {
        // THE POINT OF THE MODULE. Identical curves, different outputs.
        let mut healthy = Watch::default();
        let mut collapsed = Watch::default();
        for e in 0..60u32 {
            let (d, g) = (1.0 - e as f32 * 0.004, 0.8 + e as f32 * 0.002);
            healthy.push(Point { epoch: e, d_loss: d, g_loss: g, clusters_hit: 7, critic: 0.6 });
            collapsed.push(Point { epoch: e, d_loss: d, g_loss: g, clusters_hit: 1, critic: 0.6 });
        }
        assert_eq!(healthy.health(), Health::Learning);
        assert_eq!(collapsed.health(), Health::ModeCollapse { clusters: 1 });
        // and the two loss curves are literally identical
        for (a, b) in healthy.curve.iter().zip(collapsed.curve.iter()) {
            assert_eq!(a.d_loss, b.d_loss);
            assert_eq!(a.g_loss, b.g_loss);
        }
    }

    #[test]
    fn coverage_is_the_signal_that_it_is_going_somewhere_interesting() {
        let mut w = Watch::default();
        for e in 0..80u32 {
            w.push(Point {
                epoch: e,
                d_loss: 1.2 - e as f32 * 0.005,
                g_loss: 0.7,
                clusters_hit: (1 + e / 10) as u8,
                critic: 0.5 + e as f32 * 0.002,
            });
        }
        assert!(w.coverage_trend().unwrap() > 0.0, "coverage should be rising");
        assert!(w.best_coverage() >= 8);
        assert_eq!(w.health(), Health::Learning);
    }

    #[test]
    fn history_is_capped_so_a_long_run_does_not_grow_forever() {
        let mut w = Watch { curve: Vec::new(), cap: 100 };
        for e in 0..5000u32 {
            w.push(Point { epoch: e, d_loss: 1.0, g_loss: 0.7, clusters_hit: 5, critic: 0.5 });
        }
        assert_eq!(w.curve.len(), 100);
        // and it keeps the RECENT end, which is the interesting one
        assert_eq!(w.latest().unwrap().epoch, 4999);
    }

    #[test]
    fn observe_reads_a_batch_through_the_same_critics_that_train_it() {
        // the bridge: nothing else has to be instrumented
        let batch: Vec<crate::voxel::Volume> =
            (0..4u64).map(|s| crate::voxel::noise_volume(s, 900)).collect();
        let p = observe(7, 1.1, 0.8, &batch);
        assert_eq!(p.epoch, 7);
        assert!(p.critic > 0.0 && p.critic <= 1.5, "critic {}", p.critic);
        assert!(p.clusters_hit >= 1, "a batch landed in no class at all");
        assert!(
            (p.clusters_hit as usize) <= crate::voxel::CLUSTERS.len(),
            "more classes than exist"
        );
    }

    #[test]
    fn dust_reads_as_zero_and_that_is_a_diagnosis() {
        // A generator emitting scattered specks forms no component big enough
        // to judge, so critic and coverage are both zero. That is not a hole
        // in the readout — it is the readout saying the output is not
        // anything yet, which is exactly what an early run looks like.
        let dust: Vec<crate::voxel::Volume> =
            (0..4u64).map(|s| crate::voxel::noise_volume(s, 200)).collect();
        let p = observe(1, 1.0, 0.7, &dust);
        assert_eq!(p.clusters_hit, 0);
        assert_eq!(p.critic, 0.0);
        // and a watch full of it reports collapse rather than "learning"
        let mut w = Watch::default();
        for e in 0..60 {
            w.push(Point { epoch: e, ..p });
        }
        assert_eq!(w.health(), Health::ModeCollapse { clusters: 0 });
    }

    #[test]
    fn an_empty_batch_does_not_divide_by_zero() {
        let p = observe(0, 1.0, 0.7, &[]);
        assert_eq!(p.clusters_hit, 0);
        assert!(p.critic.is_finite());
    }

    #[test]
    fn the_summary_is_one_pasteable_line() {
        let mut w = flatline(1.0, 0.7, 5, 60);
        w.push(Point { epoch: 60, d_loss: 0.9, g_loss: 0.75, clusters_hit: 9, critic: 0.71 });
        let s = w.summary();
        assert!(!s.contains('\n'), "a summary with a newline is not pasteable");
        assert!(s.contains("epoch 60") && s.contains("9/14"), "{s}");
        assert!(s.len() < 140, "{} chars", s.len());
    }
}
