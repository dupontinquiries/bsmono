//! The frame loop. Grouping is `Library::by_day`; column/row placement is
//! `gallery_grid::grid_for` + `cell_pos`; what a press means is `selection`;
//! album changes are `App`'s methods. This file paints and routes input.
//!
//! Tiles show decoded thumbnails (`thumbs`); until one arrives a tile is a
//! placeholder in the style Settings picks (`settings::ThumbStyle`).

use crate::{App, View};
use egui::{Color32, Context, CornerRadius, Pos2, Rect, ScrollArea, Sense, Stroke, Vec2};
use photosync::gallery_grid;

pub fn draw(app: &mut App, ctx: &Context) {
    sync_library(app, ctx);
    sync_thumbnails(app, ctx);
    if app.viewing.is_some() {
        viewer(app, ctx);
        return;
    }
    let colors = uikit_egui::apply_palette(ctx, snapclone_theme::palette_by_id("minimal"));
    app.backdrop.paint(ctx, &colors);

    app.compact = crate::is_compact(ctx.content_rect().width());
    if !app.compact { app.drawer_open = false; }

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            // A phone has no room for the rail beside the grid: it becomes
            // the drawer (`drawer`), opened from the header's menu button.
            if !app.compact {
                // `Sidebar` owns its width as a plain field (see `sidebar.rs`'s
                // module note on why); clone it out so the closure below can
                // still borrow `app` for the rail's contents, then write any
                // drag/collapse change back.
                let mut rail = app.rail.clone();
                let (_resp, _) = rail.show_inside(ui, "rail", |ui| rail_body(app, ui, &colors));
                app.rail = rail;
            }

            let frame = if app.compact { egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::symmetric(2, 4)) }
                        else { egui::Frame::central_panel(ui.style()) };
            egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
                if app.compact {
                    compact_header(app, ui, &colors);
                    ui.separator();
                    if app.view == View::Settings { settings_view(app, ui, &colors); } else { gallery(app, ui, &colors); }
                } else if app.view == View::Settings {
                    settings_view(app, ui, &colors);
                } else {
                    header(app, ui, &colors);
                    ui.separator();
                    gallery(app, ui, &colors);
                }
            });
        });
    if app.compact && app.drawer_open {
        drawer(app, ctx, &colors);
    }
    if app.new_album_open {
        new_album_dialog(app, ctx);
    }
}

/// The rail's contents as a drawer over the page, from the left, with a
/// scrim that closes it. Choosing anything in it closes it too.
fn drawer(app: &mut App, ctx: &Context, colors: &uikit_egui::PaletteColors) {
    let screen = ctx.content_rect();
    let scrim = egui::Area::new(egui::Id::new("photosync-drawer-scrim"))
        .order(egui::Order::Middle).fixed_pos(screen.min).constrain(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(110));
            ui.allocate_rect(screen, Sense::click())
        });
    let width = 280.0_f32.min(screen.width() - 56.0);
    let panel = Rect::from_min_size(screen.min, Vec2::new(width, screen.height()));
    let (before_view, before_new) = (app.view.clone(), app.new_album_open);
    egui::Area::new(egui::Id::new("photosync-drawer")).order(egui::Order::Foreground)
        .fixed_pos(screen.min).constrain(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(panel, 0.0, colors.bg_base);
            ui.allocate_rect(panel, Sense::click()); // presses on its empty parts are the drawer's
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(panel.shrink2(Vec2::new(8.0, 12.0))));
            rail_body(app, &mut inner, colors);
        });
    let back = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if scrim.inner.clicked() || back || app.view != before_view || app.new_album_open != before_new {
        app.drawer_open = false;
    }
}

/// The header on a phone: one row. Menu button and title; while selecting,
/// a close button, the count and the actions, the way a phone gallery's
/// action bar replaces its title bar. Buttons are 40 px tall — a finger,
/// not a pointer.
fn compact_header(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    let big = Vec2::new(40.0, 40.0);
    ui.horizontal(|ui| {
        if app.selected.is_empty() {
            if ui.add(egui::Button::new("☰").frame(false).min_size(big)).on_hover_text("Albums and settings").clicked() {
                app.drawer_open = true;
            }
            let title = match &app.view {
                View::Library => "Library".to_string(),
                View::Album(name) => name.clone(),
                View::Settings => "Settings".to_string(),
            };
            ui.label(egui::RichText::new(title).color(colors.text).size(18.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                if let View::Album(name) = app.view.clone() {
                    album_delete(app, ui, colors, &name);
                } else if app.view == View::Library {
                    ui.label(egui::RichText::new(format!("{}", app.library.len())).color(colors.text_weak));
                }
            });
        } else {
            if ui.add(egui::Button::new("✕").frame(false).min_size(big)).on_hover_text("Deselect all").clicked() {
                app.selected.clear();
                return;
            }
            ui.label(egui::RichText::new(format!("{} selected", app.selected.len())).color(colors.text).size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let names: Vec<String> = app.library.album_names().into_iter().map(String::from).collect();
                let in_album = if let View::Album(n) = &app.view { Some(n.clone()) } else { None };
                ui.menu_button(egui::RichText::new("⋮").size(18.0), |ui| {
                    if ui.button("Select all").clicked() {
                        let all = app.visible_order();
                        app.selected.set(all);
                        ui.close();
                    }
                    if let Some(name) = &in_album {
                        if ui.button("Remove from album").clicked() {
                            app.remove_selection_from(name);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Add to album", |ui| {
                    for name in &names {
                        if in_album.as_deref() == Some(name.as_str()) { continue; }
                        if ui.button(name).clicked() {
                            app.add_selection_to(name);
                            ui.close();
                        }
                    }
                    if !names.is_empty() { ui.separator(); }
                    if ui.button("New album…").clicked() {
                        app.new_album_open = true;
                        ui.close();
                    }
                });
            });
        }
    });
    if !app.status.is_empty() {
        ui.label(egui::RichText::new(&app.status).color(colors.text_weak).size(11.0));
    }
}

/// Name a new album. A modal rather than a field in the rail: the rail
/// collapses to icons on a phone, and the field it held was not focused
/// when "New album…" opened it, so typing went nowhere.
fn new_album_dialog(app: &mut App, ctx: &Context) {
    let n = app.selected.len();
    let modal = egui::Modal::new(egui::Id::new("photosync-new-album")).show(ctx, |ui| {
        ui.set_width(280.0_f32.min(ctx.content_rect().width() - 48.0));
        ui.heading("New album");
        if n > 0 {
            ui.label(format!("With the {n} selected photo{}.", if n == 1 { "" } else { "s" }));
        }
        ui.add_space(6.0);
        let resp = ui.add(egui::TextEdit::singleline(&mut app.new_album_name)
            .id_salt("photosync-new_album").hint_text("Album name").desired_width(f32::INFINITY));
        if !resp.has_focus() && !resp.lost_focus() { resp.request_focus(); }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(8.0);
        let mut done = None;
        let taken = app.library.album(app.new_album_name.trim()).is_some();
        if taken {
            ui.colored_label(ui.visuals().warn_fg_color, "There is already an album with that name.");
        }
        ui.horizontal(|ui| {
            let ok = !app.new_album_name.trim().is_empty() && !taken;
            if ui.add_enabled(ok, egui::Button::new("Create")).clicked() || (enter && ok) { done = Some(true); }
            if ui.button("Cancel").clicked() { done = Some(false); }
        });
        done
    });
    let outcome = if modal.should_close() { Some(false) } else { modal.inner };
    match outcome {
        Some(true) => {
            let name = std::mem::take(&mut app.new_album_name);
            if app.new_album(&name) { app.new_album_open = false; } else { app.new_album_name = name; }
        }
        Some(false) => { app.new_album_open = false; app.new_album_name.clear(); }
        None => {}
    }
}

fn rail_body(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.heading(egui::RichText::new("photosync").color(colors.text).size(18.0));
    });
    ui.add_space(12.0);

    if nav_row(ui, colors, "Library", app.view == View::Library).clicked() {
        app.view = View::Library;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("ALBUMS").color(colors.text_weak).size(11.0));
    });

    let names: Vec<String> = app.library.album_names().into_iter().map(String::from).collect();
    for name in &names {
        let selected = app.view == View::Album(name.clone());
        if nav_row(ui, colors, name, selected).clicked() {
            app.view = View::Album(name.clone());
        }
    }

    ui.add_space(6.0);
    {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let label = if app.selected.is_empty() { "+ New album".to_string() }
                        else { format!("+ New album from {}", app.selected.len()) };
            if ui.button(label).clicked() {
                app.new_album_open = true;
            }
        });
    }

    // Settings as an ordinary destination in the same list, not a gear
    // icon off to a separate window — see `View::Settings`'s doc.
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(4.0);
    if nav_row(ui, colors, "Settings", app.view == View::Settings).clicked() {
        app.view = View::Settings;
    }
}

