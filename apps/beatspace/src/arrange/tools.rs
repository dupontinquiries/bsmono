//! Tools: how the pointer edits the arrangement.
//!
//! The view converts a pointer position into timeline space — a beat, a
//! track index, and what is under it (`Hit`) — and feeds press / drag /
//! release here. This module keeps the gesture's state, tells the view
//! what to draw as a preview while the gesture is in flight, and emits
//! `Command`s (and selection / seek effects) when it ends. No pixels, no
//! egui, so every gesture is a unit test with three calls.
//!
//! ## The tools
//!
//! * **Select** — click selects (shift extends, ctrl toggles); drag a clip
//!   moves the selection (alt: copies); drag a clip's edge trims it; drag
//!   empty space draws a marquee; double-click a clip splits it there.
//! * **Draw** — click or drag paints the armed source ("brush") onto the
//!   grid, one clip per cell, so a beat can be laid down across a bar in
//!   one stroke.
//! * **Split** — click a clip to cut it there.
//! * **Erase** — click a clip to delete it; drag to delete every clip the
//!   pointer crosses.
//! * **Loop** — drag on any track or the ruler to set the loop region;
//!   click clears it.
//!
//! On any tool, pressing on the RULER seeks, and dragging there scrubs.

use super::cmd::{Command, Editor};
use super::model::{Clip, ClipId, SourceRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Draw,
    Split,
    Erase,
    Loop,
}

impl Tool {
    pub const ALL: [Tool; 5] = [Tool::Select, Tool::Draw, Tool::Split, Tool::Erase, Tool::Loop];
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Draw => "draw",
            Tool::Split => "split",
            Tool::Erase => "erase",
            Tool::Loop => "loop",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "click selects · drag moves · alt-drag copies · edges trim · double-click splits",
            Tool::Draw => "click or drag paints the armed source, one clip per grid cell",
            Tool::Split => "click a clip to cut it there",
            Tool::Erase => "click or drag over clips to delete them",
            Tool::Loop => "drag to set the loop region · click to clear it",
        }
    }
}

/// Which part of a clip the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Body,
    Left,
    Right,
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Clip { id: ClipId, edge: Edge },
    /// Empty space on a track.
    Empty,
    /// The ruler above the tracks.
    Ruler,
}

/// A pointer position in timeline space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct At {
    pub beat: f64,
    /// Track index under the pointer; `None` over the ruler.
    pub track: Option<usize>,
    pub hit: Hit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// The armed source for the draw tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    pub source: SourceRef,
    /// Its length in beats at the current tempo.
    pub beats: f64,
}

/// A gesture in flight.
#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    None,
    Moving { clips: Vec<ClipId>, from_beat: f64, from_track: usize, delta_beats: f64, delta_tracks: i32, copy: bool },
    Trim { clip: ClipId, edge: Edge, orig: Clip, delta_beats: f64 },
    Marquee { from_beat: f64, from_track: usize, to_beat: f64, to_track: usize },
    Loop { from_beat: f64, to_beat: f64 },
    Painting { track: usize, painted: Vec<f64> },
    Erasing,
    Scrub,
}

/// What a gesture asked for. The view applies commands to the editor,
/// selection to the editor's selection, and seeks to the transport.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Command(Command),
    /// Replace the selection (`extend` adds to it; `toggle` flips one).
    Select { clips: Vec<ClipId>, extend: bool, toggle: bool },
    Seek(f64),
}

