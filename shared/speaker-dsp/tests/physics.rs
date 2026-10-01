//! The claims in the module docs, measured. 48 kHz, one-second analysis blocks, so every
//! integer-Hz test frequency falls exactly on a Goertzel bin (no leakage to explain away).
use audio_kit::{goertzel, peak, rms};
use speaker_dsp::{LoudnessComp, Provenance, SpeakerChain, SpeakerModel, VirtualBass, VirtualBassParams};

const SR: f32 = 48_000.0;
const N: usize = 48_000;

fn sine(f: f32, amp: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f32::consts::TAU * f * i as f32 / SR).sin()).collect()
}
fn mag(x: &[f32], f: f32) -> f32 { goertzel(x, f, SR) }
fn db(x: f32) -> f32 { 20.0 * x.max(1e-12).log10() }
/// Run `f` over `x`, return the last second (the first is warm-up for filters and envelopes).
fn settled(x: &[f32], mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    let y: Vec<f32> = x.iter().map(|&s| f(s)).collect();
    y[y.len() - N..].to_vec()
}
fn bass() -> VirtualBass {
    // The bare-phone 400 Hz speaker: the case where harmonics MUST be placed above the cutoff.
    VirtualBass::new(SR, &SpeakerModel::bare_phone_main(), VirtualBassParams { amount: 1.0, ..Default::default() })
}
fn bare_chain() -> SpeakerChain {
    SpeakerChain::new(SR, SpeakerModel::bare_phone_receiver(), SpeakerModel::bare_phone_main(), VirtualBassParams::default())
}

#[test]
fn an_80hz_note_becomes_a_harmonic_series_the_speaker_can_radiate() {
    let mut vb = bass();
    let y = settled(&sine(80.0, 0.5, 3 * N), |s| vb.process(s));
    // The series above the 400 Hz cutoff: 5th-8th harmonics of 80 Hz.
    let on: f32 = [400.0, 480.0, 560.0, 640.0].iter().map(|&f| mag(&y, f).powi(2)).sum();
    // Bins BETWEEN the harmonics: a periodic signal has no energy there.
    let off: f32 = [440.0, 520.0, 600.0, 680.0].iter().map(|&f| mag(&y, f).powi(2)).sum();
    assert!(db(on.sqrt()) > -50.0, "there must be real harmonic energy, got {:.1} dB", db(on.sqrt()));
    assert!(db(on.sqrt()) - db(off.sqrt()) > 30.0,
        "energy must sit ON the harmonic lines of 80 Hz (that is what the ear reads as an 80 Hz pitch): on {:.1} dB, between {:.1} dB",
        db(on.sqrt()), db(off.sqrt()));
}

#[test]
fn the_synthesised_signal_carries_no_energy_below_the_speakers_cutoff() {
    let mut vb = bass();
    let y = settled(&sine(80.0, 0.5, 3 * N), |s| vb.process(s));
    let in_band = [400.0, 480.0, 560.0, 640.0].iter().map(|&f| mag(&y, f)).fold(0.0f32, f32::max);
    // The stage's own 36 dB/oct filter, measured: the fundamental and 2nd harmonic are far down, the
    // 3rd (the strongest thing the curve makes, only 1.5 octaves under the cutoff) is >= 12 dB down.
    // The HARD guarantee that nothing below the cutoff reaches the speaker is the chain's own
    // protective filter on the sum — see `the_chain_removes_the_third_harmonic_too`.
    for (f, need) in [(80.0, 25.0), (160.0, 20.0), (240.0, 12.0)] {
        assert!(db(mag(&y, f)) < db(in_band) - need,
            "{f} Hz is below the cutoff ({:.1} dB vs {:.1} dB in band, wanted >= {need} dB under)", db(mag(&y, f)), db(in_band));
    }
}

#[test]
fn the_chain_removes_the_third_harmonic_too() {
    let mut c = bare_chain();
    let x = sine(80.0, 0.5, 3 * N);
    let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    c.process_block(&x, &x, &mut t, &mut b);
    let y = &b[b.len() - N..];
    let in_band = [400.0, 480.0, 560.0, 640.0].iter().map(|&f| mag(y, f)).fold(0.0f32, f32::max);
    assert!(db(in_band) > -55.0, "the harmonics must actually arrive: {:.1} dB", db(in_band));
    // The fundamental, against what went IN: a 400 Hz LR4 high-pass is ~56 dB down at 80 Hz. (Measured
    // against the harmonics instead, this test failed at 21 dB — the harmonics are simply quiet at the
    // default amount, so that yardstick said nothing about the filter.)
    let fundamental_loss = db(mag(&x[x.len() - N..], 80.0)) - db(mag(y, 80.0));
    assert!(fundamental_loss > 50.0, "80 Hz must not reach a 400 Hz speaker: only {fundamental_loss:.1} dB down");
    for f in [160.0, 240.0] {
        assert!(db(mag(y, f)) < db(in_band) - 20.0, "{f} Hz harmonic survived the protective filter: {:.1} dB vs {:.1} dB in band", db(mag(y, f)), db(in_band));
    }
}

