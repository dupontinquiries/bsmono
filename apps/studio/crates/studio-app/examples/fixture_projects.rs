//! Write the openable projects for the fixtures that are only media.
//!
//!     cargo run -p studio-app --example fixture_projects -- FIXTURES_DIR
//!
//! The Fragile Thing cartoon writes its own project (examples/cartoon_fragile).
//! The others were just video files, so neither the file picker nor the command
//! line had anything to open. This writes, beside the media and with RELATIVE
//! paths (so they open wherever the repository is):
//!
//! * `cartoon-allmylife/allmylife-cartoon.studio` -- the cartoon on a track, a
//!   marker at every cut from truth.json named for the shot that starts there;
//! * `real-proxies/allmylife.studio` -- the real proxy, a marker at every cut
//!   the analysis ESTIMATED (named "est. cut", not presented as truth);
//! * `real-proxies/fragile.studio` -- the rooftop wide and the slate take.
use std::path::Path;
use studio_core::*;

fn probe(p: &Path) -> Ticks {
    studio_video::probe(p).unwrap_or_else(|e| panic!("{}: {e:?}", p.display())).duration
}

/// The sample rate of a file's first audio stream, as ffprobe reports it. A
/// project must record the rate the audio really has: the first version of
/// these projects wrote 48 kHz for everything, and waveforms read wrong spans.
fn audio_rate(p: &Path) -> u32 {
    let out = std::process::Command::new("ffprobe").args(["-v", "error", "-select_streams", "a:0",
        "-show_entries", "stream=sample_rate", "-of", "csv=p=0"]).arg(p).output().expect("ffprobe");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or_else(|_| panic!("no audio rate for {}", p.display()))
}

/// A project with one video track per file (each clip at 0 s, routed to a Mix
/// bus) and the given markers.
fn project(name: &str, dir: &Path, files: &[&str], markers: &[(f64, String)]) -> Project {
    let mut p = Project { name: name.into(), ..Default::default() };
    p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Mix".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![], muted: false, silent: false } }).unwrap();
    for (i, f) in files.iter().enumerate() {
        let id = i as u64 + 1;
        let len = probe(&dir.join(f));
        p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video,
            name: f.trim_end_matches(".mp4").into(), muted: false, locked: false, gain: 1.0, route: Some(BusId(1)) }, at: i }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(id), f.to_string(), len, audio_rate(&dir.join(f)), 1, true) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id), pos: Ticks::ZERO,
            src_in: Ticks::ZERO, len, gain: 1.0, effects: vec![] } }).unwrap();
    }
    for (at, n) in markers { p.apply(&Op::SetMarker { at: Ticks::from_seconds(*at), name: n.clone() }).unwrap(); }
    p
}

fn write(p: &Project, path: &Path) {
    std::fs::write(path, studio_collab::file::SaveFile::from_project(p, Vec::new(), None).encode()).unwrap();
    println!("wrote {} ({} clips, {} markers)", path.display(), p.clips.len(), p.markers.len());
}

/// The values after `"key": "` in order (our own generator's JSON; no parser needed).
fn strings(text: &str, key: &str) -> Vec<String> {
    text.split(&format!("\"{key}\": \"")).skip(1).map(|s| s.split('"').next().unwrap().to_string()).collect()
}
fn numbers(text: &str, key: &str) -> Vec<f64> {
    text.split(&format!("\"{key}\": ")).skip(1)
        .filter_map(|s| s.split(|c: char| c == ',' || c == '}').next()?.trim().parse().ok()).collect()
}

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures".into()));

    // the Fragile Thing cartoon writes its own project; correct the rates it
    // recorded (without regenerating its media, which would churn the bytes)
    let fp = root.join("cartoon-fragile/fragile-cartoon.studio");
    let mut proj = studio_collab::file::SaveFile::decode(&std::fs::read(&fp).unwrap()).unwrap().to_project();
    for s in proj.sources.values_mut() {
        let r = audio_rate(&fp.parent().unwrap().join(&s.path));
        if s.sample_rate != r { println!("  {}: {} Hz -> {r} Hz", s.path, s.sample_rate); s.sample_rate = r; }
    }
    write(&proj, &fp);

    // the All My Life cartoon: a marker per shot, from the truth
    let d = root.join("cartoon-allmylife");
    let truth = std::fs::read_to_string(d.join("truth.json")).unwrap();
    let shots = truth.split("\"shots\": [").nth(1).unwrap().split(']').next().unwrap();
    let (starts, places, looks, fxs) = (numbers(shots, "start"), strings(shots, "place"), strings(shots, "look"), strings(shots, "fx"));
    let cut_kind: Vec<String> = strings(&truth, "kind");
    let markers: Vec<(f64, String)> = starts.iter().enumerate().map(|(i, t)| {
        let fx = if fxs[i] == "None" { String::new() } else { format!(", {}", fxs[i].to_lowercase()) };
        let how = if i == 0 { String::new() } else { format!(" ({})", cut_kind.get(i - 1).cloned().unwrap_or_default()) };
        (*t, format!("shot {}{how}: {}, {}{fx}", i + 1, places[i].to_lowercase(), looks[i].to_lowercase()))
    }).collect();
    write(&project("All My Life (cartoon)", &d, &["allmylife-cartoon.mp4"], &markers), &d.join("allmylife-cartoon.studio"));

    // the real proxies
    let d = root.join("real-proxies");
    let a = std::fs::read_to_string(d.join("allmylife-analysis.json")).unwrap();
    let est: Vec<f64> = a.split("\"cuts_estimated\": [").nth(1).unwrap().split(']').next().unwrap()
        .split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let markers: Vec<(f64, String)> = est.iter().enumerate()
        .map(|(i, t)| (*t, format!("est. cut {} @{t:.2}s", i + 1))).collect();
    write(&project("All My Life (256 proxy)", &d, &["allmylife-256.mp4"], &markers), &d.join("allmylife.studio"));
    write(&project("A Fragile Thing (256 proxies)", &d, &["fragile-C0032-rooftop-wide-256.mp4", "fragile-C0018-slate-256.mp4"],
                   &[(0.0, "N7: rooftop wide (C0032, from 40 s)".into())]), &d.join("fragile.studio"));
}
