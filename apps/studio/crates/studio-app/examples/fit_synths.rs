//! Fit each Poly stand-in's TEXTURE from the artist's render (studio_app::synth_fit).
//!     cargo run -p studio-app --example fit_synths -- song.studio reference.flac [out.studio]
//! Every Poly channel's notes -- alone OR layered with other synth notes (its
//! harmonics measured where they are clear of theirs; drums and audio clips
//! excluded) -- are measured in the reference; the MASKED harmonic profile
//! becomes the patch's `partials` (no added sub; a moderate cutoff the
//! envelope opens). A channel with no usable note keeps its guessed patch,
//! and one inaudible in FL is not fitted -- both listed.
use studio_app::synth_fit::{masked_profile, notes_with_company};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, refp) = (std::path::PathBuf::from(&a[1]), std::path::PathBuf::from(&a[2]));
    let out = a.get(3).map(std::path::PathBuf::from).unwrap_or_else(|| path.clone());
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).expect("project");
    let project = save.to_project();
    let mut rack = save.rack.clone().expect("a rack");
    let r = studio_media::decode_file(&refp).expect("reference");
    let mono: Vec<f32> = (0..r.frames()).map(|i| r.channels.iter().map(|c| c[i]).sum::<f32>() / r.channels.len() as f32).collect();
    let rate = r.sample_rate;
    println!("{:<28} {:>5} {:>7}  profile (harmonics 1-16)", "channel", "solo", "layered");
    let fitted: Vec<usize> = notes_with_company(&project, &rack).iter().map(|(c, _)| *c).collect();
    for (ch, items) in notes_with_company(&project, &rack) {
        let studio_live::SynthKind::Poly { hz, patch } = rack.channels[ch].kind.clone() else { continue };
        let mut use_items: Vec<_> = items.iter().filter(|(n, _)| n.end - n.start >= 0.2).collect();
        use_items.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then((b.0.end - b.0.start).total_cmp(&(a.0.end - a.0.start))));
        use_items.truncate(40);
        let (solo, layered) = (use_items.iter().filter(|(_, o)| o.is_empty()).count(), use_items.iter().filter(|(_, o)| !o.is_empty()).count());
        let wins: Vec<(Vec<f32>, u32, f32, Vec<f32>)> = use_items.iter().filter_map(|(n, o)| {
            let (i0, i1) = ((n.start * rate as f64) as usize, ((n.end.min(n.start + 0.8)) * rate as f64) as usize);
            (i1 <= mono.len() && i1 > i0).then(|| (mono[i0..i1].to_vec(), rate, n.hz, o.clone()))
        }).collect();
        match masked_profile(&wins) {
            Some(p) => {
                println!("{:<28} {:>5} {:>7}  {:?}", rack.channels[ch].name, solo, layered, p);
                rack.channels[ch].kind = studio_live::SynthKind::Poly { hz, patch: studio_live::PolyPatch {
                    partials: p, sub: 0.0, cutoff: 6_000.0, env_amount: 1.0, ..patch } };
            }
            None => println!("{:<28} {:>5} {:>7}  no usable note: guessed patch kept", rack.channels[ch].name, solo, layered),
        }
    }
    for (i, c) in rack.channels.iter().enumerate() {
        if matches!(c.kind, studio_live::SynthKind::Poly { .. }) && !fitted.contains(&i) {
            println!("{:<28} inaudible in FL (gain {:.4}): not fitted", c.name, c.gain);
        }
    }
    let f = studio_collab::file::SaveFile::from_project(&project, Vec::new(), Some(rack));
    std::fs::write(&out, f.encode()).expect("write");
    println!("wrote {}", out.display());
}