#[test]
fn the_main_speaker_gets_harmonics_the_earpiece_cannot_play() {
    // A 60 Hz note: its 3rd/4th harmonics (180/240 Hz) are useful on a case-loaded main speaker and
    // useless on a 400 Hz earpiece. Harmonics are made once down to the MOST permissive cutoff and
    // each speaker's own filter trims them; sizing the shared stage from the higher cutoff would
    // starve the main speaker of exactly what the previous chain gave it.
    let mut c = SpeakerChain::fp5(SR);
    let x = sine(60.0, 0.5, 3 * N);
    let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    c.process_block(&x, &x, &mut t, &mut b);
    let level = |y: &[f32]| [180.0, 240.0].iter().map(|&f| mag(&y[y.len() - N..], f).powi(2)).sum::<f32>().sqrt();
    assert!(db(level(&b)) > -60.0, "the main speaker must receive the 3rd/4th harmonics: {:.1} dB", db(level(&b)));
    assert!(db(level(&t)) < db(level(&b)) - 20.0, "the earpiece must not: top {:.1} dB vs bottom {:.1} dB", db(level(&t)), db(level(&b)));
}

#[test]
fn the_harmonic_stage_does_not_colour_the_bass_it_sits_on() {
    // The bug this pins: the synthesised signal's fundamental overlapped the dry bass, and their
    // relative phase (set by filter delays) rippled the bass response by about +/-3.5 dB between 45
    // and 250 Hz. Smooth is the brief, so: switching the stage on must move any bass frequency by
    // no more than 1.5 dB.
    let gain = |f: f32, amount: f32| {
        let mut c = SpeakerChain::new(SR, SpeakerModel::fp5_earpiece(), SpeakerModel::fp5_bottom(), VirtualBassParams { amount, ..Default::default() });
        let x = sine(f, 0.1, 3 * N);
        let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
        c.process_block(&x, &x, &mut t, &mut b);
        db(mag(&b[b.len() - N..], f)) - db(mag(&x[x.len() - N..], f))
    };
    for f in [45.0, 55.0, 65.0, 80.0, 100.0, 120.0, 150.0, 180.0, 250.0] {
        let delta = gain(f, 0.45) - gain(f, 0.0);
        assert!(delta.abs() < 1.5, "{f} Hz moved {delta:+.2} dB when the harmonic stage came on");
    }
}

#[test]
fn the_default_fp5_chain_keeps_the_bass_the_old_chain_sent_to_the_main_speaker() {
    // Parity with the chain that was judged better than an iPhone by ear (see model.rs): the main
    // speaker had NO high-pass, so an 80 Hz note must still reach it, only the earpiece (400 Hz) loses it.
    let mut c = SpeakerChain::fp5(SR);
    let x = sine(80.0, 0.1, 3 * N);
    let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    c.process_block(&x, &x, &mut t, &mut b);
    let (gt, gb) = (db(mag(&t[t.len() - N..], 80.0)) - db(mag(&x[x.len() - N..], 80.0)),
                    db(mag(&b[b.len() - N..], 80.0)) - db(mag(&x[x.len() - N..], 80.0)));
    assert!(gb > -2.0, "the main speaker lost its bass: {gb:.1} dB at 80 Hz (the old chain passed it)");
    assert!(gt < -30.0, "the earpiece must not be sent 80 Hz: {gt:.1} dB");
}

#[test]
fn quiet_bass_gets_the_same_harmonic_character_as_loud_bass() {
    // Without the envelope normalisation a quiet note would drive the curve less and make fewer,
    // different harmonics. The SHAPE (relative harmonic levels) must hold; the LEVEL must follow.
    let measure = |amp: f32| {
        let mut vb = bass();
        let y = settled(&sine(80.0, amp, 3 * N), |s| vb.process(s));
        [400.0, 480.0, 560.0].map(|f| mag(&y, f))
    };
    let (loud, quiet) = (measure(0.5), measure(0.025)); // 26 dB apart
    for i in 0..3 {
        let shape_loud = db(loud[i]) - db(loud[1]);
        let shape_quiet = db(quiet[i]) - db(quiet[1]);
        assert!((shape_loud - shape_quiet).abs() < 3.0, "harmonic {i}: shape moved {shape_loud:.1} vs {shape_quiet:.1} dB");
    }
    let level_gap = db(loud[1]) - db(quiet[1]);
    assert!((level_gap - 26.0).abs() < 4.0, "level must track the music: gap {level_gap:.1} dB, wanted ~26");
}

