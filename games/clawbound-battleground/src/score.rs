//! score — the match's music, synthesized: one looped bed per map, made at the start of the
//! match when no `music/<map>` file is in the bank.
//!
//! The game ships no music. `music/<map>` is a soundbank key, and with no file under it the
//! match played in silence over the wind -- `bank.pick` returned nothing and `music_on` was
//! set anyway. The fanfare (`brass`) showed the way: synthesize it. A bed is a slow modal
//! pad (three detuned saws through a filter that breathes), a bass that walks the mode on
//! the bar, a sparse pentatonic line drawn from the map's seed, and a soft shaker on the
//! off-beats; each map has its own root, mode, tempo and colour, so the Pass and the Quarry
//! do not sound alike. Rendered once into a `Clip` at the device rate and looped by the
//! mixer -- the loop is a whole number of bars, so it closes on the beat.
//!
//! Headless: [`render`] is pure DSP from a seed, so the tests can check the music is music
//! (loudness in range, no clipping, the bass on the root, the loop a whole number of bars,
//! the maps distinct) without a speaker. Nothing here has been heard in the sandbox that
//! wrote it.

use crate::brass::midi_hz;
use crate::map::MapId;

/// What a map's bed is made of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// MIDI root of the bass (the pad sits an octave up).
    pub root: u8,
    /// Semitone steps of the mode, from the root.
    pub mode: [u8; 7],
    pub bpm: f32,
    /// 0 dark (a closed filter, a low line) .. 1 bright.
    pub colour: f32,
    /// Seeds the melodic line.
    pub seed: u32,
}

const DORIAN: [u8; 7] = [0, 2, 3, 5, 7, 9, 10];
const AEOLIAN: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
const LYDIAN: [u8; 7] = [0, 2, 4, 6, 7, 9, 11];
const MIXOLYDIAN: [u8; 7] = [0, 2, 4, 5, 7, 9, 10];
const PHRYGIAN: [u8; 7] = [0, 1, 3, 5, 7, 8, 10];

/// Bars in the loop; four beats each.
pub const BARS: usize = 16;

pub fn theme(map: MapId) -> Theme {
    match map {
        // the jungle: warm, mid-tempo, dorian in D
        MapId::Jungle => Theme { root: 38, mode: DORIAN, bpm: 92.0, colour: 0.55, seed: 0x1A5E },
        // the pass: cold and open, slower, aeolian in A
        MapId::Pass => Theme { root: 33, mode: AEOLIAN, bpm: 76.0, colour: 0.3, seed: 0x2B17 },
        // the starwell: high and bright, lydian in F
        MapId::Sky => Theme { root: 41, mode: LYDIAN, bpm: 84.0, colour: 0.85, seed: 0x3C88 },
        // atlantis: slow, deep, mixolydian in G
        MapId::Atlantis => Theme { root: 31, mode: MIXOLYDIAN, bpm: 70.0, colour: 0.4, seed: 0x4D09 },
        // the quarry: tense, phrygian in E, the fastest
        MapId::Maze => Theme { root: 40, mode: PHRYGIAN, bpm: 104.0, colour: 0.5, seed: 0x5E6A },
    }
}

/// Seconds the loop lasts: a whole number of bars.
pub fn loop_secs(t: &Theme) -> f32 {
    BARS as f32 * 4.0 * 60.0 / t.bpm
}

