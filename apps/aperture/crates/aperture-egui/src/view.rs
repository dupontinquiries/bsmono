//! view — the only module that draws. Everything it renders it reads from
//! [`AppState`]; everything the user does it translates into [`Ev`] and hands
//! back. Discipline: no editing logic here — if a behavior can't be reached
//! from state.rs's tests, it doesn't belong in this file.
//!
//! Rendering is virtualized: only the visible rows (+2 overscan) are laid out
//! per frame, each as a `LayoutJob` colored from the syntax spans, so cost is
//! O(viewport), never O(file). The camera, caret and font size step through
//! [`crate::camera::Smooth`] each frame and keep requesting repaints until
//! settled — egui only redraws while something is actually moving.

use crate::palette_ui::{self, Mode};
use crate::state::{AppState, Ev, K};
use aperture_core::syntax::TokKind;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, Context, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use uikit_egui::theme_bridge::PaletteColors;

struct Metrics {
    font: FontId,
    line_h: f32,
    char_w: f32,
    gutter_w: f32,
    pad: f32,
}

/// One frame. Call from `eframe::App::ui` with the root `Ui`.
pub fn frame(root: &mut Ui, state: &mut AppState) {
    let ctx = root.ctx().clone();
    let ctx = &ctx;
    let colors = uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(state.theme_id));
    let now_ms = ctx.input(|i| (i.time * 1000.0) as u64);
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.25);

    if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
        state.show_loop_pane = !state.show_loop_pane;
    }
    state.poll_completions(now_ms);
    state.tick_completion(now_ms);
    if state.debounce.armed() || state.ghost.is_pending() {
        // A request may become due (or land) without any input event —
        // keep frames coming while the loop is live.
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }

    if state.show_loop_pane {
        egui::Panel::right("aperture_loop_pane")
            .default_size(300.0)
            .show_inside(root, |ui| crate::loop_pane::show(ui, state, &colors, now_ms));
    }

    egui::Panel::bottom("aperture_status").show_inside(root, |ui| {
        status_bar(ui, state, &colors, now_ms);
    });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(colors.bg_base))
        .show_inside(root, |ui| {
            editor_view(ui, state, &colors, dt, now_ms);
        });

    if state.palette.is_some() {
        palette_overlay(ctx, state, &colors, now_ms);
    }
}

fn metrics(ui: &Ui, state: &AppState) -> Metrics {
    let font = FontId::monospace(state.cam.font_px.pos.round().clamp(7.0, 40.0));
    let (line_h, char_w) = ui.ctx().fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, '0')));
    let digits = state.editor.buffer.len_lines().max(10).ilog10() + 1;
    let pad = char_w;
    Metrics { line_h: line_h + 2.0, char_w, gutter_w: digits as f32 * char_w + pad * 2.0, font, pad }
}