/// What to draw while a gesture is in flight.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    None,
    /// Draw these clips shifted; `fits` says whether the drop would be
    /// accepted, for colouring the ghost.
    Moving { clips: Vec<ClipId>, delta_beats: f64, delta_tracks: i32, copy: bool, fits: bool },
    /// Draw this clip at the trimmed extent.
    Trim { clip: ClipId, start: f64, len: f64, fits: bool },
    Marquee { from_beat: f64, from_track: usize, to_beat: f64, to_track: usize },
    Loop { from: f64, to: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tools {
    pub tool: Tool,
    pub brush: Option<Brush>,
    pub drag: Drag,
}

impl Default for Tools {
    fn default() -> Self {
        Self { tool: Tool::Select, brush: None, drag: Drag::None }
    }
}

impl Tools {
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.drag = Drag::None;
    }

    /// Pointer down.
    pub fn press(&mut self, ed: &Editor, at: At, mods: Mods) -> Vec<Effect> {
        let mut out = Vec::new();
        if at.hit == Hit::Ruler {
            self.drag = Drag::Scrub;
            out.push(Effect::Seek(at.beat.max(0.0)));
            return out;
        }
        let Some(track) = at.track else { return out };
        match self.tool {
            Tool::Select => match at.hit {
                Hit::Clip { id, edge: Edge::Body } => {
                    let already = ed.selection.contains(&id);
                    if mods.ctrl {
                        out.push(Effect::Select { clips: vec![id], extend: false, toggle: true });
                    } else if mods.shift {
                        out.push(Effect::Select { clips: vec![id], extend: true, toggle: false });
                    } else if !already {
                        out.push(Effect::Select { clips: vec![id], extend: false, toggle: false });
                    }
                    // What moves: the selection if the clip is in it,
                    // else the clip alone (the selection effect above has
                    // not been applied yet, so decide from `already`).
                    let clips = if already && !mods.ctrl {
                        ed.selection.iter().copied().collect()
                    } else {
                        vec![id]
                    };
                    self.drag = Drag::Moving {
                        clips,
                        from_beat: at.beat,
                        from_track: track,
                        delta_beats: 0.0,
                        delta_tracks: 0,
                        copy: mods.alt,
                    };
                }
                Hit::Clip { id, edge } => {
                    if let Some((_, c)) = ed.timeline.find(id) {
                        out.push(Effect::Select { clips: vec![id], extend: false, toggle: false });
                        self.drag = Drag::Trim { clip: id, edge, orig: c.clone(), delta_beats: 0.0 };
                    }
                }
                Hit::Empty => {
                    if !mods.shift && !mods.ctrl {
                        out.push(Effect::Select { clips: vec![], extend: false, toggle: false });
                    }
                    self.drag = Drag::Marquee { from_beat: at.beat, from_track: track, to_beat: at.beat, to_track: track };
                }
                Hit::Ruler => {}
            },
            Tool::Draw => {
                if let Some(b) = self.brush {
                    let cell = ed.timeline.grid.snap(at.beat);
                    let track_id = ed.timeline.tracks[track].id;
                    out.push(Effect::Command(Command::Place {
                        track: track_id,
                        source: b.source,
                        start: cell,
                        source_beats: b.beats,
                        exact: false,
                    }));
                    self.drag = Drag::Painting { track, painted: vec![cell] };
                }
            }
            Tool::Split => {
                if let Hit::Clip { id, .. } = at.hit {
                    out.push(Effect::Command(Command::Split { clip: id, beat: at.beat }));
                }
            }
            Tool::Erase => {
                if let Hit::Clip { id, .. } = at.hit {
                    out.push(Effect::Command(Command::Delete { clips: vec![id] }));
                }
                self.drag = Drag::Erasing;
            }
            Tool::Loop => {
                self.drag = Drag::Loop { from_beat: at.beat, to_beat: at.beat };
            }
        }
        out
    }

    /// Pointer moved with the button held.
    pub fn drag(&mut self, ed: &Editor, at: At) -> Vec<Effect> {
        let mut out = Vec::new();
        match &mut self.drag {
            Drag::None => {}
            Drag::Scrub => out.push(Effect::Seek(at.beat.max(0.0))),
            Drag::Moving { from_beat, from_track, delta_beats, delta_tracks, .. } => {
                *delta_beats = at.beat - *from_beat;
                *delta_tracks = at.track.map_or(0, |t| t as i32 - *from_track as i32);
            }
            Drag::Trim { delta_beats, .. } => {
                // Absolute from the press would need the press beat; the
                // trim is expressed as the pointer's beat against the
                // original edge, which is what the preview needs.
                *delta_beats = at.beat;
            }
            Drag::Marquee { to_beat, to_track, .. } => {
                *to_beat = at.beat;
                if let Some(t) = at.track {
                    *to_track = t;
                }
            }
            Drag::Loop { to_beat, .. } => *to_beat = at.beat,
            Drag::Painting { track, painted } => {
                if let (Some(b), Some(t)) = (self.brush, at.track) {
                    if t == *track {
                        let cell = ed.timeline.grid.snap(at.beat);
                        if !painted.iter().any(|p| (p - cell).abs() < 1e-9) {
                            painted.push(cell);
                            let track_id = ed.timeline.tracks[t].id;
                            out.push(Effect::Command(Command::Place {
                                track: track_id,
                                source: b.source,
                                start: cell,
                                source_beats: b.beats,
                                exact: false,
                            }));
                        }
                    }
                }
            }
            Drag::Erasing => {
                if let Hit::Clip { id, .. } = at.hit {
                    out.push(Effect::Command(Command::Delete { clips: vec![id] }));
                }
            }
        }
        out
    }

    /// Pointer up: commit the gesture.
    pub fn release(&mut self, ed: &Editor, at: At) -> Vec<Effect> {
        let mut out = Vec::new();
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None | Drag::Scrub | Drag::Painting { .. } | Drag::Erasing => {}
            Drag::Moving { clips, delta_beats, delta_tracks, copy, .. } => {
                let snapped = ed.timeline.grid.snap(delta_beats);
                if snapped.abs() > 1e-9 || delta_tracks != 0 {
                    if copy {
                        out.push(Effect::Command(Command::Duplicate { clips, delta_beats: snapped }));
                    } else {
                        out.push(Effect::Command(Command::Move { clips, delta_beats: snapped, delta_tracks }));
                    }
                }
            }
            Drag::Trim { clip, edge, orig, delta_beats } => {
                if let Some((start, len, offset)) = trim_extent(&ed.timeline.grid, &orig, edge, delta_beats) {
                    if (start - orig.start).abs() > 1e-9 || (len - orig.len).abs() > 1e-9 {
                        out.push(Effect::Command(Command::Trim { clip, start, len, offset }));
                    }
                }
            }
            Drag::Marquee { from_beat, from_track, to_beat, to_track } => {
                let (lo, hi) = (from_track.min(to_track), from_track.max(to_track));
                let tracks: Vec<_> = ed.timeline.tracks.iter().enumerate()
                    .filter(|(i, _)| *i >= lo && *i <= hi).map(|(_, t)| t.id).collect();
                let clips: Vec<ClipId> = ed.timeline.clips_in(from_beat, to_beat, &tracks).into_iter().collect();
                if !clips.is_empty() {
                    out.push(Effect::Select { clips, extend: true, toggle: false });
                }
            }
            Drag::Loop { from_beat, to_beat } => {
                let g = ed.timeline.grid;
                let (a, b) = (g.snap(from_beat), g.snap(to_beat));
                let region = if (a - b).abs() < 1e-9 { None } else { Some((a, b)) };
                out.push(Effect::Command(Command::SetLoop { region }));
            }
        }
        let _ = at;
        out
    }

    /// Double-click with the select tool splits the clip under the pointer.
    pub fn double_click(&mut self, at: At) -> Vec<Effect> {
        self.drag = Drag::None;
        match (self.tool, at.hit) {
            (Tool::Select, Hit::Clip { id, .. }) => vec![Effect::Command(Command::Split { clip: id, beat: at.beat })],
            _ => vec![],
        }
    }

    /// The ghost to draw this frame.
    pub fn preview(&self, ed: &Editor) -> Preview {
        match &self.drag {
            Drag::Moving { clips, delta_beats, delta_tracks, copy, .. } => {
                let snapped = ed.timeline.grid.snap(*delta_beats);
                let fits = clips.iter().all(|&id| {
                    let Some((track, clip)) = ed.timeline.find(id) else { return false };
                    let Some(ix) = ed.timeline.track_index(track.id) else { return false };
                    let to = ix as i32 + delta_tracks;
                    if to < 0 || to as usize >= ed.timeline.tracks.len() {
                        return false;
                    }
                    let to_track = ed.timeline.tracks[to as usize].id;
                    let start = clip.start + snapped;
                    let me = if *copy { u64::MAX } else { id };
                    let others: Vec<ClipId> = clips.iter().copied().collect();
                    start >= 0.0
                        && !ed.timeline.tracks[to as usize].clips.iter().any(|c| {
                            c.id != me && (*copy || !others.contains(&c.id)) && c.overlaps(start, start + clip.len)
                        })
                        && ed.timeline.track(to_track).is_some()
                });
                Preview::Moving { clips: clips.clone(), delta_beats: snapped, delta_tracks: *delta_tracks, copy: *copy, fits }
            }
            Drag::Trim { clip, edge, orig, delta_beats } => {
                match trim_extent(&ed.timeline.grid, orig, *edge, *delta_beats) {
                    Some((start, len, _)) => {
                        let track = ed.timeline.find_track_of(*clip);
                        let fits = track.map_or(false, |t| ed.timeline.fits(t, *clip, start, len));
                        Preview::Trim { clip: *clip, start, len, fits }
                    }
                    None => Preview::Trim { clip: *clip, start: orig.start, len: orig.len, fits: true },
                }
            }
            Drag::Marquee { from_beat, from_track, to_beat, to_track } => {
                Preview::Marquee { from_beat: *from_beat, from_track: *from_track, to_beat: *to_beat, to_track: *to_track }
            }
            Drag::Loop { from_beat, to_beat } => {
                let g = ed.timeline.grid;
                Preview::Loop { from: g.snap(*from_beat), to: g.snap(*to_beat) }
            }
            _ => Preview::None,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag != Drag::None
    }
}

