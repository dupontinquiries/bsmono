//! Fit each Poly stand-in's TEXTURE from the artist's render (studio_app::synth_fit).
//!     cargo run -p studio-app --example fit_synths -- song.studio reference.flac [out.studio]
//! For every Poly channel, the notes it plays ALONE (no other rack channel,
//! no audio clip sounding) are measured in the reference; the harmonic
//! profile becomes the patch's `partials`, with the filter opened (the
//! render's spectrum already carries the original synth's filtering).
//! A channel that never plays alone keeps its guessed patch -- and is listed.
use studio_app::synth_fit::harmonic_profile;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, refp) = (std::path::PathBuf::from(&a[1]), std::path::PathBuf::from(&a[2]));
    let out = a.get(3).map(std::path::PathBuf::from).unwrap_or_else(|| path.clone());
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).expect("project");
    let project = save.to_project();
    let mut rack = save.rack.clone().expect("a rack");
    let r = studio_media::decode_file(&refp).expect("reference");
    let mono: Vec<f32> = (0..r.frames()).map(|i| r.channels.iter().map(|c| c[i]).sum::<f32>() / r.channels.len() as f32).collect();
    let solo = studio_app::synth_fit::solo_notes(&project, &rack);
    println!("{:<28} {:>6}  profile (harmonics 1-16)", "channel", "alone");
    for (ch, alone) in solo {
        let studio_live::SynthKind::Poly { hz, patch } = rack.channels[ch].kind.clone() else { continue };
        match harmonic_profile(&mono, r.sample_rate, &alone) {
            Some(p) => {
                println!("{:<28} {:>6}  {:?}", rack.channels[ch].name, alone.len(), p);
                rack.channels[ch].kind = studio_live::SynthKind::Poly { hz, patch: studio_live::PolyPatch {
                    // the profile carries the real low end: no added sub; a moderate
                    // cutoff the envelope opens gives back the highs' decay a static
                    // cycle lacks
                    partials: p, sub: 0.0, cutoff: 6_000.0, env_amount: 1.0, ..patch } };
            }
            None => println!("{:<28} {:>6}  never alone long enough: guessed patch kept", rack.channels[ch].name, alone.len()),
        }
    }
    let f = studio_collab::file::SaveFile::from_project(&project, Vec::new(), Some(rack));
    std::fs::write(&out, f.encode()).expect("write");
    println!("wrote {}", out.display());
}
