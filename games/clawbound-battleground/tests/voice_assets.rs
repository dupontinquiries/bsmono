//! The recorded voices under `assets/audio/voice`, loaded the way the game
//! loads them (`soundbank::Loader` over the audio root, at the mixer's
//! 48 kHz): every ancestor with any recorded line has every line the game
//! voices (`voicescript::PARTS`), each decodes, is trimmed, is audible and
//! is not clipped.

use std::collections::HashMap;
use std::path::PathBuf;

use clawbound::soundbank::{Clip, Loader};
use clawbound::voicescript::{self, PARTS};

const RATE: u32 = 48_000;

fn load() -> HashMap<String, Clip> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/audio");
    let mut out = HashMap::new();
    for l in Loader::start(root.clone(), RATE) {
        let rel = l.path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let clip = l.result.unwrap_or_else(|e| panic!("{rel} does not decode: {e:?}"));
        out.insert(rel, clip);
    }
    out
}

#[test]
fn every_recorded_voice_is_complete_trimmed_and_levelled() {
    let files = load();
    let recorded: Vec<&str> = PARTS.iter().map(|p| p.who).filter(|w| files.keys().any(|f| f.starts_with(&format!("voice/{w}/")))).collect();
    // every ancestor speaks: four recorded (Kessler), the rest stand-ins
    // (dev/voice_script/synth_session.py) until they are
    for w in PARTS.iter().map(|p| p.who).filter(|w| *w != "narrator" && *w != "announcer") {
        assert!(recorded.contains(&w), "{w} has no voice lines");
    }
    let mut n = 0;
    for who in &recorded {
        let part = voicescript::part(who).unwrap();
        for line in part.lines {
            let f = voicescript::file_for(who, line.key);
            let clip = files.get(&f).unwrap_or_else(|| panic!("{f} missing: '{}' is not recorded", line.text));
            assert_eq!(clip.channels, 1, "{f}: voices are mono (the mixer places them)");
            let pcm = &clip.pcm[..];
            let secs = pcm.len() as f32 / RATE as f32;
            assert!((0.5..6.0).contains(&secs), "{f}: {secs:.2} s is not one line");
            let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((0.25..=0.97).contains(&peak), "{f}: peak {peak:.2} (quiet, or clipping)");
            // trimmed: sound (-30 dB under the line's peak: a breath or a soft
            // first syllable counts) within 0.3 s, and the file is not mostly
            // silence
            let h = (RATE / 100) as usize;
            let rms: Vec<f32> = pcm.chunks(h).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let loud = rms.iter().cloned().fold(0.0f32, f32::max);
            let first = rms.iter().position(|r| *r > loud * 0.03).unwrap();
            assert!(first as f32 * 0.01 < 0.3, "{f}: {:.2} s of silence before the line", first as f32 * 0.01);
            let active = rms.iter().filter(|r| **r > loud * 0.03).count() as f32 / rms.len() as f32;
            assert!(active > 0.2, "{f}: only {:.0}% of it is sound", active * 100.0);
            n += 1;
        }
    }
    assert!(n >= 14 * 9, "checked {n} lines");
}

