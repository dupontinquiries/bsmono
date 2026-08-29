//! `app::live_menu` — the fullgame's REAL title-menu body, extracted from the
//! `world_renderer::prepare` egui closure into library code.
//!
//! Why: the renderer's menu was a ~2000-line closure over dozens of copied
//! locals — invisible to tests and to the sandbox. The closure's own
//! locals-in/write-back discipline made this seam: everything the menu body
//! reads is now [`LiveMenuState`], everything it decides is [`LiveMenuOut`],
//! and the function draws with plain egui. The renderer's window calls it and
//! maps the outputs onto its existing locals (bin protocol unchanged, byte-
//! for-byte the same `menu_pick` codes); `examples/menu_snapshot.rs` calls it
//! on a headless `Context` and photographs the ACTUAL menu — not a parallel
//! implementation that can drift.
//!
//! The unendlich-ui kit (blocky font, destiny buttons, theme accent) is
//! optional: with the `unendlich-ui` feature the menu draws in the full art
//! style; without it (egui-only test/snapshot builds) plain-egui fallbacks
//! keep the structure identical so layout and flow are still the real thing.
//!
//! `menu_pick` codes (the bin's contract — do not renumber): 1 Create World,
//! 2 Multiplayer/Host, 3 Join, 4 Quit→confirm, 5 Quit-confirmed, 6 Back,
//! 7 Load, 8 Singleplayer, 9 Bending Tutorial.

/// Everything the title-menu body reads. Borrowed views over the renderer's
/// display-state fields.
pub struct LiveMenuState<'a> {
    /// Which screen: 0 title, 1 confirm-quit, 2 multiplayer, 3 load, 4 singleplayer.
    pub screen_kind: u8,
    /// Discovered LAN hosts (display labels), Multiplayer screen.
    pub mp_list: &'a [String],
    /// Saved worlds (display labels), Load screen.
    pub world_list: &'a [String],
    /// Rollback points of the peeked world (newest first), Load screen.
    pub rollback_list: &'a [String],
    /// Whether the rollback panel is open on the Load screen.
    pub show_rollbacks: bool,
    /// Index of the world the delete-confirm row is armed for (−1 = none).
    pub world_delete_ask: i32,
    /// Dev mode (shows "create history point now").
    pub dev_mode: bool,
}

/// Everything the player clicked this frame. `-1` / `false` / `0` = nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveMenuOut {
    pub menu_pick: u32,
    pub mp_pick: i32,
    pub world_pick: i32,
    pub rollback_peek: i32,
    pub rollback_restore: i32,
    pub rollback_delete: i32,
    pub world_delete_ask: i32,
    pub world_delete_confirm: i32,
    pub world_delete_cancel: bool,
    pub dev_make_point: bool,
    pub open_settings: bool,
    pub open_character: bool,
}

impl Default for LiveMenuOut {
    fn default() -> Self {
        LiveMenuOut {
            menu_pick: 0,
            mp_pick: -1,
            world_pick: -1,
            rollback_peek: -1,
            rollback_restore: -1,
            rollback_delete: -1,
            world_delete_ask: -1,
            world_delete_confirm: -1,
            world_delete_cancel: false,
            dev_make_point: false,
            open_settings: false,
            open_character: false,
        }
    }
}

/// The hero title in the blocky voxel game font (kit) or a plain heading
/// (egui-only builds + headless snapshots) — same slot either way.
fn hero_title(ui: &mut egui::Ui) {
    #[cfg(feature = "unendlich-ui")]
    {
        ui.label(
            egui::RichText::new("UNENDLICH")
                .font(unendlich_ui::menu::blocks_font(34.0))
                .color(unendlich_ui::INK)
                .extra_letter_spacing(6.0),
        );
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.heading("UNENDLICH");
    }
}

/// The destiny-style menu button at an explicit width (kit) or a plain
/// sized button — for rows that must budget horizontal space.
fn dest_button_sized(ui: &mut egui::Ui, label: &str, w: f32) -> bool {
    #[cfg(feature = "unendlich-ui")]
    {
        let accent = unendlich_ui::current_theme(ui.ctx()).accent;
        // capped, not sized: the kit's `sized` is a MINIMUM and long
        // letterspaced labels outgrow any row budget (the off-screen delete
        // button). Capped elides the label instead of breaking the row.
        unendlich_ui::button_capped(ui, label, accent, egui::vec2(w, 34.0)).clicked()
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.add_sized([w, 34.0], egui::Button::new(label)).clicked()
    }
}

