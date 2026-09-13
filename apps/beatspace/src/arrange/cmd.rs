//! Commands: every change to the arrangement, as data, with undo.
//!
//! A tool's gesture, a keyboard shortcut and a toolbar button all end in
//! the same `Command`, applied by the same `Editor::apply`. That is what
//! makes undo uniform, and what lets a test drive the editor without a
//! pointer.
//!
//! Undo is by SNAPSHOT: the timeline is clip metadata (audio lives in the
//! library by id), so a copy is a few hundred bytes, and a snapshot can
//! never disagree with the command that made it. Commands that only
//! change a preview (a drag in flight) do not go through `apply`; the tool
//! applies one command when the gesture ends.

use std::collections::BTreeSet;

use super::model::{ClipId, Grid, SourceRef, Timeline, TrackId};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Place `source` at `start` on `track`; `source_beats` is the
    /// material's length at the current tempo. `exact` skips the grid — a
    /// recorded take lands where recording started, not on the nearest
    /// beat.
    /// `lock` is a tempo the material is locked to (see `Clip::tempo_lock`);
    /// `source_beats` is then its length at that tempo.
    Place { track: TrackId, source: SourceRef, start: f64, source_beats: f64, exact: bool, lock: Option<f64> },
    /// Move clips by `delta_beats`, optionally to another track (all
    /// selected clips go to the same track — a cross-track move of a
    /// multi-selection keeps their relative tracks by index).
    Move { clips: Vec<ClipId>, delta_beats: f64, delta_tracks: i32 },
    /// Set a clip's start and length directly (a trim: the end that did
    /// not move is preserved by the caller's arithmetic).
    Trim { clip: ClipId, start: f64, len: f64, offset: f64 },
    /// Cut a clip in two at `beat`.
    Split { clip: ClipId, beat: f64 },
    Delete { clips: Vec<ClipId> },
    /// Copy clips, placed `delta_beats` later on the same tracks.
    Duplicate { clips: Vec<ClipId>, delta_beats: f64 },
    SetClipGain { clip: ClipId, gain: f32 },
    ToggleClipMute { clip: ClipId },
    /// Lock the clip to a tempo (its audio is time-stretched as the
    /// timeline tempo moves away from it), or unlock it (its extent in
    /// beats follows the tempo instead).
    SetTempoLock { clip: ClipId, bpm: Option<f64> },
    /// Fade lengths in beats, clamped to the clip.
    SetClipFades { clip: ClipId, fade_in: f64, fade_out: f64 },
    AddTrack { name: String },
    RemoveTrack { track: TrackId },
    RenameTrack { track: TrackId, name: String },
    ToggleMute { track: TrackId },
    ToggleSolo { track: TrackId },
    SetTrackGain { track: TrackId, gain: f32 },
    SetTempo { bpm: f64 },
    SetLoop { region: Option<(f64, f64)> },
    SetGrid { grid: Grid },
}

impl Command {
    /// One line for a status bar or a command palette.
    pub fn label(&self) -> String {
        match self {
            Command::Place { start, .. } => format!("place at {start:.2}"),
            Command::Move { clips, delta_beats, .. } => format!("move {} by {delta_beats:+.2}", clips.len()),
            Command::Trim { .. } => "trim".into(),
            Command::Split { beat, .. } => format!("split at {beat:.2}"),
            Command::Delete { clips } => format!("delete {}", clips.len()),
            Command::Duplicate { clips, .. } => format!("duplicate {}", clips.len()),
            Command::SetClipGain { gain, .. } => format!("clip gain {gain:.2}"),
            Command::ToggleClipMute { .. } => "mute clip".into(),
            Command::SetTempoLock { bpm: Some(b), .. } => format!("lock to {b:.0} bpm"),
            Command::SetTempoLock { bpm: None, .. } => "unlock tempo".into(),
            Command::SetClipFades { fade_in, fade_out, .. } => format!("fades {fade_in:.2}/{fade_out:.2}"),
            Command::AddTrack { name } => format!("add track {name}"),
            Command::RemoveTrack { .. } => "remove track".into(),
            Command::RenameTrack { name, .. } => format!("rename to {name}"),
            Command::ToggleMute { .. } => "mute".into(),
            Command::ToggleSolo { .. } => "solo".into(),
            Command::SetTrackGain { gain, .. } => format!("track gain {gain:.2}"),
            Command::SetTempo { bpm } => format!("tempo {bpm:.0}"),
            Command::SetLoop { region: Some((a, b)) } => format!("loop {a:.1}–{b:.1}"),
            Command::SetLoop { region: None } => "loop off".into(),
            Command::SetGrid { grid } => format!("grid {}", grid.label()),
        }
    }
}

