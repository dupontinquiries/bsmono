//! The arrangement page: the view over `arrange`.
//!
//! Everything the pointer does goes through `arrange::tools` in beat
//! coordinates and comes back as commands applied to `arrange::cmd::Editor`;
//! this file only converts pixels to beats and draws. The keyboard maps to
//! `Shortcut`s the same way. Playback lives in the generator thread, which
//! renders the arrangement from the copy of the timeline the GUI publishes
//! in `Shared` after every command.
//!
//! ## Gestures
//!
//! * tools as listed in `arrange::tools`; the ruler seeks and scrubs
//! * wheel pans (shift or a trackpad for horizontal); ctrl+wheel and pinch
//!   zoom around the pointer; two-finger drag pans
//! * space plays / pauses · delete deletes · ctrl-z / ctrl-shift-z /
//!   ctrl-y undo / redo · ctrl-d duplicates · s splits at the playhead ·
//!   l toggles the loop · ctrl-a selects all · arrows nudge · home seeks
//!   to 0 · 1–5 pick a tool
//!
//! Keys are only taken when no text field has focus, so typing a track
//! name does not split anything.

use std::collections::HashMap;

use eframe::egui;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::arrange::cmd::{Command, Editor};
use crate::arrange::model::{Grid, SourceRef, Timeline};
use crate::arrange::render::Library;
use crate::arrange::tools::{At, Brush, Edge, Effect, Hit, Mods, Preview, Shortcut, Tool, Tools};

/// What the view needs from the rest of the app each frame.
pub struct ArrangeIo<'a> {
    pub library: &'a Library,
    /// Beat the audio thread last reported playing.
    pub playhead: f64,
    pub playing: bool,
}

/// What the view wants the app to do.
#[derive(Default)]
pub struct ArrangeOut {
    /// The timeline changed; publish it to the audio thread.
    pub changed: bool,
    pub seek: Option<f64>,
    pub toggle_play: bool,
    pub capture_beat: bool,
    pub import_file: Option<String>,
    pub export_wav: Option<String>,
    pub status: Option<String>,
}

pub struct ArrangeView {
    pub editor: Editor,
    pub tools: Tools,
    px_per_beat: f32,
    scroll_beats: f64,
    track_h: f32,
    header_w: f32,
    ruler_h: f32,
    /// Peaks per source for the clip waveforms, computed once per source.
    peaks: HashMap<SourceRef, Vec<f32>>,
    /// The playhead the view shows: what the thread reported, or where the
    /// user last seeked while stopped.
    playhead: f64,
    pub import_path: String,
    pub export_path: String,
    rename: Option<(u64, String)>,
}

impl Default for ArrangeView {
    fn default() -> Self {
        Self {
            editor: Editor::default(),
            tools: Tools::default(),
            px_per_beat: 40.0,
            scroll_beats: 0.0,
            track_h: 56.0,
            header_w: 96.0,
            ruler_h: 22.0,
            peaks: HashMap::new(),
            playhead: 0.0,
            import_path: String::new(),
            export_path: "arrangement.wav".into(),
            rename: None,
        }
    }
}

const PEAK_COLS: usize = 96;
const EDGE_PX: f32 = 7.0;

impl ArrangeView {
    pub fn timeline(&self) -> &Timeline {
        &self.editor.timeline
    }

    fn x_of(&self, beat: f64, area: Rect) -> f32 {
        area.left() + self.header_w + ((beat - self.scroll_beats) as f32) * self.px_per_beat
    }
    fn beat_of(&self, x: f32, area: Rect) -> f64 {
        self.scroll_beats + ((x - area.left() - self.header_w) / self.px_per_beat) as f64
    }
    fn track_top(&self, ix: usize, area: Rect) -> f32 {
        area.top() + self.ruler_h + ix as f32 * self.track_h
    }

