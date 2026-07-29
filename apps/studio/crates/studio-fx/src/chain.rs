//! chain — the signal path, as a thing you can see and rearrange.
//!
//! The third piece of the Ozone grammar, after the shared canvas and the
//! meters: a strip showing what the signal actually passes through, in order,
//! with each stage bypassable and draggable. It matters because **order changes
//! the sound** and most interfaces hide that — an EQ before a compressor makes
//! the compressor react to the boosted band, while the same EQ after it does
//! not, and those are audibly different masters. A test asserts the difference
//! rather than asserting the theory.
//!
//! Two things the strip has to get right beyond drawing:
//!
//! * **Bypass must be true bypass.** A bypassed stage has to be identical to it
//!   not being in the chain at all — not "nearly", because the whole point of
//!   the button is to answer "is this helping?" honestly.
//! * **Latency must be reported.** The limiter has lookahead, so a chain
//!   containing one delays the signal. A host that does not know by how much
//!   will drift this track against every other one, and the error is a constant
//!   few milliseconds that sounds like sloppy playing rather than a bug.

use crate::{Compressor, Eq, Limiter, imager::Imager, multiband::Multiband, saturation::Saturator};

/// One stage of the chain.
pub enum Module {
    Eq(Eq),
    Compressor(Compressor),
    Multiband(Box<Multiband>),
    Saturator(Saturator),
    Imager(Box<Imager>),
    /// Always last in a mastering chain, and the reason `Chain` warns when it
    /// is not.
    Limiter(Limiter),
    /// Plain gain, in dB — the trim stage every chain needs and no interface
    /// ever gives you enough of.
    Gain(f32),
}

impl Module {
    pub fn name(&self) -> &'static str {
        match self {
            Module::Eq(_) => "EQ",
            Module::Compressor(_) => "Compressor",
            Module::Multiband(_) => "Multiband",
            Module::Saturator(_) => "Saturation",
            Module::Imager(_) => "Imager",
            Module::Limiter(_) => "Limiter",
            Module::Gain(_) => "Gain",
        }
    }

    /// Samples of delay this stage introduces. Only lookahead limiting does, but
    /// the chain has to ask everything so a future stage cannot silently break
    /// the sum.
    pub fn latency_samples(&self) -> usize {
        match self {
            Module::Limiter(l) => l.lookahead_samples(),
            _ => 0,
        }
    }

    /// True for stages that need both channels at once — an imager cannot be
    /// run twice, once per channel, because it *is* the relationship between
    /// them.
    pub fn is_stereo(&self) -> bool { matches!(self, Module::Imager(_)) }
}

/// One stage plus its strip state.
pub struct Stage {
    pub module: Module,
    pub bypassed: bool,
}

/// The chain.
#[derive(Default)]
pub struct Chain {
    pub stages: Vec<Stage>,
}

impl Chain {
    pub fn push(&mut self, module: Module) {
        self.stages.push(Stage { module, bypassed: false });
    }