fn editor_view(ui: &mut Ui, state: &mut AppState, colors: &PaletteColors, dt: f32, now_ms: u64) {
    let m = metrics(ui, state);
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());

    // ── input ───────────────────────────────────────────────────────────────
    let palette_open = state.palette.is_some();
    let other_widget_focused = ui.ctx().memory(|mem| mem.focused().is_some());
    if !palette_open && !other_widget_focused {
        let events = ui.ctx().input(|i| i.events.clone());
        for ev in events {
            let translated = match ev {
                egui::Event::Text(t) => Some(Ev::Text(t)),
                egui::Event::Paste(t) => Some(Ev::Paste(t)),
                egui::Event::Copy => Some(Ev::Copy),
                egui::Event::Cut => Some(Ev::Cut),
                egui::Event::Key { key, pressed: true, modifiers, .. } => map_key(key).map(|k| Ev::Key {
                    key: k,
                    ctrl: modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                }),
                _ => None,
            };
            if let Some(ev) = translated {
                let fx = state.handle(ev, now_ms);
                if let Some(text) = fx.clipboard {
                    ui.ctx().copy_text(text);
                }
            }
        }
    }

    // Wheel + pinch/ctrl-wheel zoom, only while the pointer is over us.
    if response.hovered() {
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta);
        if scroll.y != 0.0 {
            state.cam.scroll_y.target -= scroll.y;
        }
        let zoom = ui.ctx().input(|i| i.zoom_delta());
        if zoom != 1.0 {
            let t = (state.cam.font_px.target * zoom).clamp(7.0, 40.0);
            state.cam.font_px.target = t;
        }
    }

    // Pointer → caret.
    let content_left = rect.left() + m.gutter_w + m.pad;
    if response.double_clicked() {
        state.editor.move_word(-1, false);
        state.editor.move_word(1, true);
    } else if response.clicked() || response.drag_started() {
        if let Some(p) = response.interact_pointer_pos() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let ci = state.caret_from_click(
                p.x - content_left,
                p.y - rect.top() + state.cam.scroll_y.pos,
                m.line_h,
                m.char_w,
            );
            state.click_set_caret(ci, shift);
        }
    } else if response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            let ci = state.caret_from_click(
                p.x - content_left,
                p.y - rect.top() + state.cam.scroll_y.pos,
                m.line_h,
                m.char_w,
            );
            state.click_set_caret(ci, true);
        }
    }

    // ── camera ──────────────────────────────────────────────────────────────
    let n_lines = state.editor.buffer.len_lines();
    let total_h = n_lines as f32 * m.line_h;
    let caret = state.editor.caret_pos();
    state.cam.caret_x.target = caret.col as f32 * m.char_w;
    state.cam.caret_y.target = caret.line as f32 * m.line_h;
    if state.follow_caret {
        state.follow_caret = false;
        let y0 = caret.line as f32 * m.line_h;
        state.cam.ensure_visible(y0, y0 + m.line_h, rect.height(), m.line_h * 2.0);
    }
    state.cam.clamp_scroll((total_h - rect.height()).max(0.0));
    if state.cam.step(dt) {
        ui.ctx().request_repaint();
    }
    let scroll = state.cam.scroll_y.pos;
    let origin = Pos2::new(content_left, rect.top() - scroll);

    // ── paint ───────────────────────────────────────────────────────────────
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.bg_base);
    painter.rect_filled(
        Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + m.gutter_w, rect.bottom())),
        0.0,
        colors.bg_card,
    );

    let first = (scroll / m.line_h).floor().max(0.0) as usize;
    let rows = (rect.height() / m.line_h).ceil() as usize + 2;
    let last = (first + rows).min(n_lines);

    // Current-line strip + selection slabs under the text.
    let cur_y = rect.top() - scroll + caret.line as f32 * m.line_h;
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(rect.left() + m.gutter_w, cur_y), Vec2::new(rect.width() - m.gutter_w, m.line_h)),
        0.0,
        colors.bg_hover.gamma_multiply(0.6),
    );
    if !state.editor.sel.is_caret() {
        let sel_fill = colors.accent.gamma_multiply(0.25);
        let (a, b) = (state.editor.sel.min(), state.editor.sel.max());
        let (pa, pb) = (state.editor.buffer.char_to_pos(a), state.editor.buffer.char_to_pos(b));
        for line in pa.line..=pb.line {
            if line + 1 < first || line > last {
                continue;
            }
            let c0 = if line == pa.line { pa.col } else { 0 };
            let c1 = if line == pb.line { pb.col } else { state.editor.buffer.line_len(line) + 1 };
            let y = rect.top() - scroll + line as f32 * m.line_h;
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(content_left + c0 as f32 * m.char_w, y),
                    Vec2::new(((c1 - c0) as f32 * m.char_w).max(m.char_w * 0.4), m.line_h),
                ),
                2.0,
                sel_fill,
            );
        }
    }

    // Rows: gutter number + syntax-colored galley. Split the borrows so the
    // syntax cache can pull line text while we hold the rest of state.
    let AppState { editor, syntax, .. } = &mut *state;
    for line in first..last {
        let y = rect.top() - scroll + line as f32 * m.line_h;
        let num = format!("{}", line + 1);
        let num_color = if line == caret.line { colors.accent } else { colors.text_weak };
        painter.text(
            Pos2::new(rect.left() + m.gutter_w - m.pad, y),
            Align2::RIGHT_TOP,
            num,
            m.font.clone(),
            num_color,
        );
        let text = editor.buffer.line(line).into_owned();
        let spans = syntax.spans_for(line, |j| editor.buffer.line(j).into_owned());
        let mut job = LayoutJob::default();
        let mut cursor = 0usize;
        for s in &spans {
            if s.range.start > cursor {
                job.append(&text[cursor..s.range.start], 0.0, fmt(&m.font, colors.text, false));
            }
            let color = match s.kind {
                TokKind::Keyword => colors.accent,
                TokKind::Str => colors.success,
                TokKind::Comment => colors.text_weak,
                TokKind::Number => colors.warning,
                TokKind::Attr => colors.accent_soft,
                TokKind::Plain => colors.text,
            };
            let italics = s.kind == TokKind::Comment;
            job.append(&text[s.range.clone()], 0.0, fmt(&m.font, color, italics));
            cursor = s.range.end;
        }
        if cursor < text.len() {
            job.append(&text[cursor..], 0.0, fmt(&m.font, colors.text, false));
        }
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(origin.x, y), galley, colors.text);
    }

    // Ghost text — at the caret, when the caret sits at its line's end.
    if let crate::ghost::Ghost::Shown { text } = &state.ghost {
        if caret.col == state.editor.buffer.line_len(caret.line) {
            let alpha = ghost_alpha(state, now_ms);
            let gcolor = colors.text_weak.gamma_multiply(0.85 * alpha);
            let mut lines = text.split('\n');
            let head = lines.next().unwrap_or("");
            let extra = lines.count();
            let gx = origin.x + state.cam.caret_x.pos + 2.0;
            let gy = rect.top() - scroll + caret.line as f32 * m.line_h;
            let mut job = LayoutJob::default();
            job.append(head, 0.0, fmt(&m.font, gcolor, true));
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            let head_w = galley.size().x;
            painter.galley(Pos2::new(gx, gy), galley, gcolor);
            if extra > 0 {
                let chip = format!("⏎ +{extra} line{}", if extra == 1 { "" } else { "s" });
                let chip_pos = Pos2::new(gx + head_w + m.char_w, gy + 1.0);
                let chip_galley = painter.layout_no_wrap(chip, FontId::proportional(m.font.size * 0.75), gcolor);
                let chip_rect = Rect::from_min_size(chip_pos, chip_galley.size() + Vec2::splat(6.0));
                painter.rect_filled(chip_rect, 6.0, colors.bg_raised.gamma_multiply(alpha));
                painter.galley(chip_pos + Vec2::splat(3.0), chip_galley, gcolor);
            }
        }
    }

    // Caret — the smoothly-trailing bar; solid while moving, blinking at rest.
    let cx = origin.x + state.cam.caret_x.pos;
    let cy = rect.top() - scroll + state.cam.caret_y.pos;
    let moving = !state.cam.caret_x.settled() || !state.cam.caret_y.settled();
    let blink_on = moving || (now_ms / 530) % 2 == 0;
    if blink_on {
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(cx - 1.0, cy + 1.0), Vec2::new(2.0, m.line_h - 2.0)),
            1.0,
            colors.accent,
        );
    }
    if !moving {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(530));
    }

    scrollbar(ui, state, &painter, rect, total_h, colors.accent);
}

