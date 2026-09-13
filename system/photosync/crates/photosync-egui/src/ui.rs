//! The frame loop. Grouping is `Library::by_day`; column/row placement is
//! `gallery_grid::grid_for` + `cell_pos`; this file only paints.
//!
//! No thumbnail decoder is wired in yet — tiles are painted as deterministic
//! colour swatches (same seed/hue trick `mono_tile` uses for app icons) so
//! the grid, the grouping and the selection model can be seen and used
//! before that lands. Swapping in real bitmaps means loading a `TextureId`
//! per `Item::thumbnail` and drawing it in place of `paint_tile`'s
//! `painter.rect_filled` — everything else here is already the shape that
//! needs.

use crate::{App, View};
use egui::{Color32, Context, CornerRadius, Pos2, Rect, ScrollArea, Sense, Stroke, Vec2};
use photosync::album::Item;
use photosync::gallery_grid;

pub fn draw(app: &mut App, ctx: &Context) {
    let colors = uikit_egui::apply_palette(ctx, snapclone_theme::palette_by_id("minimal"));
    app.backdrop.paint(ctx, &colors);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            // `Sidebar` owns its width as a plain field (see `sidebar.rs`'s
            // module note on why); clone it out so the closure below can
            // still borrow `app` for the rail's contents, then write any
            // drag/collapse change back.
            let mut rail = app.rail.clone();
            let (_resp, _) = rail.show_inside(ui, "rail", |ui| rail_body(app, ui, &colors));
            app.rail = rail;

            egui::CentralPanel::default().show_inside(ui, |ui| {
                if app.view == View::Settings {
                    settings_view(app, ui, &colors);
                } else {
                    header(app, ui, &colors);
                    ui.separator();
                    gallery(app, ui, &colors);
                }
            });
        });
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
    if app.new_album_open {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let resp = ui.text_edit_singleline(&mut app.new_album_name);
            if ui.button("Add").clicked() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                if app.library.create_album(&app.new_album_name) {
                    app.view = View::Album(app.new_album_name.trim().to_string());
                    app.status = format!("Created album \"{}\"", app.new_album_name.trim());
                } else {
                    app.status = "Album name already taken, or blank".into();
                }
                app.new_album_name.clear();
                app.new_album_open = false;
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if ui.button("+ New album").clicked() {
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
        let title = match &app.view {
            View::Library => format!("Library — {} items", app.library.len()),
            View::Album(name) => format!("{name} — {} items", app.library.album_items(name).len()),
            View::Settings => "Settings".to_string(),
        };
        ui.heading(egui::RichText::new(title).color(colors.text));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(16.0);
            if !app.selected.is_empty() {
                ui.label(egui::RichText::new(format!("{} selected", app.selected.len())).color(colors.text_weak));
                if ui.button("Clear").clicked() {
                    app.selected.clear();
                }
            }
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Tile size").color(colors.text_weak).size(11.0));
            ui.add(egui::Slider::new(&mut app.tile_target, gallery_grid::MIN_CELL..=280.0).show_value(false));
        });
    });
    if !app.status.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(egui::RichText::new(&app.status).color(colors.text_weak).size(11.0));
        });
    }
}

fn gallery(app: &mut App, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
    const GAP: f32 = 6.0;
    const GROUP_GAP: f32 = 22.0;

    let groups: Vec<(String, Vec<&Item>)> = match &app.view {
        View::Library => app.library.by_day(),
        View::Album(name) => {
            let items = app.library.album_items(name);
            if items.is_empty() {
                Vec::new()
            } else {
                vec![("Selected".to_string(), items)]
            }
        }
        View::Settings => Vec::new(), // unreachable: draw() routes Settings to settings_view instead
    };

    if app.library.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No photos yet").color(colors.text_weak));
        });
        return;
    }

    let width = ui.available_width() - 32.0;
    let target = app.tile_target;
    let mut toggled: Option<String> = None;

    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(8.0);
        for (day, items) in &groups {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(day).color(colors.text_weak).size(12.0).strong());
            });
            ui.add_space(6.0);

            let grid = gallery_grid::grid_for(width, target, GAP);
            let height = gallery_grid::grid_height(&grid, items.len());
            let (rect, _resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
            let origin = Pos2::new(rect.min.x + 16.0, rect.min.y);

            // Only paint what's actually in the visible band — the point of
            // `visible_range` is that this stays cheap into the thousands.
            let clip = ui.clip_rect();
            let scroll_y = (clip.min.y - rect.min.y).max(0.0);
            let viewport_h = clip.height();
            let (lo, hi) = gallery_grid::visible_range(&grid, items.len(), scroll_y, viewport_h);

            if ui.is_rect_visible(rect) {
                for idx in lo..hi {
                    let item = items[idx];
                    let (x, y) = gallery_grid::cell_pos(&grid, idx);
                    let cell_rect = Rect::from_min_size(
                        Pos2::new(origin.x + x, origin.y + y),
                        Vec2::splat(grid.cell),
                    );
                    if paint_tile(ui, cell_rect, item, app.selected.contains(&item.path), colors, app.settings.thumb_style) {
                        toggled = Some(item.path.clone());
                    }
                }
            }
            ui.add_space(GROUP_GAP);
        }
    });

    if let Some(path) = toggled {
        if !app.selected.remove(&path) {
            app.selected.insert(path);
        }
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
    item: &Item,
    selected: bool,
    colors: &uikit_egui::PaletteColors,
    style: crate::settings::ThumbStyle,
) -> bool {
    let resp = ui.interact(rect, ui.id().with(("tile", &item.path)), Sense::click());
    let painter = ui.painter();

    // Deterministic placeholder colour — same FNV-1a + HSL trick as
    // `mono_tile`, seeded on the path, so a given photo always renders the
    // same hue until a real thumbnail replaces it.
    let hue = uikit_egui::mono_tile::hue(&item.path);
    let base = uikit_egui::mono_tile::hsla(hue, 0.42, 0.55, 1.0);

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
            let mut seed = uikit_egui::mono_tile::hue(&item.path) as u64;
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

    resp.clicked()
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
                egui::RichText::new("This build has no daemon behind it yet — these edit the config a future sync would run with, the same as toggling a setting before the first run.")
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
                    egui::RichText::new("Procedural, not decoded — there's no thumbnail cache on disk to grow over time.")
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