    /// Move a stage, as a drag on the strip does.
    ///
    /// Returns false for an out-of-range index or a no-op move rather than
    /// panicking or silently doing something else — a drag that ends where it
    /// started is the most common gesture on a strip and must cost nothing.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.stages.len() || to >= self.stages.len() || from == to {
            return false;
        }
        let s = self.stages.remove(from);
        self.stages.insert(to, s);
        true
    }

    /// Total delay through the chain, for the host to compensate.
    pub fn latency_samples(&self) -> usize {
        self.stages.iter()
            .filter(|s| !s.bypassed)
            .map(|s| s.module.latency_samples())
            .sum()
    }

    /// Process one stereo frame through every enabled stage, in order.
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (mut ol, mut or) = (l, r);
        for s in self.stages.iter_mut() {
            if s.bypassed { continue; }
            match &mut s.module {
                // Mono stages run per channel, which is what every plugin does
                // and is why a stereo compressor needs a linked detector to not
                // pull the image about — noted here because the honest fix is a
                // linked mode on Compressor, not a workaround in the chain.
                Module::Eq(e) => { ol = e.process(ol); or = e.process(or); }
                Module::Compressor(c) => { ol = c.process(ol); or = c.process(or); }
                Module::Multiband(m) => { ol = m.process(ol); or = m.process(or); }
                Module::Saturator(s) => { ol = s.process(ol); or = s.process(or); }
                Module::Limiter(x) => { ol = x.process(ol); or = x.process(or); }
                Module::Imager(i) => { let (a, b) = i.process(ol, or); ol = a; or = b; }
                Module::Gain(db) => {
                    let g = crate::from_db_pub(*db);
                    ol *= g; or *= g;
                }
            }
        }
        (ol, or)
    }

    /// Problems worth surfacing on the strip, in the order a mastering engineer
    /// would care about them. Advice rather than enforcement — an unusual chain
    /// is sometimes exactly what a record needs.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let enabled: Vec<&Stage> = self.stages.iter().filter(|s| !s.bypassed).collect();
        if let Some(pos) = enabled.iter().position(|s| matches!(s.module, Module::Limiter(_))) {
            if pos + 1 != enabled.len() {
                out.push("the limiter is not last — stages after it can push the signal back over the ceiling".into());
            }
        }
        if enabled.iter().filter(|s| matches!(s.module, Module::Limiter(_))).count() > 1 {
            out.push("two limiters in one chain: the second cannot improve on the first, only squash it".into());
        }
        if let (Some(i), Some(l)) = (
            enabled.iter().position(|s| matches!(s.module, Module::Imager(_))),
            enabled.iter().position(|s| matches!(s.module, Module::Limiter(_))),
        ) {
            if i > l {
                out.push("widening after limiting can lift peaks back above the ceiling".into());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BandKind, Biquad};
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (TAU * hz * i as f32 / RATE).sin() * amp).collect()
    }

    fn boosting_eq() -> Eq {
        Eq { bands: vec![Biquad::design(BandKind::Peaking, RATE, 200.0, 1.0, 12.0)] }
    }

    fn run(chain: &mut Chain, input: &[f32]) -> Vec<f32> {
        input.iter().map(|x| chain.process(*x, *x).0).collect()
    }

    #[test]
    fn order_changes_the_sound_which_is_why_the_strip_exists() {
        // An EQ before a compressor makes the compressor react to the boosted
        // band; the same EQ after it does not. Those are audibly different
        // masters, and an interface that hides the order hides that.
        let input = tone(200.0, 12_288, 0.35);

        let mut eq_first = Chain::default();
        eq_first.push(Module::Eq(boosting_eq()));
        eq_first.push(Module::Compressor(Compressor::new(RATE, -24.0, 8.0, 2.0, 80.0)));

        let mut comp_first = Chain::default();
        comp_first.push(Module::Compressor(Compressor::new(RATE, -24.0, 8.0, 2.0, 80.0)));
        comp_first.push(Module::Eq(boosting_eq()));

        let a = run(&mut eq_first, &input);
        let b = run(&mut comp_first, &input);
        let rms = |v: &[f32]| (v[8192..].iter().map(|x| (x * x) as f64).sum::<f64>()
                               / (v.len() - 8192) as f64).sqrt();
        let diff = 20.0 * (rms(&a) / rms(&b)).max(1e-9).log10();
        assert!(diff.abs() > 1.0, "order must matter: {diff:+.2} dB between the two orders");
    }

    #[test]
    fn bypass_is_true_bypass() {
        // The button answers "is this helping?", so it has to be honest — a
        // bypassed stage must be identical to it not being in the chain.
        let input = tone(200.0, 8192, 0.35);

        let mut with = Chain::default();
        with.push(Module::Eq(boosting_eq()));
        with.push(Module::Gain(0.0));
        with.stages[0].bypassed = true;

        let mut without = Chain::default();
        without.push(Module::Gain(0.0));

        let a = run(&mut with, &input);
        let b = run(&mut without, &input);
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!((x - y).abs() < 1e-6, "bypassed chain differed at sample {i}");
        }
    }

    #[test]
    fn reordering_never_loses_or_duplicates_a_stage() {
        let mut c = Chain::default();
        c.push(Module::Eq(Eq::default()));
        c.push(Module::Compressor(Compressor::new(RATE, -18.0, 3.0, 10.0, 100.0)));
        c.push(Module::Gain(-3.0));
        c.push(Module::Limiter(Limiter::new(RATE, -0.3, 5.0, 50.0)));

        assert!(c.reorder(0, 3), "a real move succeeds");
        let names: Vec<&str> = c.stages.iter().map(|s| s.module.name()).collect();
        assert_eq!(names, vec!["Compressor", "Gain", "Limiter", "EQ"]);
        assert_eq!(c.stages.len(), 4, "nothing lost or duplicated");

        // a drag that ends where it started is the most common gesture on a
        // strip and must cost nothing
        assert!(!c.reorder(2, 2));
        // and out of range is refused rather than panicking
        assert!(!c.reorder(9, 0));
        assert!(!c.reorder(0, 9));
        assert_eq!(c.stages.len(), 4);
    }

    #[test]
    fn the_chain_reports_the_latency_a_host_must_compensate() {
        // A chain with lookahead delays the signal. A host that does not know by
        // how much drifts this track against every other one, and the error is a
        // constant few milliseconds that sounds like sloppy playing.
        let mut c = Chain::default();
        c.push(Module::Eq(Eq::default()));
        assert_eq!(c.latency_samples(), 0, "filters are zero-latency here");

        let lim = Limiter::new(RATE, -0.3, 5.0, 50.0);
        let look = lim.lookahead_samples();
        assert!(look > 100);
        c.push(Module::Limiter(lim));
        assert_eq!(c.latency_samples(), look, "the limiter's lookahead is the chain's latency");

        // bypassing it removes its delay too, which is the subtle part: a host
        // told the wrong latency is worse than one told none
        c.stages[1].bypassed = true;
        assert_eq!(c.latency_samples(), 0);
    }

    #[test]
    fn the_strip_warns_about_the_orders_that_undo_themselves() {
        // Advice rather than enforcement — an unusual chain is sometimes exactly
        // what a record needs.
        let mut c = Chain::default();
        c.push(Module::Limiter(Limiter::new(RATE, -0.3, 5.0, 50.0)));
        c.push(Module::Gain(3.0));
        let w = c.warnings();
        assert!(w.iter().any(|s| s.contains("not last")), "{w:?}");

        // widening after limiting can lift peaks back over the ceiling
        let mut c = Chain::default();
        c.push(Module::Limiter(Limiter::new(RATE, -0.3, 5.0, 50.0)));
        c.push(Module::Imager(Box::new(Imager::new(RATE, &[120.0]))));
        assert!(c.warnings().iter().any(|s| s.contains("widening after limiting")));

        // two limiters
        let mut c = Chain::default();
        c.push(Module::Limiter(Limiter::new(RATE, -1.0, 5.0, 50.0)));
        c.push(Module::Limiter(Limiter::new(RATE, -0.3, 5.0, 50.0)));
        assert!(c.warnings().iter().any(|s| s.contains("two limiters")));

        // a sane chain says nothing
        let mut c = Chain::default();
        c.push(Module::Eq(Eq::default()));
        c.push(Module::Compressor(Compressor::new(RATE, -18.0, 3.0, 10.0, 100.0)));
        c.push(Module::Limiter(Limiter::new(RATE, -0.3, 5.0, 50.0)));
        assert!(c.warnings().is_empty(), "{:?}", c.warnings());

        // and a bypassed limiter does not count as being in the wrong place
        c.stages[2].bypassed = true;
        c.push(Module::Gain(0.0));
        assert!(c.warnings().is_empty(), "{:?}", c.warnings());
    }

    #[test]
    fn a_full_mastering_chain_holds_its_ceiling() {
        // End to end: everything built so far, in the order it would actually
        // be used, must still not let a sample past the limiter.
        let mut c = Chain::default();
        c.push(Module::Eq(boosting_eq()));
        c.push(Module::Multiband(Box::new(Multiband::new(RATE, &[250.0, 3000.0]))));
        c.push(Module::Saturator({
            let mut s = Saturator::new(crate::saturation::Profile::Tube);
            s.drive = 0.5;
            s
        }));
        c.push(Module::Imager(Box::new(Imager::new(RATE, &[120.0, 8000.0]))));
        c.push(Module::Gain(6.0));
        c.push(Module::Limiter(Limiter::new(RATE, -1.0, 5.0, 50.0)));

        let ceiling = crate::from_db_pub(-1.0) * 1.05;
        let input = tone(200.0, 24_576, 0.9);
        let mut worst = 0.0f32;
        for x in &input {
            let (l, r) = c.process(*x, *x);
            worst = worst.max(l.abs()).max(r.abs());
        }
        assert!(worst <= ceiling, "the chain leaked {worst} past a {ceiling} ceiling");
        assert!(c.warnings().is_empty());
    }
}
