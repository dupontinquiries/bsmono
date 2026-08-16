//! shell — the egui surface. Paints [`crate::anim::Board`] poses, feeds
//! pointer input through [`crate::tap_outcome`], and spawns
//! [`crate::LaunchSpec`]s. Everything decided here is *presentation or IO*;
//! behavior lives below in the lib.
//!
//! ## Input model (the touch rules, in one place)
//!
//! * Press remembers the origin and the tile under it; release measures total
//!   travel. [`crate::tap_outcome`] then decides launch / close / nothing —
//!   the tile must be under both press AND release.
//! * Vertical travel beyond slop scrolls the grid (finger-follows), with the
//!   pointer's release velocity carried into a decaying fling.
//! * A fast upward fling (release velocity < [`FLING_CLOSE_VELOCITY`]) closes,
//!   mirroring the plasmoid's swipe-up-to-close.
//! * Escape closes; any printable key focuses search.
//!
//! ## The keyboard inset seam
//!
//! [`Shell::kb_inset_px`] is honored by layout (search field and grid rise
//! above it) but nothing in this process *sets* it yet: winit/egui 0.34 do not
//! surface the compositor's input-panel rectangle. On the FP5 the value has to
//! come from the same place the Qt side gets it (maliit's panel surface) — a
//! zwp_input_panel/D-Bus listener feeding this field is the on-device task,
//! kept out of the core on purpose.

use crate::anim::TilePose;
use crate::grid::GridSpec;
use crate::{tap_outcome, LaunchSpec, Launcher, TapOutcome};
use egui::{Align2, Color32, CornerRadius, FontId, Frame as EguiFrame, Pos2, Rect, Sense, Stroke, Vec2};
use uikit_egui::motion::{PressFeedback, PressFeel};
use uikit_egui::PaletteColors;

/// Tile-press feel: quick dip, ripple that outlives release. Same shape as
/// the other -egui apps' tuned constants (see `motion.rs` precedent), sized
/// for a bigger touch target.
const TILE_PRESS_FEEL: PressFeel = PressFeel::new(110.0, 300.0, 0.94);

/// Touch slop: travel beyond this is a drag, not a tap.
pub const TAP_SLOP: f32 = 12.0;
/// Release velocity (px/s, upward negative) that closes the launcher.
pub const FLING_CLOSE_VELOCITY: f32 = -1800.0;
/// Bottom band reserved for the search field.
const SEARCH_BAND: f32 = 76.0;

/// Which tile (display position) sits under `p`, in grid-content space that
/// has already had scroll applied. Pure so the hit rule is testable: a point
/// in the cell's padding gutter counts as background, not tile.
pub fn hit_tile(p: Pos2, spec: &GridSpec, count: usize, inset: f32) -> Option<usize> {
    if p.x < 0.0 || p.y < 0.0 {
        return None;
    }
    let col = (p.x / spec.cell) as usize;
    let row = (p.y / spec.cell) as usize;
    if col >= spec.cols {
        return None;
    }
    let idx = row * spec.cols + col;
    if idx >= count {
        return None;
    }
    // Inside the cell, outside the tile's inset face = background.
    let cx = p.x - col as f32 * spec.cell;
    let cy = p.y - row as f32 * spec.cell;
    if cx < inset || cy < inset || cx > spec.cell - inset || cy > spec.cell - inset {
        return None;
    }
    Some(idx)
}

/// Decode an icon file's bytes into something `egui::Context::load_texture`
/// accepts. Only formats the `image` crate can decode work — this build only
/// enables its `png` codec, so a theme entry that resolved to an `.svg` (the
/// common case for modern themes' scalable dirs) decodes to `None` here and
/// [`Shell::icon_texture`] falls back to the glyph. Rasterizing SVG is a
/// real follow-up (needs `resvg` or similar), not something to fold into
/// this pass.
#[cfg(feature = "app")]
fn decode_icon(path: &std::path::Path) -> Option<egui::ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw()))
}

#[cfg(not(feature = "app"))]
fn decode_icon(_path: &std::path::Path) -> Option<egui::ColorImage> {
    None
}

