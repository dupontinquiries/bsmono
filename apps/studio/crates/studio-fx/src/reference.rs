//! reference — match a mix to a track you trust.
//!
//! Mastering by ear against a reference is the workflow every engineer
//! actually uses: play the reference, play the mix, close the gap. This
//! makes the gap a number and the closing a chain. Both sides are reduced
//! to eight octave-ish bands ([`BANDS`]) from `analysis::Spectrum` and
//! integrated loudness from `loudness::measure`; the proposal is one
//! `gain` to meet the reference's LUFS and one eight-band `eq` whose gains
//! are the band differences, halved and clamped — a nudge toward the
//! reference, not a forced copy, because a mix that needs 9 dB at 8 kHz to
//! sound like the reference does not have the reference's problem, it has
//! its own, and the assistant should say so rather than do it.
//!
//! Everything here is pure over spectra and numbers, so "a dull mix
//! against a bright reference gets a high-shelf-shaped EQ" is a test.

use crate::analysis::Spectrum;
use std::collections::BTreeMap;

/// The bands, as (low, high, centre) in Hz. Eight, because that is what an
/// engineer's ear names: sub, bass, low-mids, mids, upper mids, presence,
/// brilliance, air.
pub const BANDS: [(f32, f32, f32); 8] = [
    (20.0, 60.0, 40.0),
    (60.0, 150.0, 100.0),
    (150.0, 400.0, 250.0),
    (400.0, 1000.0, 630.0),
    (1000.0, 2500.0, 1600.0),
    (2500.0, 6000.0, 4000.0),
    (6000.0, 12000.0, 8000.0),
    (12000.0, 20000.0, 15000.0),
];

/// How far the EQ goes toward the reference: half the measured gap.
pub const FOLLOW: f32 = 0.5;
/// The most any band is moved, dB. Beyond this the gap is the mix's own
/// problem and the finding says so instead.
pub const MAX_BAND_DB: f32 = 6.0;
/// A band gap under this is noise, not a difference.
pub const DEADBAND_DB: f32 = 0.75;

#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    /// Per band: reference minus mix, dB (positive = the mix is short here).
    pub band_db: [f32; 8],
    /// Reference LUFS minus mix LUFS (positive = the mix is quieter).
    pub loudness_db: f32,
}

/// Measure the gap between two spectra and two loudnesses. Band levels are
/// normalised by each side's mean band level first, so the EQ describes
/// SHAPE and the gain describes LEVEL, and neither double-counts the other.
pub fn gap(mix: &Spectrum, mix_lufs: f32, reference: &Spectrum, reference_lufs: f32) -> Gap {
    let levels = |s: &Spectrum| -> [f32; 8] {
        let mut v = [0f32; 8];
        for (i, (lo, hi, _)) in BANDS.iter().enumerate() { v[i] = s.band_db(*lo, *hi); }
        let mean = v.iter().sum::<f32>() / 8.0;
        for x in &mut v { *x -= mean; }
        v
    };
    let (m, r) = (levels(mix), levels(reference));
    let mut band_db = [0f32; 8];
    for i in 0..8 { band_db[i] = r[i] - m[i]; }
    Gap { band_db, loudness_db: reference_lufs - mix_lufs }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// The chain: a `gain` (when the loudness gap is real) then an `eq`.
    pub chain: Vec<studio_core::Effect>,
    /// Bands whose gap exceeds what the EQ will move — the mix's own
    /// problem, named with the band's centre and the full gap.
    pub beyond: Vec<(f32, f32)>,
    /// One sentence for the panel.
    pub summary: String,
}

