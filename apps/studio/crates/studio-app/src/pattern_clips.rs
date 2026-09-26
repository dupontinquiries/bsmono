//! pattern_clips — a rack pattern on the timeline, linked, not rendered.
//!
//! A pattern clip is an ordinary clip whose source is a PATTERN source
//! (`SourceKind::Pattern`, already in the core model): it names a rack pattern
//! instead of a file. The rack stays the only truth. What the timeline, the
//! mixer, playback and export hear is a CACHE the app keeps current:
//!
//! * rendered from the rack (`studio_live::render_pattern`), one pass per loop
//!   so anything that varies pass to pass still does;
//! * long enough for every clip that uses the pattern, so stretching a clip
//!   renders more;
//! * rendered again, automatically, whenever the rack changes -- a step
//!   toggled, the tempo, a channel's gain -- so an edit to the pattern is heard
//!   on the timeline with no render step. That is the difference from a
//!   bounced clip, which would be a file that never changes again.
//!
//! The cache is local and never saved: a collaborator renders from their own
//! (synced) rack. A pattern source whose pattern is gone renders nothing and
//! still labels itself with the pattern's name (the core model's rule).

use std::collections::BTreeMap;
use studio_audio::SourceAudio;
use studio_core::{ClipId, Op, Project, Source, SourceId, SourceKind, Ticks, TrackKind};

/// The rate pattern caches render at.
pub const RATE: u32 = 48_000;
/// How long a pattern source is, for trimming: generous, because only what
/// clips use is ever rendered.
pub const SOURCE_LEN_SECS: f64 = 600.0;

/// How many frames of pattern audio the clips using `source` need.
pub fn needed_frames(project: &Project, source: SourceId, rate: u32) -> usize {
    project.clips.values().filter(|c| c.source == source)
        .map(|c| c.src_in.saturating_add(c.len).to_samples(rate) as usize).max().unwrap_or(0)
}

/// `frames` of the pattern at `ix`, stereo, rendered pass after pass. `None`
/// when the pattern is gone.
pub fn render(rack: &studio_live::Rack, ix: usize, audio: &BTreeMap<SourceId, SourceAudio>, frames: usize, rate: u32) -> Option<SourceAudio> {
    let pat = rack.patterns.get(ix)?;
    let tempo = rack.tempo();
    let mut synth = studio_live::BuiltinSynth { sources: audio };
    let (mut l, mut r): (Vec<f32>, Vec<f32>) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut pass = 0u32;
    while l.len() < frames.max(1) {
        // STEREO (c20: the rack rendered mono, duplicated into both channels --
        // every pan, width and pan-by-key was centred). Scaled by sqrt2 so a
        // centred voice is exactly the level it was (L = R = v): nothing
        // calibrated on the mono render moves.
        let (pl, pr) = studio_live::render_pattern_stereo(pat, rack, &mut synth, &tempo, rate, pass);
        if pl.is_empty() { break; }
        l.extend(pl.iter().map(|v| v * std::f32::consts::SQRT_2));
        r.extend(pr.iter().map(|v| v * std::f32::consts::SQRT_2));
        pass += 1;
    }
    l.truncate(frames.max(1));
    r.truncate(frames.max(1));
    Some(SourceAudio { sample_rate: rate, channels: vec![l, r] })
}

/// What a cache was rendered from; a different stamp means render again.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp { pub rack: studio_live::Rack, pub frames: usize }

/// The pattern sources whose cache is stale: their rack changed, a clip now
/// needs more than was rendered, or they were never rendered.
pub fn stale(project: &Project, rack: &studio_live::Rack, stamps: &BTreeMap<SourceId, Stamp>, rate: u32) -> Vec<(SourceId, usize, usize)> {
    project.sources.values().filter_map(|s| match &s.kind {
        SourceKind::Pattern { pattern, .. } => {
            let need = needed_frames(project, s.id, rate);
            let fresh = stamps.get(&s.id).map(|st| st.rack == *rack && st.frames >= need).unwrap_or(false);
            (!fresh).then_some((s.id, *pattern, need))
        }
        _ => None,
    }).collect()
}

/// Bring every pattern clip's cache up to date with the rack: render the
/// stale ones into `audio`, drop the ones whose pattern is gone, and stamp
/// what each was rendered from. Returns the sources re-rendered, so the
/// caller can rebuild their waveforms. The app calls this every frame; it
/// does nothing unless the rack or the clips changed.
pub fn refresh(project: &Project, rack: &studio_live::Rack, audio: &mut BTreeMap<SourceId, SourceAudio>,
               stamps: &mut BTreeMap<SourceId, Stamp>) -> Vec<SourceId> {
    let mut done = Vec::new();
    for (sid, ix, need) in stale(project, rack, stamps, RATE) {
        match render(rack, ix, audio, need, RATE) {
            Some(a) => { audio.insert(sid, a); }
            None => { audio.remove(&sid); }
        }
        stamps.insert(sid, Stamp { rack: rack.clone(), frames: need });
        done.push(sid);
    }
    done
}

