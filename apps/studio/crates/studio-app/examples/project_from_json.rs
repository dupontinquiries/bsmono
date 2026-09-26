//! A Studio project from a JSON description -- anything assembled from audio
//! files (a remix, a medley, a stem re-arrangement) without clicking it in.
//!     cargo run -p studio-app --example project_from_json -- spec.json out.studio
//!
//! Paths in the spec are relative to the OUTPUT's folder (where the render
//! reads them). Each source's length, rate and channel count are read from
//! the file. Times in seconds.
//! ```json
//! { "busses":  [{"id": 2, "name": "Vocals", "gain": 1.0, "parent": 1,
//!                "chain": [{"kind": "compressor", "params": {"threshold": -20}}]}],
//!   "tracks":  [{"id": 1, "name": "Drums", "gain": 1.0, "bus": 1}],
//!   "sources": [{"id": 1, "path": "drums.wav"}],
//!   "clips":   [{"source": 1, "track": 1, "pos": 0.0, "src_in": 0.0, "len": 30.0,
//!                "gain": 1.0, "fade_in": 0.01, "fade_out": 0.05}] }
//! ```
use serde_json::Value;
use studio_core::{Bus, BusId, Clip, ClipId, Effect, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(&a[1]).expect("spec")).expect("json");
    let out = std::path::PathBuf::from(&a[2]);
    let base = out.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let s = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let mut p = Project::default();
    let mut refused = 0;
    let mut apply = |p: &mut Project, op: Op| if let Err(e) = p.apply(&op) { refused += 1; eprintln!("refused: {e:?}") };
    for b in spec["busses"].as_array().into_iter().flatten() {
        let chain = b["chain"].as_array().into_iter().flatten().map(|e| Effect {
            kind: e["kind"].as_str().unwrap_or("gain").into(), enabled: true,
            params: e["params"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32))).collect()).unwrap_or_default() }).collect();
        apply(&mut p, Op::AddBus { bus: Bus { id: BusId(b["id"].as_u64().unwrap()), name: b["name"].as_str().unwrap_or("Bus").into(),
            parent: b["parent"].as_u64().map(BusId), gain: b["gain"].as_f64().unwrap_or(1.0) as f32, chain, sends: vec![], muted: false, silent: false } });
    }
    for (k, t) in spec["tracks"].as_array().into_iter().flatten().enumerate() {
        apply(&mut p, Op::AddTrack { track: Track { id: TrackId(t["id"].as_u64().unwrap()), kind: TrackKind::Audio,
            name: t["name"].as_str().unwrap_or("Track").into(), muted: t["muted"].as_bool().unwrap_or(false), locked: false,
            gain: t["gain"].as_f64().unwrap_or(1.0) as f32, route: t["bus"].as_u64().map(BusId) }, at: k });
    }
    for src in spec["sources"].as_array().into_iter().flatten() {
        let path = src["path"].as_str().expect("a path");
        let audio = studio_media::decode_file(&base.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let len = Ticks::from_samples(audio.frames() as i64, audio.sample_rate);
        apply(&mut p, Op::AddSource { source: Source::file(SourceId(src["id"].as_u64().unwrap()), path, len, audio.sample_rate, audio.channels.len() as u16, false) });
    }
    for (k, c) in spec["clips"].as_array().into_iter().flatten().enumerate() {
        let id = ClipId(k as u64 + 1);
        apply(&mut p, Op::AddClip { clip: Clip { id, source: SourceId(c["source"].as_u64().unwrap()), track: TrackId(c["track"].as_u64().unwrap()),
            pos: Ticks::from_seconds(s(c, "pos")), src_in: Ticks::from_seconds(s(c, "src_in")), len: Ticks::from_seconds(s(c, "len")),
            gain: c["gain"].as_f64().unwrap_or(1.0) as f32, effects: vec![] } });
        let (fi, fo) = (s(c, "fade_in"), s(c, "fade_out"));
        if fi > 0.0 || fo > 0.0 { apply(&mut p, Op::SetClipFade { id, fade_in: Ticks::from_seconds(fi), fade_out: Ticks::from_seconds(fo) }); }
    }
    let f = studio_collab::file::SaveFile::from_project(&p, Vec::new(), None);
    std::fs::write(&out, f.encode()).expect("write");
    println!("{}: {} tracks, {} clips, {} busses, {} sources; {} refused; {:.1} s long",
             out.display(), p.tracks.len(), p.clips.len(), p.busses.len(), p.sources.len(), refused, p.duration().to_seconds());
}
