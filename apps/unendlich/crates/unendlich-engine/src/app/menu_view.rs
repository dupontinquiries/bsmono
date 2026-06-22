//! `app::menu_view` — the canonical menu **drawing**, in the library where it
//! compiles and tests in-sandbox (egui is backend-agnostic; only the `ui-overlay`
//! feature bundles it with glfw). One source of truth for what the menu looks
//! like: the renderer's overlay pass calls `menu_ui`, the thin bin forwards the
//! returned `MenuClick` into `App`, and tests run `menu_ui` on a headless egui
//! `Context` to prove it draws the right screen — no GUI required.
//!
//! This is the wedge that ends "everything Claude wires is invisible": menu
//! rendering is now exercised here, and the same function renders the real frame.

use crate::app::AppInput;
use crate::sim::command_parse::GameMode;
use crate::sim::menu_controller::MenuController;
use crate::sim::new_world_config::SeedChoice;
use crate::sim::title_cinematic::{piece_state, MenuPiece};
use crate::sim::title_flow::{MenuId, Screen};

/// What the player clicked in the menu this frame. The shell maps this to an
/// `app::AppInput` and feeds it back into `App::tick`.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuClick {
    None,
    Select(MenuId),
    SelectSlot(u8),
    Back,
    Confirm,
    WizardNext,
    WizardBack,
    WizardCycleMode,
    WizardToggleSeed,
}

/// Draw the current menu screen and return the click (if any). Pure egui — the
/// renderer calls this inside its overlay pass; tests call it on a headless
/// `egui::Context`.
/// Scoped frosted-glass styling for the menu widgets: translucent rounded button
/// fills, a faint light edge, and a brighter hover/press — "glass" panels floating
/// over the scene. Scoped to the menu `Ui` (does not touch the global ctx style).
/// r223: a screen heading in the blocky voxel game font (UnendlichBlocks),
/// letterspaced caps — the mockup's heading treatment. Replaces plain
/// `ui.heading(..)` so menu titles carry the game's character. Falls back to
/// a plain heading when the `unendlich-ui` kit isn't compiled in (egui-only
/// builds + headless tests), so this module stays buildable on `egui` alone.
fn blocky_heading(ui: &mut egui::Ui, text: &str) {
    #[cfg(feature = "unendlich-ui")]
    {
        ui.label(
            egui::RichText::new(text.to_uppercase())
                .font(unendlich_ui::menu::blocks_font(26.0))
                .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                .extra_letter_spacing(4.0),
        );
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.heading(text);
    }
}

fn apply_frosted_style(ui: &mut egui::Ui) {
    use egui::{Color32, CornerRadius, Stroke};
    let s = ui.style_mut();
    s.spacing.button_padding = egui::vec2(14.0, 8.0);
    s.spacing.item_spacing.y = 9.0;
    s.visuals.override_text_color = Some(Color32::from_rgb(223, 230, 244));
    let v = &mut s.visuals.widgets;
    // resting glass
    v.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(92, 106, 134, 150);
    v.inactive.bg_fill = v.inactive.weak_bg_fill;
    v.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(190, 208, 235, 90));
    v.inactive.corner_radius = CornerRadius::same(8);
    v.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 228, 242));
    // hover: brighter, slightly expanded
    v.hovered.weak_bg_fill = Color32::from_rgba_unmultiplied(98, 116, 150, 160);
    v.hovered.bg_fill = v.hovered.weak_bg_fill;
    v.hovered.bg_stroke = Stroke::new(1.2, Color32::from_rgba_unmultiplied(205, 222, 250, 130));
    v.hovered.corner_radius = CornerRadius::same(8);
    v.hovered.fg_stroke = Stroke::new(1.0, Color32::from_rgb(245, 248, 255));
    v.hovered.expansion = 1.5;
    // press
    v.active.weak_bg_fill = Color32::from_rgba_unmultiplied(126, 146, 186, 190);
    v.active.bg_fill = v.active.weak_bg_fill;
    v.active.bg_stroke = Stroke::new(1.4, Color32::from_rgba_unmultiplied(222, 236, 255, 170));
    v.active.corner_radius = CornerRadius::same(8);
    v.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.active.expansion = 1.0;
}