fn fmt(font: &FontId, color: Color32, italics: bool) -> TextFormat {
    TextFormat { font_id: font.clone(), color, italics, ..Default::default() }
}

fn ghost_alpha(state: &AppState, now_ms: u64) -> f32 {
    match state.ghost_since {
        Some(t0) => {
            let t = (now_ms.saturating_sub(t0)) as f32 / 140.0;
            uikit_egui::motion::ease::out_cubic(t)
        }
        None => 1.0,
    }
}

fn scrollbar(ui: &mut Ui, state: &mut AppState, painter: &egui::Painter, rect: Rect, total_h: f32, accent: Color32) {
    if total_h <= rect.height() {
        return;
    }
    let rail = Rect::from_min_max(Pos2::new(rect.right() - 8.0, rect.top()), rect.right_bottom());
    let frac = rect.height() / total_h;
    let thumb_h = (rail.height() * frac).max(28.0);
    let max_scroll = total_h - rect.height();
    let t = (state.cam.scroll_y.pos / max_scroll).clamp(0.0, 1.0);
    let thumb_y = rail.top() + t * (rail.height() - thumb_h);
    let thumb = Rect::from_min_size(Pos2::new(rail.left() + 1.0, thumb_y), Vec2::new(6.0, thumb_h));
    let resp = ui.interact(rail, Id::new("aperture_scroll_rail"), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let rel = ((p.y - rail.top() - thumb_h * 0.5) / (rail.height() - thumb_h)).clamp(0.0, 1.0);
            state.cam.scroll_y.target = rel * max_scroll;
        }
    }
    let color = if resp.hovered() || resp.dragged() { accent } else { accent.gamma_multiply(0.45) };
    painter.rect_filled(thumb, 3.0, color);
}


fn status_bar(ui: &mut Ui, state: &mut AppState, colors: &PaletteColors, now_ms: u64) {
    ui.horizontal(|ui| {
        let name = state
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[scratch]".into());
        let dirty = if state.dirty { " •" } else { "" };
        ui.label(egui::RichText::new(format!("{name}{dirty}")).color(colors.text));
        ui.separator();
        let p = state.editor.caret_pos();
        ui.label(
            egui::RichText::new(format!("Ln {}, Col {}", p.line + 1, p.col + 1)).color(colors.text_weak),
        );
        ui.separator();
        ui.label(egui::RichText::new(format!("{:?}", state.lang)).color(colors.text_weak));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some((msg, until)) = &state.status {
                if now_ms < *until {
                    ui.label(egui::RichText::new(msg.clone()).color(colors.accent));
                } else {
                    state.status = None;
                }
            }
            let (chip, color) = if !state.ai_enabled {
                ("ai off".to_string(), colors.text_weak)
            } else if state.ghost.is_pending() {
                ("ai …".to_string(), colors.warning)
            } else if state.ghost.is_shown() {
                ("Tab ⇥ accept · Ctrl+→ word".to_string(), colors.success)
            } else {
                ("ai idle".to_string(), colors.text_weak)
            };
            ui.label(egui::RichText::new(chip).color(color));
            ui.separator();
            ui.label(egui::RichText::new("Ctrl+P commands · F2 loop").color(colors.text_weak));
        });
    });
}