/// One clickable rail row: an accent bar on the left when selected, matching
/// the flat list style the fleet's other sidebars use rather than a filled
/// pill (a photo grid wants the chrome quiet).
fn nav_row(
    ui: &mut egui::Ui,
    colors: &uikit_egui::PaletteColors,
    label: &str,
    selected: bool,
) -> egui::Response {
    let desired = Vec2::new(ui.available_width(), 34.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, CornerRadius::same(6), colors.bg_hover);
            painter.rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
                CornerRadius::same(0),
                colors.accent,
            );
        } else if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(6), colors.bg_raised);
        }
        painter.text(
            Pos2::new(rect.min.x + 16.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            if selected { colors.text } else { colors.text_weak },
        );
    }
    resp
}

fn header(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let n = app.groups().len();
        let title = match &app.view {
            View::Library => format!("Library — {n} items"),
            View::Album(name) => format!("{name} — {n} items"),
            View::Settings => "Settings".to_string(),
        };
        ui.heading(egui::RichText::new(title).color(colors.text));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(16.0);
            if app.selected.is_empty() {
                ui.label(egui::RichText::new("Tile size").color(colors.text_weak).size(11.0));
                ui.add(egui::Slider::new(&mut app.tile_target, gallery_grid::MIN_CELL..=280.0).show_value(false));
                if let View::Album(name) = app.view.clone() {
                    ui.add_space(12.0);
                    album_delete(app, ui, colors, &name);
                }
            } else {
                selection_bar(app, ui, colors);
            }
        });
    });
    if !app.status.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(egui::RichText::new(&app.status).color(colors.text_weak).size(11.0));
        });
    }
}

/// What can be done with a selection. Drawn right to left, so the first
/// control added is the rightmost: Clear sits at the edge, where a thumb or
/// the pointer finds it.
fn selection_bar(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    if ui.button("Clear").on_hover_text("Deselect all (Esc)").clicked() {
        app.selected.clear();
    }
    if ui.button("Select all").clicked() {
        let all = app.visible_order();
        app.selected.set(all);
    }
    if let View::Album(name) = app.view.clone() {
        if ui.button("Remove from album").on_hover_text("The photos stay in the library").clicked() {
            app.remove_selection_from(&name);
        }
    }
    let names: Vec<String> = app.library.album_names().into_iter().map(String::from).collect();
    ui.menu_button("Add to album", |ui| {
        for name in &names {
            if app.view == View::Album(name.clone()) { continue; }
            if ui.button(name).clicked() {
                app.add_selection_to(name);
                ui.close();
            }
        }
        if !names.is_empty() { ui.separator(); }
        if ui.button("New album…").clicked() {
            app.new_album_open = true;
            ui.close();
        }
    });
    ui.label(egui::RichText::new(format!("{} selected", app.selected.len())).color(colors.text));
}

/// Delete the open album, in two presses: the first arms, the second deletes.
/// Photos are never touched, but an album is someone's curation and one
/// stray tap should not undo it.
fn album_delete(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors, name: &str) {
    if app.confirm_delete.as_deref() == Some(name) {
        if ui.button("Cancel").clicked() { app.confirm_delete = None; }
        let confirm = egui::Button::new(egui::RichText::new("Delete it").color(Color32::WHITE)).fill(colors.danger);
        if ui.add(confirm).on_hover_text("The photos stay in the library").clicked() {
            app.delete_album(name);
        }
    } else if ui.button("Delete album").clicked() {
        app.confirm_delete = Some(name.to_string());
    }
}

