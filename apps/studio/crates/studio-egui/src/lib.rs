//! studio-egui — the timeline, drawn.
//!
//! The layout arithmetic (time↔pixel, clip rectangles, hit testing, snapping)
//! is kept in plain functions with no `egui::Ui` anywhere near them, for the
//! same reason snapclone's trace policy is Qt-free: it is the part that is easy
//! to get subtly wrong and it is only testable if nothing needs a window. The
//! drawing code below is then a thin pass over those results.
//!
//! What this shows that kdenlive and Audacity cannot: **other people**. Peer
//! playheads, their selections, and a marker on any clip somebody else is
//! working in are drawn from `studio_collab::Doc::peers`, because a
//! collaborative editor whose collaboration is invisible is just an editor with
//! a surprising undo history.

pub mod filmstrip;
pub mod beatgrid;
pub mod group_graph;
pub mod touch;
pub mod keys;
pub mod playkeys;
pub mod spectrum;

/// The step-sequencer surface for studio-live.
pub mod auto;
pub mod grid;
pub mod roll;
pub mod routing;
pub mod lanes;

/// Time-range selection — the audio editor's unit, distinct from clip picking.
pub mod range;

/// One box for transcript search, commands and routines.
pub mod palette;

/// The compressor curve and its gain-reduction meter.
pub mod dynamics;

/// What media each peer has, and what is still coming.
pub mod sync;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use studio_collab::Doc;
use studio_core::{ClipId, Project, Ticks, TrackId, TrackKind, TICKS_PER_SECOND};
use uikit_egui::PaletteColors;

/// Viewport state: where the timeline is scrolled and how far it is zoomed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Label the ruler in SMPTE timecode at this rate, instead of as a wall clock.
    ///
    /// `None` keeps `H:MM:SS.mmm`, which is right for a music session: a bar is not a frame
    /// and rounding a beat to 1/25s would be a lie about where it sits.
    ///
    /// `Some` is for video, where a duration is the WRONG OBJECT. `01:00:00:12` names a frame
    /// you can cut on; it is what the camera stamped, what the slate showed and what every
    /// note on the shot list refers to. `studio_video::timecode` has done this arithmetic —
    /// including drop-frame, which is the hard part — since it was written, and nothing has
    /// ever called it.
    pub timecode_rate: Option<(studio_video::timecode::Rate, bool)>,
    /// Horizontal zoom. Stored as pixels per SECOND rather than per tick so the
    /// number stays human — 100.0 is "a second is a hundred pixels".
    pub px_per_sec: f32,
    /// Leftmost visible time.
    pub scroll: Ticks,
    /// Row height for one track.
    pub track_h: f32,
    /// Width of the track-header gutter on the left.
    pub gutter: f32,
    /// A clip mid-drag. Interaction state, not camera — it lives here because
    /// both answer "what is this window doing right now", the view is already
    /// the `&mut` the timeline holds across frames, and a `None` default
    /// keeps every existing constructor working. The drag never touches the
    /// project: the ghost is drawn from THIS, and only the drop becomes an
    /// edit for the host to apply — grid's own returns-edits rule.
    pub drag: Option<ClipDrag>,
}

/// A grabbed clip: which one, and what the grab means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipDrag {
    pub id: ClipId,
    pub kind: DragKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragKind {
    /// Body grab: the whole clip moves. `grab_dt` is pointer time minus
    /// clip.pos at grab, so the clip doesn't jump to put its left edge under
    /// the finger.
    Move { grab_dt: Ticks },
    /// Edge grab: that edge follows the pointer, the other stays put.
    TrimLeft,
    TrimRight,
}

/// Which edge an ambiguous grab prefers (a clip a few pixels wide has both
/// edges under one finger). `alt-;` flips it — the job `Action::
/// FlipTrimSide` was minted for, back when the keymap was designed ahead of
/// this drag existing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrimSide {
    #[default]
    Left,
    Right,
}

/// How many pixels from a clip edge count as grabbing THAT EDGE rather than
/// the body.
pub const TRIM_GRIP_PX: f32 = 7.0;

/// Classify a grab at `at`: which clip, and move vs trim. Pure, so the
/// pointer-free tests can hold it still.
pub fn grab(
    project: &Project,
    view: &View,
    at: Pos2,
    origin: Pos2,
    prefer: TrimSide,
) -> Option<ClipDrag> {
    let id = hit_test(project, view, at, origin)?;
    let clip = project.clips.get(&id)?;
    let rows = track_rows(project);
    let row = rows.iter().position(|t| *t == clip.track)?;
    let r = view.clip_rect(clip.pos, clip.len, row, origin);
    let near_l = (at.x - r.min.x).abs() <= TRIM_GRIP_PX;
    let near_r = (r.max.x - at.x).abs() <= TRIM_GRIP_PX;
    let kind = match (near_l, near_r) {
        (true, true) => match prefer {
            TrimSide::Left => DragKind::TrimLeft,
            TrimSide::Right => DragKind::TrimRight,
        },
        (true, false) => DragKind::TrimLeft,
        (false, true) => DragKind::TrimRight,
        (false, false) => DragKind::Move {
            grab_dt: view.x_to_time(at.x, origin.x).saturating_sub(clip.pos),
        },
    };
    Some(ClipDrag { id, kind })
}

/// Where a trim drag would leave the clip: `(pos, src_in, len)` — the full
/// placement, because a LEFT trim moves the start while eating source
/// material, and pretending otherwise would smear audio in time. Clamped so
/// the clip keeps at least a sliver and never claims source it doesn't have;
/// the op still has the final word.
pub fn trim_target(
    project: &Project,
    view: &View,
    drag: &ClipDrag,
    at: Pos2,
    origin: Pos2,
    playhead: Ticks,
) -> Option<(Ticks, Ticks, Ticks)> {
    let clip = project.clips.get(&drag.id)?;
    let src_len = project.sources.get(&clip.source)?.len;
    let tolerance = Ticks((0.25 * TICKS_PER_SECOND as f64 * (100.0 / view.px_per_sec as f64)) as i64);
    let t = snap(project, view.x_to_time(at.x, origin.x), playhead, tolerance, Some(drag.id));
    let min = Ticks(TICKS_PER_SECOND / 100); // 10ms sliver, never zero
    match drag.kind {
        DragKind::TrimRight => {
            let max_len = src_len.saturating_sub(clip.src_in);
            let len = Ticks(t.0.saturating_sub(clip.pos.0).clamp(min.0, max_len.0));
            Some((clip.pos, clip.src_in, len))
        }
        DragKind::TrimLeft => {
            let end = Ticks(clip.pos.0 + clip.len.0);
            // The new start may not pass the end (minus the sliver), may not
            // go before time zero, and may not eat more source than exists
            // before the current in-point.
            let lo = (clip.pos.0 - clip.src_in.0).max(0);
            let pos = Ticks(t.0.clamp(lo, end.0 - min.0));
            let d = pos.0 - clip.pos.0;
            Some((pos, Ticks(clip.src_in.0 + d), Ticks(clip.len.0 - d)))
        }
        DragKind::Move { .. } => None,
    }
}

impl Default for View {
    fn default() -> Self {
        View { px_per_sec: 100.0, scroll: Ticks::ZERO, track_h: 64.0, gutter: 132.0, drag: None,
               // A wall clock by default: Studio opens as a music session, and a bar is not
               // a frame. Video projects set this when they know their rate.
               timecode_rate: None }
    }
}

impl View {
    /// Timeline time → x, in the widget's coordinate space.
    pub fn time_to_x(&self, t: Ticks, origin_x: f32) -> f32 {
        origin_x + self.gutter + (t.0 - self.scroll.0) as f32 / TICKS_PER_SECOND as f32 * self.px_per_sec
    }

    /// x → timeline time. Inverse of `time_to_x`; a test pins the round-trip,
    /// because an editor whose ruler and playhead disagree is unusable and the
    /// disagreement is always in one of these two lines.
    pub fn x_to_time(&self, x: f32, origin_x: f32) -> Ticks {
        let secs = (x - origin_x - self.gutter) / self.px_per_sec;
        Ticks(self.scroll.0 + (secs * TICKS_PER_SECOND as f32) as i64)
    }

    /// Zoom about a fixed point on screen, so the frame under the cursor stays
    /// put — the behaviour every editor has and every reimplementation forgets.
    /// Pan horizontally by screen pixels (positive = later times come into
    /// view). Clamped at zero: there is nothing before the origin to see.
    pub fn pan(&mut self, dx_px: f32) {
        let dt = (dx_px / self.px_per_sec * TICKS_PER_SECOND as f32) as i64;
        self.scroll = Ticks((self.scroll.0 + dt).max(0));
    }

    pub fn zoom_about(&mut self, factor: f32, anchor_x: f32, origin_x: f32) {
        let anchor_t = self.x_to_time(anchor_x, origin_x);
        self.px_per_sec = (self.px_per_sec * factor).clamp(0.5, 20_000.0);
        let after = self.x_to_time(anchor_x, origin_x);
        self.scroll = Ticks(self.scroll.0 + (anchor_t.0 - after.0));
    }

    /// The rectangle a clip occupies, given its track's row index.
    pub fn clip_rect(&self, pos: Ticks, len: Ticks, row: usize, origin: Pos2) -> Rect {
        let x0 = self.time_to_x(pos, origin.x);
        let x1 = self.time_to_x(pos.saturating_add(len), origin.x);
        let y0 = origin.y + RULER_H + row as f32 * self.track_h;
        Rect::from_min_max(Pos2::new(x0, y0 + 2.0), Pos2::new(x1, y0 + self.track_h - 2.0))
    }

    /// Which track row does this y fall in? `None` above the first row.
    pub fn row_at(&self, y: f32, origin_y: f32, rows: usize) -> Option<usize> {
        let rel = y - origin_y - RULER_H;
        if rel < 0.0 { return None; }
        let row = (rel / self.track_h) as usize;
        (row < rows).then_some(row)
    }
}

pub const RULER_H: f32 = 26.0;

