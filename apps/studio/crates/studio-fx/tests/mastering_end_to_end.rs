//! End-to-end proof that the mastering pieces built across this codebase's
//! recent work actually compose, not just that each compiles in isolation.
//!
//! This is a Rust integration test (`tests/`, not `#[cfg(test)] mod tests`
//! inside `src/`), which matters here specifically: it can only see
//! `studio_fx`'s PUBLIC API, the same surface a real caller (`studio-app`,
//! eventually) would use. A bug where something works internally but isn't
//! actually reachable from outside the crate would pass every unit test in
//! `src/` and fail here.

use studio_fx::analysis::Spectrum;
use studio_fx::chain::{Chain, Module};
use studio_fx::loudness::{measure, LoudnessPreset};
use studio_fx::{Eq as StudioEq, Limiter};

const SR: f32 = 48_000.0;

/// A deliberately imbalanced, dynamic signal — loud low-mid content, wide
/// crest factor — so the chain actually has something to do. A perfectly
/// clean sine would make every stage a no-op and prove nothing.
fn raw_take(n: usize) -> Vec<f32> {
    let mut seed = 12345u64;
    let mut next = move || {
        // A small xorshift, not `core_kit`, kept local and dependency-free
        // since this integration test only wants "some noise-like texture,"
        // not a reusable RNG.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed as f64 / u64::MAX as f64) as f32 - 0.5
    };
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let tone = (2.0 * std::f32::consts::PI * 250.0 * t).sin() * 0.6; // low-mid heavy
            let transient = if i % 4800 < 40 { 0.8 } else { 0.0 }; // periodic drum-like hits
            let noise = next() * 0.05;
            (tone + transient + noise).clamp(-1.0, 1.0)
        })
        .collect()
}

/// The default master chain order this codebase's README and the trap
/// expansion plan both settle on: EQ -> multiband -> saturation -> imager ->
/// gain (loudness match) -> limiter, i.e. the limiter always last.
fn build_master_chain(target_gain_db: f32) -> Chain {
    let mut chain = Chain::default();
    let mut eq = StudioEq::default();
    eq.bands.push(studio_fx::Biquad::design(studio_fx::BandKind::Peaking, SR, 300.0, 1.0, -2.0));
    chain.push(Module::Eq(eq));
    chain.push(Module::Multiband(Box::new(studio_fx::multiband::Multiband::new(SR, &[200.0, 2000.0]))));
    chain.push(Module::Saturator(studio_fx::saturation::Saturator::new(studio_fx::saturation::Profile::Tube)));
    chain.push(Module::Imager(Box::new(studio_fx::imager::Imager::new(SR, &[200.0, 2000.0]))));
    chain.push(Module::Gain(target_gain_db));
    chain.push(Module::Limiter(Limiter::new(SR, -1.0, 3.0, 50.0)));
    chain
}

fn process_stereo(chain: &mut Chain, samples: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let (l, r) = chain.process(s, s);
        out.push(l);
        out.push(r);
    }
    out
}

fn mono_from_interleaved_stereo(interleaved: &[f32]) -> Vec<f32> {
    interleaved.chunks(2).map(|c| (c[0] + c.get(1).copied().unwrap_or(c[0])) * 0.5).collect()
}

#[test]
fn the_limiter_holds_the_ceiling_regardless_of_what_gain_precedes_it() {
    // This is the invariant the codebase's own README states as the reason
    // the limiter must be last: whatever gain a loudness-matching stage
    // asks for, the limiter downstream must still hold the ceiling.
    let raw = raw_take((2.0 * SR) as usize);
    for gain_db in [-6.0, 0.0, 6.0, 12.0, 24.0] {
        let mut chain = build_master_chain(gain_db);
        let stereo = process_stereo(&mut chain, &raw);
        let peak = stereo.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let peak_db = 20.0 * peak.max(1e-9).log10();
        assert!(peak_db <= -1.0 + 0.3, "gain {gain_db}dB into the chain: limiter should hold ~-1dBFS ceiling, got {peak_db}dBFS");
    }
}