fn gallery(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    const GAP: f32 = 6.0;
    let group_gap: f32 = if app.compact { 12.0 } else { 22.0 };

    let groups = app.groups();

    if app.library.is_empty() || groups.is_empty() {
        let msg = if app.library.is_empty() { "No photos yet" }
                  else { "This album is empty. In the Library, long-press (or right-click) photos to select them, then Add to album." };
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(msg).color(colors.text_weak));
        });
        return;
    }

    // A phone spends its width on photos: 2 px margins and the tile size
    // from the screen (three or four across), not the desktop's slider.
    let margin = if app.compact { 2.0 } else { 16.0 };
    let gap = if app.compact { 3.0 } else { GAP };
    let width = ui.available_width() - 2.0 * margin;
    let target = if app.compact { crate::compact_tile_target(width, gap) } else { app.tile_target };
    app.last_tiles.clear();
    let mut pressed: Option<(String, crate::selection::Press)> = None;
    let mods = ui.input(|i| i.modifiers);

    // Where each day sits, computed for the whole library from arithmetic
    // alone (no widgets): the scroll area is given the total height and only
    // the days that intersect the viewport are laid out. Laying out every
    // day's heading every frame cost as much as the grouping did.
    const TOP: f32 = 8.0;
    const HEADING: f32 = 26.0;
    let grid = gallery_grid::grid_for(width, target, gap);
    let mut spans = Vec::with_capacity(groups.days.len());
    let mut y = TOP;
    for (_, paths) in &groups.days {
        let h = HEADING + gallery_grid::grid_height(&grid, paths.len());
        spans.push((y, h));
        y += h + group_gap;
    }
    let total = y;

    ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let (content, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), total), Sense::hover());
        let first = spans.partition_point(|(y, h)| y + h < viewport.min.y);
        for (gi, (day, paths)) in groups.days.iter().enumerate().skip(first) {
            let (gy, _) = spans[gi];
            if gy > viewport.max.y { break; }
            let top = content.min + Vec2::new(0.0, gy);
            ui.painter().text(top + Vec2::new(margin + 4.0, 4.0), egui::Align2::LEFT_TOP, day,
                egui::FontId::proportional(12.5), colors.text_weak);
            let origin = top + Vec2::new(margin, HEADING);
            // Only the rows in the visible band — the point of
            // `visible_range` is that this stays cheap into the thousands.
            // The band of THIS grid inside the viewport: from its top or the
            // viewport's, to the viewport's bottom (a day starting halfway
            // down gets half a screen, not a whole one).
            let scroll_y = (viewport.min.y - (gy + HEADING)).max(0.0);
            let band = (viewport.max.y - (gy + HEADING) - scroll_y).max(0.0);
            let (lo, hi) = gallery_grid::visible_range(&grid, paths.len(), scroll_y, band);
            for idx in lo..hi {
                let path = &paths[idx];
                let (x, y) = gallery_grid::cell_pos(&grid, idx);
                let cell_rect = Rect::from_min_size(origin + Vec2::new(x, y), Vec2::splat(grid.cell));
                // Ask for its thumbnail (only visible tiles get here, so the
                // newest requests are what is on screen), and draw the texture
                // once it has been uploaded.
                if let Some(t) = &app.thumbs { let _ = t.get(path); }
                let thumb = app.textures.get(path).map(|h| (h.id(), h.size_vec2()));
                app.last_tiles.push((path.clone(), cell_rect));
                let hit = paint_tile(ui, cell_rect, path, thumb, app.selected.contains(path), colors, app.settings.thumb_style);
                // Long-press on a touch screen and right-click both arrive as
                // secondary_clicked, and neither as clicked() (checked:
                // swapping these two tests fails nothing).
                if hit.secondary_clicked() {
                    pressed = Some((path.clone(), crate::selection::Press::Secondary));
                } else if hit.clicked() {
                    pressed = Some((path.clone(), crate::selection::Press::Primary { ctrl: mods.command, shift: mods.shift }));
                }
            }
        }
    });

    if let Some((path, press)) = pressed {
        let order = app.visible_order();
        if let Some(open) = app.selected.press(&order, &path, press) {
            app.viewing = Some(open);
        }
    }
    if !app.selected.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.selected.clear();
    }
}

/// One thumbnail. Returns `true` on click (caller toggles selection — this
/// stays a pure paint-and-report function, no state mutation here).
///
/// No decoder, no cache, nothing written to disk — see `settings::ThumbStyle`'s
/// doc. All three styles are the same per-frame painter work at a different
/// price: `Solid` is a single rect, `Blur`/`Tileset` are a handful more.
fn paint_tile(
    ui: &mut egui::Ui,
    rect: Rect,
    path: &str,
    thumb: Option<(egui::TextureId, Vec2)>,
    selected: bool,
    colors: &uikit_egui::PaletteColors,
    style: crate::settings::ThumbStyle,
) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("tile", path)), Sense::click());
    let painter = ui.painter();

    // Deterministic placeholder colour — same FNV-1a + HSL trick as
    // `mono_tile`, seeded on the path, so a given photo always renders the
    // same hue until a real thumbnail replaces it.
    let hue = uikit_egui::mono_tile::hue(path);
    let base = uikit_egui::mono_tile::hsla(hue, 0.42, 0.55, 1.0);

    if let Some((tex, size)) = thumb {
        // The real picture, cropped to fill the square cell.
        painter.image(tex, rect, cover_uv(size, rect.size()), egui::Color32::WHITE);
    } else {
    match style {
        crate::settings::ThumbStyle::Solid => {
            painter.rect_filled(rect, CornerRadius::same(8), base);
        }
        crate::settings::ThumbStyle::Blur => {
            // A soft radial gradient isn't a primitive egui exposes, so
            // fake one with a handful of concentric, widening, fading
            // circles — cheap enough to redo every frame for every visible
            // tile (`visible_range` already keeps that count small).
            let dark = uikit_egui::mono_tile::hsla(hue, 0.42, 0.30, 1.0);
            painter.rect_filled(rect, CornerRadius::same(8), dark);
            let center = rect.center();
            let max_r = rect.width().max(rect.height()) * 0.62;
            const RINGS: u32 = 5;
            for i in (0..RINGS).rev() {
                let t = i as f32 / (RINGS - 1) as f32; // 1.0 outer .. 0.0 inner
                let r = max_r * (0.35 + 0.65 * t);
                let light = uikit_egui::mono_tile::hsla(hue, 0.42, 0.40 + 0.28 * (1.0 - t), 1.0);
                painter.circle_filled(center, r, light);
            }
        }
        crate::settings::ThumbStyle::Tileset => {
            let dark = uikit_egui::mono_tile::hsla(hue, 0.30, 0.22, 1.0);
            painter.rect_filled(rect, CornerRadius::same(8), dark);
            // A small hashed dot lattice, contact-sheet style: which cells
            // in the grid get a dot (and how bright) comes from the same
            // path hash as the hue, so a tile's pattern is as stable as
            // its colour is.
            let cols = 5i32;
            let rows = 5i32;
            let cell_w = rect.width() / cols as f32;
            let cell_h = rect.height() / rows as f32;
            let mut seed = uikit_egui::mono_tile::hue(path) as u64;
            for row in 0..rows {
                for col in 0..cols {
                    // xorshift-ish mix so nearby cells don't look correlated
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if seed % 3 != 0 {
                        continue;
                    }
                    let cx = rect.min.x + cell_w * (col as f32 + 0.5);
                    let cy = rect.min.y + cell_h * (row as f32 + 0.5);
                    let light = uikit_egui::mono_tile::hsla(hue, 0.45, 0.55, 1.0);
                    painter.circle_filled(Pos2::new(cx, cy), cell_w.min(cell_h) * 0.28, light);
                }
            }
        }
    }
    }

    if resp.hovered() {
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, colors.accent_soft), egui::StrokeKind::Outside);
    }
    if selected {
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(3.0, colors.accent), egui::StrokeKind::Outside);
        let badge_r = 9.0;
        let center = Pos2::new(rect.max.x - badge_r - 6.0, rect.min.y + badge_r + 6.0);
        painter.circle_filled(center, badge_r, colors.accent);
        painter.text(center, egui::Align2::CENTER_CENTER, "✓", egui::FontId::proportional(11.0), Color32::WHITE);
    }

    resp
}