/// Which clip (if any) sits under a point. Pure, so it is unit-testable — and
/// it searches in reverse draw order so the clip you can SEE on top is the one
/// you get, which matters the moment two tracks overlap visually.
/// Where a dragged clip would land: snapped time (never negative), and the
/// track under the pointer — but ONLY if that track carries the same kind as
/// the clip's current one. Dragging audio "through" a video row keeps the
/// original track: a vertical move between kinds is a re-route the mixer
/// cannot honour, so refusing it at the gesture beats refusing it at the op.
pub fn move_target(
    project: &Project,
    view: &View,
    drag: &ClipDrag,
    at: Pos2,
    origin: Pos2,
    playhead: Ticks,
) -> Option<(Ticks, TrackId)> {
    let clip = project.clips.get(&drag.id)?;
    let DragKind::Move { grab_dt } = drag.kind else { return None };
    let raw = view.x_to_time(at.x, origin.x).saturating_sub(grab_dt);
    let tolerance = Ticks((0.25 * TICKS_PER_SECOND as f64 * (100.0 / view.px_per_sec as f64)) as i64);
    let pos = Ticks(snap(project, raw, playhead, tolerance, Some(drag.id)).0.max(0));
    let rows = track_rows(project);
    let row = ((at.y - origin.y - RULER_H) / view.track_h).floor();
    let from_kind = project.tracks.iter().find(|t| t.id == clip.track)?.kind;
    let track = if row >= 0.0 {
        rows.get(row as usize)
            .and_then(|tid| project.tracks.iter().find(|t| t.id == *tid))
            .filter(|t| t.kind == from_kind && !t.locked)
            .map(|t| t.id)
            .unwrap_or(clip.track)
    } else {
        clip.track
    };
    Some((pos, track))
}

/// What one timeline frame produced for the host.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TimelineResponse {
    /// A composition's fold chevron was clicked: flip its open/closed.
    pub toggled_group: Option<studio_core::GroupId>,
    /// A closed composition's block was clicked: the host selects its
    /// members, so the keyboard works on the composition as one thing.
    pub clicked_group: Option<studio_core::GroupId>,
    /// A clip was clicked (selection is the host's business).
    pub clicked: Option<ClipId>,
    /// A drag ended: move this clip here. An op-shaped fact, not a mutation.
    pub moved: Option<(ClipId, Ticks, TrackId)>,
    /// A trim drag ended: the clip's new full placement `(pos, src_in,
    /// len)`. A left trim changes all three, which is why this is not just
    /// a length.
    pub trimmed: Option<(ClipId, Ticks, Ticks, Ticks)>,
    /// The ruler was clicked or dragged: put the playhead here. Reported
    /// every frame OF the drag, not just at its end — scrubbing is only
    /// useful if it follows the finger.
    pub sought: Option<Ticks>,
    /// A rack pattern released over the tracks: (pattern index, snapped time, audio track).
    /// The library's rows are the drag source (`PatternDrag`); this is the drop.
    pub pattern_dropped: Option<(usize, Ticks, TrackId)>,
}

/// The payload a library pattern row carries while dragged. `len` is the pattern's length
/// at the rack tempo, so the ghost over the timeline is the size the clip will be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternDrag { pub index: usize, pub len: Ticks }

/// Where a pattern dropped at `at` lands: the time under the pointer, snapped to the same
/// edges a moved clip snaps to and clamped at zero; the track under the pointer if it is an
/// audio track, else the first audio track (a pattern is sound — it cannot land on a video
/// row, the same refusal `move_target` makes). None when the project has no audio track.
pub fn pattern_drop_target(project: &Project, view: &View, at: Pos2, origin: Pos2, playhead: Ticks) -> Option<(Ticks, TrackId)> {
    let rows = track_rows(project);
    let under = view.row_at(at.y, origin.y, rows.len()).map(|r| rows[r])
        .filter(|t| project.tracks.iter().any(|tr| tr.id == *t && tr.kind == TrackKind::Audio));
    let track = under.or_else(|| project.tracks.iter().find(|t| t.kind == TrackKind::Audio).map(|t| t.id))?;
    let raw = view.x_to_time(at.x, origin.x);
    let tolerance = Ticks((TICKS_PER_SECOND as f32 * 8.0 / view.px_per_sec) as i64);
    let t = snap(project, Ticks(raw.0.max(0)), playhead, tolerance, None);
    Some((Ticks(t.0.max(0)), track))
}

pub fn hit_test(project: &Project, view: &View, at: Pos2, origin: Pos2) -> Option<ClipId> {
    let rows = track_rows(project);
    let row = view.row_at(at.y, origin.y, rows.len())?;
    let track = rows[row];
    project
        .track_clips(track)
        .into_iter()
        .rev()
        .find(|c| view.clip_rect(c.pos, c.len, row, origin).contains(at))
        .map(|c| c.id)
}

/// Track ids in display order — video on top, audio below, mirroring both
/// kdenlive and Audacity so muscle memory transfers.
pub fn track_rows(project: &Project) -> Vec<TrackId> {
    let mut v: Vec<&studio_core::Track> = project.tracks.iter().collect();
    v.sort_by_key(|t| match t.kind { TrackKind::Video => 0, TrackKind::Audio => 1 });
    v.into_iter().map(|t| t.id).collect()
}

/// Snap a time to the nearest interesting edge within `tolerance` — clip
/// starts/ends and the playhead. Returns the snapped time, or the input when
/// nothing is near. Pure, and tested: snapping that fires at the wrong distance
/// is the single most irritating bug an NLE can have.
pub fn snap(project: &Project, t: Ticks, playhead: Ticks, tolerance: Ticks, ignore: Option<ClipId>) -> Ticks {
    let mut best: Option<(i64, Ticks)> = None;
    let mut consider = |edge: Ticks| {
        let d = (edge.0 - t.0).abs();
        if d <= tolerance.0 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, edge));
        }
    };
    for c in project.clips.values() {
        if Some(c.id) == ignore { continue; }
        consider(c.pos);
        consider(c.end());
    }
    consider(playhead);
    consider(Ticks::ZERO);
    best.map(|(_, e)| e).unwrap_or(t)
}

