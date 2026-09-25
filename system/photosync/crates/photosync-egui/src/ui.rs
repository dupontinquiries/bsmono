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
    sync_thumbnails(app, ctx);
    if app.viewing.is_some() {
        viewer(app, ctx);
        return;
    }
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
            let resp = ui.add(egui::TextEdit::singleline(&mut app.new_album_name).id_salt("photosync-new_album"));
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
    let mut open: Option<String> = None;

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
                    // Ask for its thumbnail (only visible tiles get here, so
                    // the newest requests are what is on screen), and draw the
                    // texture once it has been uploaded.
                    if let Some(t) = &app.thumbs { let _ = t.get(&item.path); }
                    let thumb = app.textures.get(&item.path).map(|h| (h.id(), h.size_vec2()));
                    let hit = paint_tile(ui, cell_rect, item, thumb, app.selected.contains(&item.path), colors, app.settings.thumb_style);
                    if hit.double_clicked() {
                        // Open it. The double-click's FIRST click already toggled
                        // the selection; toggling again leaves it as it was.
                        open = Some(item.path.clone());
                        toggled = Some(item.path.clone());
                    } else if hit.clicked() {
                        toggled = Some(item.path.clone());
                    }
                }
            }
            ui.add_space(GROUP_GAP);
        }
    });

    if let Some(path) = open {
        app.viewing = Some(path);
    }
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
    thumb: Option<(egui::TextureId, Vec2)>,
    selected: bool,
    colors: &uikit_egui::PaletteColors,
    style: crate::settings::ThumbStyle,
) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("tile", &item.path)), Sense::click());
    let painter = ui.painter();

    // Deterministic placeholder colour — same FNV-1a + HSL trick as
    // `mono_tile`, seeded on the path, so a given photo always renders the
    // same hue until a real thumbnail replaces it.
    let hue = uikit_egui::mono_tile::hue(&item.path);
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
    let order: Vec<String> = app.library.by_day().into_iter().flat_map(|(_, items)| items.into_iter().map(|i| i.path.clone())).collect();
    let (esc, left, right) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
    let mut close = esc;
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_black_alpha(245))).show(ctx, |ui| {
        let area = ui.max_rect();
        let tex = match &app.full_tex {
            Some((p, t)) if *p == path => Some((t.id(), t.size_vec2())),
            _ => app.textures.get(&path).map(|t| (t.id(), t.size_vec2())),
        };
        match tex {
            Some((id, size)) => {
                let fit = (area.width() / size.x).min(area.height() / size.y);
                let r = Rect::from_center_size(area.center(), size * fit);
                ui.painter().image(id, r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, "loading…", egui::FontId::proportional(16.0), Color32::GRAY);
            }
        }
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ui.painter().text(area.left_top() + Vec2::new(16.0, 14.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(14.0), Color32::from_gray(210));
        let close_rect = Rect::from_min_size(area.right_top() + Vec2::new(-52.0, 8.0), Vec2::splat(44.0));
        if ui.put(close_rect, egui::Button::new("✕").frame(false)).on_hover_text("Back to the grid (Esc)").clicked() {
            close = true;
        }
    });
    if close {
        app.viewing = None;
        app.full_tex = None;
    } else if left || right {
        if let Some(next) = crate::step(&order, &path, if right { 1 } else { -1 }) {
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