/// The Settings tab. A plain scrolling page reached from the rail, the same
/// way gitmind-egui's Settings page works — not a modal or a separate
/// window, which is where GTK/libadwaita apps (Fotema included) usually put
/// this. There's also deliberately no "clear thumbnail cache" control here:
/// see `settings::ThumbStyle`'s doc for why there's nothing to clear.
fn settings_view(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    use photosync::{ChunkingStrategy, HashAlgorithm};

    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.heading(egui::RichText::new("Settings").color(colors.text));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("The gallery only reads the service's library. The sync settings below are not sent to the service: they edit this window's copy, for a sync started from here in future.")
                    .color(colors.text_weak)
                    .small(),
            );
        });
        ui.add_space(14.0);

        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("APPEARANCE").color(colors.text_weak).size(11.0));
                ui.add_space(6.0);
                ui.label("Thumbnail style");
                ui.label(
                    egui::RichText::new("What a tile shows until its thumbnail has loaded. Thumbnails are held in memory only — nothing is cached on disk.")
                        .color(colors.text_weak)
                        .small(),
                );
                ui.horizontal(|ui| {
                    for s in crate::settings::ThumbStyle::ALL {
                        if ui.selectable_label(app.settings.thumb_style == s, s.label()).clicked() {
                            app.settings.thumb_style = s;
                        }
                    }
                });

                ui.add_space(18.0);
                ui.label(egui::RichText::new("SYNC — CHUNKING & DEDUPLICATION").color(colors.text_weak).size(11.0));
                ui.add_space(6.0);
                ui.label("Chunking strategy");
                ui.horizontal(|ui| {
                    for (label, val) in [("Fixed-size", ChunkingStrategy::FixedSize), ("Content-defined", ChunkingStrategy::ContentDefined)] {
                        if ui.selectable_label(app.settings.dedup.chunking_strategy == val, label).clicked() {
                            app.settings.dedup.chunking_strategy = val;
                        }
                    }
                });

                ui.add_space(8.0);
                ui.label("Hash algorithm");
                ui.horizontal(|ui| {
                    for (label, val) in [("XXH64 (fast)", HashAlgorithm::XXH64), ("SHA-256 (verified)", HashAlgorithm::SHA256)] {
                        if ui.selectable_label(app.settings.dedup.hash_algorithm == val, label).clicked() {
                            app.settings.dedup.hash_algorithm = val;
                        }
                    }
                });

                ui.add_space(8.0);
                let mut chunk_mib = app.settings.dedup.chunk_size as f32 / (1024.0 * 1024.0);
                ui.add(egui::Slider::new(&mut chunk_mib, 0.25..=8.0).text("Chunk size (MiB)"));
                app.settings.dedup.chunk_size = (chunk_mib * 1024.0 * 1024.0) as usize;

                ui.add_space(4.0);
                ui.add(egui::Slider::new(&mut app.settings.dedup.compression_level, 1..=22).text("Compression level (zstd)"));

                if app.settings.dedup.chunking_strategy == ChunkingStrategy::ContentDefined {
                    ui.add_space(8.0);
                    let mut min_kib = app.settings.dedup.min_chunk_size as f32 / 1024.0;
                    let mut max_mib = app.settings.dedup.max_chunk_size as f32 / (1024.0 * 1024.0);
                    ui.add(egui::Slider::new(&mut min_kib, 32.0..=1024.0).text("CDC min chunk (KiB)"));
                    ui.add(egui::Slider::new(&mut max_mib, 1.0..=32.0).text("CDC max chunk (MiB)"));
                    app.settings.dedup.min_chunk_size = (min_kib * 1024.0) as usize;
                    app.settings.dedup.max_chunk_size = (max_mib * 1024.0 * 1024.0) as usize;
                }
            });
        });
    });
}

/// Start watching the service's database on the first frame (it needs this
/// context to request a repaint), and swap in a library it has reloaded.
fn sync_library(app: &mut App, ctx: &Context) {
    if app.watch.is_none() {
        if let Some(db) = app.db_path.clone() {
            let repaint = ctx.clone();
            app.watch = crate::watch::Watch::start(db, app.watch_every,
                Some(std::sync::Arc::new(move || repaint.request_repaint()))).ok();
        }
    }
    if let Some(fresh) = app.watch.as_ref().and_then(|w| w.take()) {
        app.replace_library(fresh);
    }
}

/// Upload thumbnails finished since last frame and free evicted ones; start
/// the decoder on the first frame (it needs this context to request repaints).
fn sync_thumbnails(app: &mut App, ctx: &Context) {
    if app.thumbs.is_none() {
        let repaint = ctx.clone();
        // The service's stored thumbnails when this library came from its
        // database (StoreDecode falls back to the original per file); else
        // decode originals.
        let decoder: std::sync::Arc<dyn crate::thumbs::Decode> = app.db_path.as_ref()
            .and_then(|db| {
                let hashes = app.library.by_day().into_iter().flat_map(|(_, items)| items.into_iter()
                    .filter_map(|i| i.thumbnail.clone().map(|h| (i.path.clone(), h)))).collect();
                crate::thumbs::StoreDecode::open(db, hashes).ok()
            })
            .map(|d| std::sync::Arc::new(d) as std::sync::Arc<dyn crate::thumbs::Decode>)
            .unwrap_or_else(|| std::sync::Arc::new(crate::thumbs::ImageDecode));
        app.thumbs = Some(crate::thumbs::Thumbs::new(
            decoder,
            THUMB_THREADS,
            THUMB_SIDE,
            THUMB_BUDGET_BYTES,
            THUMB_MAX_PENDING,
            Some(std::sync::Arc::new(move || repaint.request_repaint())),
        ));
    }
    let Some(t) = &app.thumbs else { return };
    for (path, img) in t.take_fresh() {
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.rgba);
        let tex = ctx.load_texture(format!("thumb:{path}"), ci, egui::TextureOptions::LINEAR);
        app.textures.insert(path, tex);
    }
    for path in t.take_evicted() {
        app.textures.remove(&path);
    }
}

