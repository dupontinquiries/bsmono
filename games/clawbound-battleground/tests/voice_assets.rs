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
    // the four from Kessler's first take, at least
    for w in ["goldenmane", "oldroar", "twofang", "windclaw"] {
        assert!(recorded.contains(&w), "{w} has no recorded lines");
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
    assert!(n >= 36, "checked {n} lines");
}
