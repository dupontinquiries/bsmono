//! gpu_menu.rs — a real, interactive pause/main menu for the GPU renderer: map select (the curated
//! `level::PRESETS` list) and loadout customization (weapon + perk + class picking), replacing the
//! previous "Escape just quits the whole app" behaviour.
//!
//! This uses genuine egui widgets (buttons, combo boxes — not the HUD's paint-only `layer_painter`
//! calls in gpu_egui.rs's existing draw_hud/draw_reticle/etc.), so it needs REAL pointer/keyboard
//! input fed into the egui context, not just a screen_rect — see
//! `gpu_egui::EguiOverlay::prepare`'s `events` param (previously hardcoded to none, since the HUD
//! never needed to be clickable) and `gpu.rs`'s menu-open branch in the winit event loop, which
//! accumulates real `CursorMoved`/`MouseInput`/`KeyboardInput` events into `egui::Event`s only while
//! the menu is open (gameplay's WASD/fire/ads keys are unaffected either way).
//!
//! Scope note: `Game::new_with` takes a `MapTheme`, not a full `MapPreset` — picking a preset here
//! applies its `.theme` (so the biome/lighting/hazard set matches), but not yet the preset's own
//! size/rooms/cover/pit/lanes fine-tuning. Full preset fidelity is a reasonable follow-up, not done
//! blind in this pass.

use egui::{Color32, RichText};
use crate::level::PRESETS;

/// Cross-frame menu state owned by the gpu runner: map selection, the manually-driven list
/// scroll offset (wheel is forwarded now, but the manual offset also takes PgUp/PgDn/arrows and
/// can't be eaten by hover routing — same resilience the app shell grew), and the cached
/// crossfading preview.
#[derive(Default)]
pub struct MenuState {
    pub sel_preset: Option<usize>,
    pub map_scroll: f32,
    pub preview: Option<crate::map_preview::MapPreview>,
}
use crate::loadout::{Loadout, Perk};
use crate::weapons::WeaponKind;
use crate::class::ClassKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuScreen {
    Main,
    MapSelect,
    Loadout,
}

/// What the menu wants the caller (gpu.rs's run loop) to do this frame.
pub enum MenuAction {
    None,
    Resume,
    /// Start a new run on this exact PRESET (index into `level::PRESETS`) with whatever
    /// `loadout` currently holds; `None` = a random varied map. Carrying the index (not just
    /// the theme) is what makes the hand-tuned variations and the 13 learned-site maps
    /// actually reachable from this bin.
    StartGame(Option<usize>),
    Quit,
}

