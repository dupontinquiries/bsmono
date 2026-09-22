//! tracks — adding a track where you want it.
//!
//! The palette had no way to add a track at all (found testing the cartoon
//! fixture: "how do I insert a track below?"). A new track goes directly BELOW
//! the track of the selected clip -- where the thing you are working on is --
//! or at the bottom with nothing selected, routed to the same bus as the track
//! it sits under so it is heard where its neighbours are.

use studio_core::{ClipId, Op, Project, Track, TrackId, TrackKind};

/// The op that adds a `kind` track below the track holding `below` (a clip),
/// or at the end. Named "audio 3" / "video 2" by how many of that kind exist.
pub fn add_below(project: &Project, below: Option<ClipId>, kind: TrackKind) -> Op {
    let id = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
    let anchor = below.and_then(|c| project.clips.get(&c)).map(|c| c.track)
        .and_then(|t| project.tracks.iter().position(|x| x.id == t));
    let at = anchor.map(|i| i + 1).unwrap_or(project.tracks.len());
    let route = anchor.and_then(|i| project.tracks[i].route)
        .or_else(|| project.tracks.last().and_then(|t| t.route));
    let n = project.tracks.iter().filter(|t| t.kind == kind).count() + 1;
    let name = format!("{} {n}", if kind == TrackKind::Video { "video" } else { "audio" });
    Op::AddTrack { track: Track { id, kind, name, muted: false, locked: false, gain: 1.0, route }, at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Mix".into(), parent: None, gain: 1.0, chain: vec![],
            sends: vec![], muted: false, silent: false } }).unwrap();
        for (i, k) in [TrackKind::Video, TrackKind::Video, TrackKind::Audio].iter().enumerate() {
            let id = i as u64 + 1;
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: *k, name: format!("t{id}"), muted: false,
                locked: false, gain: 1.0, route: Some(BusId(1)) }, at: i }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "a.mp4", Ticks::from_seconds(9.0), 48_000, 1, true) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::ZERO,
            src_in: Ticks::ZERO, len: Ticks::from_seconds(9.0), gain: 1.0, effects: vec![] } }).unwrap();
        p
    }

    #[test]
    fn a_new_track_goes_right_below_the_selected_clips_track() {
        let mut p = project();
        let inv = p.apply(&add_below(&p, Some(ClipId(1)), TrackKind::Audio)).unwrap();
        let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["t1", "audio 2", "t2", "t3"], "directly under t1, named by count");
        assert_eq!(p.tracks[1].route, Some(BusId(1)), "heard where its neighbour is");
        p.apply(&inv).unwrap();
        assert_eq!(p.tracks.len(), 3, "one undo takes it away");
    }

    #[test]
    fn with_nothing_selected_it_goes_at_the_bottom() {
        let mut p = project();
        p.apply(&add_below(&p, None, TrackKind::Video)).unwrap();
        assert_eq!(p.tracks.last().unwrap().name, "video 3");
        assert_eq!(p.tracks.last().unwrap().id, TrackId(4), "a fresh id");
        // a clip that no longer exists is the same as nothing selected
        let op = add_below(&p, Some(ClipId(99)), TrackKind::Audio);
        assert!(matches!(op, Op::AddTrack { at, .. } if at == p.tracks.len()));
    }
}