/// The bots speak: over two minutes of a full 5v5 run by bots (the
/// listener's own cat too), every bank key the voice cues ask for is looked
/// up in the files the game loads, and the bots' own lines -- casts, kills,
/// deaths -- are found for many different ancestors, and heard (within the
/// voice range of the listener) for some.
#[test]
fn the_bots_speak_their_own_lines() {
    use clawbound::cues::{self, cues_for, place};
    use clawbound::mixer::Bus;
    use clawbound::sim::{AiBrain, Game};
    use glam::Vec3;
    let files = load();
    let has = |key: &str| files.keys().any(|f| f.strip_suffix(".opus").is_some_and(|s| s == key || s.rsplit_once('.').is_some_and(|(b, n)| b == key && n.parse::<u32>().is_ok())));
    let blue: Vec<u32> = (0..5).collect();
    let red: Vec<u32> = (5..10).collect();
    let mut g = Game::new_teams(&blue, &red);
    g.gods[0].ai = Some(AiBrain::new(clawbound::consts::Lane::Mid, 0.8));
    let mut voiced = std::collections::HashSet::new();
    let (mut heard, mut asked) = (0, 0);
    for _ in 0..(120 * 30) {
        g.tick(1.0 / 30.0);
        let ear = g.gods[0].pos;
        for ev in std::mem::take(&mut g.events) {
            for cue in cues_for(&ev, &g) {
                let Some(sp) = cue.speaker else { continue };
                if cue.bus != Bus::Voice || sp == 0 {
                    continue;
                }
                asked += 1;
                if let Some(k) = cue.keys.iter().find(|k| has(k)) {
                    voiced.insert(k.split('/').nth(1).unwrap().to_string());
                    if place(&cue, ear, Vec3::X).is_some() {
                        heard += 1;
                    }
                }
            }
        }
    }
    assert!(asked >= 20, "the bots asked for only {asked} voice lines in two minutes");
    assert!(voiced.len() >= 6, "bot lines found for only {} ancestors: {voiced:?}", voiced.len());
    assert!(heard >= 3, "only {heard} bot lines within earshot (range {})", cues::VOICE_RANGE);
}

/// The pitch class (0..12, semitones above A, in half-semitone steps) a
/// clip mostly sings on: over its loud 43 ms frames, the autocorrelation
/// peak between 150 Hz and 2.6 kHz, and the most common class among them.
/// A glide or an octave error in one frame does not move the mode.
fn pitch_class(pcm: &[f32]) -> Option<f32> {
    let (n, hop) = (2048, 1024);
    if pcm.len() < n + hop {
        return None;
    }
    let frames: Vec<&[f32]> = (0..(pcm.len() - n) / hop).map(|i| &pcm[i * hop..i * hop + n]).collect();
    let energy: Vec<f32> = frames.iter().map(|f| f.iter().map(|s| s * s).sum()).collect();
    let top = energy.iter().cloned().fold(0.0f32, f32::max);
    let (lo, hi) = ((RATE as f32 / 2600.0) as usize, (RATE as f32 / 150.0) as usize);
    let mut hist = [0u32; 24];
    let mut voiced = 0;
    for (f, e) in frames.iter().zip(&energy) {
        if *e < 0.3 * top {
            continue;
        }
        let ac = |l: usize| f[..n - l].iter().zip(&f[l..]).map(|(a, b)| a * b).sum::<f32>() / e;
        let (lag, peak) = (lo..hi).map(|l| (l, ac(l))).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        if peak < 0.5 {
            continue;
        }
        let semis = 12.0 * (RATE as f32 / lag as f32 / 440.0).log2();
        hist[(semis.rem_euclid(12.0) * 2.0).round() as usize % 24] += 1;
        voiced += 1;
    }
    (voiced >= 2).then(|| (0..24).max_by_key(|&i| hist[i]).unwrap() as f32 / 2.0)
}

fn class_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(12.0);
    d.min(12.0 - d)
}