/// Draw the timeline. Returns the clip clicked this frame, if any.
#[allow(clippy::too_many_arguments)]
pub fn timeline(
    ui: &mut Ui,
    doc: &Doc,
    view: &mut View,
    colors: &PaletteColors,
    playhead: Ticks,
    selection: &[ClipId],
    // Peak pyramids per source. Sources without one draw as plain blocks, so a
    // clip whose audio has not been scanned yet is still editable — analysis
    // must never gate editing.
    peaks: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::Peaks>,
    // What was said in each source, in source time. Sources without one draw
    // no words — like peaks, a transcript must never gate editing.
    words: &std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>,
    // Decoded picture previews for video clips, and where the timeline lists
    // the ones it wanted but did not have -- like peaks, never gating editing.
    thumbs: &filmstrip::Thumbs,
    trim_side: TrimSide,
    // Compositions folded CLOSED on this device — view state, passed in
    // because open-or-closed is the workspace sidecar's business while
    // membership is the project's.
    closed_groups: &std::collections::BTreeSet<studio_core::GroupId>,
    // What mode/keyboard state the HUD in the corner reports. Passed in
    // rather than read from a shared cell: this function stays a pure
    // draw-and-report over its arguments, same as the rest of the file.
    mode: keys::Mode,
    input_mode: playkeys::InputMode,
    pending: &str,
    // The active song's tempo map, when the timeline is showing a song
    // that has one. `Some` switches the ruler from clock time to the
    // musical grid — bars and beats WHERE THE MAP PUTS THEM, spacing
    // stretching through every tempo change (`beatgrid`'s tests pin the
    // arithmetic). `None` keeps the seconds ruler: the whole-project view
    // has no single musical truth until the project/rack tempo
    // relationship is decided (C12-14e territory), and a wrong grid is
    // worse than a clock.
    tempo: Option<&studio_core::TempoMap>,
) -> TimelineResponse {
    // Closed compositions: which clips are folded away, and each fold's
    // hull. Computed FIRST so drawing (skip members, draw the block) and
    // click routing (a click on the block is the group's, not a member's)
    // agree on what is folded — one truth, two consumers.
    let mut folded: std::collections::BTreeSet<studio_core::ClipId> = Default::default();
    let mut hulls: Vec<(studio_core::GroupId, String, Ticks, Ticks, usize, usize)> = Vec::new();
    {
        let row_of = |t: studio_core::TrackId|
            doc.project.tracks.iter().position(|x| x.id == t).unwrap_or(0);
        for (gid, g) in &doc.project.groups {
            if !closed_groups.contains(gid) { continue; }
            let live: Vec<_> = g.clips.iter()
                .filter_map(|c| doc.project.clips.get(c)).collect();
            if live.is_empty() { continue; } // every member deleted: nothing folds
            let from = live.iter().map(|c| c.pos).min().unwrap();
            let to = live.iter().map(|c| c.end()).max().unwrap();
            let top = live.iter().map(|c| row_of(c.track)).min().unwrap();
            let bot = live.iter().map(|c| row_of(c.track)).max().unwrap();
            for c in &live { folded.insert(c.id); }
            hulls.push((*gid, g.name.clone(), from, to, top, bot));
        }
    }
    let rows = track_rows(&doc.project);
    // + the mode HUD's own strip under the last track (see the HUD below)
    let height = RULER_H + rows.len().max(1) as f32 * view.track_h + HUD_H;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    let origin = rect.min;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.bg_base);

    // ── markers, in the ruler ──────────────────────────────────────────────
    // They were in the project and reachable from the palette, and drawn
    // NOWHERE: the assembly's review markers, the constraint check's and the
    // vfx survey's were invisible on the timeline they were about (c20).
    // A tick down the ruler and the name beside it, coloured by what made
    // it; a name longer than the gap to the next marker is cut short.
    {
        let mut ms: Vec<&studio_core::Marker> = doc.project.markers.iter().collect();
        ms.sort_by_key(|m| m.at);
        for (i, m) in ms.iter().enumerate() {
            let x = view.time_to_x(m.at, origin.x);
            if x < origin.x + view.gutter || x > rect.max.x { continue; }
            let col = match marker_kind(&m.name) {
                MarkerKind::Review => colors.warning,
                MarkerKind::Constraint => colors.danger,
                MarkerKind::Vfx => colors.accent,
                MarkerKind::Plain => colors.text_weak,
            };
            painter.line_segment([Pos2::new(x, origin.y), Pos2::new(x, origin.y + RULER_H)], Stroke::new(2.0_f32, col));
            let room = ms.get(i + 1).map(|n| view.time_to_x(n.at, origin.x) - x - 6.0).unwrap_or(rect.max.x - x - 6.0);
            let chars = ((room / 6.0).floor() as usize).clamp(0, 48);
            if chars >= 4 {
                let label: String = if m.name.chars().count() > chars { format!("{}…", m.name.chars().take(chars - 1).collect::<String>()) } else { m.name.clone() };
                painter.text(Pos2::new(x + 3.0, origin.y + RULER_H - 2.0), Align2::LEFT_BOTTOM, label, FontId::proportional(10.0), col);
            }
        }
    }

    // ── ruler ──────────────────────────────────────────────────────────────
    if let Some(map) = tempo {
        // Musical ruler: bars labelled, beats as texture, positions from
        // the map. The visible span in ticks, from the same camera the
        // clips draw with, so grid and clips can never disagree.
        let from = view.scroll.max(Ticks::ZERO);
        let to = view.x_to_time(rect.max.x, origin.x);
        for t in beatgrid::beat_grid(map, from, to, view.px_per_sec, 4, 12.0) {
            let x = view.time_to_x(t.at, origin.x);
            if x < origin.x + view.gutter || x > rect.max.x { continue; }
            let (h, stroke_w) = match t.weight {
                beatgrid::Weight::Bar => (10.0, 1.2_f32),
                beatgrid::Weight::Beat => (5.0, 1.0_f32),
            };
            painter.line_segment(
                [Pos2::new(x, origin.y + RULER_H - h), Pos2::new(x, origin.y + RULER_H)],
                Stroke::new(stroke_w, colors.border));
            if let Some(bar) = t.bar {
                painter.text(Pos2::new(x + 3.0, origin.y + 3.0), Align2::LEFT_TOP,
                             format!("{bar}"), FontId::monospace(10.0), colors.text_weak);
            }
        }
    } else {
        // Clock ruler. Tick spacing is chosen so labels never collide: step
        // up through a 1/2/5 sequence until one second's worth of pixels
        // clears the label width.
        let mut step_s = 1.0f32;
        while step_s * view.px_per_sec < 64.0 {
            step_s *= if (step_s.log10().fract() - 0.0).abs() < 0.01 { 2.0 } else { 2.5 };
        }
        let first = (view.scroll.to_seconds() / step_s as f64).floor() as i64;
        for i in 0..256 {
            let s = (first + i) as f32 * step_s;
            let x = view.time_to_x(Ticks::from_seconds(s as f64), origin.x);
            if x > rect.max.x { break; }
            if x < origin.x + view.gutter { continue; }
            painter.line_segment([Pos2::new(x, origin.y + RULER_H - 6.0), Pos2::new(x, origin.y + RULER_H)],
                                 Stroke::new(1.0_f32, colors.border));
            painter.text(Pos2::new(x + 3.0, origin.y + 3.0), Align2::LEFT_TOP,
                         ruler_label(view, Ticks::from_seconds(s as f64)),
                         FontId::monospace(10.0), colors.text_weak);
        }
    }

    // ── tracks + clips ─────────────────────────────────────────────────────
    for (row, tid) in rows.iter().enumerate() {
        let track = doc.project.tracks.iter().find(|t| t.id == *tid).unwrap();
        let y0 = origin.y + RULER_H + row as f32 * view.track_h;
        let lane = Rect::from_min_max(Pos2::new(origin.x, y0),
                                      Pos2::new(rect.max.x, y0 + view.track_h));
        painter.rect_filled(lane, 0.0, if row % 2 == 0 { colors.bg_card } else { colors.bg_base });

        // header gutter: name, and the mute/lock state as colour rather than
        // icons so it reads at a glance on a phone-sized viewport
        let head = Rect::from_min_max(lane.min, Pos2::new(origin.x + view.gutter, lane.max.y));
        painter.rect_filled(head, 0.0, colors.bg_raised);
        let label = if track.muted { format!("{} (muted)", track.name) } else { track.name.clone() };
        painter.text(head.left_center() + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER,
                     label, FontId::proportional(12.0),
                     if track.locked { colors.text_weak } else { colors.text });

        for clip in doc.project.track_clips(*tid) {
            if folded.contains(&clip.id) { continue; } // drawn as its fold's block
            let r = view.clip_rect(clip.pos, clip.len, row, origin);
            if r.max.x < origin.x + view.gutter || r.min.x > rect.max.x { continue; }
            let selected = selection.contains(&clip.id);
            let base = match track.kind { TrackKind::Video => colors.accent, TrackKind::Audio => colors.success };
            painter.rect_filled(r, 3.0, base.gamma_multiply(if selected { 0.9 } else { 0.55 }));
            painter.rect_stroke(r, 3.0, Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 },
                if selected { colors.accent } else { colors.border }), egui::StrokeKind::Inside);
            // a clip somebody ELSE is inside gets a warning edge — this is the
            // whole point of the presence channel: stop the collision, don't
            // arbitrate it afterwards
            if !doc.peers_on(clip.id).is_empty() {
                painter.rect_stroke(r.expand(1.5), 4.0, Stroke::new(2.0_f32, colors.warning), egui::StrokeKind::Outside);
            }
            // picture previews along video clips: laid from the clip's own start,
            // only the visible ones, decoded elsewhere (filmstrip.rs)
            let has_video = track.kind == TrackKind::Video
                && doc.project.sources.get(&clip.source).map(|s| s.has_video).unwrap_or(false);
            if has_video && r.height() > 24.0 {
                let body = Rect::from_min_max(Pos2::new(r.min.x, r.min.y + 15.0), Pos2::new(r.max.x, r.max.y - 1.0));
                let tile_w = body.height() * 16.0 / 9.0;
                let vis_x0 = r.min.x.max(origin.x + view.gutter);
                let vis_x1 = r.max.x.min(rect.max.x);
                let clip_painter = painter.with_clip_rect(Rect::from_min_max(Pos2::new(vis_x0, body.min.y), Pos2::new(vis_x1, body.max.y)));
                for t in filmstrip::tiles(r.min.x, r.max.x, vis_x0, vis_x1, clip.src_in, clip.len, view.px_per_sec, tile_w) {
                    let tr = Rect::from_min_size(Pos2::new(t.x, body.min.y), Vec2::new(t.w, body.height()));
                    match thumbs.tex.get(&(clip.source, t.ms)) {
                        Some(id) => {
                            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new((t.w / tile_w).min(1.0), 1.0));
                            clip_painter.image(*id, tr, uv, egui::Color32::from_white_alpha(if selected { 235 } else { 200 }));
                        }
                        None => thumbs.wanted.borrow_mut().push((clip.source, t.ms)),
                    }
                    clip_painter.line_segment([tr.left_top(), tr.left_bottom()], Stroke::new(1.0_f32, colors.bg_base.gamma_multiply(0.6)));
                }
            }
            // waveform, from the peak pyramid — one column per pixel, so the
            // cost of drawing a clip is its WIDTH, not its duration. This is
            // the whole reason Audacity's summary blocks are worth copying:
            // a ten-minute clip and a ten-second one draw at the same speed.
            if let Some(peaks) = peaks.get(&clip.source) {
                let vis = r.intersect(Rect::from_min_max(
                    Pos2::new(origin.x + view.gutter, r.min.y), r.max));
                let cols = vis.width().floor() as usize;
                if cols > 1 && vis.width() > 0.0 {
                    // the rate the PEAKS were built at, not the one the project
                    // records for the source -- they can disagree (the cartoon
                    // fixture recorded 48 kHz over 16 kHz audio)
                    let rate = if peaks.rate > 0 { peaks.rate }
                        else { doc.project.sources.get(&clip.source).map(|s| s.sample_rate).unwrap_or(48_000) };
                    // sample range of the VISIBLE part of the clip
                    let skip = Ticks(((vis.min.x - r.min.x) / view.px_per_sec
                        * TICKS_PER_SECOND as f32) as i64);
                    let s0 = clip.src_in.saturating_add(skip).to_samples(rate) as usize;
                    let s1 = s0 + Ticks(((vis.width() / view.px_per_sec)
                        * TICKS_PER_SECOND as f32) as i64).to_samples(rate) as usize;
                    let (mid, half) = if has_video && r.height() > 24.0 {
                        let band = Rect::from_min_max(Pos2::new(vis.min.x, vis.center().y + 2.0), Pos2::new(vis.max.x, vis.max.y - 13.0));
                        painter.rect_filled(band, 0.0, colors.bg_base.gamma_multiply(0.55));
                        (band.center().y, (band.height() * 0.5 - 1.0).max(1.0))
                    } else { (vis.center().y, (vis.height() * 0.5 - 3.0).max(1.0)) };
                    let wave = colors.text.gamma_multiply(0.55);
                    for (i, mm) in peaks.window(s0, s1, cols).iter().enumerate() {
                        let x = vis.min.x + i as f32;
                        let (lo, hi) = (mm.min.clamp(-1.0, 1.0), mm.max.clamp(-1.0, 1.0));
                        painter.line_segment(
                            [Pos2::new(x, mid - hi * half), Pos2::new(x, mid - lo * half)],
                            Stroke::new(1.0_f32, wave));
                    }
                }
            }
            // Fades, drawn the way every editor draws them: the part the fade
            // takes away shaded, and the ramp on top. They were in the model
            // and in the render and invisible here -- `[` made a fade and the
            // clip looked untouched (seen under Xvfb, c20).
            if let Some((fi, fo)) = doc.project.fades.get(&clip.id) {
                let px = |t: Ticks| t.to_seconds() as f32 * view.px_per_sec;
                let fade_painter = painter.with_clip_rect(r.intersect(Rect::from_min_max(
                    Pos2::new(origin.x + view.gutter, r.min.y), Pos2::new(rect.max.x, r.max.y))));
                for (tri, ramp) in fade_triangles(r, px(*fi), px(*fo)) {
                    fade_painter.add(egui::Shape::convex_polygon(tri.to_vec(), colors.bg_base.gamma_multiply(0.55), Stroke::NONE));
                    fade_painter.line_segment(ramp, Stroke::new(1.5_f32, colors.text));
                }
            }
            // What is being said, along the clip's bottom edge — the gutter
            // between this lane and the next. Laid out by
            // `studio_voice::gutter_words`, so zoomed out it is an occasional
            // word and zoomed in the whole line, and labels never collide.
            if let Some(segs) = words.get(&clip.source) {
                const CHAR_PX: f32 = 6.2;
                let strip_y = r.max.y - 12.0;
                if r.height() > 30.0 {
                    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, strip_y - 1.0), r.max),
                                        0.0, colors.bg_base.gamma_multiply(0.75));
                }
                for (dx, text) in studio_voice::gutter_words(segs, clip.src_in, clip.len,
                                                              view.px_per_sec, CHAR_PX, 48.0) {
                    let x = r.min.x + dx + 3.0;
                    if x < origin.x + view.gutter || x > rect.max.x { continue; }
                    painter.text(Pos2::new(x, strip_y), Align2::LEFT_TOP, text,
                                 FontId::proportional(10.0), colors.text_weak);
                }
            }
            if r.width() > 34.0 {
                // a pattern clip names its pattern ("Trap A"): it has no file, and
                // labelled by path it showed no name at all
                let name = doc.project.sources.get(&clip.source)
                    .map(|s| if s.is_generator() { s.display_name().to_string() }
                             else { s.path.rsplit('/').next().unwrap_or("clip").to_string() })
                    .unwrap_or_else(|| "clip".into());
                // pinned to the VISIBLE left edge: drawn at the clip's true edge, a
                // clip scrolled off the left showed a cut-off fragment of its name
                // behind the track header
                let name_x = r.min.x.max(origin.x + view.gutter);
                painter.text(Pos2::new(name_x, r.min.y) + Vec2::new(4.0, 3.0), Align2::LEFT_TOP,
                             name, FontId::proportional(11.0), colors.text);
            }
        }
    }

    // ── presence: other people's playheads, drawn ABOVE the clips ──────────
    for peer in doc.peers.values() {
        if peer.site == doc.site { continue; }
        let x = view.time_to_x(peer.playhead, origin.x);
        if x < origin.x + view.gutter || x > rect.max.x { continue; }
        let tint = peer_color(peer.site.0);
        painter.line_segment([Pos2::new(x, origin.y + RULER_H), Pos2::new(x, hud_band(rect).top())],
                             Stroke::new(1.5_f32, tint));
        // the name rides at the top so several peers stay readable
        // A live take is said in the label itself: the one thing a
        // collaborator must not do to a singer mid-take is move her.
        let label = if peer.recording { format!("{} ● rec", peer.name) } else { peer.name.clone() };
        painter.text(Pos2::new(x + 3.0, origin.y + RULER_H + 1.0), Align2::LEFT_TOP,
                     label, FontId::proportional(10.0), tint);
    }

    // ── our own playhead, last, so it is never hidden by a peer's ──────────
    let px = view.time_to_x(playhead, origin.x);
    if px >= origin.x + view.gutter {
        // down to the last track, not through the HUD strip under it
        painter.line_segment([Pos2::new(px, origin.y), Pos2::new(px, hud_band(rect).top())],
                             Stroke::new(2.0_f32, colors.danger));
    }

    // ── wheel: pan; ctrl-wheel / pinch: zoom about the pointer ─────────────
    // Hover-gated so a scrolling page elsewhere never moves the timeline.
    if resp.hovered() {
        let (scroll, zoom, pointer_x) = ui.input(|i| {
            (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos().map(|p| p.x))
        });
        if zoom != 1.0 {
            if let Some(x) = pointer_x {
                view.zoom_about(zoom, x, origin.x);
            }
        } else if scroll != Vec2::ZERO {
            // A mouse wheel reports y; a trackpad reports both. Either way
            // the timeline is a horizontal thing, so both axes pan time —
            // wheel-down meaning "later" matches every horizontal list.
            view.pan(-(scroll.x + scroll.y));
        }
    }

    // ── drag a clip: move from the body, trim from an edge ─────────────────
    let mut out = TimelineResponse::default();
    // A pattern from the library over the tracks: ghost while hovering, place on release.
    if let Some(drag) = resp.dnd_hover_payload::<PatternDrag>() {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            if let Some((t, track)) = pattern_drop_target(&doc.project, view, p, origin, playhead) {
                let rows = track_rows(&doc.project);
                if let Some(row) = rows.iter().position(|r| *r == track) {
                    let ghost = view.clip_rect(t, drag.len, row, origin).intersect(rect);
                    painter.rect_filled(ghost, 3.0, colors.accent.gamma_multiply(0.35));
                    painter.rect_stroke(ghost, 3.0, Stroke::new(1.0_f32, colors.accent), egui::StrokeKind::Inside);
                }
            }
        }
    }
    if let Some(drag) = resp.dnd_release_payload::<PatternDrag>() {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()).or_else(|| resp.interact_pointer_pos()) {
            if let Some((t, track)) = pattern_drop_target(&doc.project, view, p, origin, playhead) {
                out.pattern_dropped = Some((drag.index, t, track));
            }
        }
    }
    // A drag that starts on empty space pans time, the way a finger moves any horizontal strip; before this
    // only the wheel did, so on a touchscreen the timeline could not be scrolled at all.
    let pan_id = resp.id.with("empty-space-pan");
    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            view.drag = grab(&doc.project, view, p, origin, trim_side);
            let pans = pans_on_press(p, rect, view.drag.is_some());
            ui.data_mut(|d| d.insert_temp(pan_id, pans));
        }
    }
    if resp.dragged() && ui.data(|d| d.get_temp::<bool>(pan_id)).unwrap_or(false) {
        view.pan(-resp.drag_delta().x);
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.insert_temp(pan_id, false));
    }
    if let Some(drag) = view.drag {
        // The clip can vanish mid-drag — a peer's RemoveClip is not obliged
        // to wait for our finger. A dead drag just ends.
        if !doc.project.clips.contains_key(&drag.id) {
            view.drag = None;
        } else if let Some(p) = resp.interact_pointer_pos() {
            // Ghost or drop, per kind — the project is not touched until the
            // release becomes an op.
            let ghost = |pos: Ticks, len: Ticks, track: TrackId| {
                track_rows(&doc.project).iter().position(|t| *t == track)
                    .map(|row| view.clip_rect(pos, len, row, origin))
            };
            match drag.kind {
                DragKind::Move { .. } => {
                    if let Some((pos, track)) = move_target(&doc.project, view, &drag, p, origin, playhead) {
                        if resp.drag_stopped() {
                            out.moved = Some((drag.id, pos, track));
                        } else if let Some(r) = ghost(pos, doc.project.clips[&drag.id].len, track) {
                            painter.rect_stroke(r, 3.0, Stroke::new(2.0_f32, colors.accent),
                                                egui::StrokeKind::Inside);
                        }
                    }
                }
                DragKind::TrimLeft | DragKind::TrimRight => {
                    if let Some((pos, src_in, len)) = trim_target(&doc.project, view, &drag, p, origin, playhead) {
                        if resp.drag_stopped() {
                            out.trimmed = Some((drag.id, pos, src_in, len));
                        } else if let Some(r) = ghost(pos, len, doc.project.clips[&drag.id].track) {
                            painter.rect_stroke(r, 3.0, Stroke::new(2.0_f32, colors.warning),
                                                egui::StrokeKind::Inside);
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            view.drag = None;
        }
    }
    // Fold blocks over the hulls, chevron first — kdenlive's pre-batched
    // group, After Effects' precomp, drawn as one openable thing. Open
    // groups get only the ▾ tab above their first member, so an unfolded
    // composition costs almost no pixels.
    let mut fold_rects: Vec<(studio_core::GroupId, Rect, Rect)> = Vec::new();
    for (gid, name, from, to, top, bot) in &hulls {
        let a = view.clip_rect(*from, Ticks(to.0 - from.0), *top, origin);
        let b = view.clip_rect(*from, Ticks(to.0 - from.0), *bot, origin);
        let r = a.union(b);
        if r.max.x < origin.x + view.gutter || r.min.x > rect.max.x { continue; }
        painter.rect_filled(r, 4.0, colors.bg_raised.gamma_multiply(0.95));
        painter.rect_stroke(r, 4.0, Stroke::new(1.5_f32, colors.accent), egui::StrokeKind::Inside);
        let chev = Rect::from_min_size(r.min, egui::vec2(18.0, r.height().min(18.0)));
        painter.text(chev.center(), egui::Align2::CENTER_CENTER, "▸",
                     egui::FontId::proportional(12.0), colors.accent);
        painter.text(Pos2::new(chev.max.x + 2.0, r.min.y + 9.0),
                     egui::Align2::LEFT_CENTER, name,
                     egui::FontId::proportional(11.0), colors.text);
        fold_rects.push((*gid, r, chev));
    }
    for (gid, g) in &doc.project.groups {
        if closed_groups.contains(gid) { continue; }
        let live: Vec<_> = g.clips.iter().filter_map(|c| doc.project.clips.get(c)).collect();
        let Some(first) = live.iter().min_by_key(|c| c.pos) else { continue };
        let row = doc.project.tracks.iter().position(|t| t.id == first.track).unwrap_or(0);
        let r = view.clip_rect(first.pos, first.len, row, origin);
        let tab = Rect::from_min_size(Pos2::new(r.min.x, r.min.y - 12.0),
                                      egui::vec2(60.0, 12.0));
        painter.rect_filled(tab, 2.0, colors.bg_raised);
        painter.text(tab.left_center() + egui::vec2(2.0, 0.0), egui::Align2::LEFT_CENTER,
                     format!("▾ {}", g.name), egui::FontId::proportional(9.0), colors.text_weak);
        fold_rects.push((*gid, Rect::NOTHING, tab));
    }
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.clicked() {
            for (gid, body, chev) in &fold_rects {
                if chev.contains(p) { out.toggled_group = Some(*gid); break; }
                if body.contains(p) { out.clicked_group = Some(*gid); break; }
            }
        }
    }
    // ── the ruler seeks ────────────────────────────────────────────────
    // Until now the playhead could only be moved by `g g` / `g e`, which
    // is invisible: a timeline whose ruler does nothing gives a new user
    // no way to get back to the start at all. The ruler band takes clicks
    // AND drags, reporting every frame so scrubbing follows the finger,
    // and clamps at zero because a negative playhead is not a position.
    let ruler_band = Rect::from_min_max(
        Pos2::new(rect.left() + view.gutter, rect.top()),
        Pos2::new(rect.right(), rect.top() + RULER_H));
    if let Some(p) = resp.interact_pointer_pos() {
        if (resp.clicked() || resp.dragged()) && ruler_band.contains(p) {
            let t = view.x_to_time(p.x, origin.x);
            out.sought = Some(Ticks(t.0.max(0)));
        }
    }
    out.clicked = if out.sought.is_some()
        || out.toggled_group.is_some() || out.clicked_group.is_some() {
        None // the fold owns this click; a folded member must not be reachable
    } else {
        resp.interact_pointer_pos()
            .filter(|_| resp.clicked())
            .and_then(|p| hit_test(&doc.project, view, p, origin))
            .filter(|id| !folded.contains(id))
    };
    // The tooltip is where the tools become learnable: a shortcut nobody
    // can see is a shortcut nobody uses — across every editor ever
    // shipped. Hovering a clip shows its name and the alignment tools;
    // the LIST is a public const so the shell's key handler can pin, by
    // test, that every key it intercepts is written here — the tooltip
    // and the behavior cannot drift apart.
    if let Some(p) = resp.hover_pos() {
        if let Some(id) = hit_test(&doc.project, view, p, origin) {
            let name = doc.project.clips.get(&id)
                .and_then(|c| doc.project.sources.get(&c.source))
                .map(|s| s.path.clone()).unwrap_or_default();
            resp.clone().on_hover_ui(|ui| {
                ui.strong(name);
                for h in CLIP_HINTS { ui.weak(*h); }
            });
        }
    }
    // ── mode HUD ───────────────────────────────────────────────────────────
    // Playing mode replaces the whole keymap (see playkeys' module docs), so
    // the HUD's hints must too — showing Normal-mode Helix hints while `d`
    // is actually a piano note would be actively misleading. Bottom-left, on
    // the timeline itself rather than the top bar: it is about what the
    // NEXT keypress does here, so it belongs where the eyes already are
    // when a hand freezes over the keyboard mid-edit, not in a strip that
    // could be four scroll-lengths away in the workbench layout.
    {
        let label = if input_mode == playkeys::InputMode::Playing {
            "playing".to_string()
        } else if pending.is_empty() {
            mode.label().to_string()
        } else {
            format!("{} {pending}", mode.label())
        };
        let hints: &[&str] = if input_mode == playkeys::InputMode::Playing {
            &["any key plays a note", "C-space voice"]
        } else {
            mode.hints()
        };
        // ONE LINE in a strip of its own under the last track. It was a box
        // pinned to the bottom-left of the tracks, which is the track
        // headers: with three tracks it hid the second's name and the whole
        // of the third's header (seen under Xvfb, c20). Here it covers
        // nothing, and is where it always was relative to the tracks.
        let band = hud_band(rect);
        painter.rect_filled(band, 0.0, colors.bg_raised.gamma_multiply(0.6));
        let font_label = egui::FontId::proportional(11.0);
        let font_hint = egui::FontId::monospace(10.0);
        let label_gal = painter.layout_no_wrap(label.clone(), font_label, colors.accent);
        let mut x = band.left() + 8.0;
        painter.galley(Pos2::new(x, band.center().y - label_gal.rect.height() * 0.5), label_gal.clone(), colors.accent);
        x += label_gal.rect.width() + 12.0;
        for h in hints {
            let g = painter.layout_no_wrap((*h).to_string(), font_hint.clone(), colors.text_weak);
            // drop what does not fit rather than clip a hint mid-word
            if x + g.rect.width() > band.right() - 4.0 { break; }
            painter.galley(Pos2::new(x, band.center().y - g.rect.height() * 0.5), g.clone(), colors.text_weak);
            x += g.rect.width() + 14.0;
        }
    }
    out
}

/// The shaded triangle and the ramp line for each of a clip's fades on its
/// rect `r`, given the fade lengths in pixels (clamped to the clip). The
/// shaded part is what the fade takes away: the top-left corner for a fade
/// in, cut by the ramp from the bottom-left up to where the fade ends; the
/// top-right corner for a fade out, mirrored. Under a pixel: nothing drawn.
pub fn fade_triangles(r: Rect, in_px: f32, out_px: f32) -> Vec<([Pos2; 3], [Pos2; 2])> {
    let mut v = Vec::new();
    let w = r.width();
    let (fi, fo) = (in_px.clamp(0.0, w), out_px.clamp(0.0, w));
    if fi >= 1.0 {
        let top = Pos2::new(r.left() + fi, r.top());
        v.push(([r.left_top(), top, r.left_bottom()], [r.left_bottom(), top]));
    }
    if fo >= 1.0 {
        let top = Pos2::new(r.right() - fo, r.top());
        v.push(([top, r.right_top(), r.right_bottom()], [top, r.right_bottom()]));
    }
    v
}

/// What made a marker, by its name's prefix -- the prefixes the tools write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind { Review, Constraint, Vfx, Plain }

pub fn marker_kind(name: &str) -> MarkerKind {
    if name.starts_with("review ") { MarkerKind::Review }
    else if name.starts_with("constraint: ") { MarkerKind::Constraint }
    else if name.starts_with("vfx: ") { MarkerKind::Vfx }
    else { MarkerKind::Plain }
}

/// Height of the mode HUD's strip under the last track.
pub const HUD_H: f32 = 22.0;

/// Where the mode HUD draws inside the timeline's rect: the strip below the
/// tracks, which the timeline allocates for it and nothing else uses.
pub fn hud_band(rect: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - HUD_H), rect.max)
}

