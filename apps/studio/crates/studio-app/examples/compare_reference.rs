//! Render a Studio project and hold it up against a reference render.
//!     cargo run -p studio-app --example compare_reference -- song.studio reference.flac [bars_per_section] [--ours render.f32]
//! `--ours` compares an existing render (raw 48 kHz stereo f32le, as
//! `render_project` writes) instead of rendering again -- one render, held up
//! against several references (a mix and its master, say).
//! Renders what Studio hears (file clips, pattern clips from the rack, the
//! busses) at 48 kHz, then reports duration, timing and level per section --
//! see `studio_app::reference_check`.
use std::collections::BTreeMap;

fn mono(v: &studio_audio::SourceAudio) -> Vec<f32> {
    let n = v.channels.len().max(1) as f32;
    (0..v.frames()).map(|i| v.channels.iter().map(|c| c[i]).sum::<f32>() / n).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let proj_path = std::path::PathBuf::from(&a[1]);
    let bars: f64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(8.0);
    let ours_file = a.iter().position(|x| x == "--ours").and_then(|i| a.get(i + 1)).cloned();
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&proj_path).unwrap()).expect("project");
    let rack = save.rack.clone().unwrap_or_else(studio_live::Rack::trap_kit);
    let project = save.to_project();
    let base = proj_path.parent().unwrap();
    let rate = 48_000;
    let ours: Vec<f32> = if let Some(f) = &ours_file {
        let b = std::fs::read(f).expect("the render");
        b.chunks_exact(8).map(|c| (f32::from_le_bytes(c[0..4].try_into().unwrap()) + f32::from_le_bytes(c[4..8].try_into().unwrap())) * 0.5).collect()
    } else {
    let mut audio: BTreeMap<studio_core::SourceId, studio_audio::SourceAudio> = BTreeMap::new();
    for s in project.sources.values().filter(|s| !s.is_generator()) {
        match studio_media::decode_file(&base.join(&s.path)) {
            Ok(x) => { audio.insert(s.id, x); }
            Err(e) => eprintln!("cannot decode {}: {e:?}", s.path),
        }
    }
    let mut stamps = BTreeMap::new();
    let rendered = studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut stamps);
    eprintln!("{} file sources, {} pattern renders", audio.len() - rendered.len(), rendered.len());
    let fades: BTreeMap<studio_core::ClipId, studio_audio::Fades> = project.fades.iter()
        .map(|(id, (i, o))| (*id, studio_audio::Fades { fade_in: *i, fade_out: *o })).collect();
    let end = project.duration();
    let t0 = std::time::Instant::now();
    // In 5 s chunks through ONE effect cache -- the export's own way, which
    // matches a one-shot render sample for sample. One call over the whole
    // song holds every bus's full-length buffer at once and was killed for
    // memory on this 4 GB box (27 busses).
    let mut cache = studio_audio::EffectCache::new();
    let mut ours: Vec<f32> = Vec::new();
    let step = studio_core::Ticks::from_seconds(5.0);
    let mut t = studio_core::Ticks::ZERO;
    while t < end {
        let to = t.saturating_add(step).min(end);
        let mix = studio_audio::mixdown_with_cache(&project, &audio, &fades, t, to, rate, &[], &[], &mut cache);
        let frames = to.saturating_sub(t).to_samples(rate) as usize;
        if mix.len() >= frames * 2 { ours.extend(mix.chunks(2).map(|c| (c[0] + c[1]) * 0.5)); } else { ours.extend(mix); }
        t = to;
        eprint!("\r{:.0} s", t.to_seconds());
    }
    eprintln!();
    eprintln!("mixdown {:.1} s in {:.0} s", ours.len() as f64 / rate as f64, t0.elapsed().as_secs_f64());
    ours
    };
    let r = studio_media::decode_file(std::path::Path::new(&a[2])).expect("reference");
    assert_eq!(r.sample_rate, rate, "reference must be 48 kHz here");
    let reference = mono(&r);
    let bar = 4.0 * 60.0 / rack.tempo_bpm as f64;
    let rep = studio_app::reference_check::compare(&reference, &ours, rate, bars * bar, 0.5);
    println!("{}", rep.text(&|t| format!("bars {:>3}-{:<3}", (t / bar).round() as i64 + 1, (t / bar).round() as i64 + bars as i64)));
}
