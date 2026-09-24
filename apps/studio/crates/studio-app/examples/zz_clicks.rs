use std::collections::BTreeMap;
/// Clicks: frames whose second difference is > `k` x the local (50 ms) median
/// of |second difference| AND above an absolute floor. Returns their times.
fn clicks(x: &[f32], rate: usize, k: f32) -> Vec<f64> {
    let d2: Vec<f32> = (2..x.len()).map(|i| (x[i] - 2.0 * x[i - 1] + x[i - 2]).abs()).collect();
    let w = rate / 20;
    let mut out = Vec::new();
    let mut i = 0;
    while i < d2.len() {
        let (a, b) = (i.saturating_sub(w / 2), (i + w / 2).min(d2.len()));
        let mut win: Vec<f32> = d2[a..b].iter().step_by(8).copied().collect();
        win.sort_by(f32::total_cmp);
        let med = win[win.len() / 2].max(1e-5);
        if d2[i] > k * med && d2[i] > 0.02 {
            out.push((i + 2) as f64 / rate as f64);
            i += rate / 100; // one per 10 ms
        } else { i += 1; }
    }
    out
}
fn left(stereo: &[f32]) -> Vec<f32> { stereo.chunks(2).map(|c| c[0]).collect() }
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let (f0, f1): (f64, f64) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let (from, to) = (studio_core::Ticks::from_seconds(f0), studio_core::Ticks::from_seconds(f1));
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).unwrap();
    let rack = save.rack.clone().unwrap();
    let project = save.to_project();
    let base = path.parent().unwrap();
    let mut audio = BTreeMap::new();
    for s in project.sources.values().filter(|s| !s.is_generator()) {
        if let Ok(x) = studio_media::decode_file(&base.join(&s.path)) { audio.insert(s.id, x); }
    }
    let mut st = BTreeMap::new();
    studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut st);
    let mut dry = project.clone();
    for b in dry.busses.values_mut() { b.chain.clear(); }
    let fades = BTreeMap::new();
    let rate = 48_000;
    let full = left(&studio_audio::mixdown(&project, &audio, &fades, from, to, rate, &[], &[]));
    let peak = full.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let c = clicks(&full, rate as usize, 40.0);
    println!("FULL MIX {f0}-{f1}s: peak {:.2}, {} clicks {:?}", peak, c.len(), c.iter().take(8).map(|t| format!("{:.3}", t + f0)).collect::<Vec<_>>());
    let fulld = left(&studio_audio::mixdown(&dry, &audio, &fades, from, to, rate, &[], &[]));
    println!("FULL MIX, bus effects bypassed: peak {:.2}, {} clicks", fulld.iter().fold(0.0f32, |m, v| m.max(v.abs())), clicks(&fulld, rate as usize, 40.0).len());
    for t in &project.tracks {
        if project.track_clips(t.id).iter().all(|k| k.end() <= from || k.pos >= to) { continue; }
        let wet = left(&studio_audio::mixdown(&project, &audio, &fades, from, to, rate, &[t.id], &[]));
        let dryv = left(&studio_audio::mixdown(&dry, &audio, &fades, from, to, rate, &[t.id], &[]));
        let (cw, cd) = (clicks(&wet, rate as usize, 40.0), clicks(&dryv, rate as usize, 40.0));
        let pk = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if !cw.is_empty() || !cd.is_empty() || pk(&wet) > 1.0 {
            let bus = t.route.and_then(|r| project.busses.get(&r)).map(|b| b.name.clone()).unwrap_or("master".into());
            println!("  {:<32} -> {:<12} wet: {:3} clicks peak {:5.2} | dry: {:3} clicks peak {:5.2}", t.name, bus, cw.len(), pk(&wet), cd.len(), pk(&dryv));
        }
    }
}