pub fn menu_ui(ctx: &egui::Context, menu: &MenuController) -> MenuClick {
    let mut click = MenuClick::None;
    egui::CentralPanel::default().show(ctx, |ui| {
        apply_frosted_style(ui);
        ui.vertical_centered(|ui| {
            match menu.screen() {
                Screen::Title => {
                    // Title is built from real voxel blocks in the 3D scene (see
                    // title_cinematic / the menu world backdrop), not painted here.
                    ui.add_space(12.0);
                    if ui.button("  Singleplayer  ").clicked() {
                        click = MenuClick::Select(MenuId::NewGame);
                    }
                    if menu.catalog().has_save() && ui.button("  Continue  ").clicked() {
                        click = MenuClick::Select(MenuId::Continue);
                    }
                    if ui.button("  Load World  ").clicked() {
                        click = MenuClick::Select(MenuId::Load);
                    }
                    if ui.button("  Multiplayer  ").clicked() {
                        click = MenuClick::Select(MenuId::Multiplayer);
                    }
                    if ui.button("  Settings  ").clicked() {
                        click = MenuClick::Select(MenuId::Settings);
                    }
                    if ui.button("  Quit  ").clicked() {
                        click = MenuClick::Select(MenuId::Quit);
                    }
                }
                Screen::NewGame => {
                    blocky_heading(ui, "New World");
                    if let Some(w) = menu.wizard() {
                        ui.add_space(6.0);
                        ui.label(w.summary());
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button(" Mode ").clicked() {
                                click = MenuClick::WizardCycleMode;
                            }
                            if ui.button(" Seed ").clicked() {
                                click = MenuClick::WizardToggleSeed;
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button(" Back ").clicked() {
                                click = MenuClick::WizardBack;
                            }
                            if w.at_confirm() {
                                if ui.button(" Create ").clicked() {
                                    click = MenuClick::Confirm;
                                }
                            } else if w.can_advance() && ui.button(" Next ").clicked() {
                                click = MenuClick::WizardNext;
                            }
                        });
                    }
                }
                Screen::LoadSlots => {
                    blocky_heading(ui, "Load World");
                    ui.add_space(6.0);
                    let entries: Vec<(usize, String)> = menu
                        .catalog()
                        .entries()
                        .iter()
                        .enumerate()
                        .map(|(i, e)| (i, e.name.clone()))
                        .collect();
                    for (i, name) in entries {
                        if ui.button(format!("  {name}  ")).clicked() {
                            click = MenuClick::SelectSlot(i as u8);
                        }
                    }
                    ui.add_space(8.0);
                    if ui.button(" Back ").clicked() {
                        click = MenuClick::Back;
                    }
                }
                Screen::Multiplayer => {
                    blocky_heading(ui, "Multiplayer");
                    ui.label("Host or join (see session_mode)");
                    if ui.button(" Back ").clicked() {
                        click = MenuClick::Back;
                    }
                }
                Screen::Settings => {
                    blocky_heading(ui, "Settings");
                    if ui.button(" Done ").clicked() {
                        click = MenuClick::Confirm;
                    }
                    if ui.button(" Back ").clicked() {
                        click = MenuClick::Back;
                    }
                }
                Screen::ConfirmQuit => {
                    blocky_heading(ui, "Quit unendlich?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(" Quit ").clicked() {
                            click = MenuClick::Confirm;
                        }
                        if ui.button(" Cancel ").clicked() {
                            click = MenuClick::Back;
                        }
                    });
                }
                Screen::Credits => {
                    blocky_heading(ui, "Credits");
                    if ui.button(" Back ").clicked() {
                        click = MenuClick::Back;
                    }
                }
            }
        });
    });
    click
}

