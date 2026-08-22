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
    state.poll_note();
    state.poll_route_ai();
    state.poll_edit_ai(now_ms);
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

    if state.find.is_some() {
        find_overlay(ctx, state, &colors);
    }
    if state.palette.is_some() {
        palette_overlay(ctx, state, &colors, now_ms);
    }
    if state.pending_edit.is_some() {
        edit_review_overlay(ctx, state, &colors);
    }
}

/// "AI: Edit Selection…"'s review step — original vs. proposed, explicit
/// Accept/Reject (see `AppState::pending_edit`'s doc for why this is never
/// auto-applied). Modeled on `palette_overlay`'s centered-Area shape rather
/// than a modal window, so it reads as part of the same command surface.
fn edit_review_overlay(ctx: &Context, state: &mut AppState, colors: &PaletteColors) {
    let Some(pending) = state.pending_edit.clone() else { return };
    let mut accept = false;
    let mut reject = false;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        reject = true;
    }
    egui::Area::new(Id::new("aperture_edit_review"))
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 72.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(colors.bg_raised)
                .stroke(Stroke::new(1.0_f32, colors.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(520.0);
                    ui.label(egui::RichText::new(format!("AI edit: {}", pending.instruction)).color(colors.text_weak).small());
                    ui.add_space(4.0);
                    match &pending.proposed {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("asking the assistant…").color(colors.text_weak).italics());
                            });
                        }
                        Some(proposed) => {
                            ui.label(egui::RichText::new("current:").color(colors.text_weak).small());
                            egui::ScrollArea::vertical().id_salt("edit_review_original").max_height(120.0).show(ui, |ui| {
                                ui.label(egui::RichText::new(&pending.original).monospace().color(colors.text_weak));
                            });
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("proposed:").color(colors.accent).small());
                            egui::ScrollArea::vertical().id_salt("edit_review_proposed").max_height(200.0).show(ui, |ui| {
                                ui.label(egui::RichText::new(proposed).monospace().color(colors.text));
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button("Accept (Enter)").clicked() || ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    accept = true;
                                }
                                if ui.button("Reject (Esc)").clicked() {
                                    reject = true;
                                }
                            });
                        }
                    }
                });
        });
    if accept {
        state.accept_pending_edit();
    } else if reject {
        state.reject_pending_edit();
    }
}