#[test]
fn loudness_matching_moves_the_measured_result_toward_the_preset_target() {
    let raw = raw_take((3.0 * SR) as usize);

    // Pass 1: measure with unity gain to find where we're starting from.
    let mut probe_chain = build_master_chain(0.0);
    let probe_out = process_stereo(&mut probe_chain, &raw);
    let probe_mono = mono_from_interleaved_stereo(&probe_out);
    let starting_lufs = measure(&probe_mono, SR).integrated_lufs;
    assert!(starting_lufs.is_finite(), "the probe pass should read a real loudness, not silence");

    // Pass 2: compute the preset's suggested gain from that measurement,
    // and actually run the chain again WITH that gain applied — the real
    // round trip a mastering flow would do (measure, then correct).
    let preset = LoudnessPreset::STREAMING;
    let suggested_gain = preset.gain_db(starting_lufs);
    let mut corrected_chain = build_master_chain(suggested_gain);
    let corrected_out = process_stereo(&mut corrected_chain, &raw);
    let corrected_mono = mono_from_interleaved_stereo(&corrected_out);
    let corrected_lufs = measure(&corrected_mono, SR).integrated_lufs;

    let before_distance = (starting_lufs - preset.target_lufs).abs();
    let after_distance = (corrected_lufs - preset.target_lufs).abs();
    assert!(
        after_distance < before_distance,
        "applying the preset's suggested gain should move loudness closer to target {}: before {starting_lufs} ({before_distance} away), after {corrected_lufs} ({after_distance} away)",
        preset.target_lufs
    );

    // The limiter still has to hold the ceiling even after that correction —
    // the whole reason loudness matching is safe to apply blindly here.
    let peak = corrected_out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    let peak_db = 20.0 * peak.max(1e-9).log10();
    assert!(peak_db <= preset.ceiling_dbtp + 0.3, "post-correction peak {peak_db}dBFS should still respect the preset's ceiling {}dBTP", preset.ceiling_dbtp);
}

#[test]
fn the_full_chain_still_reports_no_reordering_warning_with_the_limiter_last() {
    let chain = build_master_chain(0.0);
    let warnings = chain.warnings();
    assert!(
        warnings.iter().all(|w| !w.contains("not last")),
        "the limiter is genuinely last in this chain's construction; the strip must not warn about it: {warnings:?}"
    );
}

#[test]
fn spectrum_analysis_reads_something_sane_off_the_chains_own_output() {
    // A different public-surface check: studio_fx::analysis::Spectrum
    // running on the CHAIN's output (not a synthetic test tone), to confirm
    // Spectrum and Chain compose without either side assuming something
    // about the other's internal representation.
    let raw = raw_take(8192);
    let mut chain = build_master_chain(0.0);
    let mut mono = Vec::with_capacity(raw.len());
    for &s in &raw {
        let (l, r) = chain.process(s, s);
        mono.push((l + r) * 0.5);
    }
    let spec = Spectrum::analyze(&mono, SR);
    assert!(spec.bins_db.iter().any(|db| *db > -60.0), "the chain's own output should show real spectral energy, not read as silence");
}

#[test]
fn dithered_export_of_the_masterd_output_round_trips_without_clipping() {
    let raw = raw_take((1.0 * SR) as usize);
    let mut chain = build_master_chain(0.0);
    let stereo = process_stereo(&mut chain, &raw);

    let dir = std::env::temp_dir();
    let path = dir.join(format!("studio_mastering_e2e_{}.wav", std::process::id()));
    studio_audio::export::export_wav_dithered(&stereo, 2, 48_000, 9, &path).expect("export should succeed on real chain output");

    let mut reader = hound::WavReader::open(&path).expect("should read back what was just written");
    let read_back: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / i16::MAX as f32).collect();
    assert_eq!(read_back.len(), stereo.len());
    let peak = read_back.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(peak < 1.0, "the limiter should have kept this under full scale even after dither, got peak {peak}");

    let _ = std::fs::remove_file(&path);
}