struct PressState {
    moved: f32,
    hit: Option<usize>,
}

pub struct Shell {
    pub launcher: Launcher,
    pub palette: PaletteColors,
    /// See the module doc: honored, not yet fed.
    pub kb_inset_px: f32,
    /// Icon theme to search (see [`crate::icontheme`]); "hicolor" is always
    /// resolvable per spec, so it's the safe default when nothing better is
    /// configured. Change with [`Shell::with_icon_theme`].
    icon_theme: String,
    icon_dirs: Vec<std::path::PathBuf>,
    /// Resolved+decoded textures, keyed by app id. `None` means "looked and
    /// found nothing decodable" (missing icon, or the theme only has an SVG
    /// — this build has no rasterizer) so a repeated miss doesn't re-walk
    /// the icon theme tree every frame; the glyph fallback paints instead.
    icon_cache: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    spec: Option<GridSpec>,
    scroll_y: f32,
    fling_vy: f32,
    press: Option<PressState>,
    feedback: PressFeedback,
    pressed_tile: Option<usize>,
    search: String,
    focus_search: bool,
    t0: std::time::Instant,
    last_now_ms: f64,
    /// Set when the user asked to close (tap on background, fling, Escape).
    pub close_requested: bool,
    /// The last spawn error, shown in the corner instead of vanishing.
    pub last_error: Option<String>,
}

impl Shell {
    pub fn new(launcher: Launcher, palette: PaletteColors) -> Shell {
        Shell {
            launcher,
            palette,
            kb_inset_px: 0.0,
            icon_theme: "hicolor".to_string(),
            icon_dirs: crate::desktop::icon_theme_dirs(),
            icon_cache: std::collections::HashMap::new(),
            spec: None,
            scroll_y: 0.0,
            fling_vy: 0.0,
            press: None,
            feedback: PressFeedback::new(TILE_PRESS_FEEL),
            pressed_tile: None,
            search: String::new(),
            focus_search: true,
            t0: std::time::Instant::now(),
            last_now_ms: 0.0,
            close_requested: false,
            last_error: None,
        }
    }

    /// Override the icon theme to search (e.g. from `[launcher] icon_theme`
    /// in config-kit prefs — see `lib.rs::run`). Takes effect on the next
    /// icon lookup; already-cached textures aren't invalidated, so changing
    /// theme mid-session needs a restart (an acceptable seam: this isn't a
    /// live settings panel yet).
    pub fn with_icon_theme(mut self, theme: impl Into<String>) -> Shell {
        self.icon_theme = theme.into();
        self
    }

    /// Resolve + decode + upload an app's icon, memoized. `size_px` only
    /// affects which on-disk directory freedesktop lookup prefers; the
    /// texture itself is whatever resolution that directory has.
    fn icon_texture(&mut self, ctx: &egui::Context, app_id: &str, icon_name: &str, size_px: f32) -> Option<egui::TextureHandle> {
        if icon_name.is_empty() {
            return None;
        }
        if let Some(cached) = self.icon_cache.get(app_id) {
            return cached.clone();
        }
        let size = (size_px.round() as u32).max(16);
        let tex = crate::icontheme::find_icon(&self.icon_dirs, &self.icon_theme, icon_name, size, 1)
            .and_then(|path| decode_icon(&path))
            .map(|img| ctx.load_texture(app_id.to_string(), img, egui::TextureOptions::LINEAR));
        self.icon_cache.insert(app_id.to_string(), tex.clone());
        tex
    }