/// Ctrl+F / Ctrl+H's floating bar: the query, match position, and — in
/// replace mode — a second row for the replacement text with the focused
/// field visually distinct (`handle_find`'s Tab toggles which one Text
/// events reach). Without this, replace mode would be editing a string with
/// zero on-screen feedback about what's typed or which field has focus —
/// the highlight wash alone was already a thin signal for find-only; it
/// would be actively confusing for two hidden fields.
fn find_overlay(ctx: &Context, state: &mut AppState, colors: &PaletteColors) {
    let Some(f) = &state.find else { return };
    let match_label = if f.matches.is_empty() {
        "no matches".to_string()
    } else {
        format!("{}/{}", f.cur + 1, f.matches.len())
    };
    let query_color = if f.focus == crate::state::FindFocus::Query || f.replace.is_none() {
        colors.text
    } else {
        colors.text_weak
    };
    let query_line = format!("find: {}  ·  {match_label}", f.query);
    let replace_line = f.replace.as_ref().map(|r| {
        let color = if f.focus == crate::state::FindFocus::Replace { colors.text } else { colors.text_weak };
        (format!("replace: {r}"), color)
    });

    egui::Area::new(Id::new("aperture_find"))
        .anchor(Align2::RIGHT_TOP, Vec2::new(-12.0, 12.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(colors.bg_raised)
                .stroke(Stroke::new(1.0_f32, colors.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(query_line).monospace().color(query_color));
                    if let Some((line, color)) = replace_line {
                        ui.label(egui::RichText::new(line).monospace().color(color));
                        ui.label(
                            egui::RichText::new("Tab switch field · Enter replace · Ctrl+Enter replace all · Esc close")
                                .color(colors.text_weak)
                                .small(),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("Enter/F3 next · Shift+Enter prev · Ctrl+↑↓ history · Ctrl+H replace · Esc close")
                                .color(colors.text_weak)
                                .small(),
                        );
                    }
                });
        });
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
                egui::Event::Ime(ime) => match ime {
                    egui::ImeEvent::Preedit(s) => Some(Ev::Preedit(s)),
                    // A commit IS a text insertion — the same thing a
                    // keystroke does, so it goes through the exact same
                    // path (auto-pairs, ghost-consumption, the works)
                    // rather than a parallel "IME insert" code path.
                    egui::ImeEvent::Commit(s) => Some(Ev::Text(s)),
                    // Enabled/Disabled don't need a reaction here: nothing
                    // in aperture's model depends on IME being "on," only
                    // on what it's currently composing (Preedit) or has
                    // finished composing (Commit).
                    egui::ImeEvent::Enabled | egui::ImeEvent::Disabled => None,
                },
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
    let caret_line_text = state.editor.buffer.line(caret.line);
    let caret_visual_col = aperture_core::width::char_col_to_visual(&caret_line_text, caret.col);
    state.cam.caret_x.target = caret_visual_col as f32 * m.char_w;
    state.cam.caret_y.target = caret.line as f32 * m.line_h;
    let max_scroll = (total_h - rect.height()).max(0.0);
    if state.follow_mode {
        // Only when the caret LINE moved (impakt-cam's event model): wheel
        // scrolling is free browsing until the next caret motion — and never
        // mid-drag, so selecting by mouse doesn't yank the world.
        let dragging = ui.ctx().input(|i| i.pointer.primary_down());
        if caret.line != state.last_follow_line && !dragging {
            state.last_follow_line = caret.line;
            let follow = crate::camera::Follow::lines(2.0, 120.0, m.line_h);
            let caret_y = caret.line as f32 * m.line_h;
            match follow.target(caret_y, m.line_h, rect.height(), max_scroll, state.cam.scroll_y.target) {
                crate::camera::FollowMove::Keep => {}
                crate::camera::FollowMove::Glide(t) => state.cam.scroll_y.target = t,
                crate::camera::FollowMove::Snap(t) => state.cam.scroll_y.snap(t),
            }
        }
        state.follow_caret = false; // the mode supersedes the one-shot
    } else if state.follow_caret {
        state.follow_caret = false;
        let y0 = caret.line as f32 * m.line_h;
        state.cam.ensure_visible(y0, y0 + m.line_h, rect.height(), m.line_h * 2.0);
    }
    state.cam.clamp_scroll(max_scroll);
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
            // Width-aware: a selection spanning a wide CJK glyph highlights
            // the full 2-cell glyph, not a narrow sliver. `_clamped` keeps
            // the existing "reaches one cell past EOL" convention for
            // full-line selections (c1 = line_len + 1, by construction above).
            let text = state.editor.buffer.line(line);
            let v0 = aperture_core::width::char_col_to_visual_clamped(&text, c0) as f32;
            let v1 = aperture_core::width::char_col_to_visual_clamped(&text, c1) as f32;
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(content_left + v0 * m.char_w, y),
                    Vec2::new(((v1 - v0) * m.char_w).max(m.char_w * 0.4), m.line_h),
                ),
                2.0,
                sel_fill,
            );
        }
    }

    // Rows: gutter number + syntax-colored galley. Split the borrows so the
    // syntax cache can pull line text while we hold the rest of state.
    let AppState { editor, syntax, find, git_changed, .. } = &mut *state;
    for line in first..last {
        let y = rect.top() - scroll + line as f32 * m.line_h;
        // Find-match slabs: every match a faint wash, the current one loud.
        if let Some(f) = find.as_ref() {
            let find_line_text = editor.buffer.line(line);
            for (c0, c1, current) in f.on_line(&editor.buffer, line) {
                // Match spans are real substrings within the line (never
                // past EOL), so the plain (non-clamped) conversion is exact.
                let v0 = aperture_core::width::char_col_to_visual(&find_line_text, c0) as f32;
                let v1 = aperture_core::width::char_col_to_visual(&find_line_text, c1) as f32;
                let x0 = rect.left() + m.gutter_w + v0 * m.char_w;
                let x1 = rect.left() + m.gutter_w + v1 * m.char_w;
                let r = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + m.line_h));
                let (fill, stroke) = if current {
                    (colors.warning.gamma_multiply(0.35), Stroke::new(1.0_f32, colors.warning))
                } else {
                    (colors.warning.gamma_multiply(0.16), Stroke::NONE)
                };
                painter.rect(r, 2.0, fill, stroke, egui::StrokeKind::Outside);
            }
        }
        // Uncommitted lines wear a green bar in the gutter — diff as
        // marginalia, the same mark the physical viewer uses on its page.
        if git_changed.iter().any(|(a, b)| line >= *a && line < *b) {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(rect.left() + 1.0, y), Vec2::new(3.0, m.line_h)),
                0.0,
                Color32::from_rgb(94, 158, 76),
            );
        }
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
        let spans = syntax.spans_for(line, &editor.buffer);
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
                TokKind::Fn => colors.glow,
                TokKind::Type => colors.accent_soft,
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

    // Tell the platform's IME candidate window where to appear — without
    // this, composing still WORKS (Preedit/Commit events still arrive),
    // but the OS's own popup would show up wherever it last was, not at
    // this caret. A thin rect at the caret, same shape egui's own
    // `TextEdit` reports.
    let caret_screen_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(1.0, m.line_h));
    ui.ctx().output_mut(|o| {
        o.ime = Some(egui::output::IMEOutput { rect, cursor_rect: caret_screen_rect });
    });

    // The composing text itself: an underline at the caret, distinct from
    // the ghost's low-alpha fill — this is real text about to exist, not
    // a suggestion, and shouldn't be mistaken for one.
    if !state.preedit.is_empty() {
        let galley = painter.layout_no_wrap(state.preedit.clone(), m.font.clone(), colors.text);
        let w = galley.size().x;
        painter.galley(Pos2::new(cx, cy), galley, colors.text);
        painter.line_segment(
            [Pos2::new(cx, cy + m.line_h - 1.0), Pos2::new(cx + w, cy + m.line_h - 1.0)],
            Stroke::new(1.5_f32, colors.accent),
        );
    }

    // Symbol popup: a compact list pinned under the caret. The selection
    // window shows at most eight rows; label bright, signature dim.
    if let Some(p) = &state.popup {
        let row_h = m.line_h + 4.0;
        let shown = p.items.len().min(8);
        let first = p.sel.saturating_sub(shown - 1).min(p.items.len() - shown);
        let w = (m.char_w * 52.0).min(rect.width() - 24.0);
        let px = cx.min(rect.right() - w - 8.0).max(rect.left() + 4.0);
        let below = cy + m.line_h + 2.0;
        let py = if below + shown as f32 * row_h + 8.0 > rect.bottom() {
            cy - shown as f32 * row_h - 10.0 // flip above when the caret is low
        } else {
            below
        };
        let panel = Rect::from_min_size(Pos2::new(px, py), Vec2::new(w, shown as f32 * row_h + 8.0));
        painter.rect_filled(panel, 6.0, colors.bg_raised);
        painter.rect_stroke(panel, 6.0, Stroke::new(1.0_f32, colors.border), egui::StrokeKind::Outside);
        for (row, i) in (first..first + shown).enumerate() {
            let item = &p.items[i];
            let y = panel.top() + 4.0 + row as f32 * row_h;
            let r = Rect::from_min_size(Pos2::new(panel.left() + 4.0, y), Vec2::new(w - 8.0, row_h));
            if i == p.sel {
                painter.rect_filled(r, 4.0, colors.accent.gamma_multiply(0.22));
            }
            let label_galley = painter.layout_no_wrap(
                item.label.clone(),
                m.font.clone(),
                if i == p.sel { colors.text } else { colors.text_weak },
            );
            let label_w = label_galley.size().x;
            painter.galley(Pos2::new(r.left() + 6.0, r.top() + 2.0), label_galley, colors.text);
            let avail = w - label_w - 34.0;
            if avail > m.char_w * 6.0 {
                painter.text(
                    Pos2::new(r.left() + label_w + 18.0, r.center().y),
                    Align2::LEFT_CENTER,
                    truncate(&item.detail, (avail / (m.char_w * 0.8)) as usize),
                    FontId::monospace(m.font.size * 0.8),
                    colors.text_weak.gamma_multiply(0.75),
                );
            }
        }
    }

    scrollbar(ui, state, &painter, rect, total_h, colors.accent);

    // A read-only revision preview gets a border, not just a status chip —
    // the cue should be visible without looking down at the chrome.
    if state.rev_ix.is_some() {
        painter.rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0_f32, colors.warning), egui::StrokeKind::Inside);
    }
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
    let total_lines = state.editor.buffer.len_lines().max(1);

    // Minimap marks: where in the WHOLE FILE (not the current scroll
    // window) does something worth noticing sit? Uncommitted diff first —
    // the same green the gutter bar already uses, so the two read as one
    // signal at two zoom levels — then find matches on top, since a person
    // actively searching cares about that more than yesterday's diff.
    for (a, b) in &state.git_changed {
        let (y0, y1) = mark_span(rail, *a, (*b).max(a + 1), total_lines);
        painter.rect_filled(Rect::from_min_max(Pos2::new(rail.left(), y0), Pos2::new(rail.right(), y1)), 0.0, Color32::from_rgb(94, 158, 76).gamma_multiply(0.55));
    }
    if let Some(f) = &state.find {
        for &(a, _) in &f.matches {
            let line = state.editor.buffer.char_to_pos(a).line;
            let (y0, y1) = mark_span(rail, line, line + 1, total_lines);
            painter.rect_filled(Rect::from_min_max(Pos2::new(rail.left(), y0), Pos2::new(rail.right(), y1.max(y0 + 1.5))), 0.0, accent.gamma_multiply(0.8));
        }
    }

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