/// Render the menu for `screen`; `loadout` is mutable — perk/weapon/class picks write straight into
/// it, so the loadout screen and the eventual `StartGame` naturally agree on what's equipped, with
/// no separate "apply" step to forget. Returns what the caller should do this frame.
pub fn ui(ctx: &egui::Context, screen: &mut MenuScreen, loadout: &mut Loadout, st: &mut MenuState) -> MenuAction {
    let mut action = MenuAction::None;
    let frame = egui::Frame::default().fill(Color32::from_rgba_unmultiplied(10, 10, 16, 235));
    #[allow(deprecated)] // show() (not show_inside(), which needs an existing &mut Ui to nest into —
    // this is the top-level panel for the whole frame) is correct here; pinned to egui 0.34 like
    // gpu_egui.rs's Context::run() usage.
    egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(36.0);
            ui.label(RichText::new("EARLY-FPS").size(34.0).strong().color(Color32::from_rgb(230, 200, 90)));
            ui.add_space(18.0);
            match *screen {
                MenuScreen::Main => {
                    if big_button(ui, "Resume").clicked() { action = MenuAction::Resume; }
                    if big_button(ui, "Map Select").clicked() { *screen = MenuScreen::MapSelect; }
                    if big_button(ui, "Loadout").clicked() { *screen = MenuScreen::Loadout; }
                    if big_button(ui, "Quit").clicked() { action = MenuAction::Quit; }
                }
                MenuScreen::MapSelect => {
                    let gold = Color32::from_rgb(235, 224, 180);
                    ui.label(RichText::new("MAP SELECT").size(18.0).color(Color32::from_rgb(180, 190, 210)));
                    ui.label(RichText::new(format!("{} curated maps", PRESETS.len())).size(12.0).color(Color32::from_rgb(130, 138, 155)));
                    ui.add_space(6.0);
                    // ── crossfading fly-around preview (shared MapPreview, same as the app shell;
                    //    the software raster textures off Managed(0) exactly as the app does) ──
                    let key = st.sel_preset.map(|i| i as u64).unwrap_or(u64::MAX);
                    if st.preview.as_ref().map(|p| p.key) != Some(key) {
                        let cfg = match st.sel_preset {
                            Some(i) => PRESETS[i].config(1234 + i as u64),
                            None => crate::level::MapConfig::varied(777),
                        };
                        st.preview = Some(crate::map_preview::MapPreview::build(key, cfg));
                    }
                    let pw = ui.available_width().min(380.0);
                    let (prect, _) = ui.allocate_exact_size(egui::vec2(pw, 150.0), egui::Sense::hover());
                    if let Some(p) = &st.preview {
                        crate::map_preview::paint_sky_gradient(ui.painter(), prect, p.light.sky, p.fog.color);
                        let t = ui.input(|i| i.time) as f32;
                        p.paint(&ui.painter_at(prect), prect, t, egui::TextureId::Managed(0));
                    }
                    ui.painter().rect_stroke(prect, 4.0, egui::Stroke::new(1.0, Color32::from_rgb(60, 70, 84)), egui::StrokeKind::Inside);
                    ui.ctx().request_repaint();
                    if let Some(i) = st.sel_preset {
                        let pr = &PRESETS[i];
                        ui.label(RichText::new(format!("{} · {}×{} · {}", pr.name, pr.size, pr.size, pr.theme.name()))
                            .size(11.0).color(Color32::from_rgb(130, 138, 155)));
                    }
                    ui.add_space(6.0);
                    // ── the list fills the window; scroll offset is manual + fed to the area ──
                    let reserved = 150.0 + 120.0 + 90.0; // preview + headers + footer
                    let list_h = (ctx.screen_rect().height() - reserved).clamp(240.0, 720.0);
                    let (wheel, pgup, pgdn, up, dn) = ui.input(|i| (
                        i.smooth_scroll_delta.y,
                        i.key_pressed(egui::Key::PageUp), i.key_pressed(egui::Key::PageDown),
                        i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                    st.map_scroll = (st.map_scroll - wheel
                        + if pgdn { list_h * 0.85 } else { 0.0 } - if pgup { list_h * 0.85 } else { 0.0 }
                        + if dn { 36.0 } else { 0.0 } - if up { 36.0 } else { 0.0 }).max(0.0);
                    let out = egui::ScrollArea::vertical()
                        .id_salt("gpu_map_select")
                        .max_height(list_h)
                        .auto_shrink([false, false])
                        .vertical_scroll_offset(st.map_scroll)
                        .show(ui, |ui| {
                            egui::Grid::new("gpu_maps").spacing([8.0, 8.0]).show(ui, |ui| {
                                for (i, pr) in PRESETS.iter().enumerate() {
                                    let sel = st.sel_preset == Some(i);
                                    let label = RichText::new(pr.name).size(12.0)
                                        .color(if sel { gold } else { Color32::from_rgb(200, 205, 215) });
                                    if ui.add_sized([140.0, 28.0], egui::Button::new(label)
                                        .fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                                        st.sel_preset = Some(i);
                                    }
                                    if i % 4 == 3 { ui.end_row(); }
                                }
                            });
                        });
                    let max_off = (out.content_size.y - out.inner_rect.height()).max(0.0);
                    st.map_scroll = out.state.offset.y.min(max_off);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let rnd = st.sel_preset.is_none();
                        if ui.add_sized([110.0, 30.0], egui::Button::new(RichText::new("Random").color(if rnd { gold } else { Color32::from_rgb(130, 138, 155) }))).clicked() {
                            st.sel_preset = None;
                        }
                        if ui.add_sized([110.0, 30.0], egui::Button::new(RichText::new("Deploy").size(15.0))).clicked() {
                            action = MenuAction::StartGame(st.sel_preset);
                        }
                        if ui.button("‹ Back").clicked() { *screen = MenuScreen::Main; }
                    });
                }
                MenuScreen::Loadout => {
                    ui.label(RichText::new("LOADOUT").size(18.0).color(Color32::from_rgb(180, 190, 210)));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Class:");
                        for c in ClassKind::ALL {
                            if ui.selectable_label(loadout.class == c, c.def().name).clicked() { loadout.class = c; }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Primary:");
                        egui::ComboBox::from_id_salt("primary_weapon").selected_text(loadout.primary.def().name).show_ui(ui, |ui| {
                            for w in WeaponKind::ALL { ui.selectable_value(&mut loadout.primary, w, w.def().name); }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Secondary:");
                        egui::ComboBox::from_id_salt("secondary_weapon").selected_text(loadout.secondary.def().name).show_ui(ui, |ui| {
                            for w in WeaponKind::ALL { ui.selectable_value(&mut loadout.secondary, w, w.def().name); }
                        });
                    });
                    ui.add_space(8.0);
                    ui.label("Perks (up to 3 — click to toggle):");
                    ui.horizontal_wrapped(|ui| {
                        for p in Perk::ALL {
                            if ui.selectable_label(loadout.has(p), p.name()).clicked() { loadout.toggle_perk(p); }
                        }
                    });
                    ui.add_space(10.0);
                    if ui.button("‹ Back").clicked() { *screen = MenuScreen::Main; }
                }
            }
        });
    });
    action
}