/// What hovering a clip teaches. One source of truth: the shell's key
/// handler tests that every alignment key it intercepts appears here.
pub const CLIP_HINTS: &[&str] = &[
    "drag: move · edge: trim (alt-; flips the grabbed side)",
    "A: align 2nd selected clip to 1st (vocal double → lead)",
    "Q: quantize selected clip starts to the beat grid",
    ", / . : nudge selected by one step · < / > : by 1 ms",
    "g: group selection into a composition · G: ungroup",
    "D: duplicate selection after itself · n: normalize to -1 dBFS",
    "[ / ] : fade in to / out from the playhead (over a lower video track: a dissolve)",
    "= / - : clip gain · m: mute track · l: lock track",
    "C-r: record a take from the microphone at the playhead",
];

/// A stable, well-separated colour per site id. Deterministic so the same peer
/// is the same colour on every screen in the session — a peer who changes
/// colour between machines is worse than no colour at all.
pub fn peer_color(site: u64) -> Color32 {
    const WHEEL: [(u8, u8, u8); 6] = [
        (232, 122, 90), (108, 190, 230), (140, 210, 120),
        (220, 180, 90), (190, 140, 230), (230, 130, 180),
    ];
    let (r, g, b) = WHEEL[(site as usize) % WHEEL.len()];
    Color32::from_rgb(r, g, b)
}