#[test]
fn silence_makes_no_harmonics_and_a_zero_amount_does_no_work() {
    let mut vb = bass();
    assert!((0..N).all(|_| vb.process(0.0) == 0.0));
    let mut off = VirtualBass::new(SR, &SpeakerModel::fp5_bottom(), VirtualBassParams { amount: 0.0, ..Default::default() });
    assert!(sine(80.0, 0.5, 4096).iter().all(|&s| off.process(s) == 0.0));
}

#[test]
fn nothing_below_the_cutoff_reaches_the_speaker_so_no_power_is_wasted() {
    // Bass-heavy programme: 90% of its energy is below the bottom speaker's 400 Hz cutoff.
    let n = 3 * N;
    let prog: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let w = std::f32::consts::TAU;
            0.15 * (w * 50.0 * t).sin() + 0.15 * (w * 80.0 * t).sin() + 0.15 * (w * 120.0 * t).sin() + 0.1 * (w * 1000.0 * t).sin()
        })
        .collect();
    let low_fraction = |x: &[f32]| {
        let low: f32 = (20..=200).step_by(5).map(|f| mag(x, f as f32).powi(2)).sum();
        2.0 * low / rms(x).powi(2)
    };
    let last = |x: &[f32]| x[x.len() - N..].to_vec();
    let before = low_fraction(&last(&prog));
    let mut c = bare_chain(); // the bare-phone 400 Hz speaker: where nothing below the cutoff is worth sending
    let (mut t, mut b) = (vec![0.0; n], vec![0.0; n]);
    c.process_block(&prog, &prog, &mut t, &mut b);
    let after = low_fraction(&last(&b));
    assert!(before > 0.7, "the programme is meant to be bass-heavy, got {before:.2}");
    assert!(after < 0.01, "sub-cutoff energy must be removed: {:.3}% of the output remains (was {:.0}%)", after * 100.0, before * 100.0);
}

#[test]
fn the_midrange_passes_untouched_at_normal_levels() {
    // 2 kHz is ~3x the earpiece's 650 Hz cutoff. (1 kHz was the first choice and was WRONG: the
    // protective high-pass is Linkwitz-Riley, -6 dB AT the cutoff by design, so 1.5x the cutoff is
    // still down 1.4 dB. The code was right and the expectation was the bug.)
    let mut c = SpeakerChain::fp5(SR);
    let x = sine(2000.0, 0.1, 3 * N); // -20 dBFS, below both compressors' thresholds
    let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    c.process_block(&x, &x, &mut t, &mut b);
    for (name, y) in [("top", &t), ("bottom", &b)] {
        let gain = db(mag(&y[y.len() - N..], 2000.0)) - db(mag(&x[x.len() - N..], 2000.0));
        assert!(gain.abs() < 0.5, "{name}: 2 kHz changed by {gain:.2} dB");
    }
}

#[test]
fn f_low_is_the_minus_6_db_point_of_each_speakers_protective_filter() {
    // The model's definition, pinned: at f_low the chain is down ~6 dB (LR4), so a measured preset
    // can be dropped in and mean what it says.
    for (m, name) in [(SpeakerModel::fp5_bottom(), "bottom"), (SpeakerModel::fp5_earpiece(), "earpiece")] {
        let mut c = SpeakerChain::fp5(SR);
        let x = sine(m.f_low_hz, 0.05, 3 * N); // quiet: compressors idle, harmonics negligible at this level
        let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
        c.process_block(&x, &x, &mut t, &mut b);
        let y = if name == "bottom" { &b } else { &t };
        let gain = db(mag(&y[y.len() - N..], m.f_low_hz)) - db(mag(&x[x.len() - N..], m.f_low_hz));
        assert!((gain + 6.0).abs() < 1.5, "{name}: {gain:.2} dB at its {} Hz cutoff, wanted about -6", m.f_low_hz);
    }
}