/// Pure geometry: a line range's [y0, y1] span on the scrollbar rail,
/// proportional to its position in the WHOLE file (`total_lines`), not the
/// current scroll window — a minimap mark answers "where in the file,"
/// the thumb answers "where in the current view." `line1` is exclusive,
/// matching every other half-open line range this codebase already uses.
fn mark_span(rail: Rect, line0: usize, line1: usize, total_lines: usize) -> (f32, f32) {
    let n = total_lines.max(1) as f32;
    let y0 = rail.top() + (line0 as f32 / n) * rail.height();
    let y1 = rail.top() + (line1 as f32 / n) * rail.height();
    (y0, y1.max(y0 + 1.0)) // never fully degenerate — a one-line mark still shows
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
        if state.syntax.is_tree_sitter() {
            ui.label(egui::RichText::new("ts").color(colors.text_weak).small());
        }
        if state.follow_mode {
            ui.separator();
            ui.label(egui::RichText::new("◉ follow").color(colors.accent));
        }
        if let Some(i) = state.rev_ix {
            ui.separator();
            let rev = &state.revs[i];
            let mark = if matches!(rev.kind, aperture_core::gitspace::RevKind::Checkpoint(_)) { "◈" } else { "●" };
            ui.label(
                egui::RichText::new(format!("👁 read-only · rev {}/{} {mark} · Esc to return", i + 1, state.revs.len()))
                    .color(colors.warning),
            );
        } else if !state.git_changed.is_empty() {
            ui.separator();
            let n: usize = state.git_changed.iter().map(|(a, b)| b - a).sum();
            ui.label(
                egui::RichText::new(format!("± {n} uncommitted"))
                    .color(Color32::from_rgb(120, 176, 100)),
            );
        }

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
    if state.rev_ix.is_some() && !state.rev_note.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(format!("✎ {}", state.rev_note)).color(colors.text_weak).small());
        });
    }
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
    let mut open_hit: Option<(std::path::PathBuf, usize)> = None;
    let mut submit_edit: Option<String> = None;

    // Go-to-route's AI tier: trigger from a tight, self-contained borrow
    // scope (owned String + Vec) so it doesn't overlap the palette borrow
    // the render closure below takes — request_route_ai needs `&mut
    // AppState` as a whole (it touches route_engine/route_generation/
    // route_pending_key), which can't coexist with a live `&mut
    // PaletteState` reborrowed from `state.palette`. Idempotent per exact
    // query+candidate set, so recomputing every frame is cheap and safe —
    // same posture as every other mode's search already being recomputed
    // every render.
    if let Some((query, cands)) = state.palette.as_ref().and_then(|p| {
        (p.mode == Mode::GoToRoute).then(|| (p.input.clone(), aperture_core::route::candidates(&p.root, &p.files, &p.symbols, &p.input, 8)))
    }) {
        state.request_route_ai(&query, &cands);
    }

    egui::Area::new(Id::new("aperture_palette"))
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 72.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let Some(p) = state.palette.as_mut() else { return };
            egui::Frame::new()
                .fill(colors.bg_raised)
                .stroke(Stroke::new(1.0_f32, colors.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(460.0);
                    let hint = match p.mode {
                        Mode::Commands => "command, theme, or :line…",
                        Mode::OpenFile => "path to open…",
                        Mode::ProjectSearch => "text to find in project…",
                        Mode::WorkspaceSymbol => "symbol name…",
                        Mode::GoToRoute => "describe what you're looking for…",
                        Mode::AiEditPrompt => "instruction for the selection…",
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
                            let ranked = crate::file_finder::rank(&p.root, &p.files, &p.input, 10);
                            if !ranked.is_empty() {
                                // Fuzzy matches against the walked file list:
                                // the TODO.md-named behavior. Typed-path
                                // entry (below) stays reachable for paths
                                // outside the walk — never removed, only
                                // no longer the ONLY way in.
                                ui.label(
                                    egui::RichText::new("↑↓ navigate · Enter open · Esc cancel")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                if up {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                                if down {
                                    p.selected = (p.selected + 1).min(ranked.len() - 1);
                                }
                                for (i, (full, rel)) in ranked.iter().enumerate() {
                                    let selected = i == p.selected;
                                    let bg = if selected { colors.accent.gamma_multiply(0.25) } else { Color32::TRANSPARENT };
                                    let text_color = if selected { colors.text } else { colors.text_weak };
                                    let row = egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(4)).inner_margin(4.0);
                                    let resp = row
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(egui::RichText::new(rel).color(text_color));
                                        })
                                        .response
                                        .interact(Sense::click());
                                    if resp.clicked() {
                                        open_path = Some(full.to_string_lossy().into_owned());
                                    }
                                }
                                if enter {
                                    let i = p.selected.min(ranked.len() - 1);
                                    open_path = Some(ranked[i].0.to_string_lossy().into_owned());
                                }
                            } else {
                                // No fuzzy hit at all (or nothing walked, e.g.
                                // an unreadable root): fall back to the
                                // original honest behavior — type the exact
                                // path, Enter opens it verbatim.
                                ui.label(
                                    egui::RichText::new("no match — Enter opens the typed path exactly · Esc cancel")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                if enter && !p.input.trim().is_empty() {
                                    open_path = Some(p.input.trim().to_string());
                                }
                            }
                        }
                        Mode::ProjectSearch => {
                            let hits = aperture_core::project_search::search_files(&p.root, &p.files, &p.input, 50, 20);
                            if !hits.is_empty() {
                                ui.label(
                                    egui::RichText::new("↑↓ navigate · Enter jump · Esc cancel")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                if up {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                                if down {
                                    p.selected = (p.selected + 1).min(hits.len() - 1);
                                }
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for (i, hit) in hits.iter().enumerate() {
                                        let selected = i == p.selected;
                                        let bg = if selected { colors.accent.gamma_multiply(0.25) } else { Color32::TRANSPARENT };
                                        let text_color = if selected { colors.text } else { colors.text_weak };
                                        let row = egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(4)).inner_margin(4.0);
                                        let resp = row
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.vertical(|ui| {
                                                    ui.label(egui::RichText::new(format!("{}:{}", hit.rel, hit.line + 1)).color(text_color).strong());
                                                    ui.label(egui::RichText::new(hit.text.trim()).color(colors.text_weak).small());
                                                });
                                            })
                                            .response
                                            .interact(Sense::click());
                                        if resp.clicked() {
                                            open_hit = Some((hit.path.clone(), hit.line));
                                        }
                                    }
                                });
                                if enter {
                                    let i = p.selected.min(hits.len() - 1);
                                    open_hit = Some((hits[i].path.clone(), hits[i].line));
                                }
                            } else if !p.input.is_empty() {
                                ui.label(egui::RichText::new("no match").color(colors.text_weak).italics());
                            }
                        }
                        Mode::WorkspaceSymbol => {
                            let ranked = palette_ui::rank_symbols(&p.symbols, &p.input, 30);
                            if !ranked.is_empty() {
                                ui.label(
                                    egui::RichText::new("↑↓ navigate · Enter jump · Esc cancel")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                if up {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                                if down {
                                    p.selected = (p.selected + 1).min(ranked.len() - 1);
                                }
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for (i, sym) in ranked.iter().enumerate() {
                                        let selected = i == p.selected;
                                        let bg = if selected { colors.accent.gamma_multiply(0.25) } else { Color32::TRANSPARENT };
                                        let text_color = if selected { colors.text } else { colors.text_weak };
                                        let row = egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(4)).inner_margin(4.0);
                                        let resp = row
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.horizontal(|ui| {
                                                    ui.label(egui::RichText::new(&sym.item.name).color(text_color).strong());
                                                    ui.label(egui::RichText::new(format!("{}:{}", sym.rel, sym.item.line + 1)).color(colors.text_weak).small());
                                                });
                                            })
                                            .response
                                            .interact(Sense::click());
                                        if resp.clicked() {
                                            open_hit = Some((sym.path.clone(), sym.item.line));
                                        }
                                    }
                                });
                                if enter {
                                    let i = p.selected.min(ranked.len() - 1);
                                    open_hit = Some((ranked[i].path.clone(), ranked[i].item.line));
                                }
                            } else if p.symbols.is_empty() {
                                ui.label(egui::RichText::new("no Rust symbols indexed in this workspace").color(colors.text_weak).italics());
                            } else if !p.input.is_empty() {
                                ui.label(egui::RichText::new("no match").color(colors.text_weak).italics());
                            }
                        }
                        Mode::GoToRoute => {
                            let cands = aperture_core::route::candidates(&p.root, &p.files, &p.symbols, &p.input, 8);
                            if !cands.is_empty() {
                                ui.label(
                                    egui::RichText::new("↑↓ navigate · Enter jump · Esc cancel — describe what you're looking for, not a name")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                if up {
                                    p.selected = p.selected.saturating_sub(1);
                                }
                                if down {
                                    p.selected = (p.selected + 1).min(cands.len() - 1);
                                }
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for (i, c) in cands.iter().enumerate() {
                                        let selected = i == p.selected;
                                        let bg = if selected { colors.accent.gamma_multiply(0.25) } else { Color32::TRANSPARENT };
                                        let text_color = if selected { colors.text } else { colors.text_weak };
                                        let row = egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(4)).inner_margin(4.0);
                                        let kind_glyph = match c.kind {
                                            aperture_core::route::RouteKind::Symbol => "\u{0192}", // ƒ — a symbol hit
                                            aperture_core::route::RouteKind::Text => "\u{201C}",    // “ — a text hit
                                        };
                                        let resp = row
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.vertical(|ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label(egui::RichText::new(kind_glyph).color(colors.accent).small());
                                                        ui.label(egui::RichText::new(&c.label).color(text_color).strong().monospace());
                                                        ui.label(egui::RichText::new(format!("{}:{}", c.rel, c.line + 1)).color(colors.text_weak).small());
                                                    });
                                                    if !c.snippet.is_empty() {
                                                        ui.label(egui::RichText::new(&c.snippet).color(colors.text_weak).small());
                                                    }
                                                });
                                            })
                                            .response
                                            .interact(Sense::click());
                                        if resp.clicked() {
                                            open_hit = Some((c.path.clone(), c.line));
                                        }
                                    }
                                });
                                if enter {
                                    let i = p.selected.min(cands.len() - 1);
                                    open_hit = Some((cands[i].path.clone(), cands[i].line));
                                }
                                // AI tier: whatever landed for this exact query+candidate
                                // set — request_route_ai (triggered above, outside this
                                // borrow scope) is what queues it; this just reads the
                                // cache. Absent if no APERTURE_CMD is configured, or
                                // still queued/in-flight.
                                let briefs: Vec<aperture_complete::route_ai::CandidateBrief> = cands
                                    .iter()
                                    .map(|c| aperture_complete::route_ai::CandidateBrief {
                                        label: c.label.clone(), rel: c.rel.clone(), line: c.line, snippet: c.snippet.clone(),
                                    })
                                    .collect();
                                let key = aperture_complete::route_ai::route_key(&p.input, &briefs);
                                if let Some(answer) = aperture_complete::route_ai::route_summary(key) {
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.label(egui::RichText::new("AI picks:").color(colors.text_weak).small().strong());
                                    ui.label(egui::RichText::new(answer.trim()).color(colors.text).small());
                                } else if aperture_complete::route_ai::in_flight(key) {
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new("asking the assistant…").color(colors.text_weak).small().italics());
                                }
                            } else if !p.input.trim().is_empty() {
                                ui.label(egui::RichText::new("no keyword matches — try different words").color(colors.text_weak).italics());
                            }
                        }
                        Mode::AiEditPrompt => {
                            ui.label(
                                egui::RichText::new("describe the change, then Enter · Esc cancel")
                                    .color(colors.text_weak)
                                    .small(),
                            );
                            if enter && !p.input.trim().is_empty() {
                                submit_edit = Some(p.input.trim().to_string());
                            }
                        }
                    }
                });
        });

    if let Some(instruction) = submit_edit {
        state.request_edit(&instruction, now_ms);
        state.palette = None;
        return;
    }

    if let Some(cmd) = apply {
        palette_ui::apply(cmd, state, now_ms);
        return;
    }
    if let Some(path) = open_path {
        state.open_file(std::path::PathBuf::from(path), now_ms);
        state.palette = None;
        return;
    }
    if let Some((path, line)) = open_hit {
        state.open_file_at_line(path, line, now_ms);
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
        EK::E => K::E,
        EK::Comma => K::Comma,
        EK::Period => K::Period,
        EK::B => K::B,
        EK::M => K::M,
        EK::H => K::H,
        EK::T => K::T,
        EK::G => K::G,
        EK::F => K::F,
        EK::F3 => K::F3,
        EK::Space => K::Space,
        _ => return None,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_span_positions_proportionally_within_the_whole_file() {
        let rail = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(8.0, 1000.0));
        // Line 0 of 100: top of the rail.
        let (y0, _) = mark_span(rail, 0, 1, 100);
        assert!((y0 - 0.0).abs() < 1e-3);
        // Line 50 of 100: halfway down.
        let (y0, _) = mark_span(rail, 50, 51, 100);
        assert!((y0 - 500.0).abs() < 1e-3, "{y0}");
        // Line 99 of 100 (the last line): near the bottom.
        let (y0, _) = mark_span(rail, 99, 100, 100);
        assert!((y0 - 990.0).abs() < 1e-3, "{y0}");
    }

    #[test]
    fn mark_span_a_range_spans_its_full_extent() {
        let rail = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(8.0, 1000.0));
        let (y0, y1) = mark_span(rail, 10, 20, 100);
        assert!((y0 - 100.0).abs() < 1e-3);
        assert!((y1 - 200.0).abs() < 1e-3);
        assert!(y1 > y0);
    }

    #[test]
    fn mark_span_never_degenerates_to_zero_height() {
        // A single line's mark must still be visible, not a 0px sliver.
        let rail = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(8.0, 10.0)); // tiny rail, big file
        let (y0, y1) = mark_span(rail, 5, 6, 100_000);
        // f32 rounding can shave a hair off the guaranteed 1.0px minimum
        // (e.g. 1.0005 - 0.0005 lands at 0.99999994, not exactly 1.0) —
        // tolerate that rounding, not the actual property being tested.
        assert!(y1 - y0 >= 0.999, "mark stays visible: {y0}..{y1}");
    }

    #[test]
    fn scrollbar_marks_reflect_real_uncommitted_lines_and_find_matches() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-minimap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("m.rs");
        let lines: Vec<String> = (0..200).map(|i| format!("let x{i} = {i};")).collect();
        std::fs::write(&f, lines.join("
") + "
").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        // An uncommitted edit near the end of the file.
        let mut edited = lines.clone();
        edited[190] = "let x190 = NEEDLE;".to_string();
        std::fs::write(&f, edited.join("
") + "
").unwrap();

        let mut s = crate::state::AppState::new(Some(f), false);
        assert!(!s.git_changed.is_empty(), "the uncommitted edit shows up");
        let (a, b) = s.git_changed[0];
        assert!(a >= 189 && b <= 192, "the change is where it really is: {a}..{b}");

        // A find query locates the same needle independently.
        s.editor.sel = crate::state::sel_range(0, 0);
        s.open_find();
        s.handle(crate::state::Ev::Text("NEEDLE".into()), 0);
        let f = s.find.as_ref().unwrap();
        assert!(!f.matches.is_empty(), "found it");
        let match_line = s.editor.buffer.char_to_pos(f.matches[0].0).line;
        assert_eq!(match_line, 190, "the match is really on line 190, not silently shifted earlier");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_span_handles_a_single_line_file_without_panicking() {
        let rail = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(8.0, 100.0));
        let (y0, y1) = mark_span(rail, 0, 1, 1);
        assert!(y0.is_finite() && y1.is_finite());
        assert!(y1 >= y0);
    }
}