/// `H:MM:SS.mmm`, the form both source editors show.
/// What the ruler writes at a tick: a frame, or a duration.
///
/// The choice is the view's, not this function's — a music session and a video edit want
/// different objects at the same position, and picking one globally would make the other's
/// ruler wrong. Drop-frame labels come out with a semicolon, which is the convention that
/// tells an editor at a glance which counting is in use.
pub fn ruler_label(view: &View, t: Ticks) -> String {
    match view.timecode_rate {
        Some((rate, drop)) => {
            let frame = (t.to_seconds() * rate.exact()).round() as i64;
            studio_video::timecode::Timecode::from_frames(frame, rate, drop).to_string()
        }
        None => format_timecode(t),
    }
}

pub fn format_timecode(t: Ticks) -> String {
    let total_ms = (t.0 as f64 / TICKS_PER_SECOND as f64 * 1000.0).round() as i64;
    let (neg, total_ms) = (total_ms < 0, total_ms.abs());
    let (h, m, s, ms) = (total_ms / 3_600_000, total_ms / 60_000 % 60,
                         total_ms / 1000 % 60, total_ms % 1000);
    format!("{}{h}:{m:02}:{s:02}.{ms:03}", if neg { "-" } else { "" })
}

#[cfg(test)]
mod seek_tests {
    use super::*;

    /// The ruler band's own arithmetic, stated once so the widget and this
    /// test cannot disagree: the strip is RULER_H tall, starts after the
    /// gutter, and maps x to time through the same view transform every
    /// clip uses.
    fn seek_at(view: &View, origin: Pos2, x: f32, y: f32) -> Option<Ticks> {
        let in_band = y >= origin.y && y < origin.y + RULER_H
            && x >= origin.x + view.gutter;
        if !in_band { return None; }
        Some(Ticks(view.x_to_time(x, origin.x).0.max(0)))
    }