/// The Title screen's cinematic fly-in: each [`MenuPiece`] flies from off-screen
/// to its anchor over `elapsed`, fading in as it arrives. The renderer calls this
/// for the Title screen (with `App::cinematic_elapsed`) while the camera orbits via
/// `App::camera_pose`; other screens use [`menu_ui`]. Returns the click, if any.
pub fn title_flythrough_ui(ctx: &egui::Context, elapsed: f32, fly_secs: f32) -> MenuClick {
    let mut click = MenuClick::None;
    let screen = ctx.screen_rect();
    let (w, h) = (screen.width(), screen.height());

    // title heading flies down from the top + fades in over the first ~0.8s
    let head_t = (elapsed / 0.8).clamp(0.0, 1.0);
    egui::Area::new(egui::Id::new("title_heading"))
        .fixed_pos(egui::pos2(w * 0.5 - 80.0, h * (0.10 + 0.06 * (1.0 - head_t))))
        .show(ctx, |ui| {
            ui.set_opacity(head_t);
            // r223: blocky voxel title font, letterspaced caps (falls back to
            // a plain heading on egui-only builds without the ui kit).
            #[cfg(feature = "unendlich-ui")]
            ui.label(
                egui::RichText::new("UNENDLICH")
                    .font(unendlich_ui::menu::blocks_font(36.0))
                    .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                    .extra_letter_spacing(6.0),
            );
            #[cfg(not(feature = "unendlich-ui"))]
            ui.heading("unendlich");
        });

    for piece in MenuPiece::ALL {
        let st = piece_state(piece, elapsed, fly_secs);
        let pos = egui::pos2(st.pos01[0] * w, st.pos01[1] * h);
        egui::Area::new(egui::Id::new(("piece", piece as u8)))
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_opacity(st.alpha01);
                if ui.button(piece_label(piece)).clicked() {
                    click = MenuClick::Select(piece_menu_id(piece));
                }
            });
    }
    click
}

fn piece_label(p: MenuPiece) -> &'static str {
    match p {
        MenuPiece::Singleplayer => "  Singleplayer  ",
        MenuPiece::Multiplayer => "  Multiplayer  ",
        MenuPiece::Settings => "  Settings  ",
        MenuPiece::Quit => "  Quit  ",
    }
}

fn piece_menu_id(p: MenuPiece) -> MenuId {
    match p {
        MenuPiece::Singleplayer => MenuId::NewGame,
        MenuPiece::Multiplayer => MenuId::Multiplayer,
        MenuPiece::Settings => MenuId::Settings,
        MenuPiece::Quit => MenuId::Quit,
    }
}

// ───────────────────────── click → AppInput mapping ─────────────────────────
//
// The renderer overlay draws via `menu_ui` / `title_flythrough_ui`, which return
// a [`MenuClick`]; the shell turns that into an [`AppInput`] and feeds it to
// `App::tick`. This mapper is that turn, living in the library (where it tests
// in-sandbox) instead of the un-compilable bin. The bin call site is then a
// one-liner: `app.tick(menu_view::map_click(click, &app), entropy)`.
//
// Most clicks map 1:1. The two *cycle* clicks (mode / seed) are stateful — they
// advance the wizard's current value to the next one — so they read `app` to
// produce the concrete `WizardSetMode` / `WizardSetSeed`. When there is no active
// wizard (clicked off the wizard screen) they degrade to `AppInput::None`.

/// Next game mode in the fixed 4-cycle (Survival→Creative→Adventure→Spectator→…).
fn next_mode(m: GameMode) -> GameMode {
    match m {
        GameMode::Survival => GameMode::Creative,
        GameMode::Creative => GameMode::Adventure,
        GameMode::Adventure => GameMode::Spectator,
        GameMode::Spectator => GameMode::Survival,
    }
}

