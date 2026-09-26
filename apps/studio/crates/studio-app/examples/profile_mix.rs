//! Where a project's render time goes: load once, then time the SAME window's
//! mixdown with everything, without each effect kind in turn, and with no
//! effects at all. Startup phases (decoding media, rendering pattern clips)
//! are timed separately.
//!     cargo run -p studio-app --example profile_mix -- song.studio [from_s] [secs]
use std::collections::BTreeMap;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let from: f64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(60.0);
    let secs: f64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(5.0);
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).unwrap();
    let rack = save.rack.clone().unwrap_or_else(studio_live::Rack::trap_kit);
    let project = save.to_project();
    let base = path.parent().unwrap();
    let t = Instant::now();
    let (mut audio, _) = studio_app::load::load_sources(&project, base, Some(48_000), 0);
    println!("decode {} media files: {:.1} s", audio.len(), t.elapsed().as_secs_f64());
    let t = Instant::now();
    let mut st = BTreeMap::new();
    let n = studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut st).len();
    println!("render {n} pattern clips: {:.1} s", t.elapsed().as_secs_f64());
    let fades: BTreeMap<studio_core::ClipId, studio_audio::Fades> = project.fades.iter()
        .map(|(id, (i, o))| (*id, studio_audio::Fades { fade_in: *i, fade_out: *o })).collect();
    let (f, to) = (studio_core::Ticks::from_seconds(from), studio_core::Ticks::from_seconds(from + secs));
    let time = |p: &studio_core::Project| {
        let mut cache = studio_audio::EffectCache::new();
        let t = Instant::now();
        let _ = studio_audio::mixdown_with_cache(p, &audio, &fades, f, to, 48_000, &[], &[], &mut cache);
        t.elapsed().as_secs_f64()
    };
    let all = time(&project);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for b in project.busses.values() { for e in &b.chain { *kinds.entry(e.kind.clone()).or_default() += 1; } }
    for c in project.clips.values() { for e in &c.effects { *kinds.entry(e.kind.clone()).or_default() += 1; } }
    let mut none = project.clone();
    for b in none.busses.values_mut() { b.chain.clear(); }
    for c in none.clips.values_mut() { c.effects.clear(); }
    let bare = time(&none);
    println!("\nmixdown of {secs:.0} s ({:.0}-{:.0} s): {all:.2} s with everything = {:.2}x real time", from, from + secs, secs / all);
    println!("  with NO effects: {bare:.2} s -- effects cost {:.2} s ({:.0}%)\n", all - bare, 100.0 * (all - bare) / all);
    let mut rows = Vec::new();
    for (k, n) in &kinds {
        let mut p = project.clone();
        for b in p.busses.values_mut() { b.chain.retain(|e| &e.kind != k); }
        for c in p.clips.values_mut() { c.effects.retain(|e| &e.kind != k); }
        rows.push((all - time(&p), k.clone(), *n));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("  {:<12} {:>4}  {:>8}  {:>6}", "kind", "n", "saves", "share");
    for (saved, k, n) in rows { println!("  {k:<12} {n:>4}  {saved:>7.2}s  {:>5.0}%", 100.0 * saved / all); }
}