/// The edits that put pattern `ix` on the timeline at `at`: a pattern source
/// (reused if one for this pattern exists), and a one-loop clip on `on` (a
/// track it was dropped on) or else the selected clip's track -- whichever is
/// an audio track with room -- or else a new audio track below. One group,
/// one undo.
pub fn place_ops(project: &Project, rack: &studio_live::Rack, ix: usize, at: Ticks, selected: Option<ClipId>,
                 on: Option<studio_core::TrackId>) -> Result<Vec<Op>, String> {
    let pat = rack.patterns.get(ix).ok_or("that pattern is gone")?;
    let name = if pat.name.trim().is_empty() { format!("pattern {}", ix + 1) } else { pat.name.clone() };
    let len = pat.len(&rack.tempo());
    let mut ops = Vec::new();
    let source = match project.sources.values().find(|s| matches!(&s.kind, SourceKind::Pattern { pattern, .. } if *pattern == ix)) {
        Some(s) => s.id,
        None => {
            let id = SourceId(project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
            ops.push(Op::AddSource { source: Source::pattern(id, ix, name.clone(), Ticks::from_seconds(SOURCE_LEN_SECS), RATE, 2) });
            id
        }
    };
    let clip = studio_core::Clip { id: ClipId(project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1), source, track: studio_core::TrackId(0),
        pos: at, src_in: Ticks::ZERO, len, gain: 1.0, effects: vec![] };
    // the selected clip's track, if it is audio and the loop fits there
    let fits = |t: &studio_core::TrackId| project.tracks.iter().any(|x| x.id == *t && x.kind == TrackKind::Audio && !x.locked)
        && !project.clips.values().any(|o| o.track == *t && o.overlaps(&studio_core::Clip { track: *t, ..clip.clone() }));
    let here = on.filter(|t| fits(t))
        .or_else(|| selected.and_then(|c| project.clips.get(&c)).map(|c| c.track).filter(|t| fits(t)));
    let track = match here {
        Some(t) => t,
        None => {
            let op = crate::tracks::add_below(project, selected, TrackKind::Audio);
            let id = if let Op::AddTrack { track, .. } = &op { track.id } else { unreachable!() };
            ops.push(op);
            id
        }
    };
    ops.push(Op::AddClip { clip: studio_core::Clip { track, ..clip } });
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn rack() -> studio_live::Rack {
        // sine channels, so the pattern sounds with no sample files
        let mut r = studio_live::Rack::trap_kit();
        for ch in r.channels.iter_mut() { ch.kind = studio_live::SynthKind::Sine { hz: 220.0 }; }
        r
    }

    fn project_with_track() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "drums".into(), muted: false,
            locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p
    }

    #[test]
    fn sending_a_pattern_places_a_linked_one_loop_clip() {
        let (mut p, r) = (project_with_track(), rack());
        let inv = p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::from_seconds(2.0), None, None).unwrap() }).unwrap();
        let src = p.sources.values().next().unwrap();
        assert!(matches!(&src.kind, SourceKind::Pattern { pattern: 0, .. }), "a pattern source, not a file");
        assert_eq!(src.display_name(), r.patterns[0].name);
        let c = p.clips.values().next().unwrap();
        assert_eq!((c.pos, c.len), (Ticks::from_seconds(2.0), r.patterns[0].len(&r.tempo())), "one loop at the playhead");
        assert_eq!(p.tracks.len(), 2, "nothing selected: a new audio track");
        // sending it again reuses the source: one pattern, one source
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::from_seconds(10.0), None, None).unwrap() }).unwrap();
        assert_eq!(p.sources.len(), 1);
        let _ = inv;
    }

    #[test]
    fn with_an_audio_clip_selected_it_goes_on_that_track_if_it_fits() {
        let (mut p, r) = (project_with_track(), rack());
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::ZERO, None, None).unwrap() }).unwrap();
        let first = *p.clips.keys().next().unwrap();
        let track_of_first = p.clips[&first].track;
        let loop_s = r.patterns[0].len(&r.tempo()).to_seconds();
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::from_seconds(loop_s + 1.0), Some(first), None).unwrap() }).unwrap();
        assert_eq!(p.clips.values().filter(|c| c.track == track_of_first).count(), 2, "same track, after the first");
        // overlapping the first: it cannot share the track, so a new one below
        let before = p.tracks.len();
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::from_seconds(0.1), Some(first), None).unwrap() }).unwrap();
        assert_eq!(p.tracks.len(), before + 1);
    }

    #[test]
    fn a_drop_lands_on_the_track_it_was_dropped_on() {
        let (mut p, r) = (project_with_track(), rack());
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio, name: "b".into(), muted: false,
            locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::ZERO, None, Some(TrackId(2))).unwrap() }).unwrap();
        assert_eq!(p.clips.values().next().unwrap().track, TrackId(2));
        assert_eq!(p.tracks.len(), 2, "no new track needed");
    }

    #[test]
    fn the_cache_follows_the_rack_and_the_clips() {
        let (mut p, mut r) = (project_with_track(), rack());
        p.apply(&Op::Group { ops: place_ops(&p, &r, 0, Ticks::ZERO, None, None).unwrap() }).unwrap();
        let sid = *p.sources.keys().next().unwrap();
        let mut stamps = BTreeMap::new();
        let st = stale(&p, &r, &stamps, RATE);
        assert_eq!(st.len(), 1, "never rendered: stale");
        let need = st[0].2;
        let a = render(&r, 0, &BTreeMap::new(), need, RATE).unwrap();
        assert_eq!(a.channels[0].len(), need);
        assert!(a.channels[0].iter().any(|v| v.abs() > 0.01), "the pattern sounds");
        stamps.insert(sid, Stamp { rack: r.clone(), frames: need });
        assert!(stale(&p, &r, &stamps, RATE).is_empty(), "rendered and unchanged: fresh");
        // toggle a step: the link means the cache is stale and renders differently
        r.patterns[0].rows[0] ^= 1 << 2;
        assert_eq!(stale(&p, &r, &stamps, RATE).len(), 1, "an edited pattern re-renders");
        let b = render(&r, 0, &BTreeMap::new(), need, RATE).unwrap();
        assert_ne!(a.channels[0], b.channels[0], "and sounds different");
        stamps.insert(sid, Stamp { rack: r.clone(), frames: need });
        // stretch the clip to two loops: more is needed, so stale again, and it loops
        let cid = *p.clips.keys().next().unwrap();
        let two = Ticks(p.clips[&cid].len.0 * 2);
        p.apply(&Op::TrimClip { id: cid, src_in: Ticks::ZERO, len: two }).unwrap();
        let st = stale(&p, &r, &stamps, RATE);
        assert_eq!(st.len(), 1, "a longer clip needs more");
        let c = render(&r, 0, &BTreeMap::new(), st[0].2, RATE).unwrap();
        assert_eq!(c.channels[0].len(), st[0].2);
        // a pattern that is gone renders nothing
        assert!(render(&r, 9, &BTreeMap::new(), 100, RATE).is_none());
    }

    /// The promise of a LINKED clip, end to end: edit the pattern in the rack
    /// and what the TIMELINE plays changes, with no render step in between.
    /// Through the real arrangement render the transport and export use.
    #[test]
    fn editing_the_pattern_changes_what_the_timeline_plays() {
        let mut s = crate::Studio::new(project_with_track(), studio_collab::SiteId(1),
                                       studio_collab::session::Team::from_contacts("solo", &["sc-local".into()]));
        s.rack = rack();
        let at = Ticks::from_seconds(1.0);
        let ops = place_ops(&s.session.doc.project, &s.rack, 0, at, None, None).unwrap();
        s.edit_group(ops, "pattern");
        let (mut audio, mut stamps) = (BTreeMap::new(), BTreeMap::new());
        assert_eq!(refresh(&s.session.doc.project, &s.rack, &mut audio, &mut stamps).len(), 1, "rendered once");
        assert!(refresh(&s.session.doc.project, &s.rack, &mut audio, &mut stamps).is_empty(), "and not again while nothing changes");
        let before = s.render_arrangement(RATE, &audio);
        let first = (at.to_seconds() * RATE as f64) as usize * 2;
        assert!(before[..first].iter().all(|v| v.abs() < 1e-6), "silent before the clip");
        assert!(before[first..].iter().any(|v| v.abs() > 0.01), "the pattern plays from the clip");
        // edit the pattern in the rack -- a step toggled, nothing done to the timeline
        s.rack.patterns[0].rows[0] ^= 1 << 2;
        assert_eq!(refresh(&s.session.doc.project, &s.rack, &mut audio, &mut stamps).len(), 1, "the edit re-renders it");
        let after = s.render_arrangement(RATE, &audio);
        assert_ne!(before, after, "and the timeline plays the edited pattern");
        assert!(after[..first].iter().all(|v| v.abs() < 1e-6), "still only where the clip is");
        // the mixer side of the rack counts too: a channel's gain
        s.rack.channels[0].gain *= 0.25;
        assert_eq!(refresh(&s.session.doc.project, &s.rack, &mut audio, &mut stamps).len(), 1);
        assert_ne!(s.render_arrangement(RATE, &audio), after, "a quieter channel is heard on the timeline");
    }
}