/// The full-size viewer: the photo fitted to the window (never cropped), the
/// large decode when it has arrived and the grid's thumbnail scaled up until
/// then, Left/Right through the library in the grid's order, Esc or the close
/// button back to the grid.
fn viewer(app: &mut App, ctx: &Context) {
    let Some(path) = app.viewing.clone() else { return };
    if app.full.is_none() {
        let repaint = ctx.clone();
        app.full = Some(crate::thumbs::Thumbs::new(std::sync::Arc::new(crate::thumbs::ImageDecode), 1, FULL_SIDE,
            FULL_BUDGET_BYTES, 2, Some(std::sync::Arc::new(move || repaint.request_repaint()))));
    }
    if let Some(f) = &app.full {
        let _ = f.get(&path);
        for (p, img) in f.take_fresh() {
            if p == path {
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.rgba);
                app.full_tex = Some((p.clone(), ctx.load_texture(format!("full:{p}"), ci, egui::TextureOptions::LINEAR)));
            }
        }
    }
    // The view's own order: stepping through an album stays in the album.
    let order = app.visible_order();
    let (esc, left, right) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
    let mut close = esc;
    let mut step: i32 = if right { 1 } else if left { -1 } else { 0 };
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_black_alpha(245))).show(ctx, |ui| {
        let area = ui.max_rect();
        // SWIPE to the neighbours: the photo follows the finger, and the
        // release decides (`crate::swipe_step`). Registered before the
        // buttons so they, drawn later, win their own presses.
        let swipe = ui.interact(area, ui.id().with("viewer-swipe"), Sense::drag());
        if swipe.dragged() { app.viewer_dx += swipe.drag_delta().x; }
        if swipe.drag_stopped() {
            let vx = ui.input(|i| i.pointer.velocity().x);
            step = crate::swipe_step(app.viewer_dx, area.width(), vx);
            app.viewer_dx = 0.0;
        }
        let dx = app.viewer_dx;
        let tex = match &app.full_tex {
            Some((p, t)) if *p == path => Some((t.id(), t.size_vec2())),
            _ => app.textures.get(&path).map(|t| (t.id(), t.size_vec2())),
        };
        match tex {
            Some((id, size)) => {
                let fit = (area.width() / size.x).min(area.height() / size.y);
                let r = Rect::from_center_size(area.center() + Vec2::new(dx, 0.0), size * fit);
                ui.painter().image(id, r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, "loading…", egui::FontId::proportional(16.0), Color32::GRAY);
            }
        }
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ui.painter().text(area.left_top() + Vec2::new(16.0, 14.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(14.0), Color32::from_gray(210));
        let close_rect = Rect::from_min_size(area.right_top() + Vec2::new(-52.0, 8.0), Vec2::splat(44.0));
        if ui.put(close_rect, egui::Button::new(egui::RichText::new("✕").size(20.0).color(Color32::from_gray(230))).frame(false))
            .on_hover_text("Back to the grid (Esc)").clicked() {
            close = true;
        }
        // Previous / next for a pointer (a finger swipes). Only where there
        // is a neighbour, so a dead button never shows.
        let at = order.iter().position(|p| *p == path);
        let side = |x: f32| Rect::from_center_size(Pos2::new(x, area.center().y), Vec2::new(44.0, 88.0));
        let arrow = |t: &str| egui::Button::new(egui::RichText::new(t).size(28.0).color(Color32::from_gray(220))).frame(false);
        if at.is_some_and(|i| i > 0) && ui.put(side(area.left() + 30.0), arrow("‹")).on_hover_text("Previous (←)").clicked() {
            step = -1;
        }
        if at.is_some_and(|i| i + 1 < order.len()) && ui.put(side(area.right() - 30.0), arrow("›")).on_hover_text("Next (→)").clicked() {
            step = 1;
        }
    });
    if close {
        app.viewing = None;
        app.full_tex = None;
        app.viewer_dx = 0.0;
    } else if step != 0 {
        if let Some(next) = crate::step(&order, &path, step) {
            app.viewing = Some(next);
        }
    }
}

/// Long side of the viewer's decode: sharp on a large window, not the camera's
/// full 12+ MP (that would be a ~50 MB texture for one photo).
const FULL_SIDE: u32 = 2048;
/// The viewer holds the photo on screen and its neighbours' leftovers.
const FULL_BUDGET_BYTES: usize = 64 << 20;

/// Decoder threads: the UI keeps a core, and a phone has few.
const THUMB_THREADS: usize = 2;
/// Long side in px: a cell is ~168 pt, so this is sharp at 1.5x.
const THUMB_SIDE: u32 = 256;
/// Resident thumbnails, bytes: ~96 MB, a few hundred at 256 px.
const THUMB_BUDGET_BYTES: usize = 96 << 20;
/// Waiting requests kept: more than a screenful, fewer than a fast fling.
const THUMB_MAX_PENDING: usize = 256;

