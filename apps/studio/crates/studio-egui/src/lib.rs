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

pub mod touch;
pub mod keys;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use studio_collab::Doc;
use studio_core::{ClipId, Project, Ticks, TrackId, TrackKind, TICKS_PER_SECOND};
use uikit_egui::PaletteColors;

/// Viewport state: where the timeline is scrolled and how far it is zoomed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Horizontal zoom. Stored as pixels per SECOND rather than per tick so the
    /// number stays human — 100.0 is "a second is a hundred pixels".
    pub px_per_sec: f32,
    /// Leftmost visible time.
    pub scroll: Ticks,
    /// Row height for one track.
    pub track_h: f32,
    /// Width of the track-header gutter on the left.
    pub gutter: f32,
}

impl Default for View {
    fn default() -> Self {
        View { px_per_sec: 100.0, scroll: Ticks::ZERO, track_h: 64.0, gutter: 132.0 }
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
) -> Option<ClipId> {
    let rows = track_rows(&doc.project);
    let height = RULER_H + rows.len().max(1) as f32 * view.track_h;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    let origin = rect.min;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.bg_base);

    // ── ruler ──────────────────────────────────────────────────────────────
    // Tick spacing is chosen so labels never collide: step up through a 1/2/5
    // sequence until one second's worth of pixels clears the label width.
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
                             Stroke::new(1.0, colors.border));
        painter.text(Pos2::new(x + 3.0, origin.y + 3.0), Align2::LEFT_TOP,
                     format_timecode(Ticks::from_seconds(s as f64)),
                     FontId::monospace(10.0), colors.text_weak);
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
            let r = view.clip_rect(clip.pos, clip.len, row, origin);
            if r.max.x < origin.x + view.gutter || r.min.x > rect.max.x { continue; }
            let selected = selection.contains(&clip.id);
            let base = match track.kind { TrackKind::Video => colors.accent, TrackKind::Audio => colors.success };
            painter.rect_filled(r, 3.0, base.gamma_multiply(if selected { 0.9 } else { 0.55 }));
            painter.rect_stroke(r, 3.0, Stroke::new(if selected { 2.0 } else { 1.0 },
                if selected { colors.accent } else { colors.border }), egui::StrokeKind::Inside);
            // a clip somebody ELSE is inside gets a warning edge — this is the
            // whole point of the presence channel: stop the collision, don't
            // arbitrate it afterwards
            if !doc.peers_on(clip.id).is_empty() {
                painter.rect_stroke(r.expand(1.5), 4.0, Stroke::new(2.0, colors.warning), egui::StrokeKind::Outside);
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
                    let rate = doc.project.sources.get(&clip.source).map(|s| s.sample_rate).unwrap_or(48_000);
                    // sample range of the VISIBLE part of the clip
                    let skip = Ticks(((vis.min.x - r.min.x) / view.px_per_sec
                        * TICKS_PER_SECOND as f32) as i64);
                    let s0 = clip.src_in.saturating_add(skip).to_samples(rate) as usize;
                    let s1 = s0 + Ticks(((vis.width() / view.px_per_sec)
                        * TICKS_PER_SECOND as f32) as i64).to_samples(rate) as usize;
                    let mid = vis.center().y;
                    let half = (vis.height() * 0.5 - 3.0).max(1.0);
                    let wave = colors.text.gamma_multiply(0.55);
                    for (i, mm) in peaks.window(s0, s1, cols).iter().enumerate() {
                        let x = vis.min.x + i as f32;
                        let (lo, hi) = (mm.min.clamp(-1.0, 1.0), mm.max.clamp(-1.0, 1.0));
                        painter.line_segment(
                            [Pos2::new(x, mid - hi * half), Pos2::new(x, mid - lo * half)],
                            Stroke::new(1.0, wave));
                    }
                }
            }
            if r.width() > 34.0 {
                let name = doc.project.sources.get(&clip.source)
                    .map(|s| s.path.rsplit('/').next().unwrap_or("clip").to_string())
                    .unwrap_or_else(|| "clip".into());
                painter.text(r.left_top() + Vec2::new(4.0, 3.0), Align2::LEFT_TOP,
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
        painter.line_segment([Pos2::new(x, origin.y + RULER_H), Pos2::new(x, rect.max.y)],
                             Stroke::new(1.5, tint));
        // the name rides at the top so several peers stay readable
        painter.text(Pos2::new(x + 3.0, origin.y + RULER_H + 1.0), Align2::LEFT_TOP,
                     &peer.name, FontId::proportional(10.0), tint);
    }

    // ── our own playhead, last, so it is never hidden by a peer's ──────────
    let px = view.time_to_x(playhead, origin.x);
    if px >= origin.x + view.gutter {
        painter.line_segment([Pos2::new(px, origin.y), Pos2::new(px, rect.max.y)],
                             Stroke::new(2.0, colors.danger));
    }

    resp.interact_pointer_pos()
        .filter(|_| resp.clicked())
        .and_then(|p| hit_test(&doc.project, view, p, origin))
}

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
pub fn format_timecode(t: Ticks) -> String {
    let total_ms = (t.0 as f64 / TICKS_PER_SECOND as f64 * 1000.0).round() as i64;
    let (neg, total_ms) = (total_ms < 0, total_ms.abs());
    let (h, m, s, ms) = (total_ms / 3_600_000, total_ms / 60_000 % 60,
                         total_ms / 1000 % 60, total_ms % 1000);
    format!("{}{h}:{m:02}:{s:02}.{ms:03}", if neg { "-" } else { "" })
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
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false }, at: 1 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "/media/take1.mov".into(),
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
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
            active_track: Some(TrackId(1)), seq: 1 });
        assert_eq!(doc.peers_on(ClipId(10)).len(), 1, "the peer is inside the 2s..6s clip");
        assert!(doc.peers_on(ClipId(20)).is_empty(), "and not inside the other one");
    }

    #[test]
    fn timecode_reads_like_an_editor_expects() {
        assert_eq!(format_timecode(Ticks::ZERO), "0:00:00.000");
        assert_eq!(format_timecode(Ticks::from_seconds(61.5)), "0:01:01.500");
        assert_eq!(format_timecode(Ticks::from_seconds(3661.25)), "1:01:01.250");
        assert_eq!(format_timecode(Ticks::from_seconds(-2.0)), "-0:00:02.000");
    }
}
