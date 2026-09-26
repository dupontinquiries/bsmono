//! Each bus that feeds the master, soloed, over one window: its level (dB RMS)
//! -- where a mix's loudness comes from.
//!     cargo run -p studio-app --example bus_levels -- song.studio [from_s] [secs]
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let from: f64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(96.0);
    let secs: f64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(4.0);
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).unwrap();
    let rack = save.rack.clone().unwrap_or_else(studio_live::Rack::trap_kit);
    let mut project = save.to_project();
    for b in project.busses.values_mut().filter(|b| b.name == "Master") { b.chain.clear(); }
    let base = path.parent().unwrap();
    let mut audio = BTreeMap::new();
    for s in project.sources.values().filter(|s| !s.is_generator()) {
        if let Ok(x) = studio_media::decode_file(&base.join(&s.path)) { audio.insert(s.id, x); }
    }
    let mut st = BTreeMap::new();
    studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut st);
    let fades = BTreeMap::new();
    let (f, t) = (studio_core::Ticks::from_seconds(from), studio_core::Ticks::from_seconds(from + secs));
    let db = |v: &[f32]| 10.0 * (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).max(1e-12).log10();
    let all = studio_audio::mixdown(&project, &audio, &fades, f, t, 48_000, &[], &[]);
    println!("whole mix (master chain bypassed): {:.1} dB", db(&all));
    let master = project.busses.values().find(|b| b.name == "Master").map(|b| b.id);
    let mut rows = Vec::new();
    for b in project.busses.values().filter(|b| b.parent == master && Some(b.id) != master && !b.silent) {
        let out = studio_audio::mixdown(&project, &audio, &fades, f, t, 48_000, &[], &[b.id]);
        // the tracks feeding it, for a name
        let feeds: Vec<&str> = project.tracks.iter().filter(|x| x.route == Some(b.id)).map(|x| x.name.as_str()).take(2).collect();
        rows.push((db(&out), b.name.clone(), b.gain, feeds.join(", ")));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (l, n, g, f) in rows.iter().take(14) { println!("  {l:6.1} dB  {n:<18} fader {g:.2}  <- {f}"); }
}
