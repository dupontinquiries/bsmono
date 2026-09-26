//! How capable are the synth stand-ins, measured: for every Poly channel with
//! notes it plays ALONE, the TIMBRE error (dB RMS over harmonics 1-8) and the
//! LEVEL error between the artist's render and Studio's, at those notes.
//!     cargo run -p studio-app --example synth_score -- song.studio reference.opus
//! Only the solo-note windows are rendered. Run on a guessed project and on a
//! fitted one (examples/fit_synths) to see what fitting buys.
use std::collections::BTreeMap;
use studio_app::synth_fit::{harmonic_profile, profile_error_db, profile_of, solo_notes};

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
    let rms_db = |v: &[f32]| 10.0 * (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).max(1e-12).log10();
    let mut errs = Vec::new();
    println!("{:<30} {:>5} {:>10} {:>9}", "channel", "solo", "timbre dB", "level dB");
    for (ch, notes) in solo_notes(&project, &rack) {
        let name = &rack.channels[ch].name;
        let use_notes: Vec<_> = notes.iter().filter(|n| n.end - n.start >= 0.2).take(24).copied().collect();
        if use_notes.is_empty() { println!("{name:<30} {:>5} {:>10} {:>9}", notes.len(), "-", "-"); continue; }
        let Some(want) = harmonic_profile(&mono, r.sample_rate, &use_notes) else { continue };
        let mut wins = Vec::new();
        let (mut lr, mut lo) = (Vec::new(), Vec::new());
        for n in &use_notes {
            let (t0, t1) = (studio_core::Ticks::from_seconds(n.start), studio_core::Ticks::from_seconds(n.end.min(n.start + 0.8)));
            let mix = studio_audio::mixdown(&project, &audio, &BTreeMap::new(), t0, t1, 48_000, &[], &[]);
            let m: Vec<f32> = mix.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
            let (i0, i1) = ((n.start * r.sample_rate as f64) as usize, ((n.end.min(n.start + 0.8)) * r.sample_rate as f64) as usize);
            if i1 <= mono.len() { lr.extend_from_slice(&mono[i0..i1]); }
            lo.extend_from_slice(&m);
            wins.push((m, 48_000u32, n.hz));
        }
        let Some(got) = profile_of(&wins) else { continue };
        let (e, l) = (profile_error_db(&want, &got), rms_db(&lo) - rms_db(&lr));
        errs.push(e);
        println!("{name:<30} {:>5} {:>10.1} {:>+9.1}", use_notes.len(), e, l);
    }
    errs.sort_by(f32::total_cmp);
    if let Some(m) = errs.get(errs.len() / 2) { println!("median timbre error over {} scored channels: {m:.1} dB", errs.len()); }
    else { println!("no channel plays alone long enough to score"); }
}