/// Why a command did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NoSuchClip,
    NoSuchTrack,
    /// The move or trim would overlap another clip, or leave the grid.
    DoesNotFit,
    /// A split at a clip edge, or outside it.
    NothingToSplit,
    /// The last track cannot be removed.
    LastTrack,
    Invalid(String),
}

/// What `apply` did.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub label: String,
    /// Ids created by the command (a placement, a split's second half,
    /// duplicates), for the caller to select.
    pub created: Vec<ClipId>,
}

pub struct Editor {
    pub timeline: Timeline,
    pub selection: BTreeSet<ClipId>,
    undo: Vec<(Timeline, String)>,
    redo: Vec<(Timeline, String)>,
    /// How many snapshots to keep. A snapshot is small, but a user who
    /// nudges a clip four hundred times does not need four hundred of them.
    pub undo_limit: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Timeline::default())
    }
}

impl Editor {
    pub fn new(timeline: Timeline) -> Self {
        Self { timeline, selection: BTreeSet::new(), undo: Vec::new(), redo: Vec::new(), undo_limit: 200 }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// The label of what undo would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(_, l)| l.as_str())
    }

    pub fn undo(&mut self) -> Option<String> {
        let (t, label) = self.undo.pop()?;
        let cur = std::mem::replace(&mut self.timeline, t);
        self.redo.push((cur, label.clone()));
        self.prune_selection();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let (t, label) = self.redo.pop()?;
        let cur = std::mem::replace(&mut self.timeline, t);
        self.undo.push((cur, label.clone()));
        self.prune_selection();
        Some(label)
    }

    fn prune_selection(&mut self) {
        let t = &self.timeline;
        self.selection.retain(|id| t.find(*id).is_some());
    }