/// UV rect that crops an image of `img` size to fill a cell of `cell` size,
/// centred ("cover"): the long axis is trimmed evenly, nothing is stretched.
pub fn cover_uv(img: Vec2, cell: Vec2) -> Rect {
    if img.x <= 0.0 || img.y <= 0.0 || cell.x <= 0.0 || cell.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let (ia, ca) = (img.x / img.y, cell.x / cell.y);
    if ia > ca {
        let keep = ca / ia; // fraction of the width kept
        let m = (1.0 - keep) / 2.0;
        Rect::from_min_max(Pos2::new(m, 0.0), Pos2::new(1.0 - m, 1.0))
    } else {
        let keep = ia / ca;
        let m = (1.0 - keep) / 2.0;
        Rect::from_min_max(Pos2::new(0.0, m), Pos2::new(1.0, 1.0 - m))
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_side_evenly_and_never_stretches() {
        let sq = Vec2::splat(100.0);
        let wide = cover_uv(Vec2::new(400.0, 200.0), sq);
        assert!((wide.min.x - 0.25).abs() < 1e-6 && (wide.max.x - 0.75).abs() < 1e-6 && wide.min.y == 0.0 && wide.max.y == 1.0, "{wide:?}");
        let tall = cover_uv(Vec2::new(200.0, 400.0), sq);
        assert!((tall.min.y - 0.25).abs() < 1e-6 && (tall.max.y - 0.75).abs() < 1e-6 && tall.min.x == 0.0, "{tall:?}");
        assert_eq!(cover_uv(Vec2::new(300.0, 300.0), sq), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        assert_eq!(cover_uv(Vec2::ZERO, sq), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), "degenerate: whole image");
        // The kept region has the cell's aspect: nothing is stretched.
        let r = cover_uv(Vec2::new(640.0, 480.0), Vec2::new(90.0, 160.0));
        assert!(((r.width() * 640.0) / (r.height() * 480.0) - 90.0 / 160.0).abs() < 1e-4);
    }
}

#[cfg(test)]
mod draw_tests {
    use super::*;

    /// End to end through the real frame function: picture files on disk, a
    /// library pointing at them, `draw` run headless until the decoder's
    /// thumbnails come back and are uploaded as textures.
    #[test]
    fn the_grid_turns_files_on_disk_into_textures() {
        let dir = std::env::temp_dir().join(format!("photosync-draw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut items = Vec::new();
        for (i, (w, h)) in [(320u32, 200u32), (200, 320), (240, 240)].into_iter().enumerate() {
            let p = dir.join(format!("p{i}.png"));
            image::RgbaImage::from_pixel(w, h, image::Rgba([40 * i as u8, 120, 200, 255])).save(&p).unwrap();
            items.push(photosync::album::Item { path: p.to_string_lossy().into(), taken_at: 1_726_000_000 + i as i64 * 60,
                thumbnail: None, bytes: 1, metadata_json: None });
        }
        let mut app = App::new(photosync::album::Library::new(items));
        let ctx = egui::Context::default();
        let start = std::time::Instant::now();
        while app.textures.len() < 3 {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "only {} textures", app.textures.len());
            let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1040.0, 720.0))), ..Default::default() },
                            |ctx| draw(&mut app, ctx));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for (path, tex) in &app.textures {
            let s = tex.size();
            assert!(s[0].max(s[1]) <= THUMB_SIDE as usize && s[0].max(s[1]) > 0, "{path}: {s:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library the way the service writes one: PNGs imported by
    /// `PhotoSyncDB::import_photo`, dated by mtime a day apart.
    fn service_library(tag: &str, n: usize) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("photosync-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("blocks")).unwrap();
        let db_path = dir.join("photosync.db");
        let mut db = photosync::PhotoSyncDB::new(&db_path, &dir.join("blocks")).unwrap();
        for i in 0..n {
            let p = dir.join(format!("s{i}.png"));
            image::RgbImage::from_pixel(120, 90, image::Rgb([30 * i as u8, 120, 200])).save(&p).unwrap();
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_750_000_000 + i as u64 * 86_400);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            db.import_photo(&p, "test", 64).unwrap();
        }
        (dir, db_path)
    }

    struct Frames { ctx: egui::Context, t: f64 }
    impl Frames {
        fn new() -> Self { Frames { ctx: egui::Context::default(), t: 0.0 } }
        fn run(&mut self, app: &mut App, events: Vec<egui::Event>) {
            self.t += 0.05;
            let _ = self.ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1040.0, 720.0))),
                time: Some(self.t), events, ..Default::default() }, |ctx| draw(app, ctx));
        }
        fn click(&mut self, app: &mut App, at: Pos2, button: egui::PointerButton) {
            let b = |pressed| egui::Event::PointerButton { pos: at, button, pressed, modifiers: Default::default() };
            // Move in its own frame: a move and a press in one frame is a
            // drag of that distance, which scrolls the grid under the press.
            self.run(app, vec![egui::Event::PointerMoved(at)]);
            self.run(app, vec![b(true)]);
            self.run(app, vec![b(false)]);
            self.run(app, vec![]);
        }
        fn tile(&self, app: &App, path: &str) -> Pos2 {
            app.last_tiles.iter().find(|(p, _)| p == path).map(|(_, r)| r.center())
                .unwrap_or_else(|| panic!("{path} not drawn: {:?}", app.last_tiles.iter().map(|t| &t.0).collect::<Vec<_>>()))
        }
    }

    /// Through the real draw: a tap opens, a right-click starts selecting and
    /// then taps toggle, "Add to album" (App::new_album here, which the rail
    /// and the menu both call) keeps the selection in the new album — and
    /// the album is in the user's albums file when the library is opened
    /// again, while the service's database is not written.
    #[test]
    fn select_by_long_press_then_tap_and_the_album_survives_a_restart() {
        let (dir, db) = service_library("albums", 4);
        let albums = dir.join("user").join("albums.db");
        let before = std::fs::metadata(&db).unwrap().modified().unwrap();
        let mut app = App::from_path(&db).unwrap();
        app.open_albums(&albums).unwrap();
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        let order = app.visible_order();
        assert_eq!(order.len(), 4);

        // A tap opens the photo (no double-click needed).
        let at = f.tile(&app, &order[0]);
        f.click(&mut app, at, egui::PointerButton::Primary);
        assert_eq!(app.viewing.as_deref(), Some(order[0].as_str()), "a tap opens");
        assert!(app.selected.is_empty(), "and selects nothing");
        f.run(&mut app, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        assert_eq!(app.viewing, None);
        f.run(&mut app, vec![]);

        // Right-click (the desktop's long-press) selects; taps then toggle.
        let a = f.tile(&app, &order[1]);
        f.click(&mut app, a, egui::PointerButton::Secondary);
        assert!(app.selected.contains(&order[1]) && app.viewing.is_none());
        let b = f.tile(&app, &order[0]); // the two newest days are what fits on screen
        f.click(&mut app, b, egui::PointerButton::Primary);
        assert_eq!(app.selected.paths().len(), 2, "a tap while selecting adds");
        assert!(app.viewing.is_none(), "and does not open");

        assert!(app.new_album("Trip"));
        assert_eq!(app.view, View::Album("Trip".into()));
        assert!(app.selected.is_empty());
        f.run(&mut app, vec![]);
        assert_eq!(app.last_tiles.len(), 2, "the album shows its two photos");

        // Opened again: the album is there, from the user's file.
        let mut again = App::from_path(&db).unwrap();
        assert!(again.library.album_names().is_empty(), "not in the service's store");
        again.open_albums(&albums).unwrap();
        assert_eq!(again.library.album_items("Trip").len(), 2);
        assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), before, "the service's database was not written");

        // Remove one, delete the album: both persist; the photos stay.
        again.view = View::Album("Trip".into());
        again.selected.set([order[1].clone()]);
        assert_eq!(again.remove_selection_from("Trip"), 1);
        let mut third = App::from_path(&db).unwrap();
        third.open_albums(&albums).unwrap();
        assert_eq!(third.library.album_items("Trip").len(), 1);
        assert!(third.delete_album("Trip"));
        assert_eq!(third.view, View::Library);
        let mut fourth = App::from_path(&db).unwrap();
        fourth.open_albums(&albums).unwrap();
        assert!(fourth.library.album_names().is_empty(), "deleting the last album sticks");
        assert_eq!(fourth.library.len(), 4, "and the photos are all still there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On a touch screen, as egui-winit reports one (a Touch event beside the
    /// emulated pointer): holding a tile selects it and does NOT open it; a
    /// short tap afterwards adds to the selection.
    #[test]
    fn a_long_press_on_a_touch_screen_selects() {
        let (dir, db) = service_library("touch", 3);
        let mut app = App::from_path(&db).unwrap();
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        let order = app.visible_order();
        let touch = |at: Pos2, phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1),
            phase, pos: at, force: None };
        let btn = |at: Pos2, pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary,
            pressed, modifiers: Default::default() };
        let hold = |f: &mut Frames, app: &mut App, at: Pos2, frames: usize| {
            f.run(app, vec![egui::Event::PointerMoved(at)]);
            f.run(app, vec![touch(at, egui::TouchPhase::Start), btn(at, true)]);
            for _ in 0..frames { f.run(app, vec![touch(at, egui::TouchPhase::Move)]); }
            f.run(app, vec![touch(at, egui::TouchPhase::End), btn(at, false), egui::Event::PointerGone]);
            f.run(app, vec![]);
        };
        let a = f.tile(&app, &order[0]);
        hold(&mut f, &mut app, a, 24); // 1.2 s
        assert!(app.selected.contains(&order[0]), "a long press selects");
        assert_eq!(app.viewing, None, "and does not open");
        let b = f.tile(&app, &order[1]);
        hold(&mut f, &mut app, b, 1); // a tap
        assert_eq!(app.selected.len(), 2, "a tap while selecting adds");
        assert_eq!(app.viewing, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn run_at(f: &mut Frames, app: &mut App, size: Vec2, events: Vec<egui::Event>) {
        f.t += 0.05;
        let _ = f.ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(f.t), events, ..Default::default() }, |ctx| draw(app, ctx));
    }

    /// At a phone's width the rail is gone (it took 160 px of 380), tiles
    /// are three across from the edge, and the drawer opens over the page
    /// and closes from its scrim.
    #[test]
    fn on_a_phone_the_rail_is_a_drawer_and_tiles_are_three_across() {
        let (dir, db) = service_library("phone", 1);
        // Four photos on one day: one full row of three and one more.
        let mut app = App::from_path(&db).unwrap();
        let p0 = app.visible_order()[0].clone();
        let items: Vec<photosync::album::Item> = (0..4).map(|i| photosync::album::Item { path: format!("{p0}#{i}"),
            taken_at: 1_750_000_000 + i, thumbnail: None, bytes: 1, metadata_json: None }).collect();
        app.library = photosync::album::Library::new(items);
        let phone = Vec2::new(380.0, 740.0);
        let mut f = Frames::new();
        run_at(&mut f, &mut app, phone, vec![]);
        run_at(&mut f, &mut app, phone, vec![]);
        assert!(app.compact);
        let top = app.last_tiles.iter().map(|(_, r)| r.min.y).fold(f32::INFINITY, f32::min);
        let row: Vec<&Rect> = app.last_tiles.iter().filter(|(_, r)| (r.min.y - top).abs() < 1.0).map(|(_, r)| r).collect();
        assert_eq!(row.len(), 3, "three across: {:?}", app.last_tiles);
        assert!(row.iter().all(|r| r.width() > 100.0), "and big enough to see: {row:?}");
        assert!(app.last_tiles.iter().all(|(_, r)| r.min.x < 380.0 && r.min.x >= 0.0));
        assert!(row.iter().map(|r| r.min.x).fold(f32::INFINITY, f32::min) < 20.0, "from the edge: no rail beside them");

        app.drawer_open = true;
        run_at(&mut f, &mut app, phone, vec![]);
        let scrim = Pos2::new(360.0, 400.0); // right of the 280 px drawer
        let b = |pressed| egui::Event::PointerButton { pos: scrim, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run_at(&mut f, &mut app, phone, vec![egui::Event::PointerMoved(scrim)]);
        run_at(&mut f, &mut app, phone, vec![b(true)]);
        run_at(&mut f, &mut app, phone, vec![b(false)]);
        assert!(!app.drawer_open, "a tap on the scrim closes the drawer");
        assert!(app.viewing.is_none(), "and does not reach the tile under it");

        // Wide again: the rail is back and a drawer cannot be left open.
        app.drawer_open = true;
        run_at(&mut f, &mut app, Vec2::new(1040.0, 720.0), vec![]);
        assert!(!app.compact && !app.drawer_open);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A finger dragged across the viewer moves to the next photo; a short
    /// drag springs back.
    #[test]
    fn swiping_the_viewer_steps_and_a_short_drag_does_not() {
        let (dir, db) = service_library("swipe", 3);
        let mut app = App::from_path(&db).unwrap();
        let order = app.visible_order();
        app.viewing = Some(order[0].clone());
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        let drag = |f: &mut Frames, app: &mut App, from: f32, to: f32| {
            let y = 360.0;
            let b = |x: f32, pressed| egui::Event::PointerButton { pos: Pos2::new(x, y), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            f.run(app, vec![egui::Event::PointerMoved(Pos2::new(from, y))]);
            f.run(app, vec![b(from, true)]);
            for k in 1..=8 { f.run(app, vec![egui::Event::PointerMoved(Pos2::new(from + (to - from) * k as f32 / 8.0, y))]); }
            f.run(app, vec![b(to, false)]);
            f.run(app, vec![]);
        };
        drag(&mut f, &mut app, 520.0, 480.0);
        assert_eq!(app.viewing.as_deref(), Some(order[0].as_str()), "40 px, slowly: springs back");
        assert_eq!(app.viewer_dx, 0.0);
        drag(&mut f, &mut app, 800.0, 300.0);
        assert_eq!(app.viewing.as_deref(), Some(order[1].as_str()), "dragged left across: the next photo");
        drag(&mut f, &mut app, 300.0, 800.0);
        assert_eq!(app.viewing.as_deref(), Some(order[0].as_str()), "and back");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only the days in view are laid out, from arithmetic: scrolled far down
    /// a large library, the tiles drawn are the ones at that depth, in the
    /// grid's order, and a tap opens the photo under the finger.
    #[test]
    fn scrolled_deep_the_right_tiles_are_drawn_and_tapped() {
        let items: Vec<photosync::album::Item> = (0..3000).map(|i| photosync::album::Item { path: format!("/p/{i:05}.jpg"),
            taken_at: 1_600_000_000 + i * 20_000, thumbnail: None, bytes: 1, metadata_json: None }).collect();
        let mut app = App::new(photosync::album::Library::new(items));
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        let top: Vec<String> = app.last_tiles.iter().map(|t| t.0.clone()).collect();
        let order = app.visible_order();
        assert_eq!(top[0], order[0]);
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -60_000.0),
            phase: egui::TouchPhase::Move, modifiers: Default::default() };
        f.run(&mut app, vec![egui::Event::PointerMoved(Pos2::new(600.0, 400.0)), wheel]);
        for _ in 0..30 { f.run(&mut app, vec![]); }
        let drawn: Vec<usize> = app.last_tiles.iter().map(|(p, _)| order.iter().position(|o| o == p).unwrap()).collect();
        assert!(!drawn.is_empty() && drawn.len() < 80, "a screenful, not the library: {}", drawn.len());
        assert!(drawn[0] > 200, "deep in the library: first drawn is #{}", drawn[0]);
        assert!(drawn.windows(2).all(|w| w[0] < w[1]), "in the grid's order");
        // Every drawn tile is on screen or within the one row of overscan
        // `visible_range` adds to decode ahead, and they do not overlap.
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1040.0, 720.0));
        let row = app.last_tiles[0].1.height() + 6.0;
        assert!(app.last_tiles.iter().all(|(_, r)| r.intersects(screen.expand2(Vec2::new(0.0, row + 1.0)))),
            "{:?}", app.last_tiles.iter().map(|t| t.1.min.y).collect::<Vec<_>>());
        for (i, (_, a)) in app.last_tiles.iter().enumerate() {
            for (_, b) in &app.last_tiles[i + 1..] { assert!(!a.shrink(0.5).intersects(b.shrink(0.5)), "{a:?} {b:?}"); }
        }
        let (path, rect) = app.last_tiles.iter().find(|(_, r)| screen.shrink(60.0).contains_rect(*r)).cloned().expect("a whole tile in view");
        f.click(&mut app, rect.center(), egui::PointerButton::Primary);
        assert_eq!(app.viewing.as_deref(), Some(path.as_str()), "the tap opens the tile under it");
    }

    /// A big day that starts halfway down the screen: only its rows that are
    /// on screen (plus the overscan row) are drawn — not a screenful counted
    /// from where it starts — and there is room for its heading between the
    /// days.
    #[test]
    fn a_long_day_below_a_short_one_draws_only_what_is_on_screen() {
        let day = 86_400;
        let mut items: Vec<photosync::album::Item> = (0..2).map(|i| photosync::album::Item { path: format!("/new/{i}"),
            taken_at: 1_750_000_000 + day + i, thumbnail: None, bytes: 1, metadata_json: None }).collect();
        items.extend((0..400).map(|i| photosync::album::Item { path: format!("/old/{i:03}"),
            taken_at: 1_750_000_000 + i, thumbnail: None, bytes: 1, metadata_json: None }));
        let mut app = App::new(photosync::album::Library::new(items));
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        f.run(&mut app, vec![]);
        let row = app.last_tiles[0].1.height() + 6.0;
        let bottom = 720.0 + row + 1.0;
        assert!(app.last_tiles.iter().all(|(_, r)| r.min.y < bottom),
            "drawn below the screen and its overscan row: {:?}", app.last_tiles.iter().map(|t| t.1.min.y).collect::<Vec<_>>());
        let new_bottom = app.last_tiles.iter().filter(|t| t.0.starts_with("/new/")).map(|t| t.1.max.y).fold(0.0, f32::max);
        let old_top = app.last_tiles.iter().filter(|t| t.0.starts_with("/old/")).map(|t| t.1.min.y).fold(f32::INFINITY, f32::min);
        assert!(old_top - new_bottom >= 22.0 + 20.0, "a gap and a heading between the days: {new_bottom} -> {old_top}");
    }

    /// A photo the service imports while the gallery is open appears, without
    /// losing the album, the selection or the open album view.
    #[test]
    fn a_photo_imported_while_open_appears_and_nothing_is_lost() {
        let (dir, db) = service_library("live", 2);
        let albums = dir.join("albums.db");
        let mut app = App::from_path(&db).unwrap();
        app.open_albums(&albums).unwrap();
        app.watch_every = std::time::Duration::from_millis(30);
        let order = app.visible_order();
        app.selected.set([order[0].clone()]);
        app.new_album("Kept");
        app.selected.set([order[1].clone()]);
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        assert!(app.watch.is_some(), "the first frame starts watching");

        let mut service = photosync::PhotoSyncDB::new(&db, &dir.join("blocks")).unwrap();
        let p = dir.join("late.png");
        image::RgbImage::from_pixel(80, 60, image::Rgb([200, 200, 20])).save(&p).unwrap();
        service.import_photo(&p, "test", 64).unwrap();
        drop(service);

        let start = std::time::Instant::now();
        while app.library.len() < 3 {
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "the new photo never appeared");
            f.run(&mut app, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(app.status, "1 new photo");
        assert_eq!(app.view, View::Album("Kept".into()), "still in the album");
        assert_eq!(app.library.album_items("Kept").len(), 1, "the album survived the reload");
        assert!(app.selected.contains(&order[1]), "and so did the selection");
        app.view = View::Library;
        assert!(app.visible_order().contains(&p.to_string_lossy().into_owned()), "and the grid shows it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In an album the viewer's arrows stay in the album; they used to walk
    /// the whole library.
    #[test]
    fn the_viewer_steps_through_the_album_it_was_opened_from() {
        let (dir, db) = service_library("albumview", 5);
        let mut app = App::from_path(&db).unwrap();
        let order = app.visible_order();
        app.library.create_album("Two");
        app.library.add_to_album("Two", &[order[0].clone(), order[3].clone()]);
        app.view = View::Album("Two".into());
        let mut f = Frames::new();
        f.run(&mut app, vec![]);
        let first = f.tile(&app, &order[0]);
        f.click(&mut app, first, egui::PointerButton::Primary);
        assert_eq!(app.viewing.as_deref(), Some(order[0].as_str()));
        let key = |k| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        f.run(&mut app, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(app.viewing.as_deref(), Some(order[3].as_str()), "the album's next photo, not the library's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The viewer, through the real frame function: opened on a photo, the
    /// large decode arrives at the picture's own size (it is under FULL_SIDE),
    /// the arrow keys step in the grid's order, Escape closes.
    #[test]
    fn the_viewer_shows_the_photo_large_steps_and_closes() {
        let dir = std::env::temp_dir().join(format!("photosync-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut items = Vec::new();
        for i in 0..3 {
            let p = dir.join(format!("v{i}.png"));
            image::RgbaImage::from_pixel(600, 400, image::Rgba([30 * i as u8, 90, 160, 255])).save(&p).unwrap();
            items.push(photosync::album::Item { path: p.to_string_lossy().into(), taken_at: 1_726_000_000 + i as i64 * 60,
                thumbnail: None, bytes: 1, metadata_json: None });
        }
        let mut app = App::new(photosync::album::Library::new(items));
        let order: Vec<String> = app.library.by_day().into_iter().flat_map(|(_, v)| v.into_iter().map(|i| i.path.clone())).collect();
        app.viewing = Some(order[1].clone());
        let ctx = egui::Context::default();
        let screen = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1040.0, 720.0)));
        let key = |k| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let start = std::time::Instant::now();
        while app.full_tex.as_ref().map(|(p, _)| p != &order[1]).unwrap_or(true) {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the large decode never arrived");
            let _ = ctx.run(egui::RawInput { screen_rect: screen, ..Default::default() }, |ctx| draw(&mut app, ctx));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.full_tex.as_ref().unwrap().1.size(), [600, 400], "full size, not the thumbnail");
        let _ = ctx.run(egui::RawInput { screen_rect: screen, events: vec![key(egui::Key::ArrowRight)], ..Default::default() }, |ctx| draw(&mut app, ctx));
        assert_eq!(app.viewing.as_deref(), Some(order[2].as_str()));
        let _ = ctx.run(egui::RawInput { screen_rect: screen, events: vec![key(egui::Key::ArrowRight)], ..Default::default() }, |ctx| draw(&mut app, ctx));
        assert_eq!(app.viewing.as_deref(), Some(order[2].as_str()), "clamped at the end");
        let _ = ctx.run(egui::RawInput { screen_rect: screen, events: vec![key(egui::Key::Escape)], ..Default::default() }, |ctx| draw(&mut app, ctx));
        assert_eq!(app.viewing, None);
        assert_eq!(crate::step(&order, "missing", 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

