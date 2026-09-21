//! A small project with a camera group, for trying the signals window.
//!     cargo run -p studio-app --example signals_project -- ANGLE1.mkv ANGLE2.mkv OUT.studio
use studio_core::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut p = Project { name: "signals trial".into(), ..Default::default() };
    p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Mix".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }], muted: false, silent: false } }).unwrap();
    p.apply(&Op::AddBus { bus: Bus { id: BusId(2), name: "Control".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![], muted: false, silent: true } }).unwrap();
    for (i, path) in a[..2].iter().enumerate() {
        let id = i as u64 + 1;
        p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video, name: format!("angle {id}"),
            muted: false, locked: false, gain: 1.0, route: Some(BusId(1)) }, at: i }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(id), path.clone(), Ticks::from_seconds(if i == 0 { 150.0 } else { 19.9 }), 48_000, 2, true) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id), pos: Ticks::ZERO,
            // each angle 18 s from inside its own file (the second proxy is only 20 s long)
            src_in: Ticks::from_seconds(if i == 0 { 40.0 } else { 1.0 }), len: Ticks::from_seconds(18.0), gain: 1.0, effects: vec![] } }).unwrap();
    }
    p.apply(&Op::SetGroup { id: GroupId(1), name: "rooftop angles".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
    p.apply(&Op::SetMarker { at: Ticks::from_seconds(8.0), name: "no cut — hold the look".into() }).unwrap();
    let f = studio_collab::file::SaveFile::from_project(&p, Vec::new(), None);
    std::fs::write(&a[2], f.encode()).unwrap();
    println!("wrote {}", a[2]);
}
