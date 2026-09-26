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
    let (rl, rr) = (r.channels[0].clone(), r.channels[r.channels.len().min(2) - 1].clone());
    let rate = r.sample_rate;
    println!("{:<28} {:>5} {:>7}  profile (harmonics 1-16)", "channel", "solo", "layered");
    let fitted: Vec<usize> = notes_with_company(&project, &rack).iter().map(|(c, _)| *c).collect();
    for (ch, items) in notes_with_company(&project, &rack) {
        let studio_live::SynthKind::Poly { hz, patch } = rack.channels[ch].kind.clone() else { continue };
        // each note measured in its longest stretch clear of drum hits
        let mut use_items: Vec<(studio_app::synth_fit::NoteAt, Vec<f32>)> = items.iter().filter_map(|c|
            studio_app::synth_fit::clean_window(c, 0.8).map(|(a, b)| (studio_app::synth_fit::NoteAt { start: a, end: b, hz: c.note.hz }, c.others.clone()))).collect();
        use_items.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then((b.0.end - b.0.start).total_cmp(&(a.0.end - a.0.start))));
        use_items.truncate(40);
        let (solo, layered) = (use_items.iter().filter(|(_, o)| o.is_empty()).count(), use_items.iter().filter(|(_, o)| !o.is_empty()).count());
        let wins: Vec<(Vec<f32>, u32, f32, Vec<f32>)> = use_items.iter().filter_map(|(n, o)| {
            let (i0, i1) = ((n.start * rate as f64) as usize, ((n.end.min(n.start + 0.8)) * rate as f64) as usize);
            (i1 <= mono.len() && i1 > i0).then(|| (mono[i0..i1].to_vec(), rate, n.hz, o.clone()))
        }).collect();
        // the IMAGE: the same notes, up to 2 s (the pan envelope needs >= 1 s), L and R
        let swins: Vec<(Vec<f32>, Vec<f32>, u32, f32, Vec<f32>)> = items.iter().filter_map(|c| {
            let (a, b) = studio_app::synth_fit::clean_window(c, 2.0)?;
            let (i0, i1) = ((a * rate as f64) as usize, (b * rate as f64) as usize);
            (i1 <= rl.len() && i1 > i0).then(|| (rl[i0..i1].to_vec(), rr[i0..i1].to_vec(), rate, c.note.hz, c.others.clone()))
        }).take(40).collect();
        let img = studio_app::synth_fit::stereo_image(&swins);
        match masked_profile(&wins) {
            Some(p) => {
                println!("{:<28} {:>5} {:>7}  {:?}", rack.channels[ch].name, solo, layered, p);
                let mut np = studio_live::PolyPatch { partials: p, sub: 0.0, cutoff: 6_000.0, env_amount: 1.0, ..patch };
                if let Some(im) = img {
                    // width / phase from |side|/|mid| -- APPROXIMATE: assumes our width-1
                    // unison has side ~ mid; phase p gives side/mid (1+p)/(1-p)
                    let w = im.width_high;
                    if w >= 1.0 { np.width = 1.0; np.phase = ((w - 1.0) / (w + 1.0)).clamp(0.0, 0.95); }
                    else { np.width = w.clamp(0.0, 1.0); np.phase = 0.0; }
                    if np.width > 0.2 { np.voices = np.voices.max(3); if np.detune < 6.0 { np.detune = 8.0; } }
                    np.low_width = if w > 1e-3 { (im.width_low / w).clamp(0.0, 1.0) } else { 1.0 };
                    if im.key_pan.abs() > 0.1 { np.key_pan = im.key_pan.clamp(-1.0, 1.0); }
                    np.pan_start = im.pan_start; np.pan_end = im.pan_end; np.pan_time_ms = 1500.0;
                    println!("{:<28} image: pan {:+.2} width hi {:.2} lo {:.2} key_pan {:+.2} env {:+.2} -> {:+.2} ({} notes) => width {:.2} phase {:.2} low_width {:.2}",
                             "", im.pan, im.width_high, im.width_low, im.key_pan, im.pan_start, im.pan_end, im.notes, np.width, np.phase, np.low_width);
                }
                rack.channels[ch].kind = studio_live::SynthKind::Poly { hz, patch: np };
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