    fn now_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1000.0
    }

    /// One frame. Called from the `eframe::App` impl below; also directly
    /// usable under an offscreen `egui::Context` in tests, independent of
    /// whatever eframe's per-frame entry point is named this version.
    pub fn render(&mut self, ctx: &egui::Context) {
        let now = self.now_ms();
        let dt_ms = (now - self.last_now_ms).max(0.0) as f32;
        self.last_now_ms = now;

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.search.is_empty() {
                self.close_requested = true;
            } else {
                self.search.clear();
            }
        }

        let panel = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.palette.bg_base));
        panel.show(ctx, |ui| {
            let full = ui.max_rect();
            let reserved = SEARCH_BAND + self.kb_inset_px;
            let grid_rect = Rect::from_min_max(
                full.min,
                Pos2::new(full.max.x, (full.max.y - reserved).max(full.min.y + 1.0)),
            );

            // Layout: recompute spec on size change and retarget homes.
            let spec = GridSpec::fit(grid_rect.width(), grid_rect.height(), 168.0, 0.0);
            if self.spec != Some(spec) {
                self.spec = Some(spec);
                self.launcher.retarget(&spec, now);
            }

            self.grid_input(ui, grid_rect, spec, now, dt_ms);
            self.paint_grid(ui, grid_rect, spec, now);
            self.search_band(ui, full, spec, now);

            if let Some(err) = &self.last_error {
                ui.painter().text(
                    full.left_bottom() + Vec2::new(8.0, -8.0),
                    Align2::LEFT_BOTTOM,
                    err,
                    FontId::proportional(12.0),
                    self.palette.danger,
                );
            }
        });

        // Keep frames coming while anything moves.
        let animating = !self.launcher.board.is_settled()
            || self.fling_vy.abs() > 1.0
            || self.feedback.rippling()
            || self.feedback.is_down();
        if animating {
            ctx.request_repaint();
        }
    }

    fn grid_input(&mut self, ui: &mut egui::Ui, rect: Rect, spec: GridSpec, now: f64, dt_ms: f32) {
        let resp = ui.interact(rect, ui.id().with("grid"), Sense::click_and_drag());
        let count = self.launcher.filtered.len();
        let inset = spec.cell * 0.08;

        let to_content = |p: Pos2, scroll_y: f32| Pos2::new(p.x - rect.min.x, p.y - rect.min.y + scroll_y);

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let hit = hit_tile(to_content(p, self.scroll_y), &spec, count, inset);
                self.press = Some(PressState { moved: 0.0, hit });
                self.pressed_tile = hit;
                if hit.is_some() {
                    self.feedback.press(p.x, p.y);
                }
                self.fling_vy = 0.0;
            }
        }

        if resp.dragged() {
            let d = resp.drag_delta();
            if let Some(ps) = &mut self.press {
                ps.moved += d.length();
                if ps.moved > TAP_SLOP {
                    // It's a scroll now; the pressed tile visual lets go.
                    self.pressed_tile = None;
                    self.feedback.release();
                    self.scroll_y -= d.y;
                }
            }
        }

        if resp.drag_stopped() {
            self.feedback.release();
            let vel = ui.input(|i| i.pointer.velocity());
            if let (Some(ps), Some(p)) = (self.press.take(), resp.interact_pointer_pos()) {
                if ps.moved > TAP_SLOP {
                    if vel.y < FLING_CLOSE_VELOCITY {
                        self.close_requested = true;
                    } else {
                        self.fling_vy = -vel.y;
                    }
                } else {
                    // Tap: tile must be under press AND release.
                    let release_hit = hit_tile(to_content(p, self.scroll_y), &spec, count, inset);
                    let hit = if release_hit == ps.hit { ps.hit } else { None };
                    // A slipped tap (press-on-tile, release-off) is Nothing,
                    // which the combined `hit=None` + moved<=slop would call
                    // Close — guard it explicitly.
                    let outcome = if hit.is_none() && ps.hit.is_some() {
                        TapOutcome::Nothing
                    } else {
                        tap_outcome(ps.moved, TAP_SLOP, hit)
                    };
                    match outcome {
                        TapOutcome::Launch(pos) => self.launch(pos),
                        TapOutcome::Close => self.close_requested = true,
                        TapOutcome::Nothing => {}
                    }
                }
            }
            self.pressed_tile = None;
        }

        // Wheel scroll + fling decay.
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        self.scroll_y -= wheel;
        if self.fling_vy.abs() > 1.0 {
            self.scroll_y += self.fling_vy * (dt_ms / 1000.0);
            self.fling_vy *= (1.0 - dt_ms / 400.0).clamp(0.0, 1.0);
        }
        let max_scroll = (spec.content_height(count) - rect.height()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);

        self.feedback.tick(dt_ms);
        let _ = now;
    }

    fn paint_grid(&mut self, ui: &mut egui::Ui, rect: Rect, spec: GridSpec, now: f64) {
        self.launcher.board.tick(now);
        let painter = ui.painter_at(rect);
        let ctx = ui.ctx().clone();
        let inset = spec.cell * 0.08;

        // Board poses are keyed by app id; label/glyph come from the entry.
        // Positional lookup: display pos for pressed-tile check. Owned
        // copies (not &AppEntry borrows) so the loop below is free to call
        // `&mut self` methods (icon_texture) without fighting the borrow
        // checker over `self.launcher.apps`.
        let pos_of: std::collections::HashMap<String, usize> = self
            .launcher
            .filtered
            .iter()
            .enumerate()
            .map(|(pos, &i)| (self.launcher.apps[i].id.clone(), pos))
            .collect();
        let by_id: std::collections::HashMap<String, (String, String)> = self
            .launcher
            .apps
            .iter()
            .map(|a| (a.id.clone(), (a.name.clone(), a.icon.clone())))
            .collect();

        let poses: Vec<(String, TilePose)> = self
            .launcher
            .board
            .poses()
            .map(|(k, p)| (k.to_string(), p))
            .collect();

        for (key, pose) in poses {
            let Some((name, icon_name)) = by_id.get(&key).cloned() else { continue };
            let pressed = pos_of.get(key.as_str()).copied() == self.pressed_tile
                && self.pressed_tile.is_some();
            let scale = pose.scale * if pressed { self.feedback.scale() } else { 1.0 };

            let cell_origin = Pos2::new(rect.min.x + pose.x, rect.min.y + pose.y - self.scroll_y);
            let face = Rect::from_min_size(
                cell_origin + Vec2::splat(inset),
                Vec2::splat(spec.cell - 2.0 * inset),
            );
            if face.max.y < rect.min.y || face.min.y > rect.max.y {
                continue;
            }
            let face = Rect::from_center_size(face.center(), face.size() * scale);
            let a = (pose.alpha.clamp(0.0, 1.0) * 255.0) as u8;
            if a == 0 {
                continue;
            }

            let card = self.palette.bg_card.gamma_multiply(pose.alpha);
            painter.rect(
                face,
                CornerRadius::same(12),
                card,
                Stroke::new(1.0, self.palette.border.gamma_multiply(pose.alpha * 0.6)),
                egui::StrokeKind::Inside,
            );

            let glyph_c = face.center() - Vec2::new(0.0, face.height() * 0.12);
            let tex = self.icon_texture(&ctx, &key, &icon_name, face.width());
            if let Some(tex) = tex {
                // Real icon: draw it in the same footprint the glyph would
                // have used, so swapping a placeholder for a real icon later
                // (once a theme actually resolves) doesn't reflow the tile.
                let icon_rect = Rect::from_center_size(glyph_c, Vec2::splat(face.width() * 0.5));
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE.gamma_multiply(pose.alpha),
                );
            } else {
                // No decodable icon (missing, or theme-only-has-SVG — see
                // decode_icon's doc comment): initial-letter glyph fallback.
                let r = face.width() * 0.22;
                painter.circle_filled(glyph_c, r, self.palette.accent_soft.gamma_multiply(pose.alpha));
                let initial = name.chars().next().unwrap_or('?').to_uppercase().to_string();
                painter.text(
                    glyph_c,
                    Align2::CENTER_CENTER,
                    initial,
                    FontId::proportional(r * 1.2),
                    self.palette.text.gamma_multiply(pose.alpha),
                );
            }
            painter.text(
                Pos2::new(face.center().x, face.max.y - face.height() * 0.14),
                Align2::CENTER_CENTER,
                truncate(&name, 16),
                FontId::proportional(13.0),
                self.palette.text_weak.gamma_multiply(pose.alpha),
            );
        }
    }

    fn search_band(&mut self, ui: &mut egui::Ui, full: Rect, spec: GridSpec, now: f64) {
        let band = Rect::from_min_max(
            Pos2::new(full.min.x, full.max.y - SEARCH_BAND - self.kb_inset_px),
            Pos2::new(full.max.x, full.max.y - self.kb_inset_px),
        );
        let field_w = (band.width() * 0.6).min(420.0);
        let field = Rect::from_center_size(band.center(), Vec2::new(field_w, 40.0));

        ui.painter().rect(
            field.expand(2.0),
            CornerRadius::same(20),
            self.palette.bg_raised,
            Stroke::new(1.0, self.palette.border),
            egui::StrokeKind::Inside,
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(Vec2::new(14.0, 6.0))));
        let edit = egui::TextEdit::singleline(&mut self.search)
            .hint_text("Search")
            .frame(EguiFrame::NONE)
            .desired_width(f32::INFINITY);
        let resp = child.add(edit);
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if resp.changed() {
            let q = self.search.clone();
            self.launcher.set_query(&q, &spec, now);
            self.scroll_y = 0.0;
        }
        // Enter launches the top hit.
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if !self.launcher.filtered.is_empty() {
                self.launch(0);
            }
        }
    }

    fn launch(&mut self, pos: usize) {
        let Some(spec) = self.launcher.launch_spec(pos) else { return };
        match spawn(&spec) {
            Ok(()) => self.close_requested = true,
            Err(e) => self.last_error = Some(format!("launch failed: {e}")),
        }
    }
}