    /// Apply a command. On refusal the timeline is untouched and nothing
    /// is recorded.
    pub fn apply(&mut self, cmd: Command) -> Result<Applied, Refused> {
        let label = cmd.label();
        let mut next = self.timeline.clone();
        let created = Self::run(&mut next, &cmd)?;
        self.undo.push((std::mem::replace(&mut self.timeline, next), label.clone()));
        if self.undo.len() > self.undo_limit {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.prune_selection();
        Ok(Applied { label, created })
    }

    /// The command's effect on a timeline, with nothing recorded. Pure.
    fn run(t: &mut Timeline, cmd: &Command) -> Result<Vec<ClipId>, Refused> {
        match cmd {
            Command::Place { track, source, start, source_beats, exact, lock } => {
                if t.track(*track).is_none() {
                    return Err(Refused::NoSuchTrack);
                }
                let start = if *exact { *start } else { t.grid.snap(*start) };
                t.place_locked(*track, *source, start, *source_beats, *lock).map(|id| vec![id]).ok_or(Refused::DoesNotFit)
            }
            Command::Move { clips, delta_beats, delta_tracks } => {
                // Resolve every destination first, then commit: a move that
                // would leave one clip overlapping is refused whole.
                let mut plan = Vec::new();
                for &id in clips {
                    let (track, clip) = t.find(id).ok_or(Refused::NoSuchClip)?;
                    let from_ix = t.track_index(track.id).ok_or(Refused::NoSuchTrack)? as i32;
                    let to_ix = from_ix + delta_tracks;
                    if to_ix < 0 || to_ix as usize >= t.tracks.len() {
                        return Err(Refused::DoesNotFit);
                    }
                    let to_track = t.tracks[to_ix as usize].id;
                    let start = t.grid.snap(clip.start + delta_beats);
                    plan.push((id, track.id, to_track, start, clip.len));
                }
                // Overlap check against everything NOT being moved.
                for (id, _, to_track, start, len) in &plan {
                    let others: Vec<ClipId> = clips.iter().copied().filter(|c| c != id).collect();
                    let tr = t.track(*to_track).ok_or(Refused::NoSuchTrack)?;
                    if *start < 0.0
                        || tr.clips.iter().any(|c| c.id != *id && !others.contains(&c.id) && c.overlaps(*start, start + len))
                    {
                        return Err(Refused::DoesNotFit);
                    }
                }
                // And against each other at their destinations.
                for (i, a) in plan.iter().enumerate() {
                    for b in plan.iter().skip(i + 1) {
                        if a.2 == b.2 && a.3 < b.3 + b.4 && b.3 < a.3 + a.4 {
                            return Err(Refused::DoesNotFit);
                        }
                    }
                }
                for (id, from, to, start, _) in plan {
                    let clip = {
                        let ft = t.track_mut(from).ok_or(Refused::NoSuchTrack)?;
                        let ix = ft.clips.iter().position(|c| c.id == id).ok_or(Refused::NoSuchClip)?;
                        ft.clips.remove(ix)
                    };
                    let mut clip = clip;
                    clip.start = start;
                    let tt = t.track_mut(to).ok_or(Refused::NoSuchTrack)?;
                    tt.clips.push(clip);
                    tt.clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
                }
                Ok(vec![])
            }
            Command::Trim { clip, start, len, offset } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                if !t.fits(track, *clip, *start, *len) || *offset < 0.0 {
                    return Err(Refused::DoesNotFit);
                }
                let c = t.track_mut(track).and_then(|tr| tr.clip_mut(*clip)).ok_or(Refused::NoSuchClip)?;
                c.start = *start;
                c.len = *len;
                c.offset = *offset;
                Ok(vec![])
            }
            Command::Split { clip, beat } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                let beat = t.grid.snap(*beat);
                let new_id = t.fresh_id();
                let tr = t.track_mut(track).ok_or(Refused::NoSuchTrack)?;
                let c = tr.clip_mut(*clip).ok_or(Refused::NoSuchClip)?;
                if beat <= c.start + 1e-9 || beat >= c.end() - 1e-9 {
                    return Err(Refused::NothingToSplit);
                }
                let cut = beat - c.start;
                let mut right = c.clone();
                right.id = new_id;
                right.start = beat;
                right.len = c.len - cut;
                right.offset = c.offset + cut;
                c.len = cut;
                tr.clips.push(right);
                tr.clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
                Ok(vec![new_id])
            }
            Command::Delete { clips } => {
                let mut any = false;
                for tr in &mut t.tracks {
                    let before = tr.clips.len();
                    tr.clips.retain(|c| !clips.contains(&c.id));
                    any |= tr.clips.len() != before;
                }
                if any { Ok(vec![]) } else { Err(Refused::NoSuchClip) }
            }
            Command::Duplicate { clips, delta_beats } => {
                let mut created = Vec::new();
                let mut copies = Vec::new();
                for &id in clips {
                    let (track, clip) = t.find(id).ok_or(Refused::NoSuchClip)?;
                    copies.push((track.id, clip.clone()));
                }
                for (track, mut clip) in copies {
                    clip.id = t.fresh_id();
                    clip.start = t.grid.snap(clip.start + delta_beats);
                    if !t.fits(track, clip.id, clip.start, clip.len) {
                        return Err(Refused::DoesNotFit);
                    }
                    created.push(clip.id);
                    let tr = t.track_mut(track).ok_or(Refused::NoSuchTrack)?;
                    tr.clips.push(clip);
                    tr.clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
                }
                Ok(created)
            }
            Command::SetClipGain { clip, gain } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                let c = t.track_mut(track).and_then(|tr| tr.clip_mut(*clip)).ok_or(Refused::NoSuchClip)?;
                c.gain = gain.clamp(0.0, 2.0);
                Ok(vec![])
            }
            Command::ToggleClipMute { clip } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                let c = t.track_mut(track).and_then(|tr| tr.clip_mut(*clip)).ok_or(Refused::NoSuchClip)?;
                c.muted = !c.muted;
                Ok(vec![])
            }
            Command::AddTrack { name } => {
                t.add_track(name);
                Ok(vec![])
            }
            Command::RemoveTrack { track } => {
                if t.tracks.len() <= 1 {
                    return Err(Refused::LastTrack);
                }
                let ix = t.track_index(*track).ok_or(Refused::NoSuchTrack)?;
                t.tracks.remove(ix);
                Ok(vec![])
            }
            Command::RenameTrack { track, name } => {
                t.track_mut(*track).ok_or(Refused::NoSuchTrack)?.name = name.clone();
                Ok(vec![])
            }
            Command::ToggleMute { track } => {
                let tr = t.track_mut(*track).ok_or(Refused::NoSuchTrack)?;
                tr.muted = !tr.muted;
                Ok(vec![])
            }
            Command::ToggleSolo { track } => {
                let tr = t.track_mut(*track).ok_or(Refused::NoSuchTrack)?;
                tr.solo = !tr.solo;
                Ok(vec![])
            }
            Command::SetTrackGain { track, gain } => {
                t.track_mut(*track).ok_or(Refused::NoSuchTrack)?.gain = gain.clamp(0.0, 2.0);
                Ok(vec![])
            }
            Command::SetTempo { bpm } => {
                if !(20.0..=400.0).contains(bpm) {
                    return Err(Refused::Invalid(format!("tempo {bpm} is outside 20–400")));
                }
                t.set_tempo(*bpm);
                Ok(vec![])
            }
            Command::SetTempoLock { clip, bpm } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                let tempo = t.bpm;
                let tr = t.track_mut(track).ok_or(Refused::NoSuchTrack)?;
                let ix = tr.clips.iter().position(|c| c.id == *clip).ok_or(Refused::NoSuchClip)?;
                let next_start = tr.clips.get(ix + 1).map(|c| c.start);
                let c = &mut tr.clips[ix];
                match (c.tempo_lock, bpm) {
                    (Some(lock), None) => {
                        // Unlocking at tempo T from lock L: the material
                        // now plays at natural speed, so its extent scales
                        // by T/L. Trimmed to the neighbour if it grows.
                        let k = tempo / lock;
                        c.len *= k;
                        c.offset *= k;
                        c.fade_in *= k;
                        c.fade_out *= k;
                        if let Some(ns) = next_start {
                            c.len = c.len.min(ns - c.start);
                        }
                        c.tempo_lock = None;
                    }
                    (_, Some(b)) => {
                        if !(20.0..=400.0).contains(b) {
                            return Err(Refused::Invalid(format!("lock {b} is outside 20–400")));
                        }
                        c.tempo_lock = Some(*b);
                    }
                    (None, None) => {}
                }
                Ok(vec![])
            }
            Command::SetClipFades { clip, fade_in, fade_out } => {
                let track = t.find_track_of(*clip).ok_or(Refused::NoSuchClip)?;
                let c = t.track_mut(track).and_then(|tr| tr.clip_mut(*clip)).ok_or(Refused::NoSuchClip)?;
                c.fade_in = fade_in.clamp(0.0, c.len);
                c.fade_out = fade_out.clamp(0.0, c.len);
                Ok(vec![])
            }
            Command::SetLoop { region } => {
                t.loop_region = region.map(|(a, b)| (a.min(b).max(0.0), a.max(b).max(0.0))).filter(|(a, b)| b - a > 1e-6);
                Ok(vec![])
            }
            Command::SetGrid { grid } => {
                t.grid = *grid;
                Ok(vec![])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> (Editor, TrackId, TrackId) {
        let e = Editor::default();
        let (a, b) = (e.timeline.tracks[0].id, e.timeline.tracks[1].id);
        (e, a, b)
    }

    #[test]
    fn place_snaps_to_the_grid_and_is_undoable() {
        let (mut e, t0, _) = ed();
        let r = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 1.3, source_beats: 2.0, exact: false, lock: None }).unwrap();
        assert_eq!(r.created.len(), 1);
        let c = e.timeline.find(r.created[0]).unwrap().1;
        assert_eq!(c.start, 1.0, "snapped to the beat grid");
        assert!(e.can_undo());
        assert_eq!(e.undo_label(), Some("place at 1.30"));
        e.undo();
        assert!(e.timeline.tracks[0].clips.is_empty());
        assert!(e.can_redo());
        e.redo();
        assert_eq!(e.timeline.tracks[0].clips.len(), 1);
    }

    #[test]
    fn an_exact_placement_ignores_the_grid() {
        let (mut e, t0, _) = ed();
        let r = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 1.3, source_beats: 2.0, exact: true, lock: None }).unwrap();
        assert_eq!(e.timeline.find(r.created[0]).unwrap().1.start, 1.3);
    }

    #[test]
    fn a_refused_command_records_nothing() {
        let (mut e, t0, _) = ed();
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 4.0, exact: false, lock: None }).unwrap();
        let depth = e.undo.len();
        let err = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 1.0, source_beats: 1.0, exact: false, lock: None }).unwrap_err();
        assert_eq!(err, Refused::DoesNotFit);
        assert_eq!(e.undo.len(), depth);
        assert_eq!(e.apply(Command::Delete { clips: vec![999] }).unwrap_err(), Refused::NoSuchClip);
        assert_eq!(e.apply(Command::SetTempo { bpm: 1000.0 }).unwrap_err(), Refused::Invalid("tempo 1000 is outside 20–400".into()));
    }

    #[test]
    fn a_move_across_tracks_is_all_or_nothing() {
        let (mut e, t0, t1) = ed();
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        let b = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 4.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        let blocker = e.apply(Command::Place { track: t1, source: SourceRef::File(0), start: 5.0, source_beats: 1.0, exact: false, lock: None }).unwrap().created[0];
        // b would land on the blocker: the whole move is refused, a stays.
        assert_eq!(e.apply(Command::Move { clips: vec![a, b], delta_beats: 0.0, delta_tracks: 1 }).unwrap_err(), Refused::DoesNotFit);
        assert_eq!(e.timeline.find_track_of(a), Some(t0));
        // Without the blocker it goes, and the relative spacing survives.
        e.apply(Command::Delete { clips: vec![blocker] }).unwrap();
        e.apply(Command::Move { clips: vec![a, b], delta_beats: 1.0, delta_tracks: 1 }).unwrap();
        assert_eq!(e.timeline.find_track_of(a), Some(t1));
        assert_eq!(e.timeline.find(a).unwrap().1.start, 1.0);
        assert_eq!(e.timeline.find(b).unwrap().1.start, 5.0);
        // Two selected clips may swap places past each other: they are not
        // obstacles to one another.
        e.apply(Command::Move { clips: vec![a, b], delta_beats: 2.0, delta_tracks: 0 }).unwrap();
        // But a move before the origin is refused.
        assert_eq!(e.apply(Command::Move { clips: vec![a], delta_beats: -10.0, delta_tracks: 0 }).unwrap_err(), Refused::DoesNotFit);
        assert_eq!(e.apply(Command::Move { clips: vec![a], delta_beats: 0.0, delta_tracks: 5 }).unwrap_err(), Refused::DoesNotFit);
    }

    #[test]
    fn split_makes_two_clips_that_continue_the_same_material() {
        let (mut e, t0, _) = ed();
        let a = e.apply(Command::Place { track: t0, source: SourceRef::File(0), start: 2.0, source_beats: 4.0, exact: false, lock: None }).unwrap().created[0];
        let r = e.apply(Command::Split { clip: a, beat: 3.0 }).unwrap();
        let right = r.created[0];
        let (_, l) = e.timeline.find(a).unwrap();
        let (_, rc) = e.timeline.find(right).unwrap();
        assert_eq!((l.start, l.len, l.offset), (2.0, 1.0, 0.0));
        assert_eq!((rc.start, rc.len, rc.offset), (3.0, 3.0, 1.0), "the right half starts one beat into the material");
        assert_eq!(rc.source, SourceRef::File(0));
        // At an edge there is nothing to split; outside, nothing either.
        assert_eq!(e.apply(Command::Split { clip: a, beat: 2.0 }).unwrap_err(), Refused::NothingToSplit);
        assert_eq!(e.apply(Command::Split { clip: a, beat: 9.0 }).unwrap_err(), Refused::NothingToSplit);
        // Selection follows: a deleted clip leaves the selection.
        e.selection.insert(right);
        e.apply(Command::Delete { clips: vec![right] }).unwrap();
        assert!(e.selection.is_empty());
    }

    #[test]
    fn trim_keeps_material_offsets_and_respects_neighbours() {
        let (mut e, t0, _) = ed();
        let a = e.apply(Command::Place { track: t0, source: SourceRef::File(0), start: 0.0, source_beats: 4.0, exact: false, lock: None }).unwrap().created[0];
        let _b = e.apply(Command::Place { track: t0, source: SourceRef::File(0), start: 6.0, source_beats: 1.0, exact: false, lock: None }).unwrap().created[0];
        // Head trim: start moves, offset grows, end stays.
        e.apply(Command::Trim { clip: a, start: 1.0, len: 3.0, offset: 1.0 }).unwrap();
        // Tail trim into the neighbour is refused.
        assert_eq!(e.apply(Command::Trim { clip: a, start: 1.0, len: 6.0, offset: 1.0 }).unwrap_err(), Refused::DoesNotFit);
        assert_eq!(e.apply(Command::Trim { clip: a, start: 1.0, len: 1.0, offset: -1.0 }).unwrap_err(), Refused::DoesNotFit);
    }

    #[test]
    fn duplicate_places_copies_later_and_reports_them() {
        let (mut e, t0, _) = ed();
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        let r = e.apply(Command::Duplicate { clips: vec![a], delta_beats: 2.0 }).unwrap();
        assert_eq!(r.created.len(), 1);
        assert_eq!(e.timeline.find(r.created[0]).unwrap().1.start, 2.0);
        // Onto itself: refused.
        assert_eq!(e.apply(Command::Duplicate { clips: vec![a], delta_beats: 1.0 }).unwrap_err(), Refused::DoesNotFit);
    }

    #[test]
    fn tracks_can_be_added_renamed_and_removed_but_not_to_zero() {
        let (mut e, t0, t1) = ed();
        e.apply(Command::AddTrack { name: "fx".into() }).unwrap();
        assert_eq!(e.timeline.tracks.len(), 3);
        e.apply(Command::RenameTrack { track: t0, name: "drums".into() }).unwrap();
        assert_eq!(e.timeline.track(t0).unwrap().name, "drums");
        e.apply(Command::RemoveTrack { track: t1 }).unwrap();
        e.apply(Command::RemoveTrack { track: t0 }).unwrap();
        let last = e.timeline.tracks[0].id;
        assert_eq!(e.apply(Command::RemoveTrack { track: last }).unwrap_err(), Refused::LastTrack);
    }

    #[test]
    fn loop_regions_are_ordered_and_degenerate_ones_vanish() {
        let (mut e, _, _) = ed();
        e.apply(Command::SetLoop { region: Some((8.0, 4.0)) }).unwrap();
        assert_eq!(e.timeline.loop_region, Some((4.0, 8.0)));
        e.apply(Command::SetLoop { region: Some((3.0, 3.0)) }).unwrap();
        assert_eq!(e.timeline.loop_region, None);
    }

    #[test]
    fn the_undo_stack_is_bounded() {
        let (mut e, _, _) = ed();
        e.undo_limit = 3;
        for i in 0..10 {
            e.apply(Command::SetTempo { bpm: 100.0 + i as f64 }).unwrap();
        }
        assert_eq!(e.undo.len(), 3);
    }
}