/// Turn a gap into a chain. `strength` scales the follow (1.0 = the
/// default half-way; 2.0 goes all the way, still clamped).
pub fn propose(gap: &Gap, strength: f32) -> Proposal {
    let follow = (FOLLOW * strength).clamp(0.0, 1.0);
    let mut chain = Vec::new();
    let mut beyond = Vec::new();
    if gap.loudness_db.abs() >= DEADBAND_DB {
        let mut p = BTreeMap::new();
        p.insert("db".to_string(), gap.loudness_db.clamp(-24.0, 24.0));
        chain.push(studio_core::Effect { kind: "gain".into(), params: p, enabled: true });
    }
    let mut eq = BTreeMap::new();
    let mut moved = 0usize;
    eq.insert("bands".to_string(), 8.0);
    for (i, (_, _, centre)) in BANDS.iter().enumerate() {
        let full = gap.band_db[i];
        let wanted = if full.abs() < DEADBAND_DB { 0.0 } else { (full * follow).clamp(-MAX_BAND_DB, MAX_BAND_DB) };
        if (full * follow).abs() > MAX_BAND_DB { beyond.push((*centre, full)); }
        if wanted != 0.0 { moved += 1; }
        // Shelves at the ends, peaks between: the shape an engineer would draw.
        let kind = match i { 0 => 1.0, 7 => 2.0, _ => 0.0 };
        eq.insert(format!("kind{i}"), kind);
        eq.insert(format!("freq{i}"), *centre);
        eq.insert(format!("gain{i}"), wanted);
        eq.insert(format!("q{i}"), 0.9);
    }
    if moved > 0 {
        chain.push(studio_core::Effect { kind: "eq".into(), params: eq, enabled: true });
    }
    let level = if gap.loudness_db.abs() >= DEADBAND_DB { format!(", {:+.1} dB of level", gap.loudness_db) } else { String::new() };
    let summary = match (chain.len(), beyond.len()) {
        (0, _) => "already matches the reference within a dB".to_string(),
        (_, 0) => format!("{} band{} nudged toward the reference{level}", moved, if moved == 1 { "" } else { "s" }),
        (_, n) => format!("{moved} bands nudged{level}; {n} band{} differ by more than the EQ will move — {}", if n == 1 { "" } else { "s" },
            beyond.iter().map(|(c, g)| format!("{:+.0} dB at {}", g, hz(*c))).collect::<Vec<_>>().join(", ")),
    };
    Proposal { chain, beyond, summary }
}

fn hz(f: f32) -> String { if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{f:.0} Hz") } }

#[cfg(test)]
mod tests {
    use super::*;

    /// A spectrum with a given slope in dB per octave over 20 Hz–20 kHz.
    fn tilted(rate: f32, db_per_octave: f32) -> Spectrum {
        let n = 2048;
        let bins_db = (0..n).map(|k| {
            let f = (k as f32 + 0.5) * rate / (2.0 * n as f32);
            let oct = (f.max(20.0) / 1000.0).log2();
            db_per_octave * oct
        }).collect();
        Spectrum { bins_db, rate }
    }

    #[test]
    fn a_matching_mix_proposes_nothing() {
        let r = tilted(48_000.0, -3.0);
        let g = gap(&r, -14.0, &r, -14.0);
        assert!(g.band_db.iter().all(|d| d.abs() < 1e-3));
        let p = propose(&g, 1.0);
        assert!(p.chain.is_empty(), "{:?}", p.chain);
        assert!(p.summary.contains("already matches"));
    }

    #[test]
    fn a_dull_quiet_mix_gets_gain_and_a_tilt_toward_bright() {
        let mix = tilted(48_000.0, -6.0);     // dull: falls 6 dB per octave
        let reference = tilted(48_000.0, -3.0);
        let g = gap(&mix, -18.0, &reference, -14.0);
        assert!((g.loudness_db - 4.0).abs() < 1e-6);
        assert!(g.band_db[7] > 0.0 && g.band_db[0] < 0.0, "highs short, lows long: {:?}", g.band_db);
        let p = propose(&g, 1.0);
        assert_eq!(p.chain[0].kind, "gain");
        assert!((p.chain[0].params["db"] - 4.0).abs() < 1e-6);
        let eq = &p.chain[1];
        assert_eq!(eq.kind, "eq");
        assert_eq!(eq.params["bands"], 8.0);
        assert!(eq.params["gain7"] > 0.0 && eq.params["gain0"] < 0.0);
        assert!((eq.params["gain7"] - g.band_db[7] * FOLLOW).abs() < 1e-4 || eq.params["gain7"] == MAX_BAND_DB, "half the gap, clamped");
        assert_eq!(eq.params["kind0"], 1.0, "low shelf at the bottom");
        assert_eq!(eq.params["kind7"], 2.0, "high shelf at the top");
        assert!(crate::instantiate(eq, 48_000.0).is_some(), "the proposed eq is a real effect");
        assert!(p.summary.contains("+4.0 dB of level"), "{}", p.summary);
    }

    #[test]
    fn a_gap_too_wide_is_named_not_forced() {
        let mix = tilted(48_000.0, -12.0);
        let reference = tilted(48_000.0, 0.0);
        let p = propose(&gap(&mix, -14.0, &reference, -14.0), 2.0);
        let eq = p.chain.iter().find(|e| e.kind == "eq").unwrap();
        assert!(eq.params.iter().filter(|(k, _)| k.starts_with("gain")).all(|(_, v)| v.abs() <= MAX_BAND_DB + 1e-6));
        assert!(!p.beyond.is_empty(), "{}", p.summary);
        assert!(p.summary.contains("more than the EQ will move"), "{}", p.summary);
    }
}