    #[test]
    fn the_ruler_seeks_and_the_tracks_below_it_do_not() {
        let view = View::default();
        let origin = Pos2::new(10.0, 20.0);
        // A click 300px into the ruler lands where the ruler's own label
        // arithmetic says it should — same transform, so they agree by
        // construction rather than by tuning.
        let x = origin.x + view.gutter + 300.0;
        let t = seek_at(&view, origin, x, origin.y + 4.0).expect("the band seeks");
        assert_eq!(t, view.x_to_time(x, origin.x));
        // Below the band is clip territory: a click there must never move
        // the playhead, or selecting a clip would scrub.
        assert!(seek_at(&view, origin, x, origin.y + RULER_H + 1.0).is_none());
        // Left of the gutter is the track-name column, not time.
        assert!(seek_at(&view, origin, origin.x + 2.0, origin.y + 4.0).is_none());
        // Scrolled back past zero, a seek clamps: a negative playhead is
        // not a position.
        let mut scrolled = View::default();
        scrolled.scroll = Ticks::ZERO;
        let left_edge = scrolled.gutter + origin.x;
        assert_eq!(seek_at(&scrolled, origin, left_edge, origin.y + 2.0), Some(Ticks(0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_collab::{Presence, SiteId};
    use studio_core::{Clip, Op, Source, SourceId, Track};

    fn origin() -> Pos2 { Pos2::new(0.0, 0.0) }

    fn project() -> Project {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "/media/take1.mov".into(),
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(10), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(2.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(4.0),
            gain: 1.0, effects: Vec::new() } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(20), source: SourceId(1), track: TrackId(2),
            pos: Ticks::from_seconds(1.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(3.0),
            gain: 1.0, effects: Vec::new() } }).unwrap();
        p
    }

    #[test]
    fn time_and_pixels_round_trip() {
        // If these two disagree, the ruler and the playhead point at different
        // frames and nothing else in the editor can be trusted.
        let v = View::default();
        for secs in [0.0, 0.5, 1.0, 12.5, 3600.0] {
            let t = Ticks::from_seconds(secs);
            let back = v.x_to_time(v.time_to_x(t, 0.0), 0.0);
            assert!((back.to_seconds() - secs).abs() < 0.001, "{secs}s round-tripped to {}", back.to_seconds());
        }
    }

    #[test]
    fn zooming_keeps_the_frame_under_the_cursor_still() {
        // The behaviour every editor has and every reimplementation forgets.
        let mut v = View::default();
        let anchor_x = 500.0;
        let before = v.x_to_time(anchor_x, 0.0);
        v.zoom_about(2.5, anchor_x, 0.0);
        let after = v.x_to_time(anchor_x, 0.0);
        assert!((after.to_seconds() - before.to_seconds()).abs() < 0.01,
                "the anchored time moved: {} -> {}", before.to_seconds(), after.to_seconds());
        v.zoom_about(0.2, anchor_x, 0.0);
        let back = v.x_to_time(anchor_x, 0.0);
        assert!((back.to_seconds() - before.to_seconds()).abs() < 0.01, "and zooming out holds too");
        // zoom is clamped, so a wild gesture cannot produce a degenerate view
        for _ in 0..80 { v.zoom_about(4.0, anchor_x, 0.0); }
        assert!(v.px_per_sec <= 20_000.0 && v.px_per_sec > 0.0);
    }

    #[test]
    fn hit_testing_finds_the_clip_under_the_pointer_and_nothing_else() {
        let p = project();
        let v = View::default();
        let rows = track_rows(&p);
        assert_eq!(rows, vec![TrackId(1), TrackId(2)], "video sits above audio");

        // inside the video clip (2s..6s on row 0)
        let inside = Pos2::new(v.time_to_x(Ticks::from_seconds(3.0), 0.0),
                               RULER_H + v.track_h * 0.5);
        assert_eq!(hit_test(&p, &v, inside, origin()), Some(ClipId(10)));

        // same x, but the audio row — a different clip, which is exactly the
        // confusion a row-agnostic hit test would produce
        let audio_row = Pos2::new(v.time_to_x(Ticks::from_seconds(3.0), 0.0),
                                  RULER_H + v.track_h * 1.5);
        assert_eq!(hit_test(&p, &v, audio_row, origin()), Some(ClipId(20)));

        // a gap on the video row
        let gap = Pos2::new(v.time_to_x(Ticks::from_seconds(30.0), 0.0),
                            RULER_H + v.track_h * 0.5);
        assert_eq!(hit_test(&p, &v, gap, origin()), None);

        // in the ruler, above every track
        let ruler = Pos2::new(inside.x, 4.0);
        assert_eq!(hit_test(&p, &v, ruler, origin()), None);
    }

    #[test]
    fn snapping_prefers_the_nearest_edge_and_leaves_distant_times_alone() {
        let p = project();
        let tol = Ticks::from_seconds(0.25);
        // just past the video clip's start (2.0s) → snaps back to it
        let near = Ticks::from_seconds(2.1);
        assert_eq!(snap(&p, near, Ticks::ZERO, tol, None), Ticks::from_seconds(2.0));
        // near its end (6.0s)
        assert_eq!(snap(&p, Ticks::from_seconds(5.9), Ticks::ZERO, tol, None), Ticks::from_seconds(6.0));
        // nothing within tolerance → unchanged, so dragging in open space is free
        let far = Ticks::from_seconds(40.0);
        assert_eq!(snap(&p, far, Ticks::ZERO, tol, None), far);
        // the playhead is a snap target too
        assert_eq!(snap(&p, Ticks::from_seconds(20.05), Ticks::from_seconds(20.0), tol, None),
                   Ticks::from_seconds(20.0));
        // a clip being dragged must not snap to ITSELF
        assert_eq!(snap(&p, near, Ticks::ZERO, tol, Some(ClipId(10))), near);
    }

    #[test]
    fn a_peer_colour_is_stable_and_peers_are_told_apart() {
        // Same peer, same colour, on every machine in the session.
        assert_eq!(peer_color(3), peer_color(3));
        assert_ne!(peer_color(1), peer_color(2));
    }

    #[test]
    fn presence_marks_the_clip_a_peer_is_working_in() {
        // The overlay's whole job: warn BEFORE two people collide.
        let mut doc = Doc::new(SiteId(1), project());
        assert!(doc.peers_on(ClipId(10)).is_empty());
        doc.presence(Presence { site: SiteId(2), name: "ada".into(),
            playhead: Ticks::from_seconds(3.0), selection: vec![ClipId(10)],
            active_track: Some(TrackId(1)), seq: 1, recording: false,
            following: None, view_mode: Default::default() });
        assert_eq!(doc.peers_on(ClipId(10)).len(), 1, "the peer is inside the 2s..6s clip");
        assert!(doc.peers_on(ClipId(20)).is_empty(), "and not inside the other one");
    }

    /// A duration is the WRONG OBJECT for a video ruler. `01:00:00:12` names a frame you can
    /// cut on — what the camera stamped and what the shot list refers to — and
    /// `studio_video::timecode` has done the arithmetic, drop-frame included, with no caller
    /// since it was written.
    #[test]
    fn the_ruler_names_frames_when_the_view_is_a_video_project() {
        use studio_video::timecode::Rate;
        let mut v = View::default();
        // Music session: unchanged, because a bar is not a frame and rounding a beat to
        // 1/25s would misplace it.
        assert_eq!(ruler_label(&v, Ticks::from_seconds(61.5)), format_timecode(Ticks::from_seconds(61.5)));
        assert!(ruler_label(&v, Ticks::from_seconds(61.5)).contains('.'));

        // Video: frames, colon-separated.
        v.timecode_rate = Some((Rate::Fps25, false));
        assert_eq!(ruler_label(&v, Ticks::from_seconds(0.0)), "00:00:00:00");
        assert_eq!(ruler_label(&v, Ticks::from_seconds(1.0)), "00:00:01:00");
        // 61.5s at 25fps is frame 1537.5, which ROUNDS to 1538 -> 00:01:01:13. I wrote :12
        // first by truncating in my head; the label names the nearest frame, because a ruler
        // tick that fell between two frames has to pick one and the nearer is the honest
        // choice.
        assert_eq!(ruler_label(&v, Ticks::from_seconds(61.5)), "00:01:01:13");

        // Drop-frame prints a SEMICOLON, the convention that tells an editor at a glance
        // which counting is in use — the two disagree by 3.6 seconds an hour.
        v.timecode_rate = Some((Rate::Fps29_97, true));
        let df = ruler_label(&v, Ticks::from_seconds(3600.0));
        assert!(df.contains(';'), "drop-frame should be marked: {df}");
        v.timecode_rate = Some((Rate::Fps29_97, false));
        let ndf = ruler_label(&v, Ticks::from_seconds(3600.0));
        assert!(!ndf.contains(';'), "{ndf}");
        assert_ne!(df, ndf, "drop and non-drop must not label an hour the same");
    }

    #[test]
    fn timecode_reads_like_an_editor_expects() {
        assert_eq!(format_timecode(Ticks::ZERO), "0:00:00.000");
        assert_eq!(format_timecode(Ticks::from_seconds(61.5)), "0:01:01.500");
        assert_eq!(format_timecode(Ticks::from_seconds(3661.25)), "1:01:01.250");
        assert_eq!(format_timecode(Ticks::from_seconds(-2.0)), "-0:00:02.000");
    }
}

/// Drive the real widget with real pointer events.
///
/// Every other test in this file holds the PURE half still — `grab`,
/// `move_target`, `snap` — and they all pass while the field report says a
/// dragged clip does not land. That is the gap those tests cannot see: the
/// lifecycle between them (does `drag_started` fire, is `interact_pointer_pos`
/// still valid on the release frame, does `drag_stopped` reach the arm that
/// fills `moved`) lives in egui, not in this crate's arithmetic.
///
/// # Status: the harness does not work yet, and that is not evidence
///
/// The drag test is `#[ignore]`d. Synthetic frames get as far as a held button
/// with 400px of travel and `view.drag` still `None`, which means
/// `resp.drag_started()` never fires. **This says nothing about the reported
/// bug** — a simulated pointer that fails to start a drag is the likeliest
/// explanation, and reading it as a reproduction would be exactly the mistake
/// of trusting a red result as much as a green one.
///
/// What was ruled out along the way, so the next attempt does not redo it:
///
/// * **Geometry.** `grab()` called directly at the same coordinates the test
///   aims at returns `Some(ClipDrag { kind: Move, .. })`. The aim is right.
/// * **Panel margins.** A `CentralPanel` insets its contents, so the widget's
///   origin is not `(0, 0)` and every coordinate computed against `origin_x =
///   0.0` lands a few pixels off. Fixed with `Frame::NONE`; the symptom did
///   not change.
/// * **Pointer state.** Instrumented per frame: press registers
///   (`press_origin = Some`), the button stays down, and the pointer reaches
///   400px away — well past egui's 6px `max_click_dist`.
/// * **`interact_pointer_pos` on release.** egui 0.34 sets it when
///   `drag_stopped()`, with a comment saying so, so the release frame is not
///   where a position is lost.
///
/// What is left to check: `Interaction::potential_drag_id` is captured from
/// `hits.drag` on the Pressed event, so if the widget is not in the PREVIOUS
/// frame's registered rects at that moment, the drag can never start no matter
/// how far the pointer travels. That is a plausible harness fault (frame
/// ordering) and a plausible product fault (the timeline re-allocating under a
/// different id between frames), and telling them apart is the next step.
/// `egui_kittest` exists for precisely this and is probably the shorter road
/// than hand-rolling the frames.
/// Whether a press at `p` on the timeline starts a pan: not on a clip (that is a move or a trim), not in
/// the ruler band (that seeks), and not in the track-name gutter.
fn pans_on_press(p: Pos2, rect: Rect, grabbed_clip: bool) -> bool {
    !grabbed_clip && p.y > rect.top() + RULER_H && rect.contains(p)
}

#[cfg(test)]
mod pan_tests {
    use super::*;

    #[test]
    fn empty_space_pans_but_a_clip_the_ruler_and_the_outside_do_not() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(400.0, 300.0));
        assert!(pans_on_press(Pos2::new(200.0, RULER_H + 40.0), rect, false), "empty lane");
        assert!(!pans_on_press(Pos2::new(200.0, RULER_H + 40.0), rect, true), "a clip is dragged, not the view");
        assert!(!pans_on_press(Pos2::new(200.0, RULER_H - 4.0), rect, false), "the ruler seeks");
        assert!(!pans_on_press(Pos2::new(500.0, RULER_H + 40.0), rect, false), "outside the timeline");
    }
}