fn saw(phase: f32) -> f32 {
    phase * 2.0 - 1.0
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// The melodic line: one event per half-bar, mostly, as (beat, degree, octave, velocity);
/// a rest is a skipped slot. Drawn from the seed, so a map's line is always its own.
fn line(t: &Theme) -> Vec<(f32, usize, u8, f32)> {
    let mut rng = Rng(t.seed | 1);
    let pent = [0usize, 1, 2, 4, 5]; // degrees of the mode that make a pentatonic
    let mut out = Vec::new();
    let mut deg = 0usize;
    for half in 0..(BARS * 2) {
        if rng.f() < 0.35 {
            continue; // a rest: the line breathes
        }
        let step = (rng.next() % 3) as i32 - 1; // -1, 0, +1 along the pentatonic
        deg = (deg as i32 + step).rem_euclid(pent.len() as i32) as usize;
        let oct = if rng.f() < 0.2 { 1 } else { 0 };
        let vel = 0.5 + 0.5 * rng.f();
        let beat = half as f32 * 2.0 + if rng.f() < 0.3 { 0.5 } else { 0.0 };
        out.push((beat, pent[deg], oct, vel));
    }
    out
}

/// Render the bed, stereo interleaved at `sr`, one loop long.
pub fn render(t: &Theme, sr: f32) -> Vec<f32> {
    let sr = sr.max(8000.0);
    let dt = 1.0 / sr;
    let secs = loop_secs(t);
    let frames = (secs * sr).round() as usize;
    let beat_s = 60.0 / t.bpm;
    let mut out = vec![0.0f32; frames * 2];
    let line = line(t);
    // pad: a chord of root, fifth and the mode's third, an octave over the bass
    let pad_midi = [t.root + 12, t.root + 12 + t.mode[4], t.root + 24 + t.mode[2]];
    let detune = [0.0f32, 0.004, -0.005];
    let mut pad_phase = [[0.0f32; 3]; 3];
    let mut pad_lp = [0.0f32; 2];
    let mut bass_phase = 0.0f32;
    let mut bass_lp = 0.0f32;
    let mut lead_phase = 0.0f32;
    let mut rng = Rng(t.seed ^ 0xA5A5);
    for i in 0..frames {
        let time = i as f32 * dt;
        let beat = time / beat_s;
        let bar = (beat / 4.0).floor() as usize;
        // PAD: the filter breathes over two bars, opening with the colour
        let breathe = 0.5 + 0.5 * (time / (beat_s * 8.0) * std::f32::consts::TAU).sin();
        let fc = 180.0 + (500.0 + 1800.0 * t.colour) * breathe;
        let g = 1.0 - (-std::f32::consts::TAU * fc.min(sr * 0.4) * dt).exp();
        let mut pad = 0.0f32;
        for (v, &m) in pad_midi.iter().enumerate() {
            for (d, &det) in detune.iter().enumerate() {
                let f = midi_hz(m) * (1.0 + det);
                pad_phase[v][d] = (pad_phase[v][d] + f * dt).fract();
                pad += saw(pad_phase[v][d]) * [1.0, 0.7, 0.55][v];
            }
        }
        pad_lp[0] += (pad - pad_lp[0]) * g;
        pad_lp[1] += (pad_lp[0] - pad_lp[1]) * g;
        let pad = pad_lp[1] * 0.045;
        // BASS: the root on the bar, the fifth on bar three of four, a soft pluck envelope
        let bass_deg = if bar % 4 == 2 { t.mode[4] } else { 0 };
        let bass_f = midi_hz(t.root + bass_deg);
        bass_phase = (bass_phase + bass_f * dt).fract();
        let in_bar = beat.rem_euclid(4.0);
        let bass_env = (-in_bar * 0.9).exp() * (in_bar * 40.0).min(1.0);
        let gb = 1.0 - (-std::f32::consts::TAU * (bass_f * 3.0) * dt).exp();
        bass_lp += (saw(bass_phase) - bass_lp) * gb;
        let bass = bass_lp * bass_env * 0.22;
        // LEAD: a sine with a little second harmonic, plucked
        let mut lead = 0.0f32;
        for &(at, deg, oct, vel) in &line {
            let since = (beat - at) * beat_s;
            if (0.0..1.6).contains(&since) {
                let f = midi_hz(t.root + 24 + t.mode[deg] + 12 * oct);
                lead_phase = (lead_phase + f * dt).fract();
                let e = (-since * 2.2).exp() * (since * 60.0).min(1.0);
                let s = (lead_phase * std::f32::consts::TAU).sin() + 0.25 * (lead_phase * 2.0 * std::f32::consts::TAU).sin();
                lead += s * e * vel * 0.07;
                break; // one note at a time
            }
        }
        // SHAKER: a noise tick on the off-beats, brighter with the colour
        let off = (beat + 0.5).rem_euclid(1.0);
        let shaker = if off < 0.08 { (rng.next() as f32 / u32::MAX as f32 * 2.0 - 1.0) * (1.0 - off / 0.08) * (0.02 + 0.03 * t.colour) } else { 0.0 };
        // stereo: the pad wide, the bass centred, the lead a little right, the shaker left
        let l = pad * 1.0 + bass + lead * 0.8 + shaker * 1.2;
        let r = pad * 0.95 + bass + lead * 1.1 + shaker * 0.7;
        out[i * 2] = (l * 1.3).tanh() * 0.8;
        out[i * 2 + 1] = (r * 1.3).tanh() * 0.8;
    }
    out
}

/// The bed as a soundbank clip.
pub fn clip(map: MapId, sr: f32) -> crate::soundbank::Clip {
    crate::soundbank::Clip { pcm: render(&theme(map), sr).into(), channels: 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt() as f32
    }

    /// Every map's bed is a whole number of bars long, loud enough to hear and never
    /// clipping, and no two maps are the same music.
    #[test]
    fn every_map_has_a_bed_that_is_music_and_its_own() {
        let mut seen: Vec<Vec<f32>> = Vec::new();
        for m in MapId::ALL {
            let t = theme(m);
            let pcm = render(&t, 22_050.0);
            let secs = pcm.len() as f32 / 2.0 / 22_050.0;
            assert!((secs - loop_secs(&t)).abs() < 0.01, "{m:?}: {secs}s vs {}", loop_secs(&t));
            let bars = secs * t.bpm / 60.0 / 4.0;
            assert!((bars - BARS as f32).abs() < 1e-3, "{m:?}: the loop closes on the bar: {bars}");
            let level = rms(&pcm);
            assert!(level > 0.03 && level < 0.4, "{m:?}: level {level}");
            assert!(pcm.iter().all(|x| x.abs() <= 0.8 && x.is_finite()), "{m:?}: clips or NaN");
            for other in &seen {
                let n = other.len().min(pcm.len()).min(22_050 * 4);
                let diff = rms(&pcm[..n].iter().zip(&other[..n]).map(|(a, b)| a - b).collect::<Vec<_>>());
                assert!(diff > 0.02, "{m:?} sounds like another map: {diff}");
            }
            seen.push(pcm);
        }
    }

    /// The bass sits on the root: the first bar's low end autocorrelates at the root's period.
    #[test]
    fn the_bass_is_on_the_root() {
        for m in MapId::ALL {
            let t = theme(m);
            let sr = 22_050.0f32;
            let pcm = render(&t, sr);
            // mono, low-passed by a crude running mean, the first bar
            let beat_s = 60.0 / t.bpm;
            let n = (beat_s * 4.0 * sr) as usize;
            let mono: Vec<f32> = (0..n).map(|i| (pcm[i * 2] + pcm[i * 2 + 1]) * 0.5).collect();
            let w = (sr / 400.0) as usize;
            let low: Vec<f32> = (0..mono.len().saturating_sub(w)).map(|i| mono[i..i + w].iter().sum::<f32>() / w as f32).collect();
            let period = sr / midi_hz(t.root);
            let corr = |lag: usize| -> f32 {
                let k = low.len() - lag;
                low[..k].iter().zip(&low[lag..]).map(|(a, b)| a * b).sum::<f32>() / k as f32
            };
            let at_root = corr(period.round() as usize);
            let at_tritone = corr((period / 1.414).round() as usize);
            assert!(at_root > at_tritone, "{m:?}: the bass does not sit on its root ({at_root} vs {at_tritone})");
            assert!(at_root > 0.0);
        }
    }

    /// The clip is stereo at the asked rate; a low rate is clamped rather than dividing by zero.
    #[test]
    fn the_clip_is_stereo_and_a_silly_rate_is_clamped() {
        let c = clip(MapId::Jungle, 8000.0);
        assert_eq!(c.channels, 2);
        assert_eq!(c.frames(), (loop_secs(&theme(MapId::Jungle)) * 8000.0).round() as usize);
        let c = clip(MapId::Pass, 0.0);
        assert!(c.frames() > 0);
    }
}