/// Toggle the seed choice between "let the engine pick" (`Random`) and a fixed,
/// player-set seed. Toggling away from `Random` lands on a deterministic
/// placeholder `Fixed(0)` the seed-entry step then lets the player edit; toggling
/// from any concrete choice returns to `Random`. (A full UI may offer `Text`
/// entry; the toggle only flips the *kind*, leaving text editing to `WizardSetSeed`.)
fn toggle_seed(s: &SeedChoice) -> SeedChoice {
    match s {
        SeedChoice::Random => SeedChoice::Fixed(0),
        _ => SeedChoice::Random,
    }
}

/// Map a [`MenuClick`] from the overlay into the [`AppInput`] fed to `App::tick`.
///
/// Pure and total: every `MenuClick` yields exactly one `AppInput`; the stateful
/// cycle clicks read the current wizard config from `app`. No panics, no IO.
pub fn map_click(click: MenuClick, app: &crate::app::App) -> AppInput {
    match click {
        MenuClick::None => AppInput::None,
        MenuClick::Select(id) => AppInput::Select(id),
        MenuClick::SelectSlot(s) => AppInput::SelectSlot(s),
        MenuClick::Back => AppInput::Back,
        MenuClick::Confirm => AppInput::Confirm,
        MenuClick::WizardNext => AppInput::WizardNext,
        MenuClick::WizardBack => AppInput::WizardBack,
        MenuClick::WizardCycleMode => match app.menu().wizard() {
            Some(w) => AppInput::WizardSetMode(next_mode(w.config().mode)),
            None => AppInput::None,
        },
        MenuClick::WizardToggleSeed => match app.menu().wizard() {
            Some(w) => AppInput::WizardSetSeed(toggle_seed(&w.config().seed)),
            None => AppInput::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the menu drawing on a headless egui Context — proves it executes and
    /// produces geometry without any window/GPU. This is menu *rendering* under
    /// test for the first time.
    fn run_once(menu: &MenuController) -> egui::FullOutput {
        let ctx = egui::Context::default();
        install_test_fonts(&ctx);
        ctx.run(egui::RawInput::default(), |c| {
            let _ = menu_ui(c, menu);
        })
    }

    /// Mirror `EguiOverlay::new`: register the blocky/glyph fonts so the
    /// menu's `blocky_heading` (which selects `FontFamily::Name("blocks")`)
    /// resolves under headless test too. No-op without the kit.
    fn install_test_fonts(_ctx: &egui::Context) {
        #[cfg(feature = "unendlich-ui")]
        unendlich_ui::menu::install_fonts(
            _ctx,
            include_bytes!("../../../../docs/design/UnendlichBlocks-Regular.ttf"),
            include_bytes!("../../../../docs/design/UnendlichGlyphs-Regular.ttf"),
        );
    }

    #[test]
    fn title_menu_draws_geometry_headless() {
        let menu = MenuController::with_defaults();
        let out = run_once(&menu);
        assert!(!out.shapes.is_empty(), "the title menu drew shapes");
    }

    #[test]
    fn wizard_screen_draws_headless() {
        let mut menu = MenuController::with_defaults();
        menu.select(MenuId::NewGame);
        assert_eq!(menu.screen(), Screen::NewGame);
        let out = run_once(&menu);
        assert!(!out.shapes.is_empty(), "the wizard screen drew shapes");
    }

    #[test]
    fn title_flythrough_draws_pieces_headless() {
        let ctx = egui::Context::default();
        install_test_fonts(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect =
            Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(720.0, 520.0)));
        // Areas size on the first frame; read shapes from the second.
        let _ = ctx.run(input.clone(), |c| {
            let _ = title_flythrough_ui(c, 1.5, 0.6);
        });
        let out = ctx.run(input, |c| {
            let _ = title_flythrough_ui(c, 1.5, 0.6);
        });
        assert!(!out.shapes.is_empty(), "the flythrough drew menu pieces");
    }

    // ───────────── click → AppInput → App::tick wiring (the F2 proof) ─────────────

    use crate::app::{App, AppStep};

    #[test]
    fn map_click_simple_variants_are_one_to_one() {
        let app = App::with_defaults();
        assert_eq!(map_click(MenuClick::None, &app), AppInput::None);
        assert_eq!(map_click(MenuClick::Select(MenuId::Settings), &app),
                   AppInput::Select(MenuId::Settings));
        assert_eq!(map_click(MenuClick::SelectSlot(2), &app), AppInput::SelectSlot(2));
        assert_eq!(map_click(MenuClick::Back, &app), AppInput::Back);
        assert_eq!(map_click(MenuClick::Confirm, &app), AppInput::Confirm);
        assert_eq!(map_click(MenuClick::WizardNext, &app), AppInput::WizardNext);
        assert_eq!(map_click(MenuClick::WizardBack, &app), AppInput::WizardBack);
    }

    #[test]
    fn cycle_clicks_without_wizard_are_none() {
        // On the Title screen there is no active wizard → cycle clicks degrade.
        let app = App::with_defaults();
        assert!(app.menu().wizard().is_none());
        assert_eq!(map_click(MenuClick::WizardCycleMode, &app), AppInput::None);
        assert_eq!(map_click(MenuClick::WizardToggleSeed, &app), AppInput::None);
    }

    #[test]
    fn cycle_mode_reads_wizard_and_advances() {
        // Enter the wizard, then a CycleMode click should target the NEXT mode.
        let mut app = App::with_defaults();
        let _ = app.tick(AppInput::Select(MenuId::NewGame), 0);
        let w = app.menu().wizard().expect("wizard active after NewGame");
        let cur = w.config().mode;
        let expected_next = match cur {
            GameMode::Survival => GameMode::Creative,
            GameMode::Creative => GameMode::Adventure,
            GameMode::Adventure => GameMode::Spectator,
            GameMode::Spectator => GameMode::Survival,
        };
        match map_click(MenuClick::WizardCycleMode, &app) {
            AppInput::WizardSetMode(m) => assert_eq!(m, expected_next),
            other => panic!("expected WizardSetMode, got {other:?}"),
        }
    }

    #[test]
    fn toggle_seed_flips_kind() {
        let mut app = App::with_defaults();
        let _ = app.tick(AppInput::Select(MenuId::NewGame), 0);
        // default seed is Random → toggle should request a Fixed seed
        match map_click(MenuClick::WizardToggleSeed, &app) {
            AppInput::WizardSetSeed(SeedChoice::Fixed(_)) => {}
            other => panic!("expected WizardSetSeed(Fixed), got {other:?}"),
        }
    }

    #[test]
    fn full_pipeline_click_feeds_app_tick() {
        // The end-to-end claim the brief asks to prove WITHOUT the bin:
        // a Title "Singleplayer" click maps to Select(NewGame) and, fed to
        // App::tick, advances the app into the NewGame wizard.
        let mut app = App::with_defaults();
        assert_eq!(app.screen(), Screen::Title);

        let click = MenuClick::Select(MenuId::NewGame); // what title_flythrough_ui returns
        let input = map_click(click, &app);
        assert_eq!(input, AppInput::Select(MenuId::NewGame));

        let step = app.tick(input, 0);
        assert_eq!(step, AppStep::Menu, "selecting NewGame stays in the menu");
        assert_eq!(app.screen(), Screen::NewGame, "app advanced into the wizard");
    }

    #[test]
    fn quit_click_maps_and_quits() {
        let mut app = App::with_defaults();
        let input = map_click(MenuClick::Select(MenuId::Quit), &app);
        assert_eq!(input, AppInput::Select(MenuId::Quit));
        // Quit goes through a confirm in the flow; selecting Quit should at least
        // map cleanly and tick without panicking.
        let _ = app.tick(input, 0);
    }
}
