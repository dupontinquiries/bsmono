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
