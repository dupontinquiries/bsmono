//! Fit each Poly stand-in's TEXTURE from the artist's render (studio_app::synth_fit).
//!     cargo run -p studio-app --example fit_synths -- song.studio reference.flac [out.studio]
//! For every Poly channel, the notes it plays ALONE (no other rack channel,
//! no audio clip sounding) are measured in the reference; the harmonic
//! profile becomes the patch's `partials`, with the filter opened (the
//! render's spectrum already carries the original synth's filtering).
//! A channel that never plays alone keeps its guessed patch -- and is listed.
use studio_app::synth_fit::{harmonic_profile, NoteAt};
use studio_core::SourceKind;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, refp) = (std::path::PathBuf::from(&a[1]), std::path::PathBuf::from(&a[2]));
    let out = a.get(3).map(std::path::PathBuf::from).unwrap_or_else(|| path.clone());
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).expect("project");
    let project = save.to_project();
    let mut rack = save.rack.clone().expect("a rack");
    let step = 60.0 / rack.tempo_bpm as f64 / 4.0;
    let muted: std::collections::BTreeSet<_> = project.tracks.iter().filter(|t| t.muted).map(|t| t.id).collect();
    // every note, per rack channel: (start s, end s, key)
    let mut notes: Vec<Vec<(f64, f64, i32)>> = vec![Vec::new(); rack.channels.len()];
    let mut audio_busy: Vec<(f64, f64)> = Vec::new();
    for c in project.clips.values().filter(|c| !muted.contains(&c.track)) {
        let (c0, c1, off) = (c.pos.to_seconds(), c.end().to_seconds(), c.src_in.to_seconds());
        match project.sources.get(&c.source).map(|s| &s.kind) {
            Some(SourceKind::Pattern { pattern, .. }) => {
                let Some(pt) = rack.patterns.get(*pattern) else { continue };
                for ch in 0..rack.channels.len() {
                    if rack.channels[ch].muted { continue; }
                    for n in pt.roll(ch) {
                        let t0 = c0 + n.start as f64 * step - off;
                        let t1 = t0 + n.len as f64 * step;
                        if t0 >= c0 && t0 < c1 { notes[ch].push((t0, t1.min(c1), n.key as i32)); }
                    }
                }
            }
            _ => audio_busy.push((c0, c1)),
        }
    }
    let r = studio_media::decode_file(&refp).expect("reference");
    let mono: Vec<f32> = (0..r.frames()).map(|i| r.channels.iter().map(|c| c[i]).sum::<f32>() / r.channels.len() as f32).collect();
    let overlaps = |a: (f64, f64), b: (f64, f64)| a.0 < b.1 && b.0 < a.1;
    println!("{:<28} {:>6} {:>6}  profile (harmonics 1-16)", "channel", "notes", "alone");
    for ch in 0..rack.channels.len() {
        let studio_live::SynthKind::Poly { hz, patch } = rack.channels[ch].kind.clone() else { continue };
        let alone: Vec<NoteAt> = notes[ch].iter().filter(|n| {
            !audio_busy.iter().any(|b| overlaps((n.0, n.1), *b))
                && !notes.iter().enumerate().any(|(o, v)| o != ch && v.iter().any(|m| overlaps((n.0, n.1), (m.0, m.1))))
        }).map(|n| NoteAt { start: n.0, end: n.1, hz: hz * 2f32.powf(n.2 as f32 / 12.0) }).collect();
        match harmonic_profile(&mono, r.sample_rate, &alone) {
            Some(p) => {
                println!("{:<28} {:>6} {:>6}  {:?}", rack.channels[ch].name, notes[ch].len(), alone.len(), p);
                rack.channels[ch].kind = studio_live::SynthKind::Poly { hz, patch: studio_live::PolyPatch {
                    // the profile carries the real low end: no added sub; a moderate
                    // cutoff the envelope opens gives back the highs' decay a static
                    // cycle lacks (tried first: cutoff 16k, sub kept -- ~4 dB too
                    // bright above 1 kHz and +8 dB in the lows on Blue Moon's keys)
                    partials: p, sub: 0.0, cutoff: 6_000.0, env_amount: 1.0, ..patch } };
            }
            None => println!("{:<28} {:>6} {:>6}  never alone long enough: guessed patch kept", rack.channels[ch].name, notes[ch].len(), alone.len()),
        }
    }
    let f = studio_collab::file::SaveFile::from_project(&project, Vec::new(), Some(rack));
    std::fs::write(&out, f.encode()).expect("write");
    println!("wrote {}", out.display());
}