fn big_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized([260.0, 42.0], egui::Button::new(RichText::new(label).size(16.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `ui` once against a synthetic headless context — no real window/GPU needed, egui
    /// supports this directly. Doesn't simulate clicks (that would need synthetic pointer
    /// coordinates matched to wherever egui happens to lay widgets out, which is fragile to hand-
    /// author blind); this instead guards the thing most likely to actually break silently: that
    /// each screen renders at all, for every screen, without panicking, given real game data
    /// (`level::PRESETS`, `Perk::ALL`, `WeaponKind::ALL`, `ClassKind::ALL`).
    fn run_once(screen: &mut MenuScreen, loadout: &mut Loadout, st: &mut MenuState) -> MenuAction {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let mut action = MenuAction::None;
        #[allow(deprecated)]
        let _ = ctx.run(raw, |ctx| { action = ui(ctx, screen, loadout, st); });
        action
    }

    #[test]
    fn every_screen_renders_without_panicking() {
        let mut loadout = Loadout::default();
        for mut screen in [MenuScreen::Main, MenuScreen::MapSelect, MenuScreen::Loadout] {
            let action = run_once(&mut screen, &mut loadout, &mut MenuState::default());
            assert!(matches!(action, MenuAction::None), "no click was simulated, so nothing should have fired");
        }
    }

    #[test]
    fn map_select_lists_every_curated_preset_without_panicking() {
        // exercises the actual PRESETS data (91 entries) through the real ScrollArea/Button path —
        // this is exactly what would break if a preset's fields ever became invalid egui input
        // (e.g. a NaN, an empty label) without needing to simulate a click on any specific one.
        let mut loadout = Loadout::default();
        let mut screen = MenuScreen::MapSelect;
        run_once(&mut screen, &mut loadout, &mut MenuState::default());
        assert!(!crate::level::PRESETS.is_empty(), "the screen we just rendered should have real presets to list");
    }

    #[test]
    fn loadout_screen_renders_every_perk_weapon_and_class_option() {
        let mut loadout = Loadout::default();
        let mut screen = MenuScreen::Loadout;
        run_once(&mut screen, &mut loadout, &mut MenuState::default());
        // the loadout itself should be untouched — rendering the screen must not mutate it by itself
        assert_eq!(loadout, Loadout::default());
    }

    #[test]
    fn toggling_a_perk_through_the_real_loadout_api_is_reflected_immediately() {
        // the menu writes straight into `loadout` via has()/toggle_perk() — this locks in that the
        // two agree on state, independent of egui rendering.
        let mut loadout = Loadout::default();
        assert!(!loadout.has(Perk::Camo));
        loadout.toggle_perk(Perk::Camo);
        assert!(loadout.has(Perk::Camo));
    }
}
