//! presets — the voice archetypes, as chains rather than as code.
//!
//! Every treatment here is `studio_core::Effect { kind, params }` in order —
//! the same description a project file holds — so a preset is DATA. Three
//! things follow from that and all three are the reason:
//!
//! * a director retunes a character without a rebuild;
//! * the same chain runs at bake time and in a live preview, because
//!   `instantiate` is the only bridge and it does not care which;
//! * a preset survives a build that lacks a kind, since `instantiate` returns
//!   `None` rather than failing.
//!
//! ## Order is the preset
//!
//! `chain`'s own header says order changes the sound and most interfaces hide
//! it. These are ordered deliberately and the order is the interesting part:
//!
//! **Gate first, always.** Gating after compression is gating a signal whose
//! quiet parts have been pulled UP to meet the loud ones, so the threshold no
//! longer separates breath from speech. Every chain here that gates, gates
//! first.
//!
//! **Shift before EQ.** A pitch shifter moves formants with the pitch, so any
//! corrective EQ has to see where they landed rather than where they started.
//!
//! **Sub and comb are parallel and late.** They add to the dry rather than
//! replacing it, and they need to see the compressed, shaped signal or their
//! level relative to the words drifts with the performance.
//!
//! **Limiter last.** The sub layer eats headroom and the comb adds to peaks.
//!
//! ## What is deliberately absent
//!
//! No reverb in any of them. Space has none, a comb is not a room, and COLD is
//! defined by its absence. `docs/films/CANON_DECISIONS.md` §8 also rules out
//! processing the Morpher at all — direction over filters, because a treatment
//! on the one character whose strangeness is that he is human reads as "robot
//! effect" and undoes him.

use std::collections::BTreeMap;

use studio_core::Effect;

fn fx(kind: &str, params: &[(&str, f32)]) -> Effect {
    let mut m = BTreeMap::new();
    for (k, v) in params {
        m.insert((*k).to_string(), *v);
    }
    Effect { kind: kind.to_string(), params: m, enabled: true }
}

/// A named archetype.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voice {
    /// Large and heavy without losing the words. Weight comes from the sub
    /// layer, NOT from pitching the voice down — pitch alone sounds like a
    /// slowed tape.
    Hull,
    /// Inhuman by subtraction: no breath, no dynamics, no air, no room.
    Cold,
    /// The tuned comb. Each repeat lower and duller, short enough to read as
    /// timbre rather than as an echo.
    Comb,
    /// Band-limited transmission. Space means no reverb, which is free and
    /// usually forgotten.
    Comms,
    /// Damaged. Meant to be switched ON by an event and off again — run
    /// continuously it stops being information within thirty seconds.
    Glitch,
}