fn palette_overlay(ctx: &Context, state: &mut AppState, colors: &PaletteColors, now_ms: u64) {
    // Keys first (read-only view of events; the TextEdit acts on them too,
    // harmlessly, since it's single-line).
    let (mut up, mut down, mut enter, mut esc) = (false, false, false, false);
    ctx.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, .. } = e {
                match key {
                    egui::Key::ArrowUp => up = true,
                    egui::Key::ArrowDown => down = true,
                    egui::Key::Enter => enter = true,
                    egui::Key::Escape => esc = true,
                    _ => {}
                }
            }
        }
    });

    let close = esc;
    let mut apply: Option<palette_ui::Cmd> = None;
    let mut open_path: Option<String> = None;

    egui::Area::new(Id::new("aperture_palette"))
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 72.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let Some(p) = state.palette.as_mut() else { return };
            egui::Frame::new()
                .fill(colors.bg_raised)
                .stroke(Stroke::new(1.0, colors.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(460.0);
                    let hint = match p.mode {
                        Mode::Commands => "command, theme, or :line…",
                        Mode::OpenFile => "path to open…",
                    };
                    let edit = egui::TextEdit::singleline(&mut p.input)
                        .hint_text(hint)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    if !p.focus_placed {
                        r.request_focus();
                        p.focus_placed = true;
                    }
                    if r.changed() {
                        p.selected = 0;
                    }
                    match p.mode {
                        Mode::Commands => {
                            let items = palette_ui::matches(&p.input);
                            let shown = items.len().min(10);
                            if up {
                                p.selected = p.selected.saturating_sub(1);
                            }
                            if down && shown > 0 {
                                p.selected = (p.selected + 1).min(shown - 1);
                            }
                            for (i, (label, _)) in items.iter().take(shown).enumerate() {
                                let selected = i == p.selected;
                                let bg = if selected { colors.accent.gamma_multiply(0.25) } else { Color32::TRANSPARENT };
                                let text_color = if selected { colors.text } else { colors.text_weak };
                                let row = egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(4)).inner_margin(4.0);
                                let resp = row
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(egui::RichText::new(label).color(text_color));
                                    })
                                    .response
                                    .interact(Sense::click());
                                if resp.clicked() {
                                    apply = Some(items[i].1.clone());
                                }
                            }
                            if enter && shown > 0 {
                                apply = Some(items[p.selected.min(shown - 1)].1.clone());
                            }
                            if shown == 0 && !p.input.is_empty() {
                                ui.label(egui::RichText::new("no match").color(colors.text_weak).italics());
                            }
                        }
                        Mode::OpenFile => {
                            ui.label(
                                egui::RichText::new("Enter to open · Esc to cancel")
                                    .color(colors.text_weak)
                                    .small(),
                            );
                            if enter && !p.input.trim().is_empty() {
                                open_path = Some(p.input.trim().to_string());
                            }
                        }
                    }
                });
        });

    if let Some(cmd) = apply {
        palette_ui::apply(cmd, state, now_ms);
        return;
    }
    if let Some(path) = open_path {
        state.open_file(std::path::PathBuf::from(path), now_ms);
        state.palette = None;
        return;
    }
    if close {
        state.palette = None;
    }
}

fn map_key(key: egui::Key) -> Option<K> {
    use egui::Key as EK;
    Some(match key {
        EK::ArrowLeft => K::Left,
        EK::ArrowRight => K::Right,
        EK::ArrowUp => K::Up,
        EK::ArrowDown => K::Down,
        EK::Home => K::Home,
        EK::End => K::End,
        EK::PageUp => K::PageUp,
        EK::PageDown => K::PageDown,
        EK::Backspace => K::Backspace,
        EK::Delete => K::Delete,
        EK::Enter => K::Enter,
        EK::Tab => K::Tab,
        EK::Escape => K::Escape,
        EK::A => K::A,
        EK::O => K::O,
        EK::P => K::P,
        EK::S => K::S,
        EK::Y => K::Y,
        EK::Z => K::Z,
        EK::Space => K::Space,
        _ => return None,
    })
}