/// eframe wiring. Gated behind the `app` feature (same as the `eframe` dep
/// itself) so the headless core keeps building without a windowing backend.
#[cfg(feature = "app")]
impl eframe::App for Shell {
    // eframe 0.34 renamed the per-frame entry point: `App::update(&Context)`
    // is deprecated in favor of `App::ui(&mut egui::Ui)`, which now hands you
    // the root viewport's Ui directly instead of a bare Context (the old
    // pattern of building your own top-level `CentralPanel::show(ctx, ..)`
    // still works via `ui.ctx()` — that's the only reason it's not a bigger
    // rewrite here — but note it *is* now redundant nesting: the panel below
    // is-a-panel-inside-what's-already-effectively-a-panel).
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.render(&ctx);
        if self.close_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fully opaque — this is a fullscreen shell, not a layered overlay.
        let c = self.palette.bg_base;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Spawn detached. Terminal entries wrap in `$TERMINAL` (default `foot` — the
/// common Wayland-phone choice; override in the environment).
pub fn spawn(spec: &LaunchSpec) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut argv = spec.argv.clone();
    if spec.terminal {
        let term = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".into());
        let mut wrapped = vec![term, "-e".into()];
        wrapped.append(&mut argv);
        argv = wrapped;
    }
    let (head, tail) = argv.split_first().expect("launch_spec guarantees non-empty argv");
    Command::new(head)
        .args(tail)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> GridSpec {
        GridSpec { cols: 4, rows_visible: 3, cell: 100.0 }
    }

    #[test]
    fn hit_test_maps_points_to_tiles() {
        let s = spec();
        // Center of cell (1,0) = index 1.
        assert_eq!(hit_tile(Pos2::new(150.0, 50.0), &s, 8, 8.0), Some(1));
        // Center of cell (0,1) = index 4.
        assert_eq!(hit_tile(Pos2::new(50.0, 150.0), &s, 8, 8.0), Some(4));
    }

    #[test]
    fn gutters_and_out_of_range_are_background() {
        let s = spec();
        // Cell boundary gutter (within inset of the cell edge).
        assert_eq!(hit_tile(Pos2::new(102.0, 50.0), &s, 8, 8.0), None);
        // A cell past the item count.
        assert_eq!(hit_tile(Pos2::new(50.0, 250.0), &s, 8, 8.0), None);
        // Off the grid entirely.
        assert_eq!(hit_tile(Pos2::new(-5.0, 50.0), &s, 8, 8.0), None);
        assert_eq!(hit_tile(Pos2::new(450.0, 50.0), &s, 8, 8.0), None);
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("short", 16), "short");
        assert_eq!(truncate("allesübertrieben lang", 8), "allesüb…");
    }
}