    /// Pixels to timeline space, with the hit under the pointer.
    fn at(&self, p: Pos2, area: Rect) -> At {
        let beat = self.beat_of(p.x, area);
        if p.y < area.top() + self.ruler_h {
            return At { beat, track: None, hit: Hit::Ruler };
        }
        let ix = ((p.y - area.top() - self.ruler_h) / self.track_h).floor();
        let t = &self.editor.timeline;
        if ix < 0.0 || ix as usize >= t.tracks.len() {
            return At { beat, track: None, hit: Hit::Empty };
        }
        let ix = ix as usize;
        let edge_beats = (EDGE_PX / self.px_per_beat) as f64;
        let hit = t.tracks[ix]
            .clips
            .iter()
            .find(|c| beat >= c.start - edge_beats && beat < c.end() + edge_beats)
            .map(|c| {
                let edge = if beat < c.start + edge_beats {
                    Edge::Left
                } else if beat > c.end() - edge_beats {
                    Edge::Right
                } else {
                    Edge::Body
                };
                Hit::Clip { id: c.id, edge }
            })
            .unwrap_or(Hit::Empty);
        At { beat, track: Some(ix), hit }
    }

    fn apply_effects(&mut self, effects: Vec<Effect>, out: &mut ArrangeOut) {
        for ef in effects {
            match ef {
                Effect::Command(c) => match self.editor.apply(c) {
                    Ok(a) => {
                        out.changed = true;
                        if !a.created.is_empty() {
                            self.editor.selection.clear();
                            self.editor.selection.extend(a.created);
                        }
                        out.status = Some(a.label);
                    }
                    Err(e) => out.status = Some(format!("refused: {e:?}")),
                },
                Effect::Select { clips, extend, toggle } => {
                    if toggle {
                        for c in clips {
                            if !self.editor.selection.remove(&c) {
                                self.editor.selection.insert(c);
                            }
                        }
                    } else {
                        if !extend {
                            self.editor.selection.clear();
                        }
                        self.editor.selection.extend(clips);
                    }
                }
                Effect::Seek(b) => {
                    self.playhead = b;
                    out.seek = Some(b);
                }
            }
        }
    }

    fn peaks_for(&mut self, src: SourceRef, lib: &Library) {
        if !self.peaks.contains_key(&src) {
            if let Some(s) = lib.get(src) {
                self.peaks.insert(src, s.peaks(PEAK_COLS));
            }
        }
    }

    // ---------------------------------------------------------------- ui