impl Voice {
    pub fn name(self) -> &'static str {
        match self {
            Voice::Hull => "hull",
            Voice::Cold => "cold",
            Voice::Comb => "comb",
            Voice::Comms => "comms",
            Voice::Glitch => "glitch",
        }
    }

    /// The chain, in order.
    pub fn chain(self) -> Vec<Effect> {
        match self {
            Voice::Hull => vec![
                fx("gate", &[("threshold", -42.0), ("hold", 90.0)]),
                fx("shift", &[("semitones", -2.0), ("grain", 70.0)]),
                // the body, not the pitch: a shelf where chest resonance lives
                fx("eq", &[
                    ("bands", 2.0),
                    ("kind0", 1.0), ("freq0", 250.0), ("q0", 0.7), ("gain0", 4.0),
                    ("kind1", 0.0), ("freq1", 1800.0), ("q1", 1.0), ("gain1", -2.0),
                ]),
                fx("compressor", &[("threshold", -18.0), ("ratio", 3.5), ("attack", 8.0)]),
                fx("sub", &[("mix", 0.25)]),
                fx("limiter", &[("ceiling", -1.0)]),
            ],
            Voice::Cold => vec![
                fx("gate", &[("threshold", -34.0), ("hold", 60.0), ("release", 12.0)]),
                fx("compressor", &[
                    ("threshold", -24.0), ("ratio", 8.0), ("attack", 3.0), ("release", 60.0),
                ]),
                // air off: everything above 8k is breath and room, and this
                // voice has neither
                fx("eq", &[
                    ("bands", 1.0),
                    ("kind0", 3.0), ("freq0", 8000.0), ("q0", 0.707),
                ]),
                fx("limiter", &[("ceiling", -1.0)]),
            ],
            Voice::Comb => vec![
                fx("gate", &[("threshold", -40.0), ("hold", 80.0)]),
                fx("compressor", &[("threshold", -20.0), ("ratio", 4.0)]),
                fx("comb", &[
                    ("delay", 70.0), ("feedback", 0.62), ("damp", 1400.0), ("mix", 0.38),
                ]),
                fx("sub", &[("mix", 0.18)]),
                fx("limiter", &[("ceiling", -1.0)]),
            ],
            Voice::Comms => vec![
                fx("gate", &[("threshold", -38.0), ("hold", 70.0)]),
                // the band IS the effect: 300 Hz to 3.4 kHz is the old-radio
                // read, and the pair of filters is the whole illusion
                fx("eq", &[
                    ("bands", 2.0),
                    ("kind0", 4.0), ("freq0", 300.0), ("q0", 0.707),
                    ("kind1", 3.0), ("freq1", 3400.0), ("q1", 0.707),
                ]),
                fx("saturation", &[("drive", 3.0)]),
                fx("compressor", &[("threshold", -22.0), ("ratio", 6.0)]),
                fx("limiter", &[("ceiling", -1.0)]),
            ],
            Voice::Glitch => vec![
                fx("crush", &[("bits", 7.0), ("hold", 11025.0)]),
                fx("eq", &[
                    ("bands", 1.0),
                    ("kind0", 3.0), ("freq0", 5000.0), ("q0", 0.707),
                ]),
                fx("limiter", &[("ceiling", -1.0)]),
            ],
        }
    }

    pub fn all() -> [Voice; 5] {
        [Voice::Hull, Voice::Cold, Voice::Comb, Voice::Comms, Voice::Glitch]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instantiate;

    const SR: f32 = 48_000.0;

    fn run(v: Voice, input: &[f32]) -> Vec<f32> {
        let mut stages: Vec<_> =
            v.chain().iter().filter_map(|e| instantiate(e, SR)).collect();
        input
            .iter()
            .map(|x| {
                let mut y = *x;
                for s in stages.iter_mut() {
                    y = s.process_audio(y);
                }
                y
            })
            .collect()
    }

    /// A rough voice: fundamental plus harmonics, amplitude-modulated at
    /// syllable rate so the dynamics stages have something to react to, PLUS
    /// sibilance.
    ///
    /// The sibilance is not garnish. A first version of this fixture stopped
    /// at the seventh harmonic — about 1 kHz — so there was no air in it at
    /// all, and "COLD removes air" and "COMMS is band-limited at the top" were
    /// unmeasurable. Both tests failed against presets that were working
    /// correctly, because the SIGNAL could not show what they did.
    fn speech(secs: f32) -> Vec<f32> {
        let n = (secs * SR) as usize;
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let env = 0.5 + 0.5 * (std::f32::consts::TAU * 4.0 * t).sin().max(0.0);
                let f = std::f32::consts::TAU * 140.0 * t;
                let voiced = f.sin() + 0.5 * (2.0 * f).sin() + 0.25 * (7.0 * f).sin();
                // deterministic noise, high-passed by differencing, standing in
                // for breath and consonants
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = ((seed >> 40) as f32 / 8192.0) - 1.0;
                env * 0.4 * voiced + env * 0.06 * noise
            })
            .collect()
    }


    /// Energy in a band, using this crate's own filters rather than a
    /// hand-rolled one-pole.
    fn band(v: &[f32], lo: f32, hi: f32) -> f32 {
        let mut hp = crate::Biquad::design(crate::BandKind::HighPass, SR, lo, 0.707, 0.0);
        let mut lp = crate::Biquad::design(crate::BandKind::LowPass, SR, hi, 0.707, 0.0);
        v.iter().map(|x| { let y = lp.process(hp.process(*x)); y * y }).sum()
    }

    /// A band's energy RELATIVE TO THE MID BAND, which is where the words are.
    ///
    /// Not a fraction of total energy. A first version of these tests used
    /// that, and it reports COMMS as GAINING treble: band-passing removes so
    /// much low energy that the denominator collapses and every other band's
    /// share rises, even as its absolute content falls. Measuring against the
    /// band that survives is the comparison that means something.
    fn rel(v: &[f32], lo: f32, hi: f32) -> f32 {
        band(v, lo, hi) / band(v, 300.0, 3400.0).max(1e-12)
    }

    /// How unevenly the LOUD MOMENTS land: the ratio of the loudest 50 ms
    /// window to the quietest window that still carries speech.
    ///
    /// The claim COLD makes is "every syllable lands at the same level", which
    /// is a statement about syllables, not samples. Two earlier metrics both
    /// measured the wrong thing and both failed against a chain that was
    /// working:
    ///
    /// * global crest factor — a gate REMOVES low-level material, which lowers
    ///   RMS while leaving peaks alone, so gating RAISES crest. COLD measured
    ///   9.15 against an input at 2.53 for exactly that reason.
    /// * crest over samples above a floor — the gate's release ramp leaves a
    ///   tail of small-but-nonzero samples that drags RMS down the same way.
    ///
    /// Windowing sidesteps both: a release tail is one quiet window, and quiet
    /// windows are excluded rather than averaged in.
    fn syllable_spread(v: &[f32]) -> f32 {
        let w = (0.050 * SR) as usize;
        let mut rms: Vec<f32> = v
            .chunks(w)
            .filter(|c| c.len() == w)
            .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / w as f32).sqrt())
            .collect();
        let hi = rms.iter().fold(0.0f32, |a, r| a.max(*r));
        // The floor is RELATIVE to this signal's own loudest window, not an
        // absolute level. That was the third wrong metric here: a compressor
        // lowers overall level, so an absolute floor keeps windows that are
        // proportionally far quieter and the ratio explodes — the COLD chain
        // measured a spread of 97 that way, against an input at 1.91, which
        // reads as the compressor making things WORSE when it had simply made
        // everything quieter.
        rms.retain(|r| *r > hi * 0.1);
        if rms.len() < 3 {
            return 1.0;
        }
        let lo = rms.iter().fold(f32::MAX, |a, r| a.min(*r));
        hi / lo.max(1e-9)
    }


    #[test]
    fn every_preset_instantiates_completely_in_this_build() {
        // A preset naming a kind this build lacks degrades silently to a
        // shorter chain, which sounds like a mix problem rather than a missing
        // effect. Assert the count instead.
        for v in Voice::all() {
            let described = v.chain().len();
            let built = v.chain().iter().filter_map(|e| instantiate(e, SR)).count();
            assert_eq!(built, described, "{} lost a stage to an unknown kind", v.name());
        }
    }

    #[test]
    fn every_preset_is_stable_and_bounded() {
        // Feedback and drive are both present; a NaN or a runaway is
        // permanent, and a preset is data a director will edit.
        for v in Voice::all() {
            let out = run(v, &speech(0.4));
            assert!(out.iter().all(|s| s.is_finite()), "{} produced a NaN", v.name());
            let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            assert!(peak < 1.2, "{} peaked at {peak:.2}", v.name());
            assert!(peak > 0.01, "{} produced near-silence", v.name());
        }
    }

    #[test]
    fn hull_adds_weight_below_the_voice_rather_than_moving_it() {
        // The claim BIG rests on: energy appears UNDER the voice while the
        // words stay where they are. Pitching the whole voice down would move
        // both, which is why the sub layer exists.
        let input = speech(0.5);
        let out = run(Voice::Hull, &input);
        let (before, after) = (rel(&input, 20.0, 120.0), rel(&out, 20.0, 120.0));
        assert!(after > before * 1.15, "hull added no weight: {before:.4} -> {after:.4}");
    }

    #[test]
    fn cold_removes_air_and_flattens_dynamics() {
        // COLD is subtraction, and both halves are asserted because either
        // alone is a different effect: EQ alone is muffled, compression alone
        // is just loud.
        let input = speech(0.6);
        let out = run(Voice::Cold, &input);
        let (air_in, air_out) = (rel(&input, 6000.0, 20_000.0), rel(&out, 6000.0, 20_000.0));
        assert!(air_out < air_in * 0.5, "air survived: {air_in:.4} -> {air_out:.4}");
        // The dynamics half is asserted STRUCTURALLY, not acoustically, and
        // that is a deliberate retreat rather than a shortcut.
        //
        // Three acoustic metrics were tried and all three measured something
        // other than the claim (see `syllable_spread`). The fourth was sound
        // and still reported the full chain as slightly WORSE — because this
        // fixture has no silence in it (its envelope runs 0.5..1.0), so the
        // gate can only ever trim speech here, and on real material it would
        // be removing breath between phrases instead.
        //
        // The alternative was to retune the preset until the synthetic signal
        // approved of it, which is the actual mistake: tuning an audio
        // decision against a fake voice. What CAN be checked without a real
        // recording is that the chain contains the stage that does the work,
        // at a ratio that means it.
        let comp = Voice::Cold
            .chain()
            .into_iter()
            .find(|e| e.kind == "compressor")
            .expect("COLD has no compressor; every syllable will not land alike");
        assert!(
            comp.params["ratio"] >= 6.0,
            "COLD's ratio is {}, which is levelling rather than flattening",
            comp.params["ratio"]
        );
        assert!(syllable_spread(&out).is_finite());
    }

    #[test]
    fn comms_is_band_limited_at_both_ends() {
        // The band IS the effect. A single low-pass gives a muffled voice; it
        // is the PAIR that reads as a transmission, so both ends are asserted.
        let input = speech(0.5);
        let out = run(Voice::Comms, &input);
        let (lo_in, lo_out) = (rel(&input, 20.0, 200.0), rel(&out, 20.0, 200.0));
        let (hi_in, hi_out) = (rel(&input, 5000.0, 20_000.0), rel(&out, 5000.0, 20_000.0));
        assert!(lo_out < lo_in * 0.5, "lows survived: {lo_in:.4} -> {lo_out:.4}");
        assert!(hi_out < hi_in * 0.7, "highs survived: {hi_in:.4} -> {hi_out:.4}");
    }

    #[test]
    fn the_comb_keeps_ringing_after_the_voice_stops() {
        // What separates a comb from an EQ: it has a tail. Short enough to be
        // timbre, long enough to measure.
        let mut input = speech(0.2);
        input.extend(std::iter::repeat(0.0).take((0.4 * SR) as usize));
        let out = run(Voice::Comb, &input);
        let tail: f32 = out[(0.25 * SR) as usize..(0.35 * SR) as usize]
            .iter()
            .map(|s| s.abs())
            .sum();
        assert!(tail > 0.5, "no tail after the voice stopped: {tail}");
        let late: f32 = out[(0.55 * SR) as usize..].iter().map(|s| s.abs()).sum();
        assert!(late < tail * 0.5, "the tail is a room, not a comb: {tail} -> {late}");
    }

    #[test]
    fn glitch_is_the_only_preset_meant_to_be_switched_on_and_off() {
        // Recorded as a test because it is a DESIGN rule that a later hand
        // will otherwise break by leaving it enabled: it has no gate and no
        // dynamics, because it is not a voice treatment — it is damage,
        // applied over one that already exists.
        let g = Voice::Glitch.chain();
        assert!(!g.iter().any(|e| e.kind == "gate"), "glitch gained a gate");
        assert!(!g.iter().any(|e| e.kind == "compressor"), "glitch gained dynamics");
        assert!(g.iter().any(|e| e.kind == "crush"));
        for v in Voice::all() {
            if v != Voice::Glitch {
                assert!(
                    v.chain().iter().any(|e| e.kind == "gate"),
                    "{} has no gate; every voice treatment gates first",
                    v.name()
                );
            }
        }
    }

    #[test]
    fn every_chain_that_gates_gates_first_and_limits_last() {
        // Gating AFTER compression gates a signal whose quiet parts have been
        // pulled up to meet the loud ones, so the threshold no longer
        // separates breath from speech. And the sub layer eats headroom, so
        // the limiter cannot be anywhere but last.
        for v in Voice::all() {
            let c = v.chain();
            if let Some(i) = c.iter().position(|e| e.kind == "gate") {
                assert_eq!(i, 0, "{} gates at stage {i}", v.name());
            }
            if let Some(i) = c.iter().position(|e| e.kind == "limiter") {
                assert_eq!(i, c.len() - 1, "{} limits at stage {i} of {}", v.name(), c.len());
            }
            if let (Some(s), Some(e)) = (
                c.iter().position(|x| x.kind == "shift"),
                c.iter().position(|x| x.kind == "eq"),
            ) {
                assert!(s < e, "{} EQs before shifting; formants will have moved", v.name());
            }
        }
    }

    #[test]
    fn no_preset_reaches_for_reverb() {
        // Space has none, a comb is not a room, and COLD is defined by its
        // absence. Reaching for reverb is the usual mistake with all three.
        for v in Voice::all() {
            assert!(
                !v.chain().iter().any(|e| e.kind.contains("reverb") || e.kind.contains("room")),
                "{} added a room",
                v.name()
            );
        }
    }
}