/// The destiny-style full-width menu button (kit) or a plain button.
fn dest_button(ui: &mut egui::Ui, label: &str) -> bool {
    dest_button_sized(ui, label, 220.0)
}

/// The theme accent (the menu's #6fc3ff), kit or fallback.
fn accent(ui: &egui::Ui) -> egui::Color32 {
    #[cfg(feature = "unendlich-ui")]
    {
        return unendlich_ui::current_theme(ui.ctx()).accent;
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        let _ = ui;
        egui::Color32::from_rgb(0x6f, 0xc3, 0xff)
    }
}

/// A section header in the menu language (kit blocks font or plain strong).
fn section_header(ui: &mut egui::Ui, text: &str) {
    #[cfg(feature = "unendlich-ui")]
    {
        ui.label(
            egui::RichText::new(text)
                .font(unendlich_ui::menu::blocks_font(15.0))
                .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                .extra_letter_spacing(3.0),
        );
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.strong(text);
    }
}

/// What the pause menu decided this frame. `pick` uses the bin's r167 codes:
/// 0 nothing, 1 Resume, 2 Settings, 3 Save & Quit to Title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PauseOut {
    pub pick: u8,
    pub open_character: bool,
}

/// The pause menu's button — faithful to the original `tbtn_sized` art
/// (kit `button_sized` at exactly 220×32; labels here are short and fixed,
/// so no capping is needed).
fn pause_button(ui: &mut egui::Ui, label: &str) -> bool {
    #[cfg(feature = "unendlich-ui")]
    {
        let accent = unendlich_ui::current_theme(ui.ctx()).accent;
        unendlich_ui::button_sized(ui, label, accent, egui::vec2(220.0, 32.0)).clicked()
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.add_sized([220.0, 32.0], egui::Button::new(label)).clicked()
    }
}

/// The in-world PAUSE menu body (Esc), extracted from the renderer window
/// like [`live_menu_body`] — same contract: the caller owns the window
/// chrome, this is the content, drawn identically in-game and headless.
/// `build_tag` is the r170 stale-binary detector line.
pub fn pause_menu_body(ui: &mut egui::Ui, build_tag: &str) -> PauseOut {
    let mut out = PauseOut::default();
    ui.set_min_width(300.0);
    ui.vertical_centered(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("unendlich").size(30.0).strong());
        ui.label(egui::RichText::new("\u{2014} paused \u{2014}").size(13.0).weak());
        ui.weak(build_tag); // r170: stale-binary detector
        ui.add_space(14.0);
        if pause_button(ui, "Resume") {
            out.pick = 1;
        }
        ui.add_space(6.0);
        if pause_button(ui, "Settings") {
            out.pick = 2;
        }
        ui.add_space(6.0);
        // r533: change character WITHOUT leaving the world — the selector
        // was previously only on the title screen.
        if pause_button(ui, "Character") {
            out.open_character = true;
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        if pause_button(ui, "Save & Quit to Title") {
            out.pick = 3;
        }
    });
    out
}

