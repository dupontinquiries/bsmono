//! How capable are the synth stand-ins, measured: for every Poly channel, at
//! its notes -- alone, or LAYERED with other synth notes (harmonics measured
//! where they are clear of theirs) -- the TIMBRE error (dB RMS over harmonics
//! 1-8) between the artist's render and Studio's, the SAME masking on both.
//!     cargo run -p studio-app --example synth_score -- song.studio reference.opus
//! Only the note windows are rendered.
use std::collections::BTreeMap;
use studio_app::synth_fit::{masked_profile, notes_with_company, profile_error_db};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).expect("project");
    let project = save.to_project();
    let rack = save.rack.clone().expect("a rack");
    let r = studio_media::decode_file(std::path::Path::new(&a[2])).expect("reference");
    let mono: Vec<f32> = (0..r.frames()).map(|i| r.channels.iter().map(|c| c[i]).sum::<f32>() / r.channels.len() as f32).collect();
    let base = path.parent().unwrap();
    let mut audio = BTreeMap::new();
    for s in project.sources.values().filter(|s| !s.is_generator()) {
        if let Ok(x) = studio_media::decode_file(&base.join(&s.path)) { audio.insert(s.id, x); }
    }
    let mut st = BTreeMap::new();
    studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut st);
    let mut errs = Vec::new();
    println!("{:<30} {:>5} {:>7} {:>10}", "channel", "solo", "layered", "timbre dB");
    for (ch, items) in notes_with_company(&project, &rack) {
        let name = &rack.channels[ch].name;
        let mut use_items: Vec<_> = items.iter().filter(|(n, _)| n.end - n.start >= 0.2).collect();
        use_items.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then((b.0.end - b.0.start).total_cmp(&(a.0.end - a.0.start))));
        use_items.truncate(24);
        let (solo, layered) = (use_items.iter().filter(|(_, o)| o.is_empty()).count(), use_items.iter().filter(|(_, o)| !o.is_empty()).count());
        let (mut wr, mut wo) = (Vec::new(), Vec::new());
        for (n, o) in &use_items {
            let end = n.end.min(n.start + 0.8);
            let (i0, i1) = ((n.start * r.sample_rate as f64) as usize, (end * r.sample_rate as f64) as usize);
            if i1 > mono.len() || i1 <= i0 { continue; }
            wr.push((mono[i0..i1].to_vec(), r.sample_rate, n.hz, o.clone()));
            let mix = studio_audio::mixdown(&project, &audio, &BTreeMap::new(), studio_core::Ticks::from_seconds(n.start),
                                            studio_core::Ticks::from_seconds(end), 48_000, &[], &[]);
            wo.push((mix.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect(), 48_000u32, n.hz, o.clone()));
        }
        match (masked_profile(&wr), masked_profile(&wo)) {
            (Some(want), Some(got)) => {
                let e = profile_error_db(&want, &got);
                errs.push(e);
                println!("{name:<30} {solo:>5} {layered:>7} {e:>10.1}");
            }
            _ => println!("{name:<30} {solo:>5} {layered:>7} {:>10}", "-"),
        }
    }
    errs.sort_by(f32::total_cmp);
    match errs.get(errs.len() / 2) {
        Some(m) => println!("median timbre error over {} scored channels: {m:.1} dB", errs.len()),
        None => println!("no channel has a usable note to score"),
    }
}