#[cfg(test)]
mod drag_lifecycle_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId};

    fn doc() -> Doc {
        let mut p = Project::default();
        for (i, (kind, nm)) in [(studio_core::TrackKind::Video, "V1"),
                                (studio_core::TrackKind::Audio, "A1"),
                                (studio_core::TrackKind::Audio, "A2")].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: (*nm).into(), muted: false, locked: false, gain: 1.0, route: None },
                at: i }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        // Audio clip on A1 (row 1), 3s..8s — the same shape the pure tests use.
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(20), source: SourceId(1),
            track: TrackId(2), pos: Ticks::from_seconds(3.0), src_in: Ticks::ZERO,
            len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
        Doc::new(studio_collab::SiteId(1), p)
    }

    /// One timeline frame with the pointer in a given state. Returns what the
    /// widget reported and where the frame put its origin, so a caller can aim
    /// the next event at a real clip.
    fn frame(
        ctx: &egui::Context,
        doc: &Doc,
        view: &mut View,
        events: Vec<egui::Event>,
        pointer: Option<Pos2>,
    ) -> TimelineResponse {
        let mut input = egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            ..Default::default()
        };
        if let Some(p) = pointer {
            input.events.insert(0, egui::Event::PointerMoved(p));
        }
        let mut out = TimelineResponse::default();
        let colors = PaletteColors::from_palette(&snapclone_theme::palette::DARK);
        let peaks = std::collections::BTreeMap::new();
        let closed = std::collections::BTreeSet::new();
        // An Area at a fixed origin, NOT a CentralPanel: a panel applies an
        // inner margin, so the widget's rect.min would not be (0, 0) and every
        // coordinate this test computes with `origin_x = 0.0` would be aimed a
        // few pixels off the thing it means to hit. That failure looks exactly
        // like the bug under investigation, which is the worst way for a test
        // fixture to be wrong.
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                out = timeline(ui, doc, view, &colors, Ticks::ZERO, &[], &peaks, &Default::default(),
                               &filmstrip::Thumbs::default(), TrimSide::Left, &closed, keys::Mode::Normal,
                               playkeys::InputMode::Motions, "", None);
            });
        });
        out
    }

    // IGNORED: the HARNESS is unproven, not the widget. See the module note
    // below for exactly what was ruled out and what is left to check.
    #[test]
    #[ignore = "synthetic pointer frames do not start an egui drag yet; harness, not product"]
    fn a_body_drag_reports_a_move_on_release() {
        let d = doc();
        let mut view = View::default();
        let ctx = egui::Context::default();

        // The panel's origin is not known until a frame has run, and the first
        // frame of any egui context is a layout pass. So: settle, then aim.
        frame(&ctx, &d, &mut view, vec![], None);

        // Somewhere in the middle of clip 20's body on row 1 — away from both
        // edges, so this is a Move grab and not a trim.
        let grab_x = view.time_to_x(Ticks::from_seconds(5.0), 0.0);
        let row1_y = RULER_H + view.track_h * 1.5;
        let from = Pos2::new(grab_x, row1_y);
        let to = Pos2::new(view.time_to_x(Ticks::from_seconds(9.0), 0.0), row1_y);

        // Press. egui does not call this a DRAG until the pointer has moved
        // past its threshold, so nothing is grabbed yet — a press that never
        // moves is a click, which is correct and is why the grab is asserted
        // after the motion rather than here.
        frame(&ctx, &d, &mut view, vec![egui::Event::PointerButton {
            pos: from, button: egui::PointerButton::Primary, pressed: true,
            modifiers: Default::default() }], Some(from));

        // Move, still held. Two frames: the first crosses the threshold and is
        // where `drag_started` fires, the second is an ordinary drag frame.
        frame(&ctx, &d, &mut view, vec![], Some(to));
        assert!(view.drag.is_some(), "the drag did not grab the clip");
        let mid = frame(&ctx, &d, &mut view, vec![], Some(to));
        assert!(mid.moved.is_none(), "a move must not commit before release");

        // Release.
        let out = frame(&ctx, &d, &mut view, vec![egui::Event::PointerButton {
            pos: to, button: egui::PointerButton::Primary, pressed: false,
            modifiers: Default::default() }], Some(to));

        let (id, pos, track) = out.moved.expect(
            "release over the timeline reported no move — the drop is being dropped");
        assert_eq!(id, ClipId(20));
        assert_eq!(track, TrackId(2), "a horizontal drag keeps the track");
        assert!(pos > Ticks::from_seconds(3.0), "the clip should have moved right, got {pos:?}");
        assert!(view.drag.is_none(), "the drag state outlived the release");
    }

    #[test]
    fn a_release_without_a_grab_reports_nothing() {
        // The complement: if `moved` fired on any release the test above would
        // pass while the widget was broken in the other direction.
        let d = doc();
        let mut view = View::default();
        let ctx = egui::Context::default();
        frame(&ctx, &d, &mut view, vec![], None);
        // Empty space well past every clip.
        let at = Pos2::new(view.time_to_x(Ticks::from_seconds(40.0), 0.0),
                           RULER_H + view.track_h * 2.5);
        frame(&ctx, &d, &mut view, vec![egui::Event::PointerButton {
            pos: at, button: egui::PointerButton::Primary, pressed: true,
            modifiers: Default::default() }], Some(at));
        let out = frame(&ctx, &d, &mut view, vec![egui::Event::PointerButton {
            pos: at, button: egui::PointerButton::Primary, pressed: false,
            modifiers: Default::default() }], Some(at));
        assert!(out.moved.is_none(), "a release on empty space moved something");
    }
}

#[cfg(test)]
mod move_target_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId};

    // Reuses the sibling module's fixture shape: V1 (video) row 0 with clip
    // 10 at 2s..6s, A1 (audio) row 1 with clip 20 at 3s..8s.
    fn project() -> Project {
        let mut p = Project::default();
        for (i, (kind, nm)) in [(studio_core::TrackKind::Video, "V1"),
                                 (studio_core::TrackKind::Audio, "A1"),
                                 (studio_core::TrackKind::Audio, "A2")].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: (*nm).into(), muted: false, locked: false, gain: 1.0, route: None },
                at: i }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(20), source: SourceId(1),
            track: TrackId(2), pos: Ticks::from_seconds(3.0), src_in: Ticks::ZERO,
            len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
        p
    }

    fn drag() -> ClipDrag {
        ClipDrag { id: ClipId(20), kind: DragKind::Move { grab_dt: Ticks::ZERO } }
    }
    fn v() -> View { View::default() }

    #[test]
    fn a_vertical_move_lands_on_the_same_kind_and_never_on_the_wrong_one() {
        let p = project();
        // Pointer over row 2 (A2, audio): the audio clip may move there.
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(20.0), 0.0),
                           RULER_H + v().track_h * 2.5);
        let (_, track) = move_target(&p, &v(), &drag(), at, Pos2::ZERO,
                                     Ticks::ZERO).unwrap();
        assert_eq!(track, TrackId(3), "audio moved to the other audio track");
        // Pointer over row 0 (V1, video): the kind mismatch keeps the track.
        let at = Pos2::new(at.x, RULER_H + v().track_h * 0.5);
        let (_, track) = move_target(&p, &v(), &drag(), at, Pos2::ZERO,
                                     Ticks::ZERO).unwrap();
        assert_eq!(track, TrackId(2), "audio does not land on a video row");
    }

    #[test]
    fn the_drop_snaps_to_edges_and_never_goes_negative() {
        let p = project();
        // Near the clip's own old start (3.0s) — must NOT snap to itself,
        // but 0s is far, so an open-space drop is exact.
        let x = v().time_to_x(Ticks::from_seconds(20.02), 0.0);
        let at = Pos2::new(x, RULER_H + v().track_h * 1.5);
        let (pos, _) = move_target(&p, &v(), &drag(), at, Pos2::ZERO, Ticks::from_seconds(20.0)).unwrap();
        assert_eq!(pos, Ticks::from_seconds(20.0), "snapped to the playhead");
        // A grab deep inside the clip dragged left of zero clamps at zero.
        let deep = ClipDrag { id: ClipId(20),
            kind: DragKind::Move { grab_dt: Ticks::from_seconds(4.0) } };
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(1.0), 0.0),
                           RULER_H + v().track_h * 1.5);
        let (pos, _) = move_target(&p, &v(), &deep, at, Pos2::ZERO, Ticks::from_seconds(500.0)).unwrap();
        assert_eq!(pos, Ticks::ZERO, "a clip cannot be dragged before the origin");
    }
}