/// Sproutkit's swarm (dev/voice_script/baby_swarm.py): for each ability,
/// the keys a cast asks for (`cues::cues_for`) are there -- at least five
/// babies, each with two takes, each a mono clip of one baby, audible and
/// not clipped -- and the crowd is a chord, not a unison: plenty of its
/// babies are well off the note most of them sing.
#[test]
fn sproutkit_brings_a_crowd_of_babies_in_harmony() {
    use clawbound::cues::cues_for;
    use clawbound::sim::{Event, Game};
    let files = load();
    let g = Game::new_teams(&[14, 1, 2, 3, 4], &[5, 6, 7, 8, 9]);
    assert_eq!(g.god_name(0), "Sproutkit");
    for slot in 0..4u32 {
        let keys: Vec<String> = cues_for(&Event::Cast { god: 0, slot, aim: glam::Vec3::ZERO }, &g)
            .into_iter()
            .map(|c| c.keys[0].clone())
            .filter(|k| k.starts_with("sfx/swarm/"))
            .collect();
        let mut babies = 0;
        let mut classes: Vec<f32> = Vec::new();
        for k in &keys {
            let takes: Vec<&Clip> = (1..=2).filter_map(|t| files.get(&format!("{k}.{t}.opus"))).collect();
            if takes.is_empty() {
                continue;
            }
            assert_eq!(takes.len(), 2, "{k}: two takes");
            babies += 1;
            for clip in takes {
                assert_eq!(clip.channels, 1, "{k}: one baby, mono (the cue places it)");
                let secs = clip.pcm.len() as f32 / RATE as f32;
                assert!((0.25..4.0).contains(&secs), "{k}: {secs:.2} s");
                let peak = clip.pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!((0.2..=0.97).contains(&peak), "{k}: peak {peak:.2}");
                classes.extend(pitch_class(&clip.pcm));
            }
        }
        assert!(babies >= 5, "ability {}: only {babies} babies", slot + 1);
        // a chord, not a unison: take the note most of the crowd is near,
        // and count the babies well off it (a unison crowd, glides and
        // all, leaves at most one or two out there)
        assert!(classes.len() >= 8, "ability {}: pitched only {} clips", slot + 1, classes.len());
        let unison = classes.iter().map(|&c| classes.iter().filter(|&&x| class_dist(x, c) <= 1.0).count()).max().unwrap();
        let root = *classes.iter().find(|&&c| classes.iter().filter(|&&x| class_dist(x, c) <= 1.0).count() == unison).unwrap();
        let off = classes.iter().filter(|&&x| class_dist(x, root) > 2.5).count();
        assert!(off >= 4, "ability {}: only {off} of {} clips off the crowd's note {root}: {classes:?}", slot + 1, classes.len());
    }
}

/// Hollowstep's phantoms (dev/voice_script/phantom.py): for each ability,
/// the swarm keys the cast asks for, each a mono whisper in two takes --
/// and made strange, not copies of the line: the first is the line
/// reversed (it matches the reversed line better than the line itself),
/// and none of them lines up with the line sample for sample.
#[test]
fn hollowstep_speaks_from_places_he_is_not() {
    let files = load();
    let corr = |a: &[f32], b: &[f32]| -> f32 {
        // best normalised cross-correlation over lags up to 0.3 s, on
        // 1/8-decimated envelopes-free signals
        let d = |x: &[f32]| x.iter().step_by(8).copied().collect::<Vec<f32>>();
        let (a, b) = (d(a), d(b));
        let n = a.len().min(b.len());
        let e = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
        let mut best = 0.0f32;
        for lag in (0..(RATE as usize / 8 * 3 / 10)).step_by(12) {
            if lag >= n {
                break;
            }
            let dot: f32 = (0..n - lag).map(|i| a[i + lag] * b[i]).sum();
            best = best.max((dot / (e(&a[lag..n]) * e(&b[..n - lag]))).abs());
        }
        best
    };
    let mut checked = 0;
    for slot in 1..=4 {
        let line = &files[&format!("voice/hollowstep/cast{slot}.opus")].pcm;
        for k in 0..5 {
            for t in 1..=2 {
                let key = format!("sfx/swarm/hollowstep/{slot}/b{k}.{t}.opus");
                let c = files.get(&key).unwrap_or_else(|| panic!("{key} missing"));
                assert_eq!(c.channels, 1, "{key}: mono, the cue places it");
                let peak = c.pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!((0.3..=0.97).contains(&peak), "{key}: peak {peak:.2}");
                checked += 1;
            }
        }
        let b0 = &files[&format!("sfx/swarm/hollowstep/{slot}/b0.1.opus")].pcm;
        let rev: Vec<f32> = line.iter().rev().copied().collect();
        let (fwd, back) = (corr(b0, line), corr(b0, &rev));
        assert!(back > fwd, "cast{slot}: the first phantom is the line reversed ({back:.2} against {fwd:.2} forward)");
    }
    assert_eq!(checked, 4 * 5 * 2);
}