    /// The whole page. Returns what the app should do.
    pub fn ui(&mut self, ui: &mut egui::Ui, io: &ArrangeIo<'_>) -> ArrangeOut {
        let mut out = ArrangeOut::default();
        if io.playing {
            self.playhead = io.playhead;
        }
        self.toolbar(ui, io, &mut out);
        ui.add_space(4.0);
        self.canvas(ui, io, &mut out);
        self.keys(ui, io, &mut out);
        ui.add_space(4.0);
        let hint = self.tools.tool.hint();
        ui.label(egui::RichText::new(hint).small().weak());
        out
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, io: &ArrangeIo<'_>, out: &mut ArrangeOut) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(if io.playing { "■" } else { "▶" }).on_hover_text("space").clicked() {
                out.toggle_play = true;
            }
            if ui.button("⏮").on_hover_text("home").clicked() {
                self.playhead = 0.0;
                out.seek = Some(0.0);
            }
            ui.separator();
            for (i, t) in Tool::ALL.iter().enumerate() {
                let r = ui.selectable_label(self.tools.tool == *t, t.name()).on_hover_text(format!("{} ({})", t.hint(), i + 1));
                if r.clicked() {
                    self.tools.set_tool(*t);
                }
            }
            ui.separator();
            let mut grid = self.editor.timeline.grid;
            egui::ComboBox::from_id_salt("arr-grid")
                .selected_text(format!("snap {}", grid.label()))
                .width(90.0)
                .show_ui(ui, |ui| {
                    for g in [Grid::BAR, Grid::BEAT, Grid::HALF, Grid::QUARTER, Grid::OFF] {
                        ui.selectable_value(&mut grid, g, g.label());
                    }
                });
            if grid != self.editor.timeline.grid {
                self.apply_effects(vec![Effect::Command(Command::SetGrid { grid })], out);
            }
            let mut bpm = self.editor.timeline.bpm;
            let r = ui.add(egui::DragValue::new(&mut bpm).range(20.0..=400.0).speed(0.5).suffix(" bpm"));
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                if (bpm - self.editor.timeline.bpm).abs() > 1e-9 {
                    self.apply_effects(vec![Effect::Command(Command::SetTempo { bpm })], out);
                }
            }
            let looping = self.editor.timeline.loop_region.is_some();
            if ui.selectable_label(looping, "loop").on_hover_text("l — the selection, or the bar under the playhead").clicked() {
                let fx = Shortcut::ToggleLoop.effects(&self.editor, self.playhead, &mut self.tools);
                self.apply_effects(fx, out);
            }
            ui.separator();
            if ui.add_enabled(self.editor.can_undo(), egui::Button::new("↶"))
                .on_hover_text(format!("undo {}", self.editor.undo_label().unwrap_or("")))
                .clicked()
            {
                if let Some(l) = self.editor.undo() {
                    out.changed = true;
                    out.status = Some(format!("undid {l}"));
                }
            }
            if ui.add_enabled(self.editor.can_redo(), egui::Button::new("↷")).clicked() {
                if let Some(l) = self.editor.redo() {
                    out.changed = true;
                    out.status = Some(format!("redid {l}"));
                }
            }
            ui.separator();
            if ui.button("−").on_hover_text("zoom out (ctrl+wheel, pinch)").clicked() {
                self.px_per_beat = (self.px_per_beat / 1.25).max(4.0);
            }
            if ui.button("+").clicked() {
                self.px_per_beat = (self.px_per_beat * 1.25).min(400.0);
            }
            if ui.button("+ track").clicked() {
                let n = self.editor.timeline.tracks.len() + 1;
                self.apply_effects(vec![Effect::Command(Command::AddTrack { name: format!("track {n}") })], out);
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui, io: &ArrangeIo<'_>, out: &mut ArrangeOut) {
        let n_tracks = self.editor.timeline.tracks.len();
        let want_h = self.ruler_h + n_tracks as f32 * self.track_h + 8.0;
        let h = ui.available_height().max(want_h.min(ui.available_height().max(120.0)));
        let (resp, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), h), Sense::click_and_drag());
        let area = resp.rect;
        let lane = Rect::from_min_max(egui::pos2(area.left() + self.header_w, area.top()), area.right_bottom());

        // ---- wheel, pinch, two-finger: pan and zoom ----
        if resp.hovered() {
            let (scroll, zoom, touch) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.multi_touch().map(|m| m.translation_delta)));
            let pan = touch.unwrap_or(scroll);
            let dx = if pan.x.abs() > 0.0 { pan.x } else { pan.y };
            if dx.abs() > 0.0 {
                self.scroll_beats = (self.scroll_beats - (dx / self.px_per_beat) as f64).max(0.0);
            }
            if (zoom - 1.0).abs() > 1e-4 {
                if let Some(p) = resp.hover_pos() {
                    let anchor = self.beat_of(p.x, area);
                    self.px_per_beat = (self.px_per_beat * zoom).clamp(4.0, 400.0);
                    // Keep the beat under the pointer under the pointer.
                    let after = self.beat_of(p.x, area);
                    self.scroll_beats = (self.scroll_beats + anchor - after).max(0.0);
                }
            }
        }

        // ---- pointer -> tools ----
        let mods = ui.input(|i| Mods { shift: i.modifiers.shift, ctrl: i.modifiers.command, alt: i.modifiers.alt });
        let in_lane = |p: Pos2| p.x >= lane.left();
        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos());
            if let Some(p) = origin.filter(|p| in_lane(*p)) {
                let at = self.at(p, area);
                let fx = self.tools.press(&self.editor, at, mods);
                self.apply_effects(fx, out);
            }
        } else if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let at = self.at(p, area);
                let fx = self.tools.drag(&self.editor, at);
                self.apply_effects(fx, out);
            }
        }
        if resp.drag_stopped() {
            if let Some(p) = resp.interact_pointer_pos() {
                let at = self.at(p, area);
                let fx = self.tools.release(&self.editor, at);
                self.apply_effects(fx, out);
            }
        }
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos().filter(|p| in_lane(*p)) {
                let at = self.at(p, area);
                let fx = self.tools.double_click(at);
                self.apply_effects(fx, out);
            }
        } else if resp.clicked() && !self.tools.dragging() {
            if let Some(p) = resp.interact_pointer_pos().filter(|p| in_lane(*p)) {
                let at = self.at(p, area);
                let fx = self.tools.press(&self.editor, at, mods);
                self.apply_effects(fx, out);
                let fx = self.tools.release(&self.editor, at);
                self.apply_effects(fx, out);
            }
        }
        if let Some(p) = resp.hover_pos().filter(|p| in_lane(*p)) {
            let at = self.at(p, area);
            let icon = match (self.tools.tool, at.hit) {
                (_, Hit::Ruler) => egui::CursorIcon::ResizeHorizontal,
                (Tool::Select, Hit::Clip { edge: Edge::Left, .. }) | (Tool::Select, Hit::Clip { edge: Edge::Right, .. }) => egui::CursorIcon::ResizeHorizontal,
                (Tool::Select, Hit::Clip { .. }) => egui::CursorIcon::Grab,
                (Tool::Split, Hit::Clip { .. }) => egui::CursorIcon::VerticalText,
                (Tool::Erase, _) => egui::CursorIcon::NotAllowed,
                (Tool::Draw, _) => egui::CursorIcon::Cell,
                (Tool::Loop, _) => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        // ---- draw ----
        // Peaks for every source on screen, cached once per source, filled
        // in here before the timeline is borrowed for drawing.
        let sources: Vec<SourceRef> = self.editor.timeline.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.source)).collect();
        for src in sources {
            self.peaks_for(src, io.library);
        }
        let t = &self.editor.timeline;
        let px = self.px_per_beat;
        painter.rect_filled(area, 4.0, Color32::from_gray(20));
        let visible_beats = (lane.width() / px) as f64 + 1.0;
        let first = self.scroll_beats.floor() as i64;
        let last = (self.scroll_beats + visible_beats).ceil() as i64;
        let bpb = t.beats_per_bar.max(1) as i64;
        let lanes_bottom = self.track_top(n_tracks, area).min(area.bottom());

        // Loop region.
        if let Some((a, b)) = t.loop_region {
            let r = Rect::from_min_max(egui::pos2(self.x_of(a, area), area.top()), egui::pos2(self.x_of(b, area), lanes_bottom));
            painter.rect_filled(r.intersect(lane), 0.0, Color32::from_rgba_unmultiplied(120, 200, 255, 22));
            painter.rect_filled(Rect::from_min_max(r.left_top(), egui::pos2(r.right(), area.top() + self.ruler_h)).intersect(lane), 0.0, Color32::from_rgba_unmultiplied(120, 200, 255, 60));
        }
        // Grid lines and ruler labels. Subdivide when zoomed in.
        let sub = if px >= 120.0 { 4 } else if px >= 60.0 { 2 } else { 1 };
        for b in first..=last {
            for s in 0..sub {
                let beat = b as f64 + s as f64 / sub as f64;
                let x = self.x_of(beat, area);
                if x < lane.left() || x > lane.right() {
                    continue;
                }
                let is_bar = s == 0 && b % bpb == 0;
                let col = if is_bar { Color32::from_gray(70) } else if s == 0 { Color32::from_gray(40) } else { Color32::from_gray(30) };
                painter.line_segment([egui::pos2(x, area.top() + self.ruler_h), egui::pos2(x, lanes_bottom)], Stroke::new(1.0, col));
                if is_bar {
                    painter.line_segment([egui::pos2(x, area.top() + 4.0), egui::pos2(x, area.top() + self.ruler_h)], Stroke::new(1.0, Color32::from_gray(110)));
                    painter.text(egui::pos2(x + 3.0, area.top() + 3.0), egui::Align2::LEFT_TOP, format!("{}", b / bpb + 1), egui::FontId::proportional(11.0), Color32::from_gray(160));
                } else if s == 0 && px >= 24.0 {
                    painter.line_segment([egui::pos2(x, area.top() + self.ruler_h - 6.0), egui::pos2(x, area.top() + self.ruler_h)], Stroke::new(1.0, Color32::from_gray(80)));
                }
            }
        }
        // Track lanes and headers.
        let preview = self.tools.preview(&self.editor);
        let sel = self.editor.selection.clone();
        let mut header_actions: Vec<Command> = Vec::new();
        for (ix, track) in t.tracks.iter().enumerate() {
            let top = self.track_top(ix, area);
            let row = Rect::from_min_max(egui::pos2(area.left(), top), egui::pos2(area.right(), top + self.track_h));
            if row.top() > area.bottom() {
                break;
            }
            painter.line_segment([egui::pos2(area.left(), row.bottom()), egui::pos2(area.right(), row.bottom())], Stroke::new(1.0, Color32::from_gray(38)));
            // Header.
            let hdr = Rect::from_min_max(row.left_top(), egui::pos2(area.left() + self.header_w - 4.0, row.bottom()));
            painter.rect_filled(hdr, 0.0, Color32::from_gray(26));
            let audible = t.audible(track);
            painter.text(hdr.left_top() + Vec2::new(6.0, 5.0), egui::Align2::LEFT_TOP, &track.name, egui::FontId::proportional(12.0), if audible { Color32::from_gray(210) } else { Color32::from_gray(110) });
            let bw = 22.0;
            let by = hdr.bottom() - 22.0;
            let m = ui.put(Rect::from_min_size(egui::pos2(hdr.left() + 6.0, by), Vec2::new(bw, 18.0)), egui::SelectableLabel::new(track.muted, "M"));
            if m.clicked() {
                header_actions.push(Command::ToggleMute { track: track.id });
            }
            let s = ui.put(Rect::from_min_size(egui::pos2(hdr.left() + 6.0 + bw + 4.0, by), Vec2::new(bw, 18.0)), egui::SelectableLabel::new(track.solo, "S"));
            if s.clicked() {
                header_actions.push(Command::ToggleSolo { track: track.id });
            }
            let mut gain = track.gain;
            let g = ui.put(Rect::from_min_size(egui::pos2(hdr.left() + 6.0 + 2.0 * (bw + 4.0), by), Vec2::new(hdr.width() - 2.0 * (bw + 4.0) - 12.0, 18.0)), egui::DragValue::new(&mut gain).range(0.0..=2.0).speed(0.01).max_decimals(2));
            if g.drag_stopped() || (g.changed() && !g.dragged()) {
                if (gain - track.gain).abs() > 1e-6 {
                    header_actions.push(Command::SetTrackGain { track: track.id, gain });
                }
            }
            // Clips.
            let lane_row = row.intersect(lane);
            let clip_painter = painter.with_clip_rect(lane_row);
            for clip in &track.clips {
                let (mut start, mut len) = (clip.start, clip.len);
                let mut ghost = false;
                match &preview {
                    Preview::Trim { clip: id, start: s, len: l, .. } if *id == clip.id => {
                        start = *s;
                        len = *l;
                    }
                    Preview::Moving { clips, .. } if clips.contains(&clip.id) => ghost = true,
                    _ => {}
                }
                let r = Rect::from_min_max(
                    egui::pos2(self.x_of(start, area), row.top() + 3.0),
                    egui::pos2(self.x_of(start + len, area), row.bottom() - 3.0),
                );
                if r.right() < lane.left() || r.left() > lane.right() {
                    continue;
                }
                let selected = sel.contains(&clip.id);
                let base = match clip.source {
                    SourceRef::Beat(_) => Color32::from_rgb(70, 150, 110),
                    SourceRef::File(_) => Color32::from_rgb(70, 110, 170),
                };
                let fill = if clip.muted || !audible { base.gamma_multiply(0.4) } else if ghost { base.gamma_multiply(0.5) } else { base };
                clip_painter.rect_filled(r, 4.0, fill);
                if let Some(pk) = self.peaks_for_readonly(clip.source, io.library) {
                    let src_beats = io.library.beats_of(clip.source, t).unwrap_or(len).max(1e-6);
                    let cols = r.width().max(1.0) as usize / 2;
                    for c in 0..cols {
                        let frac_clip = c as f64 / cols.max(1) as f64;
                        let pos = (clip.offset + frac_clip * len) / src_beats;
                        let idx = ((pos * pk.len() as f64) as usize).min(pk.len().saturating_sub(1));
                        let v = pk[idx].clamp(0.0, 1.0);
                        let x = r.left() + c as f32 * 2.0;
                        let hh = v * (r.height() * 0.5 - 4.0);
                        clip_painter.line_segment([egui::pos2(x, r.center().y - hh), egui::pos2(x, r.center().y + hh)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 70)));
                    }
                }
                clip_painter.text(r.left_top() + Vec2::new(5.0, 3.0), egui::Align2::LEFT_TOP, io.library.name(clip.source), egui::FontId::proportional(11.0), Color32::from_gray(235));
                let stroke = if selected { Stroke::new(2.0, Color32::from_rgb(255, 220, 120)) } else { Stroke::new(1.0, Color32::from_gray(15)) };
                clip_painter.rect_stroke(r, 4.0, stroke, StrokeKind::Inside);
            }
        }
        for c in header_actions {
            self.apply_effects(vec![Effect::Command(c)], out);
        }
        // Previews on top.
        match &preview {
            Preview::Moving { clips, delta_beats, delta_tracks, copy, fits } => {
                let t = &self.editor.timeline;
                for id in clips {
                    if let Some((track, clip)) = t.find(*id) {
                        let ix = t.track_index(track.id).unwrap_or(0) as i32 + delta_tracks;
                        if ix < 0 || ix as usize >= t.tracks.len() {
                            continue;
                        }
                        let top = self.track_top(ix as usize, area);
                        let r = Rect::from_min_max(
                            egui::pos2(self.x_of(clip.start + delta_beats, area), top + 3.0),
                            egui::pos2(self.x_of(clip.end() + delta_beats, area), top + self.track_h - 3.0),
                        );
                        let col = if *fits { Color32::from_rgba_unmultiplied(255, 255, 255, 60) } else { Color32::from_rgba_unmultiplied(255, 80, 80, 70) };
                        painter.with_clip_rect(lane).rect_filled(r, 4.0, col);
                        if *copy {
                            painter.with_clip_rect(lane).text(r.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(14.0), Color32::WHITE);
                        }
                    }
                }
            }
            Preview::Marquee { from_beat, from_track, to_beat, to_track } => {
                let (a, b) = (from_track.min(to_track), from_track.max(to_track));
                let r = Rect::from_min_max(
                    egui::pos2(self.x_of(from_beat.min(*to_beat), area), self.track_top(*a, area)),
                    egui::pos2(self.x_of(from_beat.max(*to_beat), area), self.track_top(b + 1, area)),
                );
                painter.with_clip_rect(lane).rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 220, 120, 30));
                painter.with_clip_rect(lane).rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 220, 120)), StrokeKind::Inside);
            }
            Preview::Loop { from, to } => {
                let r = Rect::from_min_max(egui::pos2(self.x_of(from.min(*to), area), area.top()), egui::pos2(self.x_of(from.max(*to), area), lanes_bottom));
                painter.with_clip_rect(lane).rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(120, 200, 255, 50));
            }
            _ => {}
        }
        // Playhead.
        let x = self.x_of(self.playhead, area);
        if x >= lane.left() && x <= lane.right() {
            painter.line_segment([egui::pos2(x, area.top()), egui::pos2(x, lanes_bottom)], Stroke::new(2.0, Color32::from_rgb(255, 120, 90)));
        }
        // Follow while playing when the playhead leaves the view.
        if io.playing && (self.playhead < self.scroll_beats || self.playhead > self.scroll_beats + visible_beats - 1.0) {
            self.scroll_beats = (self.playhead - 1.0).max(0.0);
        }
    }

    /// The cached peaks, for the draw loop (which holds the timeline).
    fn peaks_for_readonly(&self, src: SourceRef, _lib: &Library) -> Option<&[f32]> {
        self.peaks.get(&src).map(|v| v.as_slice())
    }

    fn keys(&mut self, ui: &mut egui::Ui, _io: &ArrangeIo<'_>, out: &mut ArrangeOut) {
        if ui.memory(|m| m.focused().is_some()) {
            return; // a text field has the keyboard
        }
        use egui::{Key, Modifiers};
        let mut fx: Vec<Effect> = Vec::new();
        let cmd = Modifiers::COMMAND;
        let take = |ui: &mut egui::Ui, m: Modifiers, k: Key| ui.input_mut(|i| i.consume_key(m, k));
        if take(ui, Modifiers::NONE, Key::Space) {
            out.toggle_play = true;
        }
        if take(ui, Modifiers::NONE, Key::Delete) || take(ui, Modifiers::NONE, Key::Backspace) {
            fx.extend(Shortcut::Delete.effects(&self.editor, self.playhead, &mut self.tools));
        }
        if take(ui, cmd | Modifiers::SHIFT, Key::Z) || take(ui, cmd, Key::Y) {
            if let Some(l) = self.editor.redo() {
                out.changed = true;
                out.status = Some(format!("redid {l}"));
            }
        } else if take(ui, cmd, Key::Z) {
            if let Some(l) = self.editor.undo() {
                out.changed = true;
                out.status = Some(format!("undid {l}"));
            }
        }
        if take(ui, cmd, Key::D) {
            fx.extend(Shortcut::Duplicate.effects(&self.editor, self.playhead, &mut self.tools));
        }
        if take(ui, cmd, Key::A) {
            fx.extend(Shortcut::SelectAll.effects(&self.editor, self.playhead, &mut self.tools));
        }
        if take(ui, Modifiers::NONE, Key::S) {
            fx.extend(Shortcut::SplitAtPlayhead.effects(&self.editor, self.playhead, &mut self.tools));
        }
        if take(ui, Modifiers::NONE, Key::L) {
            fx.extend(Shortcut::ToggleLoop.effects(&self.editor, self.playhead, &mut self.tools));
        }
        if take(ui, Modifiers::NONE, Key::Home) {
            fx.push(Effect::Seek(0.0));
        }
        for (k, s) in [
            (Key::ArrowLeft, Shortcut::NudgeLeft),
            (Key::ArrowRight, Shortcut::NudgeRight),
            (Key::ArrowUp, Shortcut::NudgeUp),
            (Key::ArrowDown, Shortcut::NudgeDown),
        ] {
            if take(ui, Modifiers::NONE, k) {
                fx.extend(s.effects(&self.editor, self.playhead, &mut self.tools));
            }
        }
        for (k, t) in [(Key::Num1, Tool::Select), (Key::Num2, Tool::Draw), (Key::Num3, Tool::Split), (Key::Num4, Tool::Erase), (Key::Num5, Tool::Loop)] {
            if take(ui, Modifiers::NONE, k) {
                self.tools.set_tool(t);
            }
        }
        if take(ui, Modifiers::NONE, Key::Escape) {
            self.editor.selection.clear();
            self.tools.drag = crate::arrange::tools::Drag::None;
        }
        self.apply_effects(fx, out);
    }

    /// The library, for the controls column: captured beats and files,
    /// arming the draw brush, and dropping a source at the playhead.
    pub fn library_ui(&mut self, ui: &mut egui::Ui, io: &ArrangeIo<'_>, out: &mut ArrangeOut) {
        ui.label(egui::RichText::new("library").small());
        if ui.button("＋ capture playing beat").on_hover_text("render the window the generator is playing now, at full quality, into the library").clicked() {
            out.capture_beat = true;
        }
        let armed = self.tools.brush.map(|b| b.source);
        let t_beats = self.editor.timeline.tracks.first().map(|t| t.id);
        let t_files = self.editor.timeline.tracks.get(1).or(self.editor.timeline.tracks.first()).map(|t| t.id);
        let mut arm: Option<SourceRef> = None;
        let mut drop: Option<(SourceRef, u64)> = None;
        let mut list = |ui: &mut egui::Ui, items: &[crate::arrange::render::Sample], kind: fn(usize) -> SourceRef, track: Option<u64>| {
            for (i, s) in items.iter().enumerate() {
                let r = kind(i);
                ui.horizontal(|ui| {
                    let sel = ui.selectable_label(armed == Some(r), format!("{} · {:.2}s", s.name, s.secs()))
                        .on_hover_text("click arms it for the draw tool · double-click drops it at the playhead");
                    if sel.double_clicked() {
                        if let Some(t) = track {
                            drop = Some((r, t));
                        }
                    } else if sel.clicked() {
                        arm = Some(r);
                    }
                });
            }
        };
        ui.label(egui::RichText::new("beats").small().weak());
        list(ui, &io.library.beats, SourceRef::Beat, t_beats);
        if io.library.beats.is_empty() {
            ui.label(egui::RichText::new("none yet — play something on the morph page and capture it").small().italics());
        }
        ui.label(egui::RichText::new("files").small().weak());
        list(ui, &io.library.files, SourceRef::File, t_files);
        if let Some(r) = arm {
            if let Some(beats) = io.library.beats_of(r, &self.editor.timeline) {
                self.tools.brush = Some(Brush { source: r, beats });
                if self.tools.tool == Tool::Select {
                    self.tools.set_tool(Tool::Draw);
                }
                out.status = Some(format!("armed {} — draw it onto the grid", io.library.name(r)));
            }
        }
        if let Some((r, track)) = drop {
            if let Some(beats) = io.library.beats_of(r, &self.editor.timeline) {
                let fx = vec![Effect::Command(Command::Place { track, source: r, start: self.playhead, source_beats: beats })];
                self.apply_effects(fx, out);
            }
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.import_path).id_salt("arr-import").hint_text("path to a wav/flac/mp3").desired_width(150.0));
            if ui.button("import").clicked() && !self.import_path.trim().is_empty() {
                out.import_file = Some(self.import_path.trim().to_string());
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.export_path).id_salt("arr-export").desired_width(150.0));
            if ui.button("export wav").on_hover_text("render the whole arrangement to a file").clicked() {
                out.export_wav = Some(self.export_path.trim().to_string());
            }
        });
        if let Some(sel) = self.editor.selection.iter().next().copied() {
            if let Some((_, c)) = self.editor.timeline.find(sel) {
                ui.separator();
                ui.label(egui::RichText::new(format!("clip {} · {:.2}–{:.2}", io.library.name(c.source), c.start, c.end())).small());
                let mut gain = c.gain;
                let muted = c.muted;
                let r = ui.add(egui::Slider::new(&mut gain, 0.0..=2.0).text("gain"));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.apply_effects(vec![Effect::Command(Command::SetClipGain { clip: sel, gain })], out);
                }
                if ui.selectable_label(muted, "mute clip").clicked() {
                    self.apply_effects(vec![Effect::Command(Command::ToggleClipMute { clip: sel })], out);
                }
            }
        }
        // Rename or remove the track the selection is on, or the first.
        let track_id = self.editor.selection.iter().next().and_then(|c| self.editor.timeline.find_track_of(*c)).or(t_beats);
        if let Some(tid) = track_id {
            let name = self.editor.timeline.track(tid).map(|t| t.name.clone()).unwrap_or_default();
            // The edit buffer follows the track; a different track means a
            // fresh buffer, not the previous track's half-typed name.
            if self.rename.as_ref().map(|(id, _)| *id) != Some(tid) {
                self.rename = Some((tid, name.clone()));
            }
            let mut commit: Option<String> = None;
            let mut remove = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("track").small());
                if let Some((_, buf)) = self.rename.as_mut() {
                    let r = ui.add(egui::TextEdit::singleline(buf).id_salt("arr-rename").desired_width(110.0));
                    if r.lost_focus() && *buf != name && !buf.trim().is_empty() {
                        commit = Some(buf.clone());
                    }
                }
                if ui.small_button("✕").on_hover_text("remove this track").clicked() {
                    remove = true;
                }
            });
            if let Some(n) = commit {
                self.apply_effects(vec![Effect::Command(Command::RenameTrack { track: tid, name: n })], out);
            }
            if remove {
                self.apply_effects(vec![Effect::Command(Command::RemoveTrack { track: tid })], out);
            }
        }
    }
}