/// Draw the title-menu body (hero title + the current screen) and return what
/// was clicked. The caller owns the window/frame chrome (glass, fresnel,
/// anchoring); this is the CONTENT — the part worth photographing and testing.
pub fn live_menu_body(ui: &mut egui::Ui, s: &LiveMenuState) -> LiveMenuOut {
    let mut out = LiveMenuOut::default();
    ui.vertical_centered(|ui| {
        // r223: title in the blocky voxel game font (UnendlichBlocks),
        // letterspaced caps — the mockup's hero treatment.
        hero_title(ui);
        ui.add_space(14.0);
        match s.screen_kind {
            1 => {
                // r186: ConfirmQuit screen (was undrawn -> quit "did nothing").
                ui.label("quit unendlich?");
                ui.add_space(8.0);
                if dest_button(ui, "Quit") {
                    out.menu_pick = 5;
                }
                ui.add_space(4.0);
                if dest_button(ui, "Back") {
                    out.menu_pick = 6;
                }
            }
            2 => {
                ui.label("multiplayer");
                ui.add_space(8.0);
                if dest_button(ui, "Host World") {
                    out.menu_pick = 2;
                }
                ui.add_space(4.0);
                if dest_button(ui, "Join Server") {
                    out.menu_pick = 3;
                }
                ui.add_space(8.0);
                // r394: LAN hosts discovered via the gossip beacon.
                ui.separator();
                ui.label("LAN games");
                if s.mp_list.is_empty() {
                    ui.weak("(searching… none found yet)");
                } else {
                    // Snapshot-loop finding: without a scroll window, every
                    // discovered host grew the window taller — a LAN party's
                    // worth of hosts walked the menu off the screen. Same
                    // treatment as the world list.
                    egui::ScrollArea::vertical()
                        .id_salt("mp_host_scroll")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for (i, label) in s.mp_list.iter().enumerate() {
                                // Snapshot-loop finding: the full advert label
                                // ("name — world (seed) @ ip:port") overflows
                                // the letterspaced caps button. The human part
                                // rides the button; the address sits small
                                // beneath it. Click = join, either way.
                                let (head, addr) = match label.split_once(" @ ") {
                                    Some((h, a)) => (h, Some(a)),
                                    None => (label.as_str(), None),
                                };
                                if dest_button(ui, head) {
                                    out.mp_pick = i as i32;
                                }
                                if let Some(a) = addr {
                                    ui.weak(a);
                                }
                                ui.add_space(2.0);
                            }
                        });
                }
                ui.add_space(8.0);
                if dest_button(ui, "Back") {
                    out.menu_pick = 6;
                }
            }
            3 => {
                // r197: LOAD WORLD — list saved worlds. Each row is a button;
                // click sets menu_world_pick for the bin.
                ui.label("load world");
                ui.add_space(8.0);
                if s.world_list.is_empty() {
                    ui.weak("(no saved worlds yet)");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("world_list_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for (i, label) in s.world_list.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    // Snapshot-loop finding (kit pass): the
                                    // letterspaced caps grow the world button
                                    // with its label, shoving `history` to the
                                    // edge and the delete button OFF-CANVAS.
                                    // Budget the row instead: the world button
                                    // takes what's left after the two fixed
                                    // controls, so they always fit.
                                    let fixed = 70.0 + 28.0 + ui.spacing().item_spacing.x * 3.0;
                                    let btn_w = (ui.available_width() - fixed).max(160.0);
                                    if dest_button_sized(ui, label, btn_w) {
                                        out.world_pick = i as i32;
                                    }
                                    // r203: peek this world's rollback history
                                    // without launching it.
                                    if ui
                                        .add_sized([70.0, 20.0], egui::Button::new("history"))
                                        .clicked()
                                    {
                                        out.rollback_peek = i as i32;
                                    }
                                    // r205: delete (with confirm).
                                    let del = egui::Button::new("\u{1f5d1}")
                                        .fill(egui::Color32::from_rgb(60, 24, 24));
                                    if ui
                                        .add_sized([28.0, 20.0], del)
                                        .on_hover_text("delete world")
                                        .clicked()
                                    {
                                        out.world_delete_ask = i as i32;
                                    }
                                });
                            }
                        });
                    // r205: delete confirmation row — armed by the trash
                    // button, shows the world name and a confirm/cancel pair
                    // so a misclick can't wipe a world.
                    if s.world_delete_ask >= 0
                        && (s.world_delete_ask as usize) < s.world_list.len()
                    {
                        ui.add_space(6.0);
                        ui.separator();
                        let idx = s.world_delete_ask as usize;
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 140, 140),
                            format!(
                                "delete \u{201c}{}\u{201d}? this cannot be undone.",
                                s.world_list[idx]
                            ),
                        );
                        ui.horizontal(|ui| {
                            let yes = egui::Button::new("  delete  ")
                                .fill(egui::Color32::from_rgb(120, 30, 30));
                            if ui.add(yes).clicked() {
                                out.world_delete_confirm = idx as i32;
                            }
                            if ui.button("  cancel  ").clicked() {
                                out.world_delete_cancel = true;
                            }
                        });
                    }
                }
                // r203: ROLLBACK BROWSE — see the points, then restore.
                if s.show_rollbacks {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        section_header(ui, "WORLD HISTORY");
                        ui.weak("(newest first)");
                        // r206: a small "more info" hint.
                        ui.label("\u{2139}").on_hover_text(
                            "World-history points snapshot your world so you can step back.\n\
                             They're created on a timer, after long travel, and on crafting.\n\
                             In dev mode (UNENDLICH_DEV=1) they're created far more often,\n\
                             and you can force one with the button below. Restore loads a\n\
                             point on next launch; delete prunes a bad one.",
                        );
                    });
                    ui.add_space(4.0);
                    if s.rollback_list.is_empty() {
                        ui.weak("(no rollback points for this world yet)");
                    } else {
                        let accent = accent(ui);
                        egui::ScrollArea::vertical()
                            .id_salt("rollback_scroll")
                            .max_height(140.0)
                            .show(ui, |ui| {
                                for (i, label) in s.rollback_list.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        // r227: diamond marker (the menu's
                                        // history-point motif) before each row.
                                        let (drect, _) = ui.allocate_exact_size(
                                            egui::vec2(16.0, 20.0),
                                            egui::Sense::hover(),
                                        );
                                        let c = drect.center();
                                        let h = 5.0;
                                        ui.painter().add(egui::Shape::convex_polygon(
                                            vec![
                                                egui::pos2(c.x, c.y - h),
                                                egui::pos2(c.x + h, c.y),
                                                egui::pos2(c.x, c.y + h),
                                                egui::pos2(c.x - h, c.y),
                                            ],
                                            accent,
                                            egui::Stroke::NONE,
                                        ));
                                        ui.add_sized(
                                            [186.0, 20.0],
                                            egui::Label::new(label.as_str()),
                                        );
                                        if ui
                                            .add_sized([72.0, 20.0], egui::Button::new("restore"))
                                            .clicked()
                                        {
                                            out.rollback_restore = i as i32;
                                        }
                                        // r206: delete a bad point.
                                        let del = egui::Button::new("\u{1f5d1}")
                                            .fill(egui::Color32::from_rgb(60, 24, 24));
                                        if ui
                                            .add_sized([26.0, 20.0], del)
                                            .on_hover_text("delete this point")
                                            .clicked()
                                        {
                                            out.rollback_delete = i as i32;
                                        }
                                    });
                                }
                            });
                    }
                    // r206: dev-only "create history point now".
                    if s.dev_mode {
                        ui.add_space(4.0);
                        if ui.button("\u{2795} create history point now").clicked() {
                            out.dev_make_point = true;
                        }
                    }
                }
                ui.add_space(8.0);
                if dest_button(ui, "Back") {
                    out.menu_pick = 6;
                }
            }
            4 => {
                // r656: Singleplayer submenu — Create / Load / Bending Tutorial.
                ui.label("singleplayer");
                ui.add_space(8.0);
                if dest_button(ui, "Create World") {
                    out.menu_pick = 1;
                }
                ui.add_space(4.0);
                if dest_button(ui, "Load World") {
                    out.menu_pick = 7;
                }
                ui.add_space(4.0);
                if dest_button(ui, "Bending Tutorial") {
                    out.menu_pick = 9;
                }
                ui.add_space(8.0);
                if dest_button(ui, "Back") {
                    out.menu_pick = 6;
                }
            }
            _ => {
                // r656: top level groups play modes — Singleplayer and
                // Multiplayer each open a submenu.
                if dest_button(ui, "Singleplayer") {
                    out.menu_pick = 8;
                }
                ui.add_space(4.0);
                if dest_button(ui, "Multiplayer") {
                    out.menu_pick = 2;
                }
                ui.add_space(4.0);
                // r203: Settings reachable from the MAIN MENU.
                if dest_button(ui, "Settings") {
                    out.open_settings = true;
                }
                ui.add_space(4.0);
                // r488: open the character-select / customizer.
                if dest_button(ui, "Character") {
                    out.open_character = true;
                }
                ui.add_space(12.0);
                if dest_button(ui, "Quit") {
                    out.menu_pick = 4;
                }
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A headless Context with the kit fonts bound when the kit is compiled
    /// in — every test draws through it, so the SAME test bodies cover both
    /// the plain-egui fallback build and the real art-style build. (Without
    /// the install, any blocks_font label panics — the exact hazard the
    /// snapshot loop caught on its first kit run.)
    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        #[cfg(feature = "unendlich-ui")]
        unendlich_ui::menu::install_fonts(
            &ctx,
            include_bytes!("../../../../docs/design/UnendlichBlocks-Regular.ttf"),
            include_bytes!("../../../../docs/design/UnendlichGlyphs-Regular.ttf"),
        );
        #[cfg(feature = "unendlich-ui")]
        unendlich_ui::set_theme(&ctx, unendlich_ui::Theme::DARK);
        ctx
    }

    fn run_screen(kind: u8, lists: (&[String], &[String], &[String])) -> egui::FullOutput {
        let ctx = test_ctx();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 520.0),
        ));
        ctx.run(input, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let s = LiveMenuState {
                    screen_kind: kind,
                    mp_list: lists.0,
                    world_list: lists.1,
                    rollback_list: lists.2,
                    show_rollbacks: !lists.2.is_empty(),
                    world_delete_ask: -1,
                    dev_mode: false,
                };
                let out = live_menu_body(ui, &s);
                assert_eq!(out, LiveMenuOut::default(), "no clicks → no outputs");
            });
        })
    }

    /// The real menu draws on a headless Context for every screen kind — the
    /// property the extraction exists for. Shapes must be non-trivial (the
    /// screen actually rendered something).
    #[test]
    fn every_screen_kind_draws_headless() {
        let mp = vec!["kessler — Vale (seed 7) @ 192.168.1.10:9878".to_string()];
        let worlds = vec!["Vale  ·  seed 7  ·  creative".to_string()];
        let rb = vec!["#3 · autosave · 12 chunks".to_string()];
        for kind in [0u8, 1, 2, 3, 4] {
            let out = run_screen(kind, (&mp, &worlds, &rb));
            assert!(
                !out.shapes.is_empty(),
                "screen kind {kind} produced no shapes — the menu did not draw"
            );
        }
    }

    /// The pause menu draws headless with a default (no-click) outcome —
    /// same guarantee as the title screens, for the same reason.
    #[test]
    fn pause_menu_draws_headless_with_no_outputs() {
        let ctx = test_ctx();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 520.0),
        ));
        let out = ctx.run(input, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let p = pause_menu_body(ui, "menu build test");
                assert_eq!(p, PauseOut::default(), "no clicks → no outputs");
            });
        });
        assert!(!out.shapes.is_empty(), "the pause menu drew nothing");
    }

    /// Empty lists must not panic and must show their placeholder states.
    #[test]
    fn empty_lists_are_safe() {
        for kind in [2u8, 3] {
            run_screen(kind, (&[], &[], &[]));
        }
    }

    /// ART-STYLE REGRESSION GUARD (kit path). With the unendlich-ui feature,
    /// the menu must draw through the REAL art — blocks font, destiny
    /// buttons, theme accent — on a headless Context with the theme applied,
    /// exactly as the renderer applies it. A missing font registration or a
    /// broken kit call panics or draws nothing here BEFORE it ships blank
    /// in-game. (The plain-egui fallback is covered by the tests above; this
    /// one exists so the reingest can't silently lose the style.)
    #[cfg(feature = "unendlich-ui")]
    #[test]
    fn kit_path_draws_the_real_art_headless() {
        let ctx = test_ctx(); // fonts + theme bound, as EguiOverlay::new does in-game
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 520.0),
        ));
        let worlds = vec!["Vale  ·  seed 7".to_string()];
        for kind in [0u8, 1, 2, 3, 4] {
            let out = ctx.run(input.clone(), |c| {
                egui::CentralPanel::default().show(c, |ui| {
                    let s = LiveMenuState {
                        screen_kind: kind,
                        mp_list: &worlds,
                        world_list: &worlds,
                        rollback_list: &worlds,
                        show_rollbacks: kind == 3,
                        world_delete_ask: -1,
                        dev_mode: false,
                    };
                    let _ = live_menu_body(ui, &s);
                });
            });
            assert!(
                !out.shapes.is_empty(),
                "kit-styled screen kind {kind} drew nothing"
            );
        }
    }

    /// The delete-ask index is bounds-checked against the CURRENT list — a
    /// stale index (world deleted, list shrank) must not panic.
    #[test]
    fn a_stale_delete_ask_index_is_ignored() {
        let ctx = test_ctx();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 520.0),
        ));
        let worlds = vec!["only".to_string()];
        ctx.run(input, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let s = LiveMenuState {
                    screen_kind: 3,
                    mp_list: &[],
                    world_list: &worlds,
                    rollback_list: &[],
                    show_rollbacks: false,
                    world_delete_ask: 5, // stale — beyond the list
                    dev_mode: false,
                };
                let _ = live_menu_body(ui, &s);
            });
        });
    }
}