#[test]
fn the_ceiling_holds_whatever_the_harmonic_stage_adds() {
    let mut c = SpeakerChain::fp5(SR);
    let n = 3 * N;
    // Loud, bass-heavy: the worst case for an exciter.
    let x: Vec<f32> = (0..n).map(|i| 0.6 * (std::f32::consts::TAU * 70.0 * i as f32 / SR).sin() + 0.35 * (std::f32::consts::TAU * 500.0 * i as f32 / SR).sin()).collect();
    let (mut t, mut b) = (vec![0.0; n], vec![0.0; n]);
    c.process_block(&x, &x, &mut t, &mut b);
    let skip = c.latency_samples() + 4800;
    let lin = |dbfs: f32| 10f32.powf(dbfs / 20.0);
    assert!(peak(&t[skip..]) <= lin(-6.0) * 1.01, "earpiece ceiling -6 dBFS: peak {:.3}", peak(&t[skip..]));
    assert!(peak(&b[skip..]) <= lin(-3.0) * 1.01, "main speaker ceiling -3 dBFS: peak {:.3}", peak(&b[skip..]));
}

#[test]
fn only_the_limiter_can_stop_a_fast_transient_and_it_does() {
    // The test above is satisfied by the compressor alone (a break that disabled the limiter was NOT
    // caught). A 3 ms full-scale burst is too fast for the compressor's 15 ms attack: whatever
    // holds the ceiling here is the lookahead limiter, and nothing else.
    let mut c = SpeakerChain::fp5(SR);
    let n = N;
    let mut x = vec![0.0f32; n];
    let start = n / 2;
    for i in 0..(0.003 * SR) as usize { x[start + i] = (std::f32::consts::TAU * 2000.0 * i as f32 / SR).sin(); }
    let (mut t, mut b) = (vec![0.0; n], vec![0.0; n]);
    c.process_block(&x, &x, &mut t, &mut b);
    let lin = |dbfs: f32| 10f32.powf(dbfs / 20.0);
    let (pt, pb) = (peak(&t), peak(&b));
    assert!(pt > 0.05 && pb > 0.05, "the burst must reach the output at all (top {pt:.3}, bottom {pb:.3}), or this tests nothing");
    assert!(pt <= lin(-6.0) * 1.01, "earpiece ceiling -6 dBFS held against a fast burst: peak {pt:.3}");
    assert!(pb <= lin(-3.0) * 1.01, "main speaker ceiling -3 dBFS held against a fast burst: peak {pb:.3}");
}

#[test]
fn loudness_compensation_is_smooth_monotone_and_off_at_the_reference() {
    let lc = LoudnessComp::default();
    let r = lc.reference_level;
    assert_eq!(lc.at(r).vbass_scale, 1.0);
    assert_eq!(lc.at(1.0).warmth_db, 0.0);
    let mut prev = lc.at(1.0);
    for k in (0..=100).rev() {
        let c = lc.at(k as f32 / 100.0);
        assert!(c.vbass_scale >= prev.vbass_scale && c.warmth_db >= prev.warmth_db, "not monotone at {k}%");
        assert!((c.vbass_scale - prev.vbass_scale).abs() < 0.05, "a step at {k}%: the volume key would click");
        prev = c;
    }
    assert!(lc.at(0.0).vbass_scale > 1.5 && lc.at(0.0).warmth_db > 2.0, "quiet must be compensated");
    assert_eq!(lc.at(f32::NAN), lc.at(r), "garbage volume is treated as the reference, not silence");
}

#[test]
fn noise_in_stays_finite_and_dc_free() {
    let mut c = SpeakerChain::fp5(SR);
    let mut seed = 0x1234_5678u32;
    let x: Vec<f32> = (0..2 * N).map(|_| { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; (seed as f32 / u32::MAX as f32 - 0.5) * 1.0 }).collect();
    let (mut t, mut b) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    c.process_block(&x, &x, &mut t, &mut b);
    assert!(t.iter().chain(&b).all(|v| v.is_finite()));
    let dc = b[N..].iter().sum::<f32>() / N as f32;
    assert!(dc.abs() < 2e-3, "DC {dc} would eat speaker headroom");
}

#[test]
fn the_latency_is_reported_and_the_presets_say_they_are_guesses() {
    assert_eq!(SpeakerChain::fp5(SR).latency_samples(), 96, "2 ms lookahead at 48 kHz");
    for m in [SpeakerModel::fp5_bottom(), SpeakerModel::fp5_earpiece(), SpeakerModel::bare_phone_main(), SpeakerModel::bare_phone_receiver()] {
        assert_eq!(m.provenance, Provenance::Assumed, "{}: nothing here has been measured yet", m.name);
    }
    let (e, m) = (SpeakerModel::fp5_earpiece(), SpeakerModel::fp5_bottom());
    assert!(e.f_low_hz > m.f_low_hz && e.ceiling_dbfs < m.ceiling_dbfs, "a receiver starts higher and takes less power");
    assert_eq!(e.f_low_hz, 400.0, "the deployed hpTop default");
    assert_eq!(m.f_low_hz, 40.0, "bankstown's own final_hp"); 
}