/// The extent a trim gives: `(start, len, offset)`, or `None` when the
/// pointer would collapse the clip. `pointer_beat` is where the pointer is
/// now; the moved edge snaps there and the other edge stays.
fn trim_extent(grid: &super::model::Grid, orig: &Clip, edge: Edge, pointer_beat: f64) -> Option<(f64, f64, f64)> {
    let min_len = grid.0.unwrap_or(0.05).max(1e-3);
    match edge {
        Edge::Left => {
            // The head cannot move earlier than the material allows.
            let earliest = orig.start - orig.offset;
            let start = grid.snap(pointer_beat).max(earliest).max(0.0);
            let start = start.min(orig.end() - min_len);
            let len = orig.end() - start;
            let offset = orig.offset + (start - orig.start);
            if len < min_len - 1e-9 { None } else { Some((start, len, offset)) }
        }
        Edge::Right => {
            let end = grid.snap(pointer_beat).max(orig.start + min_len);
            let len = end - orig.start;
            if len < min_len - 1e-9 { None } else { Some((orig.start, len, orig.offset)) }
        }
        Edge::Body => Some((orig.start, orig.len, orig.offset)),
    }
}

/// Keyboard shortcuts, as intents. The view maps keys to these; this maps
/// them to effects, so the mapping is one table and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Delete,
    Duplicate,
    SplitAtPlayhead,
    SelectAll,
    NudgeLeft,
    NudgeRight,
    NudgeUp,
    NudgeDown,
    Tool(Tool),
    ToggleLoop,
}