#[cfg(test)]
mod grab_and_trim_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId};

    // A1 (row 0 here: audio-only project) with clip 20 at 3s..8s over a 60s
    // source whose in-point is 1s — so there is material on BOTH sides.
    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: studio_core::TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(20), source: SourceId(1),
            track: TrackId(1), pos: Ticks::from_seconds(3.0), src_in: Ticks::from_seconds(1.0),
            len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
        p
    }
    fn v() -> View { View::default() }
    fn mid_y() -> f32 { RULER_H + v().track_h * 0.5 }

    #[test]
    fn edges_trim_and_the_body_moves() {
        let p = project();
        let left_x = v().time_to_x(Ticks::from_seconds(3.0), 0.0) + 2.0;
        let body_x = v().time_to_x(Ticks::from_seconds(5.5), 0.0);
        let right_x = v().time_to_x(Ticks::from_seconds(8.0), 0.0) - 2.0;
        let k = |x: f32| grab(&p, &v(), Pos2::new(x, mid_y()), Pos2::ZERO,
                              TrimSide::default()).unwrap().kind;
        assert_eq!(k(left_x), DragKind::TrimLeft);
        assert!(matches!(k(body_x), DragKind::Move { .. }));
        assert_eq!(k(right_x), DragKind::TrimRight);
    }

    #[test]
    fn a_tie_goes_to_the_preferred_side_and_flips() {
        // Zoom out until the whole clip is narrower than two grips: both
        // edges under the finger, the exact case alt-; exists for.
        let p = project();
        let mut view = v();
        view.px_per_sec = 2.0; // 5s clip -> 10px wide
        let x = view.time_to_x(Ticks::from_seconds(5.5), 0.0);
        let at = Pos2::new(x, mid_y());
        let g = |side| grab(&p, &view, at, Pos2::ZERO, side).unwrap().kind;
        assert_eq!(g(TrimSide::Left), DragKind::TrimLeft);
        assert_eq!(g(TrimSide::Right), DragKind::TrimRight);
    }

    #[test]
    fn a_right_trim_stops_at_the_source_and_a_sliver() {
        let p = project();
        let d = ClipDrag { id: ClipId(20), kind: DragKind::TrimRight };
        // Way past the source's material (in at 1s of 60s → max len 59s).
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(500.0), 0.0), mid_y());
        let (pos, src_in, len) = trim_target(&p, &v(), &d, at, Pos2::ZERO, Ticks::from_seconds(900.0)).unwrap();
        assert_eq!((pos, src_in), (Ticks::from_seconds(3.0), Ticks::from_seconds(1.0)));
        assert_eq!(len, Ticks::from_seconds(59.0), "the clip cannot claim audio the source lacks");
        // And dragged left of its own start it keeps a 10ms sliver.
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(1.0), 0.0), mid_y());
        let (_, _, len) = trim_target(&p, &v(), &d, at, Pos2::ZERO, Ticks::from_seconds(900.0)).unwrap();
        assert_eq!(len, Ticks(TICKS_PER_SECOND / 100));
    }

    #[test]
    fn a_left_trim_moves_the_start_and_the_in_point_together() {
        let p = project();
        let d = ClipDrag { id: ClipId(20), kind: DragKind::TrimLeft };
        // Pull the left edge one second later: pos 3→4, src_in 1→2, len 5→4.
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(4.0), 0.0), mid_y());
        let (pos, src_in, len) = trim_target(&p, &v(), &d, at, Pos2::ZERO, Ticks::from_seconds(900.0)).unwrap();
        assert_eq!(pos, Ticks::from_seconds(4.0));
        assert_eq!(src_in, Ticks::from_seconds(2.0), "the material window slides with the edge");
        assert_eq!(len, Ticks::from_seconds(4.0));
        // Pulled EARLIER it reveals material — but only the 1s that exists
        // before the in-point.
        let at = Pos2::new(v().time_to_x(Ticks::from_seconds(0.5), 0.0), mid_y());
        let (pos, src_in, len) = trim_target(&p, &v(), &d, at, Pos2::ZERO, Ticks::from_seconds(900.0)).unwrap();
        assert_eq!(src_in, Ticks::ZERO, "no material exists before the source's start");
        assert_eq!(pos, Ticks::from_seconds(2.0));
        assert_eq!(len, Ticks::from_seconds(6.0));
    }

    #[test]
    fn panning_never_shows_time_before_zero() {
        let mut view = v();
        view.pan(500.0);
        assert!(view.scroll > Ticks::ZERO);
        view.pan(-100_000.0);
        assert_eq!(view.scroll, Ticks::ZERO, "there is nothing before the origin to see");
    }
}

#[cfg(test)]
mod pattern_drop_tests {
    use super::*;

    fn track(id: u64, kind: TrackKind) -> studio_core::Track {
        studio_core::Track { id: TrackId(id), kind, name: format!("t{id}"), muted: false, locked: false, gain: 1.0, route: None }
    }
    fn project() -> Project {
        let mut p = Project::default();
        for (i, (id, kind)) in [(1u64, TrackKind::Video), (2, TrackKind::Audio), (3, TrackKind::Audio)].into_iter().enumerate() {
            p.apply(&studio_core::Op::AddTrack { track: track(id, kind), at: i }).unwrap();
        }
        p
    }

    #[test]
    fn a_drop_on_a_video_row_goes_to_the_first_audio_track_and_an_audio_row_keeps_its_track() {
        let p = project();
        let view = View::default();
        let origin = Pos2::new(0.0, 0.0);
        let x = origin.x + view.gutter + view.px_per_sec * 2.0; // two seconds in
        let video_row_y = origin.y + RULER_H + 0.5 * view.track_h;
        let (t, track) = pattern_drop_target(&p, &view, Pos2::new(x, video_row_y), origin, Ticks::ZERO).unwrap();
        assert_eq!(track, TrackId(2), "video row → first audio track");
        assert!((t.0 - 2 * TICKS_PER_SECOND).abs() < TICKS_PER_SECOND / 100, "{t:?}");
        let third_row_y = origin.y + RULER_H + 2.5 * view.track_h;
        let (_, track) = pattern_drop_target(&p, &view, Pos2::new(x, third_row_y), origin, Ticks::ZERO).unwrap();
        assert_eq!(track, TrackId(3));
    }

    #[test]
    fn a_drop_left_of_zero_clamps_and_a_project_without_audio_refuses() {
        let p = project();
        let view = View::default();
        let origin = Pos2::new(0.0, 0.0);
        let (t, _) = pattern_drop_target(&p, &view, Pos2::new(origin.x - 500.0, origin.y + RULER_H + 1.0), origin, Ticks::ZERO).unwrap();
        assert_eq!(t, Ticks::ZERO);
        let mut only_video = Project::default();
        only_video.apply(&studio_core::Op::AddTrack { track: track(1, TrackKind::Video), at: 0 }).unwrap();
        assert!(pattern_drop_target(&only_video, &view, Pos2::new(50.0, 50.0), origin, Ticks::ZERO).is_none());
    }
}

#[cfg(test)]
mod fade_and_hud_tests {
    use super::*;

    #[test]
    fn a_fade_shades_the_corner_it_takes_away_and_the_ramp_rises_toward_full() {
        let r = Rect::from_min_size(Pos2::new(100.0, 10.0), Vec2::new(200.0, 40.0));
        let f = fade_triangles(r, 50.0, 20.0);
        assert_eq!(f.len(), 2);
        let (tri, ramp) = f[0];
        assert_eq!(tri, [Pos2::new(100.0, 10.0), Pos2::new(150.0, 10.0), Pos2::new(100.0, 50.0)]);
        assert_eq!(ramp, [Pos2::new(100.0, 50.0), Pos2::new(150.0, 10.0)], "fade in: silent bottom-left up to full");
        let (tri, ramp) = f[1];
        assert_eq!(tri, [Pos2::new(280.0, 10.0), Pos2::new(300.0, 10.0), Pos2::new(300.0, 50.0)]);
        assert_eq!(ramp, [Pos2::new(280.0, 10.0), Pos2::new(300.0, 50.0)], "fade out: full down to silent bottom-right");
        // longer than the clip: clamped to it; under a pixel: nothing
        assert_eq!(fade_triangles(r, 900.0, 0.0)[0].1[1].x, 300.0);
        assert!(fade_triangles(r, 0.4, 0.0).is_empty());
    }

    #[test]
    fn markers_are_coloured_by_the_tool_that_wrote_them() {
        assert_eq!(marker_kind("review 1: missing: \"x\""), MarkerKind::Review);
        assert_eq!(marker_kind("constraint: 2 violated by 1.0 s"), MarkerKind::Constraint);
        assert_eq!(marker_kind("vfx: 1 A001_wide: shake"), MarkerKind::Vfx);
        assert_eq!(marker_kind("the argument"), MarkerKind::Plain);
        assert_eq!(marker_kind("reviewed by jo"), MarkerKind::Plain, "a word, not the prefix");
    }

    #[test]
    fn the_hud_has_its_own_strip_below_every_track() {
        let (p, view) = (Project::default(), View::default());
        let _ = p;
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, RULER_H + 3.0 * view.track_h + HUD_H));
        let band = hud_band(rect);
        assert_eq!(band.height(), HUD_H);
        assert!(band.top() >= RULER_H + 3.0 * view.track_h - 1e-3, "starts below the third track: {band:?}");
        // and no track row maps into it
        assert_eq!(view.row_at(band.center().y, 0.0, 3), None);
    }
}
