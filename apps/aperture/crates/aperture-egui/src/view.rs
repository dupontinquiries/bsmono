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
    state.poll_lsp(now_ms);
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

    // THE MOBILE COMMAND STRIP, above the status bar so the status line
    // stays the last thing on screen. Drawn only when asked for — it
    // costs screen height from the file the person came to read, so the
    // palette carries the toggle ("View: Bottom command strip").
    //
    // This is the way into the palette on a touch screen: Ctrl+P is
    // unreachable there, which made every command in it desktop-only by
    // accident rather than by decision.
    // THE MULTIPASS MENU. A keymap you cannot see is not a keymap, so
    // the blocks and their keys are drawn above the status bar whenever
    // there are any — no toggle, because the menu only exists for as long
    // as an unaccepted reply does, and it disappears on ESC or on any
    // choice.
    if !state.menu.is_empty() || !state.menu_prose.is_empty() {
        egui::Panel::bottom("aperture_completion_menu").show_inside(root, |ui| {
            completion_menu(ui, state, &colors);
        });
    }

    if state.cmd_strip.visible {
        egui::Panel::bottom("aperture_cmd_strip").show_inside(root, |ui| {
            cmd_strip_bar(ui, state, &colors, now_ms);
        });
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
    if state.hover_overlay.is_some() {
        hover_overlay(ctx, state, &colors);
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
                    // Clamped: 520 is wider than a phone. See the palette below.
                    ui.set_width(uikit_egui::dialog::dialog_width(
                        ui.ctx().content_rect().width(), 520.0));
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

/// Ctrl+I's full result, distinct from the one-line status headline
/// `AppState::poll_lsp` already sets — a real type signature plus doc
/// comment is often several lines, and this is the space to show all of
/// it rather than truncate to what fits in the status bar. Dismissed by
/// `AppState` itself on Escape or the next edit (see `hover_overlay`
/// field's own doc), not by anything in this function — this is a pure
/// render of whatever state currently holds, same discipline every other
/// overlay in this file follows.
fn hover_overlay(ctx: &Context, state: &mut AppState, colors: &PaletteColors) {
    let Some(contents) = state.hover_overlay.clone() else { return };
    let mut dismiss = false;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        dismiss = true;
    }
    egui::Area::new(Id::new("aperture_hover"))
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 72.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(colors.bg_raised)
                .stroke(Stroke::new(1.0_f32, colors.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(uikit_egui::dialog::dialog_width(
                        ui.ctx().content_rect().width(), 520.0));
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("hover").color(colors.text_weak).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("×").clicked() {
                                dismiss = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("hover_contents").max_height(240.0).show(ui, |ui| {
                        ui.label(egui::RichText::new(&contents).monospace().color(colors.text));
                    });
                });
        });
    if dismiss {
        state.hover_overlay = None;
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
    // FOCUS DOES NOT SURVIVE THE PANE THAT OWNED IT. egui keeps the focused
    // widget id in memory indefinitely, so clicking a field in the F2 pane
    // and then CLOSING the pane left focus pointing at a widget that is no
    // longer drawn — `mem.focused()` stayed `Some`, the guard below kept
    // skipping the editor, and the editor was silently dead: no typing, no
    // debounce, no completion. That is the "autocomplete does not work when
    // the F2 pane is closed" report. A focused widget that is not on screen
    // is not focus; clear it.
    //
    // BUT ONLY THE PANE'S. The first version of this surrendered focus
    // whenever the pane was closed, full stop — which also killed the
    // PALETTE's own text box (Ctrl+P, and "AI: Edit Selection…", whose
    // prompt is a palette mode). That box calls `request_focus()` on the
    // frame it appears and then had it taken away again immediately, so it
    // rendered but could not be typed into. The palette is a legitimate
    // focus owner that is very much on screen, so it is excluded here
    // rather than fought with.
    if !state.show_loop_pane && state.palette.is_none() {
        let stale = ui.ctx().memory(|m| m.focused());
        if let Some(id) = stale {
            ui.ctx().memory_mut(|m| m.surrender_focus(id));
        }
    }
    let other_widget_focused = ui.ctx().memory(|mem| mem.focused().is_some());
    // THE TAB FIGHT, settled properly this time. Tab means indent /
    // accept-ghost to the editor, but egui also uses it for focus
    // traversal. The previous attempt consumed Tab INSIDE the
    // `!other_widget_focused` branch — which is exactly the branch that
    // does not run when the pane holds focus, so in the one situation the
    // fix existed for, it never fired. Consume it out here, whenever the
    // editor is the intended target (pane closed, or open but with nothing
    // in it focused), so the focus system never sees it. When a pane field
    // IS focused, Tab is left alone and field-hops as any form does.
    let editor_owns_tab = !palette_open && (!state.show_loop_pane || !other_widget_focused);
    if !palette_open && !other_widget_focused {
        // CLONE BEFORE CONSUMING — and this ordering is the whole bug the
        // last two attempts got wrong. `consume_key` REMOVES the event
        // from egui's queue; that is the point (it stops the focus system
        // seeing Tab). But the editor reads its keys from this same queue,
        // so consuming first meant the editor never saw Tab either: it no
        // longer indented and no longer accepted the ghost. Snapshot the
        // events, THEN consume from egui's copy. The editor works from the
        // snapshot; the focus system works from the drained queue.
        let events = ui.ctx().input(|i| i.events.clone());
        if editor_owns_tab {
            ui.ctx().input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
            });
        }
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

    // Pointer → caret. The click grid is built from the camera as it stands
    // BEFORE this frame's step — the same value the old inline formula used —
    // because that is the page the user was looking at when they clicked.
    // Going through `PageGrid` rather than dividing by `line_h` is what keeps
    // the click and the paint agreeing on the boundary pixel of every row;
    // see page.rs, "Hit-testing".
    let content_left = rect.left() + m.gutter_w + m.pad;
    let click_grid = crate::page::PageGrid::new(
        rect.top(),
        state.cam.scroll_y.pos,
        m.line_h,
        ui.ctx().pixels_per_point(),
    );
    if response.double_clicked() {
        state.editor.move_word(-1, false);
        state.editor.move_word(1, true);
    } else if response.clicked() || response.drag_started() {
        if let Some(p) = response.interact_pointer_pos() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let ci = state.caret_from_click_line(
                p.x - content_left,
                click_grid.line_at(p.y),
                m.char_w,
            );
            state.click_set_caret(ci, shift);
        }
    } else if response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            let ci = state.caret_from_click_line(
                p.x - content_left,
                click_grid.line_at(p.y),
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
    state.note_text_motion(); // arm the line-slide before rows paint
    // The slide, in px for this frame: lines >= shift_from render offset
    // by shift_px, decaying to 0 — an insert slides the tail down into
    // place, a delete slides it up. Settled = exactly the old render.
    let shift_px = if state.line_shift.settled() { 0.0 } else { state.line_shift.pos * m.line_h };
    let shift_from = state.line_shift_from;
    if state.typewriter_mode {
        // Strict centering, film-style: every caret LINE change re-aims
        // the center (no dead zone — the dead zone IS follow_mode; this
        // mode is its strict sibling). Wheel browsing works between caret
        // moves and the camera re-asserts on the next one, same event
        // model as follow. Past ~120 lines, teleport: a dolly across a
        // whole file is a blur, not a shot.
        let dragging = ui.ctx().input(|i| i.pointer.primary_down());
        if caret.line != state.last_follow_line && !dragging {
            state.last_follow_line = caret.line;
            let caret_y = caret.line as f32 * m.line_h;
            let want = crate::camera::typewriter_target(caret_y, m.line_h, rect.height(), max_scroll);
            if (want - state.cam.scroll_y.pos).abs() > 120.0 * m.line_h {
                state.cam.scroll_y.snap(want);
            } else {
                state.cam.scroll_y.target = want;
            }
        }
        state.follow_caret = false;
    } else if state.follow_mode {
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
    if state.cam.step(dt) | state.line_shift.step(dt) {
        ui.ctx().request_repaint();
    }
    let scroll = state.cam.scroll_y.pos;
    // ONE map from content y to screen y, for the whole frame. Every painter
    // call below goes through it. See `page.rs` for why eight copies of
    // `rect.top() - scroll + line * line_h` shimmered against each other even
    // though they were the same expression: epaint rounds each shape to the
    // pixel grid separately, so with a fractional `line_h` adjacent rows
    // crossed the boundary on different frames. Quantizing the ORIGIN once
    // makes the page translate rigidly.
    let grid = crate::page::PageGrid::new(rect.top(), scroll, m.line_h, ui.ctx().pixels_per_point());
    // Row top in screen space, including the insert/delete line-slide.
    // Crisp when the slide is not running (the overwhelmingly common case);
    // un-quantized for the rows that are actually moving, because a 1-px
    // quantum is bigger than a frame of that animation and would read as
    // stutter. Those rows are text, and epaint would re-round the galley
    // anyway — accepted: the slide is brief and its endpoint is crisp.
    let row_y = |line: usize| -> f32 {
        if shift_px != 0.0 && line >= shift_from {
            grid.y_smooth(line as f32 * m.line_h + shift_px)
        } else {
            grid.row_y(line)
        }
    };
    let origin = Pos2::new(content_left, grid.y(0.0));

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
    let cur_y = grid.row_y(caret.line);
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(rect.left() + m.gutter_w, cur_y), Vec2::new(rect.width() - m.gutter_w, grid.row_h(caret.line))),
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
            let y = row_y(line);
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

    // Secondary selections — same fill, drawn for every extra cursor that
    // isn't a bare caret. A separate loop rather than folding into the block
    // above: the primary reads `state.editor.sel` directly (it always has,
    // and every other caller of that field is untouched by multi-cursor), so
    // duplicating its five lines here is cheaper than threading an iterator
    // through code that has to keep working for the zero-extra-cursors case.
    for s in &state.extra_cursors {
        if s.is_caret() {
            continue;
        }
        let sel_fill = colors.accent.gamma_multiply(0.25);
        let (a, b) = (s.min(), s.max());
        let (pa, pb) = (state.editor.buffer.char_to_pos(a), state.editor.buffer.char_to_pos(b));
        for line in pa.line..=pb.line {
            if line + 1 < first || line > last {
                continue;
            }
            let c0 = if line == pa.line { pa.col } else { 0 };
            let c1 = if line == pb.line { pb.col } else { state.editor.buffer.line_len(line) + 1 };
            let y = row_y(line);
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
    // syntax cache can pull line text while we hold the rest of state.
    let AppState { editor, syntax, find, git_changed, lsp_diagnostics, .. } = &mut *state;
    for line in first..last {
        let y = row_y(line);
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
        // LSP diagnostics get a SECOND, separately-positioned gutter bar —
        // deliberately not sharing the git-diff bar's x position, since an
        // uncommitted line with a live error is a real, common combination
        // and the two marks answer different questions ("did I touch this"
        // vs "is this wrong right now"). Worst severity wins when a line
        // has more than one diagnostic — a warning shouldn't visually hide
        // a co-located error.
        if let Some(d) = lsp_diagnostics.iter()
            .filter(|d| line as u32 >= d.start_line && line as u32 <= d.end_line)
            .min_by_key(|d| match d.severity {
                aperture_lsp::DiagnosticSeverity::Error => 0,
                aperture_lsp::DiagnosticSeverity::Warning => 1,
                aperture_lsp::DiagnosticSeverity::Info => 2,
            })
        {
            let color = match d.severity {
                aperture_lsp::DiagnosticSeverity::Error => Color32::from_rgb(224, 90, 90),
                aperture_lsp::DiagnosticSeverity::Warning => Color32::from_rgb(224, 180, 90),
                aperture_lsp::DiagnosticSeverity::Info => Color32::from_rgb(120, 150, 200),
            };
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(rect.left() + 5.0, y), Vec2::new(3.0, m.line_h)),
                0.0,
                color,
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
        // CLAMP SPANS TO THE RENDERED LINE. `text` is `Buffer::line`, which
        // trims the terminator; a span source that measures a line as
        // `line_to_byte(line + 1) - line_to_byte(line)` (tree-sitter's
        // bound) or that returns a memo from a previous, longer buffer will
        // hand back a range reaching past it. Either way the editor used to
        // die on the spot:
        //
        //   panicked at view.rs: end byte index 43 is out of bounds for
        //   string of length 42
        //
        // — on OPENING A FILE, which is not a failure a text editor may
        // have. Clamped here, at the point of use, because this is the
        // boundary every span crosses no matter which backend produced it
        // and no matter how stale it is. `floor_char_boundary` because a
        // byte clamp that lands mid-UTF-8 panics exactly as hard as the
        // overflow it replaced.
        let clamp = |i: usize| {
            let i = i.min(text.len());
            let mut i = i;
            while i > 0 && !text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        for s in &spans {
            let (sa, sb) = (clamp(s.range.start), clamp(s.range.end));
            if sa >= sb {
                continue; // clamped to nothing: the span is past this line
            }
            if sa > cursor {
                job.append(&text[cursor..sa], 0.0, fmt(&m.font, colors.text, false));
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
            job.append(&text[sa..sb], 0.0, fmt(&m.font, color, italics));
            cursor = sb;
        }
        if cursor < text.len() {
            job.append(&text[cursor..], 0.0, fmt(&m.font, colors.text, false));
        }
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(origin.x, y), galley, colors.text);
    }

    // Ghost text — at the caret, when the caret sits at its line's end.
    if let crate::ghost::Ghost::Shown { text, .. } = &state.ghost {
        if caret.col == state.editor.buffer.line_len(caret.line) {
            let alpha = ghost_alpha(state, now_ms);
            let gcolor = colors.text_weak.gamma_multiply(0.85 * alpha);
            let mut lines = text.split('\n');
            let head = lines.next().unwrap_or("");
            let extra = lines.count();
            let gx = origin.x + state.cam.caret_x.pos + 2.0;
            let gy = grid.row_y(caret.line);
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
    let moving = !state.cam.caret_x.settled() || !state.cam.caret_y.settled();
    // The one place a snap rule differs between the animation and its end,
    // ON PURPOSE (page.rs, "crisp vs smooth"): a 2 px bar quantized to the
    // pixel grid every frame stutters visibly as it glides, and left
    // un-quantized at rest it is a permanently blurry caret. So: smooth
    // while moving, crisp once settled. The `with_round_to_pixels(false)`
    // below is not optional — without it epaint re-rounds the shape and
    // throws the smooth position away.
    let cy = if moving { grid.y_smooth(state.cam.caret_y.pos) } else { grid.y(state.cam.caret_y.pos) };
    // The caret's look AND its frame budget come from one policy — see
    // `camera::CaretPolicy`, which exists because the two lines this
    // replaced asked for a repaint every 530 ms forever, focused or not.
    let focused = ui.ctx().input(|i| i.focused);
    let cd = state.caret.draw(now_ms, state.last_input_ms, focused, moving);
    if cd.alpha > 0.0 {
        let bar = Rect::from_min_size(Pos2::new(cx - 1.0, cy + 1.0), Vec2::new(2.0, m.line_h - 2.0));
        painter.add(
            egui::epaint::RectShape::filled(bar, 1.0, colors.accent.gamma_multiply(cd.alpha))
                // See `cy` above: rounding a gliding caret to the pixel grid
                // is the stutter, so the moving case opts out of it.
                .with_round_to_pixels(!moving),
        );
    }
    if let Some(ms) = cd.repaint_in_ms {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(ms));
    }

    // Secondary carets — static, not blinking: blink timing is a "where is
    // MY cursor" cue for the one the keyboard follows, and would just be
    // visual noise multiplied across however many are on screen.
    for s in &state.extra_cursors {
        let p = state.editor.buffer.char_to_pos(s.head);
        if p.line + 1 < first || p.line > last {
            continue;
        }
        let line_text = state.editor.buffer.line(p.line);
        let vx = aperture_core::width::char_col_to_visual(&line_text, p.col) as f32;
        let ex = content_left + vx * m.char_w;
        let ey = row_y(p.line);
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(ex - 1.0, ey + 1.0), Vec2::new(2.0, m.line_h - 2.0)),
            1.0,
            colors.accent.gamma_multiply(0.8),
        );
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


/// How many palette rows are BUILT, at most.
///
/// Not a display limit — the list scrolls — but a bound on the work one frame
/// does. Ranking is already O(files) and cheap; building egui rows is not, and
/// a fifty-thousand-file monorepo with an empty query would otherwise lay out
/// fifty thousand of them every frame.
///
/// Two hundred rather than ten. Ten was small enough that on a phone, with no
/// arrow keys, everything past the tenth match was unreachable by ANY means —
/// which is what "C-o doesn't add more files as you scroll" was describing:
/// not a paging bug, an absent eleventh row. Two hundred is past the point
/// where scrolling beats typing another character, so the cap stops being the
/// thing the user is fighting.
pub const PALETTE_ROWS_MAX: usize = 200;

/// How tall the palette's result list may grow before it scrolls.
///
/// The floor, and the value on a small screen. Bounded so the palette does not
/// run off the bottom of a phone — the failure C13_TODO §24 records for
/// settings panels across the fleet, where a page was a bare stack with
/// nothing bounding it and a Ui does not scroll, it clips.
pub const PALETTE_LIST_H: f32 = 360.0;

/// The fraction of the window the result list may occupy.
///
/// The rest is the palette's own input row and margins plus whatever is
/// behind it, and leaving that much is what keeps the thing readable as a
/// popup rather than a full-screen takeover.
const PALETTE_LIST_FRACTION: f32 = 0.62;

/// Hard ceiling, for a tall desktop monitor.
///
/// Past roughly this height the list stops being scannable — you are reading
/// a page, not picking from a menu, and the fuzzy filter is the better tool
/// for narrowing than the eye is.
const PALETTE_LIST_H_MAX: f32 = 720.0;

/// The result list's height for a window of `screen_h` pixels.
///
/// Pure, so the phone bound below is testable without a window — the whole
/// risk in making this taller is regressing the small-screen case, and that
/// is not something to find out on a device.
///
/// A fixed 360 was the same on a 2160px monitor as on a phone, which on the
/// desktop meant about eight visible rows with the scrollbar doing the rest —
/// the palette was short for no reason other than that the phone needed it to
/// be.
pub fn palette_list_h_for(screen_h: f32) -> f32 {
    (screen_h * PALETTE_LIST_FRACTION)
        .max(PALETTE_LIST_H)
        .min(PALETTE_LIST_H_MAX)
        // Never taller than the window itself, which the `max` above could
        // otherwise force on a short window — a list taller than the screen
        // cannot be scrolled to the bottom of. Applied LAST so it wins over
        // the floor.
        .min((screen_h - 96.0).max(120.0))
}

/// The result list's height for THIS window.
///
/// Takes the `Ui` rather than reading a global so it reflects the actual
/// window, and so it stays correct when the window is resized rather than
/// being sampled once at startup.
///
/// Reads `content_rect()`, not `screen_rect()` -- the same fix
/// `dialog_width` already applied and documented (`screen_rect` does not
/// exclude what sits under the system bars, so a title bar/inset made the
/// usable area smaller than this function assumed). This sibling function
/// had not received the same fix, so the list could compute a fraction of
/// a taller-than-real height and render shorter than the window actually
/// allowed for it -- reported as "too short for the window."
pub fn palette_list_h(ui: &egui::Ui) -> f32 {
    palette_list_h_for(ui.ctx().content_rect().height())
}


/// The strip: a palette button that opens on tap-and-hold, then recents
/// and pinned quick actions.
///
/// The HOLD is what reaches the palette without a keyboard. A tap runs
/// the first recent instead, because on a strip whose whole point is the
/// last thing you used, a tap that opened a menu would put a step in
/// front of the common case.
/// Draw the blocks a reply produced, each with the key that takes it.
///
/// PROSE FIRST, as a comment. The model's explanation is often the most
/// useful part of a reply and the part that must never be compiled;
/// showing it here is what lets the code blocks stay code.
fn completion_menu(ui: &mut Ui, state: &mut AppState, colors: &PaletteColors) {
    use aperture_complete::multipass::Kind;
    for p in &state.menu_prose {
        // Rendered as a comment, visually: it is the model talking, and
        // it should look like the thing it is rather than like an option.
        let first = p.lines().next().unwrap_or("").trim();
        ui.label(
            egui::RichText::new(format!("// {first}"))
                .color(colors.text_weak)
                .small()
                .italics(),
        );
    }
    ui.horizontal_wrapped(|ui| {
        for b in &state.menu {
            match b.key {
                Some(k) => {
                    // The key first and in strong type: it is what the
                    // hand needs, and the hint is what the eye reads.
                    ui.label(egui::RichText::new(format!("[{k}]")).strong().color(colors.accent));
                    ui.label(egui::RichText::new(&b.hint).color(colors.text).small());
                }
                None => {
                    // No key: an elision, a stub, or past the eighth
                    // block. Shown greyed WITH its reason rather than
                    // hidden, so "why is that not offered" has an answer
                    // on screen instead of looking like a dropped
                    // suggestion.
                    let why = match b.kind {
                        Kind::Elision => "would delete code",
                        Kind::Stub => "stub",
                        Kind::Prose => "prose",
                        Kind::Code => "no key left",
                    };
                    ui.label(
                        egui::RichText::new(format!("{} ({why})", b.hint))
                            .color(colors.text_weak)
                            .small()
                            .strikethrough(),
                    );
                }
            }
            ui.separator();
        }
        ui.label(egui::RichText::new("esc to type").color(colors.text_weak).small());
    });
}

fn cmd_strip_bar(ui: &mut Ui, state: &mut AppState, colors: &PaletteColors, now_ms: u64) {
    use uikit_egui::cmdstrip::Release;
    let dt = ui.input(|i| i.stable_dt);
    // Tick BEFORE drawing, so the menu opens on the frame the threshold
    // is crossed rather than one frame late — a hold that visibly lags
    // reads as a missed press.
    if state.cmd_hold.tick(dt) {
        state.open_palette();
    }

    ui.horizontal(|ui| {
        let palette = ui.button("⌘");
        if palette.is_pointer_button_down_on() && !state.cmd_hold.is_open() {
            state.cmd_hold.press();
        }
        if palette.drag_stopped() || palette.clicked() {
            match state.cmd_hold.release() {
                Some(Release::Tap) => state.open_palette(),
                Some(Release::Hold) | None => {}
            }
        }
        palette.on_hover_text("tap or hold for the command palette");

        ui.separator();
        // Entries are recents first, then pinned — see `cmdstrip::Strip`.
        let entries: Vec<(String, String)> = state
            .cmd_strip
            .entries()
            .iter()
            .map(|c| (c.id.clone(), c.label.clone()))
            .collect();
        if entries.is_empty() {
            ui.label(
                egui::RichText::new("recents appear here as you use commands")
                    .color(colors.text_weak)
                    .small(),
            );
        }
        for (id, label) in entries {
            if ui.small_button(&label).clicked() {
                state.run_strip_command(&id, now_ms);
            }
        }
    });
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
        // The way back, where a finger can reach it: Alt+Left / Alt+Right
        // do the same, but a phone has no Alt key.
        if !state.nav.is_empty() {
            ui.separator();
            if ui.add_enabled(state.nav.can_go_back(), egui::Button::new("‹").min_size(egui::vec2(28.0, 0.0)))
                .on_hover_text("back to where the jump started (Alt+Left)").clicked()
            {
                state.nav_go(true, now_ms);
            }
            if ui.add_enabled(state.nav.can_go_forward(), egui::Button::new("›").min_size(egui::vec2(28.0, 0.0)))
                .on_hover_text("forward again (Alt+Right)").clicked()
            {
                state.nav_go(false, now_ms);
            }
            let crumb = state.nav.breadcrumb(3);
            if !crumb.is_empty() {
                ui.label(egui::RichText::new(crumb).color(colors.text_weak).small());
            }
        }
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
        if !state.lsp_diagnostics.is_empty() {
            let errors = state.lsp_diagnostics.iter()
                .filter(|d| d.severity == aperture_lsp::DiagnosticSeverity::Error).count();
            let warnings = state.lsp_diagnostics.iter()
                .filter(|d| d.severity == aperture_lsp::DiagnosticSeverity::Warning).count();
            ui.separator();
            let (text, color) = if errors > 0 {
                (format!("✕ {errors} error{}", if errors == 1 { "" } else { "s" }),
                 Color32::from_rgb(224, 90, 90))
            } else {
                (format!("⚠ {warnings} warning{}", if warnings == 1 { "" } else { "s" }),
                 Color32::from_rgb(224, 180, 90))
            };
            ui.label(egui::RichText::new(text).color(color));
        }

        // Measured on the OUTER horizontal, right before the right-aligned
        // section starts — this is what's actually left after everything
        // drawn so far on the left (filename, Ln/Col, language, and
        // whichever of the optional follow/rev/git markers fired). The
        // right-to-left child ui below does NOT get this number for free:
        // it lays out from ITS OWN right edge outward, with no awareness of
        // how far the left-hand cursor already advanced, so on a narrow
        // window (a phone screen, not just a small desktop window) the two
        // sides drew on top of each other instead of one giving way.
        let room = ui.available_width();
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
            // The hint is the single most droppable thing in this bar — a
            // reminder of a shortcut, not state the person needs to see —
            // so it is the one thing that gives way when space runs out,
            // rather than silently overlapping whatever drew first.
            // 220px comfortably fits "Ctrl+P commands · F2 loop" at the
            // status bar's font size; below that it would either truncate
            // mid-word or collide with the chip just drawn.
            if room > 220.0 {
                ui.separator();
                ui.label(egui::RichText::new("Ctrl+P commands · F2 loop").color(colors.text_weak));
            }
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
    // The chosen command's LABEL travels with it, so a palette use feeds
    // the strip's recents. Without this the strip would only ever learn
    // from itself, and the command you actually reach for most — picked
    // from the palette — would never appear on it.
    let mut apply: Option<(String, palette_ui::Cmd)> = None;
    let mut open_path: Option<String> = None;
    let mut open_hit: Option<(std::path::PathBuf, usize)> = None;
    // Which palette produced a hit, for the nav step's relation.
    let hit_mode = state.palette.as_ref().map(|p| p.mode.clone());
    let mut submit_edit: Option<String> = None;
    let mut submit_voice: Option<String> = None;

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
                    // `set_width(460.0)` was a HARD width, wider than a phone
                    // screen, so the palette ran off both edges — the same bug
                    // tomescope's palette had. `dialog_width` asks for 460 and
                    // takes what the surface can actually give, measured from
                    // content_rect (the usable area, excluding what sits under
                    // the system bars) rather than screen_rect.
                    ui.set_width(uikit_egui::dialog::dialog_width(
                        ctx.content_rect().width(), 460.0));
                    let hint = match p.mode {
                        Mode::Commands => "command, theme, or :line…",
                        Mode::OpenFile => "path to open…",
                        Mode::ProjectSearch => "text to find in project…",
                        Mode::WorkspaceSymbol => "symbol name…",
                        Mode::GoToRoute => "describe what you're looking for…",
                        Mode::AiEditPrompt => "instruction for the selection…",
                        Mode::VoiceCommand => "say a command — \"save\", \"find total\", \"go to line 12\"…",
                    };
                    let edit = egui::TextEdit::singleline(&mut p.input).id_salt("view-p_input")
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
                            let shown = items.len().min(PALETTE_ROWS_MAX);
                            if up {
                                p.selected = p.selected.saturating_sub(1);
                                p.follow_selection = true;
                            }
                            if down && shown > 0 {
                                p.selected = (p.selected + 1).min(shown - 1);
                                p.follow_selection = true;
                            }
                            // A SCROLL AREA. There was none, and the list was
                            // capped at ten — so on a phone, where there is no
                            // ↑↓, everything past the tenth match was
                            // unreachable by any means. Reported as "the
                            // command palette is still not tappable to scroll
                            // on phone"; the palette was not scrollable at all,
                            // it just looked like it on a desktop where the ten
                            // rows fit.
                            egui::ScrollArea::vertical()
                                .id_salt("palette_commands")
                                // SIZE TO THE CONTENT, capped.
                                //
                                // `min_scrolled_height == max_height` pinned the box to an
                                // ESTIMATE — `interact_size.y` per row — and each row is
                                // actually a Frame with its own 4pt inner margin, so the
                                // estimate is short and the list scrolled even with three
                                // entries in it. Reported as "the height is too small, it
                                // doesn't feel like it is responding to the content".
                                //
                                // `auto_shrink[1] = true` sizes to the real content instead.
                                // That does NOT bring back the latch it replaced: the latch
                                // came from `max_height` alone taking whatever the Area
                                // offered, which depended on last frame. Content height does
                                // not.
                                //
                                // STATUS: reported again after this landed (the box still does
                                // not grow back once text is entered then cleared). Nothing in
                                // this crate can run a live egui::Context frame, so the claim
                                // above that auto_shrink fixes the latch was never actually
                                // exercised -- only reasoned about. It may be wrong about which
                                // index of `auto_shrink([bool; 2])` controls height, or `Area`
                                // may have its own last-frame-size memory that a ScrollArea
                                // fix cannot reach regardless of the axis. Needs a device or an
                                // egui integration-test harness to settle either way; I do not
                                // have either from this sandbox and am not patching this again
                                // on a second unverified guess.
                                .max_height(palette_list_h(ui))
                                .auto_shrink([false, true])
                                // The scrollbar was always reserving its gutter, which is
                                // the "excessive padding on the right hand side".
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                .show(ui, |ui| {
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
                                // Keyboard and touch share one list, so the
                                // keyboard's selection has to drag the view
                                // with it — otherwise ↓ walks the highlight off
                                // the bottom and the palette appears frozen.
                                // ONLY when the keyboard moved it. Following the
                                // selection every frame fights a touch drag:
                                // the list scrolls under the finger and snaps
                                // home the moment it lifts.
                                if selected && p.follow_selection {
                                    // `Some(Center)`, not `None`. `None` scrolls the MINIMUM
                                    // distance to bring the row into view, which parks
                                    // the selection hard against whichever edge it
                                    // entered from — hold ↓ and the highlight pins to
                                    // the bottom pixel row with the next results
                                    // invisible below it, which reads as the list
                                    // lagging one item behind the keyboard. Centring
                                    // keeps the neighbours on both sides visible, so
                                    // you can see what you are about to land on.
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                if resp.clicked() {
                                    apply = Some((items[i].0.clone(), items[i].1.clone()));
                                }
                            }
                                });
                            if enter && shown > 0 {
                                let pick = &items[p.selected.min(shown - 1)];
                                apply = Some((pick.0.clone(), pick.1.clone()));
                            }
                            if shown == 0 && !p.input.is_empty() {
                                ui.label(egui::RichText::new("no match").color(colors.text_weak).italics());
                            }
                        }
                        Mode::OpenFile => {
                            let ranked = crate::file_finder::rank(&p.root, &p.files, &p.input, PALETTE_ROWS_MAX);
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
                                    p.follow_selection = true;
                                }
                                if down {
                                    p.selected = (p.selected + 1).min(ranked.len() - 1);
                                    p.follow_selection = true;
                                }
                                // Same scroll area, same reason — and here the
                                // ten-row cap was the whole of "C-o doesn't add
                                // more files as you scroll": there was nothing
                                // below the tenth match to scroll TO. `rank`
                                // always took a limit; it was being handed 10.
                                egui::ScrollArea::vertical()
                                    .id_salt("palette_open_file")
                                    // HEIGHT FROM THE CONTENT, capped — not `max_height` alone.
                                    //
                                    // The palette lives in an auto-sizing Area, so a ScrollArea
                                    // asked only for a MAXIMUM takes whatever height it is
                                    // offered, which in an Area that sizes to its content is
                                    // whatever the last frame happened to be. That latches: the
                                    // box shrinks on a narrow result set and never grows back
                                    // when the results do. Reported as "the window height also
                                    // shrinks and never grows even with ample elements".
                                    //
                                    // Asking for an exact height computed from the row count
                                    // breaks the loop: the size is a function of the results,
                                    // not of the previous size.
                                    // Same change as the commands list above: size to the
                                    // real content, capped, and only show the scrollbar when
                                    // it is needed. See that comment for why this does not
                                    // reintroduce the latch it replaced.
                                    .max_height(palette_list_h(ui))
                                    .auto_shrink([false, true])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                    .show(ui, |ui| {
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
                                    if selected && p.follow_selection {
                                        resp.scroll_to_me(Some(egui::Align::Center)); // see the note at the commands list
                                    }
                                    if resp.clicked() {
                                        open_path = Some(full.to_string_lossy().into_owned());
                                    }
                                }
                                    });
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
                                // THE THIRD LIST. `Ctrl+G` opens this mode, and the
                                // padding/height report was about it — the two lists
                                // fixed in 9f4b34b0 are Commands and OpenFile, and this
                                // one kept the old shape: a bare `max_height`, no
                                // `auto_shrink`, and a scrollbar gutter reserved whether
                                // or not it scrolls.
                                //
                                // Found by running the app and reading the hint text:
                                // what I had been screenshotting as "the palette" was
                                // this mode, not the ones I had changed.
                                //
                                // Same treatment: size to the real content, cap it, and
                                // only show the bar when it is needed. `PALETTE_LIST_H`
                                // rather than a second hard-coded 320 — one number for
                                // how tall a palette list may get.
                                egui::ScrollArea::vertical()
                                    .id_salt("palette_go_to_route")
                                    .max_height(palette_list_h(ui))
                                    .auto_shrink([false, true])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                    .show(ui, |ui| {
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
                                egui::ScrollArea::vertical()
                                    .id_salt("palette_workspace_symbol")
                                    .max_height(palette_list_h(ui))
                                    .auto_shrink([false, true])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                    .show(ui, |ui| {
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
                                egui::ScrollArea::vertical()
                                    .id_salt("palette_project_search")
                                    .max_height(palette_list_h(ui))
                                    .auto_shrink([false, true])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                    .show(ui, |ui| {
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
                        Mode::VoiceCommand => {
                            // Computed once, before the row, so both the
                            // button (inside the closure below) and the
                            // poll check (after it) see the same value --
                            // an earlier draft computed this INSIDE the
                            // closure and then tried to read it outside,
                            // which does not compile (out of scope).
                            // Spoken input needs the `voice` feature (mic +
                            // whisper); without it the mode is typed only.
                            #[cfg(not(feature = "voice"))]
                            ui.label(
                                egui::RichText::new("type what you'd say, then Enter · Esc cancel (no microphone in this build)")
                                    .color(colors.text_weak)
                                    .small(),
                            );
                            #[cfg(feature = "voice")]
                            {
                            let stage_listening = matches!(
                                state.voice_popup.stage,
                                uikit_egui::voice_popup::Stage::Listening
                                    | uikit_egui::voice_popup::Stage::Transcribing
                            );
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("type what you'd say, then Enter · Esc cancel")
                                        .color(colors.text_weak)
                                        .small(),
                                );
                                // The real mic control this mode never had --
                                // `aperture_core::voice`'s own doc named this
                                // as future work (no capture existed here at
                                // all before). Click starts, right-click/hold
                                // cancels; a result FILLS `p.input` rather
                                // than auto-submitting, so speaking and
                                // typing produce the identical, reviewable
                                // outcome the "typed stand-in" framing
                                // promises -- Enter still does the actual
                                // submitting either way.
                                let dt = ui.ctx().input(|i| i.stable_dt);
                                match state.mic_indicator.draw(ui, colors, 0.0, stage_listening, dt) {
                                    uikit_egui::mic_indicator::MicAction::Start => {
                                        state.voice_popup.start(false);
                                    }
                                    uikit_egui::mic_indicator::MicAction::ForceOff => {
                                        state.voice_popup.cancel();
                                    }
                                    uikit_egui::mic_indicator::MicAction::None => {}
                                }
                            });
                            if stage_listening {
                                state.voice_popup.tick();
                                ui.ctx().request_repaint();
                            }
                            match &state.voice_popup.stage {
                                uikit_egui::voice_popup::Stage::Listening => {
                                    ui.label(egui::RichText::new("listening…").color(colors.text_weak));
                                }
                                uikit_egui::voice_popup::Stage::Transcribing => {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(egui::RichText::new("thinking…").color(colors.text_weak));
                                    });
                                }
                                uikit_egui::voice_popup::Stage::Done { text } => {
                                    // Filled once: reset to Listening right
                                    // after copying out, so the SAME result
                                    // is not re-copied into `p.input` on
                                    // every subsequent frame (Stage::Done
                                    // stays Done until the next capture).
                                    if !text.is_empty() {
                                        p.input = text.clone();
                                    }
                                    state.voice_popup.stage = uikit_egui::voice_popup::Stage::Listening;
                                }
                                uikit_egui::voice_popup::Stage::Error { message } => {
                                    // The "no notifier of a missing mic"
                                    // report, addressed here: whatever
                                    // voice-engine-kit's real failure
                                    // string says (no input device, no
                                    // model found, arecord/whisper
                                    // missing) is now visible instead of
                                    // the mode silently doing nothing.
                                    ui.label(egui::RichText::new(message.as_str()).color(colors.danger));
                                }
                            }
                            }
                            if enter && !p.input.trim().is_empty() {
                                submit_voice = Some(p.input.trim().to_string());
                            }
                        }
                    }
                });
        });

    // One frame of follow, then off. Set by the arrow handling above and
    // cleared here, after the list has been drawn and had its chance to call
    // `scroll_to_me` — so the view chases the keyboard exactly once per press
    // and never on a frame the user is dragging.
    if let Some(p) = state.palette.as_mut() {
        p.follow_selection = false;
    }

    if let Some(instruction) = submit_edit {
        state.request_edit(&instruction, now_ms);
        state.palette = None;
        return;
    }

    if let Some(utterance) = submit_voice {
        if let Some(clip) = state.run_voice_command(&utterance, now_ms) {
            ctx.copy_text(clip);
        }
        state.palette = None;
        return;
    }

    if let Some((label, cmd)) = apply {
        state
            .cmd_strip
            .used(uikit_egui::cmdstrip::Cmd::new(&label, &label));
        palette_ui::apply(cmd, state, now_ms);
        return;
    }
    use aperture_core::entity::Relation;
    if let Some(path) = open_path {
        state.palette_jump(std::path::PathBuf::from(path), None, Relation::Opened, now_ms);
        state.palette = None;
        return;
    }
    if let Some((path, line)) = open_hit {
        // A search hit is a text match; a symbol or route is a reference.
        let rel = if hit_mode == Some(Mode::ProjectSearch) { Relation::TextMatch } else { Relation::References };
        state.palette_jump(path, Some(line), rel, now_ms);
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
        EK::D => K::D,
        EK::U => K::U,
        EK::L => K::L,
        EK::I => K::I,
        EK::F => K::F,
        EK::F3 => K::F3,
        EK::F12 => K::F12,
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
    use super::{palette_list_h_for, PALETTE_LIST_H, PALETTE_LIST_H_MAX};

    #[test]
    fn a_phone_keeps_the_bound_the_fixed_height_gave_it() {
        // The regression that matters. PALETTE_LIST_H exists because a
        // palette taller than the screen runs off the bottom of a phone and
        // a Ui clips rather than scrolls (C13_TODO §24). Making the list
        // taller on desktop must not touch that.
        //
        // 720p portrait phone, and a short one.
        for h in [1280.0_f32, 960.0, 800.0] {
            let got = palette_list_h_for(h);
            assert!(got <= h - 96.0, "{h}px screen got a {got}px list, leaving no room");
        }
    }

    #[test]
    fn a_desktop_gets_a_taller_list_than_the_old_fixed_one() {
        // The point of the change: on a big monitor the fixed 360 showed
        // about eight rows and left the rest to the scrollbar.
        assert!(palette_list_h_for(2160.0) > PALETTE_LIST_H);
        assert!(palette_list_h_for(1440.0) > PALETTE_LIST_H);
    }

    #[test]
    fn it_stops_growing_before_the_list_stops_being_scannable() {
        // Past the ceiling you are reading a page rather than picking from
        // a menu, and the fuzzy filter narrows better than the eye does.
        assert_eq!(palette_list_h_for(4320.0), PALETTE_LIST_H_MAX);
    }

    #[test]
    fn a_very_short_window_never_gets_a_list_taller_than_itself() {
        // The `.max(PALETTE_LIST_H)` floor would otherwise force 360px onto
        // a 200px window, and a list taller than the screen cannot be
        // scrolled to the bottom of. The window clamp is applied last for
        // exactly this reason.
        for h in [400.0_f32, 300.0, 200.0] {
            assert!(
                palette_list_h_for(h) < h,
                "{h}px window got a list at least as tall as itself"
            );
        }
    }

    #[test]
    fn it_never_returns_a_negative_or_zero_height() {
        // A degenerate window (minimised, or mid-resize) must not produce a
        // negative max_height — egui takes it as a constraint and the
        // arithmetic below it would go strange rather than loud.
        for h in [0.0_f32, 1.0, 50.0, 96.0] {
            assert!(palette_list_h_for(h) > 0.0, "{h}px screen produced a non-positive height");
        }
    }

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