impl Shortcut {
    pub fn effects(self, ed: &Editor, playhead: f64, tools: &mut Tools) -> Vec<Effect> {
        let sel: Vec<ClipId> = ed.selection.iter().copied().collect();
        let step = ed.timeline.grid.0.unwrap_or(0.25);
        match self {
            Shortcut::Delete if !sel.is_empty() => vec![Effect::Command(Command::Delete { clips: sel })],
            Shortcut::Duplicate if !sel.is_empty() => {
                // Copies land right after the selection's last end.
                let (min_start, max_end) = sel.iter().filter_map(|id| ed.timeline.find(*id)).map(|(_, c)| (c.start, c.end()))
                    .fold((f64::MAX, 0.0f64), |(a, b), (s, e)| (a.min(s), b.max(e)));
                vec![Effect::Command(Command::Duplicate { clips: sel, delta_beats: max_end - min_start })]
            }
            Shortcut::SplitAtPlayhead => ed
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.contains(playhead) && (sel.is_empty() || sel.contains(&c.id)))
                .map(|c| Effect::Command(Command::Split { clip: c.id, beat: playhead }))
                .collect(),
            Shortcut::SelectAll => vec![Effect::Select {
                clips: ed.timeline.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id)).collect(),
                extend: false,
                toggle: false,
            }],
            Shortcut::NudgeLeft if !sel.is_empty() => vec![Effect::Command(Command::Move { clips: sel, delta_beats: -step, delta_tracks: 0 })],
            Shortcut::NudgeRight if !sel.is_empty() => vec![Effect::Command(Command::Move { clips: sel, delta_beats: step, delta_tracks: 0 })],
            Shortcut::NudgeUp if !sel.is_empty() => vec![Effect::Command(Command::Move { clips: sel, delta_beats: 0.0, delta_tracks: -1 })],
            Shortcut::NudgeDown if !sel.is_empty() => vec![Effect::Command(Command::Move { clips: sel, delta_beats: 0.0, delta_tracks: 1 })],
            Shortcut::Tool(t) => {
                tools.set_tool(t);
                vec![]
            }
            Shortcut::ToggleLoop => {
                let region = match ed.timeline.loop_region {
                    Some(_) => None,
                    None => {
                        // The selection's extent, or the first bar.
                        let bar = ed.timeline.beats_per_bar as f64;
                        if sel.is_empty() {
                            let b = (playhead / bar).floor() * bar;
                            Some((b, b + bar))
                        } else {
                            let ext = sel.iter().filter_map(|id| ed.timeline.find(*id)).map(|(_, c)| (c.start, c.end()))
                                .fold((f64::MAX, 0.0f64), |(a, b), (s, e)| (a.min(s), b.max(e)));
                            Some(ext)
                        }
                    }
                };
                vec![Effect::Command(Command::SetLoop { region })]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::{Grid, TrackId};

    fn setup() -> (Editor, TrackId, TrackId, ClipId, ClipId) {
        let mut e = Editor::default();
        let (t0, t1) = (e.timeline.tracks[0].id, e.timeline.tracks[1].id);
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false }).unwrap().created[0];
        let b = e.apply(Command::Place { track: t0, source: SourceRef::Beat(1), start: 4.0, source_beats: 2.0, exact: false }).unwrap().created[0];
        (e, t0, t1, a, b)
    }

    fn at(beat: f64, track: usize, hit: Hit) -> At {
        At { beat, track: Some(track), hit }
    }
    fn body(id: ClipId) -> Hit {
        Hit::Clip { id, edge: Edge::Body }
    }

    /// Apply a gesture's effects to the editor the way the view does.
    fn apply(e: &mut Editor, effects: Vec<Effect>) {
        for ef in effects {
            match ef {
                Effect::Command(c) => {
                    let _ = e.apply(c);
                }
                Effect::Select { clips, extend, toggle } => {
                    if toggle {
                        for c in clips {
                            if !e.selection.remove(&c) {
                                e.selection.insert(c);
                            }
                        }
                    } else {
                        if !extend {
                            e.selection.clear();
                        }
                        e.selection.extend(clips);
                    }
                }
                Effect::Seek(_) => {}
            }
        }
    }

    #[test]
    fn select_click_drag_moves_a_clip_by_a_snapped_delta() {
        let (mut e, _, _, a, _) = setup();
        let mut t = Tools::default();
        { let fx = t.press(&e, at(0.5, 0, body(a)), Mods::default()); apply(&mut e, fx); }
        assert_eq!(e.selection.iter().copied().collect::<Vec<_>>(), vec![a]);
        t.drag(&e, at(1.9, 0, Hit::Empty));
        match t.preview(&e) {
            Preview::Moving { delta_beats, fits, .. } => {
                assert_eq!(delta_beats, 1.0, "1.4 snapped to the beat grid");
                assert!(fits, "0..2 moved to 1..3 does not reach b at 4");
            }
            p => panic!("{p:?}"),
        }
        { let fx = t.release(&e, at(1.9, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(a).unwrap().1.start, 1.0);
        assert!(!t.dragging());
    }

    #[test]
    fn a_move_that_would_overlap_previews_as_not_fitting_and_is_refused() {
        let (mut e, _, _, a, _) = setup();
        let mut t = Tools::default();
        t.press(&e, at(0.5, 0, body(a)), Mods::default());
        t.drag(&e, at(4.0, 0, Hit::Empty)); // delta 3.5 -> 4: onto b
        assert!(matches!(t.preview(&e), Preview::Moving { fits: false, .. }));
        { let fx = t.release(&e, at(4.0, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(a).unwrap().1.start, 0.0, "refused, so untouched");
    }

    #[test]
    fn alt_drag_copies_instead_of_moving() {
        let (mut e, _, _, a, _) = setup();
        let mut t = Tools::default();
        t.press(&e, at(0.5, 0, body(a)), Mods { alt: true, ..Default::default() });
        t.drag(&e, at(2.5, 0, Hit::Empty));
        assert!(matches!(t.preview(&e), Preview::Moving { copy: true, fits: true, .. }));
        { let fx = t.release(&e, at(2.5, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.tracks[0].clips.len(), 3);
        assert_eq!(e.timeline.find(a).unwrap().1.start, 0.0, "the original stayed");
    }

    #[test]
    fn dragging_a_selected_clip_moves_the_whole_selection_across_tracks() {
        let (mut e, _, t1, a, b) = setup();
        e.selection.extend([a, b]);
        let mut t = Tools::default();
        let effects = t.press(&e, at(4.5, 0, body(b)), Mods::default());
        assert!(effects.is_empty(), "pressing a selected clip does not reselect");
        t.drag(&e, at(4.5, 1, Hit::Empty));
        { let fx = t.release(&e, at(4.5, 1, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find_track_of(a), Some(t1));
        assert_eq!(e.timeline.find_track_of(b), Some(t1));
    }

    #[test]
    fn shift_extends_and_ctrl_toggles_the_selection() {
        let (mut e, _, _, a, b) = setup();
        let mut t = Tools::default();
        { let fx = t.press(&e, at(0.5, 0, body(a)), Mods::default()); apply(&mut e, fx); }
        t.release(&e, at(0.5, 0, body(a)));
        { let fx = t.press(&e, at(4.5, 0, body(b)), Mods { shift: true, ..Default::default() }); apply(&mut e, fx); }
        t.release(&e, at(4.5, 0, body(b)));
        assert_eq!(e.selection.len(), 2);
        { let fx = t.press(&e, at(0.5, 0, body(a)), Mods { ctrl: true, ..Default::default() }); apply(&mut e, fx); }
        t.release(&e, at(0.5, 0, body(a)));
        assert_eq!(e.selection.iter().copied().collect::<Vec<_>>(), vec![b]);
        // Clicking empty space clears.
        { let fx = t.press(&e, at(9.0, 0, Hit::Empty), Mods::default()); apply(&mut e, fx); }
        t.release(&e, at(9.0, 0, Hit::Empty));
        assert!(e.selection.is_empty());
    }

    #[test]
    fn edge_drags_trim_and_the_head_trim_keeps_the_material_aligned() {
        let (mut e, _, _, a, _) = setup();
        let mut t = Tools::default();
        // Tail: drag the right edge from 2 to 3.
        t.press(&e, at(2.0, 0, Hit::Clip { id: a, edge: Edge::Right }), Mods::default());
        t.drag(&e, at(3.2, 0, Hit::Empty));
        assert!(matches!(t.preview(&e), Preview::Trim { start, len, fits: true, .. } if start == 0.0 && len == 3.0));
        { let fx = t.release(&e, at(3.2, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(a).unwrap().1.len, 3.0);
        // Tail into b: does not fit, refused.
        t.press(&e, at(3.0, 0, Hit::Clip { id: a, edge: Edge::Right }), Mods::default());
        t.drag(&e, at(5.0, 0, Hit::Empty));
        assert!(matches!(t.preview(&e), Preview::Trim { fits: false, .. }));
        { let fx = t.release(&e, at(5.0, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(a).unwrap().1.len, 3.0);
        // Head: drag the left edge from 0 to 1 — offset follows.
        t.press(&e, at(0.0, 0, Hit::Clip { id: a, edge: Edge::Left }), Mods::default());
        t.drag(&e, at(1.1, 0, Hit::Empty));
        { let fx = t.release(&e, at(1.1, 0, Hit::Empty)); apply(&mut e, fx); }
        let c = e.timeline.find(a).unwrap().1;
        assert_eq!((c.start, c.len, c.offset), (1.0, 2.0, 1.0));
        // And the head cannot go earlier than the material: back to 0 is
        // the earliest (offset 1 -> 0), not -1.
        t.press(&e, at(1.0, 0, Hit::Clip { id: a, edge: Edge::Left }), Mods::default());
        t.drag(&e, at(-3.0, 0, Hit::Empty));
        { let fx = t.release(&e, at(-3.0, 0, Hit::Empty)); apply(&mut e, fx); }
        let c = e.timeline.find(a).unwrap().1;
        assert_eq!((c.start, c.len, c.offset), (0.0, 3.0, 0.0));
        // A trim can never collapse a clip below one grid step.
        t.press(&e, at(3.0, 0, Hit::Clip { id: a, edge: Edge::Right }), Mods::default());
        t.drag(&e, at(-5.0, 0, Hit::Empty));
        { let fx = t.release(&e, at(-5.0, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(a).unwrap().1.len, 1.0);
    }

    #[test]
    fn marquee_selects_what_it_crosses_on_the_tracks_it_spans() {
        let (mut e, _, t1, a, b) = setup();
        let c = e.apply(Command::Place { track: t1, source: SourceRef::File(0), start: 1.0, source_beats: 1.0, exact: false }).unwrap().created[0];
        let mut t = Tools::default();
        t.press(&e, at(3.0, 0, Hit::Empty), Mods::default());
        t.drag(&e, at(0.5, 1, Hit::Empty));
        assert!(matches!(t.preview(&e), Preview::Marquee { .. }));
        { let fx = t.release(&e, at(0.5, 1, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.selection.iter().copied().collect::<Vec<_>>(), vec![a, c]);
        assert!(!e.selection.contains(&b));
    }

    #[test]
    fn draw_paints_the_brush_once_per_cell_across_a_stroke() {
        let (mut e, t0, _, _, _) = setup();
        let mut t = Tools::default();
        t.set_tool(Tool::Draw);
        // No brush: nothing happens.
        assert!(t.press(&e, at(8.0, 0, Hit::Empty), Mods::default()).is_empty());
        t.brush = Some(Brush { source: SourceRef::Beat(2), beats: 1.0 });
        { let fx = t.press(&e, at(8.2, 0, Hit::Empty), Mods::default()); apply(&mut e, fx); }
        for x in [8.4, 8.6, 9.1, 9.4, 10.2, 10.2] {
            { let fx = t.drag(&e, at(x, 0, Hit::Empty)); apply(&mut e, fx); }
        }
        t.release(&e, at(10.2, 0, Hit::Empty));
        let starts: Vec<f64> = e.timeline.track(t0).unwrap().clips.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0.0, 4.0, 8.0, 9.0, 10.0], "one clip per cell, none twice");
        // Painting onto another track mid-stroke is ignored (the stroke
        // belongs to the track it started on).
        assert!(t.drag(&e, at(11.0, 1, Hit::Empty)).is_empty());
    }

    #[test]
    fn split_erase_and_loop_tools_do_what_they_say() {
        let (mut e, _, _, a, b) = setup();
        let mut t = Tools::default();
        t.set_tool(Tool::Split);
        { let fx = t.press(&e, at(1.0, 0, body(a)), Mods::default()); apply(&mut e, fx); }
        assert_eq!(e.timeline.tracks[0].clips.len(), 3);
        t.set_tool(Tool::Erase);
        { let fx = t.press(&e, at(4.5, 0, body(b)), Mods::default()); apply(&mut e, fx); }
        assert!(e.timeline.find(b).is_none());
        // Dragging the eraser over another clip deletes it too.
        { let fx = t.drag(&e, at(0.5, 0, body(a))); apply(&mut e, fx); }
        assert!(e.timeline.find(a).is_none());
        t.release(&e, at(0.5, 0, Hit::Empty));
        t.set_tool(Tool::Loop);
        t.press(&e, at(2.2, 0, Hit::Empty), Mods::default());
        t.drag(&e, at(6.4, 0, Hit::Empty));
        assert_eq!(t.preview(&e), Preview::Loop { from: 2.0, to: 6.0 });
        { let fx = t.release(&e, at(6.4, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.loop_region, Some((2.0, 6.0)));
        // A click (no travel) clears it.
        t.press(&e, at(3.0, 0, Hit::Empty), Mods::default());
        { let fx = t.release(&e, at(3.0, 0, Hit::Empty)); apply(&mut e, fx); }
        assert_eq!(e.timeline.loop_region, None);
    }

    #[test]
    fn the_ruler_seeks_and_scrubs_on_any_tool() {
        let (e, _, _, _, _) = setup();
        for tool in Tool::ALL {
            let mut t = Tools::default();
            t.set_tool(tool);
            let ruler = At { beat: 3.3, track: None, hit: Hit::Ruler };
            assert_eq!(t.press(&e, ruler, Mods::default()), vec![Effect::Seek(3.3)]);
            assert_eq!(t.drag(&e, At { beat: -1.0, ..ruler }), vec![Effect::Seek(0.0)], "never before the origin");
            assert!(t.release(&e, ruler).is_empty());
        }
    }

    #[test]
    fn double_click_splits_only_with_the_select_tool() {
        let (_e, _, _, a, _) = setup();
        let mut t = Tools::default();
        assert_eq!(t.double_click(at(1.0, 0, body(a))), vec![Effect::Command(Command::Split { clip: a, beat: 1.0 })]);
        t.set_tool(Tool::Erase);
        assert!(t.double_click(at(1.0, 0, body(a))).is_empty());
    }

    #[test]
    fn shortcuts_act_on_the_selection_and_the_playhead() {
        let (mut e, _, _, a, b) = setup();
        let mut t = Tools::default();
        assert!(Shortcut::Delete.effects(&e, 0.0, &mut t).is_empty(), "nothing selected, nothing deleted");
        e.selection.insert(a);
        { let fx = Shortcut::Duplicate.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.tracks[0].clips.len(), 3, "a copy after a, before b");
        assert_eq!(e.timeline.tracks[0].clips[1].start, 2.0);
        e.selection.clear();
        { let fx = Shortcut::SplitAtPlayhead.effects(&e, 5.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.tracks[0].clips.len(), 4, "b split at the playhead");
        { let fx = Shortcut::SelectAll.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.selection.len(), 4);
        // Nudging b right into its own right half is refused; clear the
        // way first.
        let right_half = e.timeline.tracks[0].clips.last().unwrap().id;
        e.selection.clear();
        e.selection.insert(b);
        { let fx = Shortcut::NudgeRight.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(b).unwrap().1.start, 4.0, "blocked by the right half");
        e.apply(Command::Delete { clips: vec![right_half] }).unwrap();
        { let fx = Shortcut::NudgeRight.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.find(b).unwrap().1.start, 5.0);
        { let fx = Shortcut::NudgeDown.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.track_index(e.timeline.find_track_of(b).unwrap()), Some(1));
        Shortcut::Tool(Tool::Loop).effects(&e, 0.0, &mut t);
        assert_eq!(t.tool, Tool::Loop);
        // Loop toggle: the selection's extent, then off.
        { let fx = Shortcut::ToggleLoop.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.loop_region, Some((5.0, 6.0)));
        { let fx = Shortcut::ToggleLoop.effects(&e, 0.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.loop_region, None);
        e.selection.clear();
        { let fx = Shortcut::ToggleLoop.effects(&e, 9.0, &mut t); apply(&mut e, fx); }
        assert_eq!(e.timeline.loop_region, Some((8.0, 12.0)), "no selection: the bar under the playhead");
    }

    #[test]
    fn free_grid_moves_are_not_snapped_and_trims_keep_a_small_floor() {
        let (mut e, _, _, a, _) = setup();
        e.apply(Command::SetGrid { grid: Grid::OFF }).unwrap();
        let mut t = Tools::default();
        t.press(&e, at(0.5, 0, body(a)), Mods::default());
        t.drag(&e, at(0.83, 0, Hit::Empty));
        { let fx = t.release(&e, at(0.83, 0, Hit::Empty)); apply(&mut e, fx); }
        assert!((e.timeline.find(a).unwrap().1.start - 0.33).abs() < 1e-9);
    }
}
