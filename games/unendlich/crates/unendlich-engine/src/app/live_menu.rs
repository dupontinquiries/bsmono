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
//! 7 Load, 8 Singleplayer, 9 Bending Tutorial, 10 Devices (multi-device session
//! over the DHT: `sim::net::device_session`), 11 Stop-hosting-devices.

/// Everything the title-menu body reads. Borrowed views over the renderer's
/// display-state fields.
pub struct LiveMenuState<'a> {
    /// Which screen: 0 title, 1 confirm-quit, 2 multiplayer, 3 load, 4 singleplayer,
    /// 5 devices (the multi-device session: code, QR payload, joined devices).
    pub screen_kind: u8,
    /// The multi-device session as the menu sees it (`DeviceSession::view` on a render box,
    /// `DevicesView::searching` on a phone), or `None` when nothing is hosting or searching.
    pub devices: Option<&'a crate::sim::net::device_session::DevicesView>,
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
    /// Devices screen: the slot the host asked to kick (−1 = none).
    pub device_kick: i32,
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
            device_kick: -1,
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
///
/// Height 46px (was 34px): the touch-target minimum recommended by both
/// Apple's and Android's HIGs is ~44-48pt. 34px was fine for a mouse but
/// unreliable to tap on a phone (Fairphone-class hardware is the actual
/// target per user_prefs). Doesn't hurt desktop/mouse use — just a bit
/// more generous.
fn dest_button_sized(ui: &mut egui::Ui, label: &str, w: f32) -> bool {
    #[cfg(feature = "unendlich-ui")]
    {
        let accent = unendlich_ui::current_theme(ui.ctx()).accent;
        // capped, not sized: the kit's `sized` is a MINIMUM and long
        // letterspaced labels outgrow any row budget (the off-screen delete
        // button). Capped elides the label instead of breaking the row.
        unendlich_ui::button_capped(ui, label, accent, egui::vec2(w, 46.0)).clicked()
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.add_sized([w, 46.0], egui::Button::new(label)).clicked()
    }
}

/// The destiny-style full-width menu button (kit) or a plain button.
///
/// Width now clamps to the available column width (capped at 220, floored
/// at 160) instead of a hard 220px — so it doesn't overflow a narrow
/// portrait phone viewport instead of just happening to fit one by luck.
fn dest_button(ui: &mut egui::Ui, label: &str) -> bool {
    let w = ui.available_width().clamp(160.0, 220.0);
    dest_button_sized(ui, label, w)
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

/// r330-items wiring: rarity tier → title tint. Tier 0 stays body-colored;
/// higher tiers use the familiar ramp. Saturates like `rarity_name`.
pub fn rarity_color(tier: u8) -> egui::Color32 {
    match tier {
        0 => egui::Color32::from_rgb(0xdf, 0xe6, 0xf4), // common — ice body
        1 => egui::Color32::from_rgb(0x7f, 0xd0, 0x8a), // uncommon — green
        2 => egui::Color32::from_rgb(0x6f, 0xc3, 0xff), // rare — the accent blue
        3 => egui::Color32::from_rgb(0xc0, 0x8a, 0xf0), // epic — violet
        4 => egui::Color32::from_rgb(0xf0, 0xa8, 0x4a), // legendary — ember
        _ => egui::Color32::from_rgb(0xf0, 0x6a, 0x8a), // mythic — rose
    }
}

/// r330-items wiring: paint `item_tooltip::compose` lines with their roles —
/// strong title, rarity-tinted subtitle, plain affixes, ITALIC lore, muted
/// attribution — replacing the one flat grey `on_hover_text` blob (r176).
pub fn item_tooltip_ui(ui: &mut egui::Ui, lines: &[crate::sim::item_tooltip::TooltipLine], tier: u8) {
    use crate::sim::item_tooltip::{Emphasis, TooltipRole};
    ui.set_max_width(260.0);
    for l in lines {
        if l.text.is_empty() {
            ui.add_space(4.0); // the blank separator before lore
            continue;
        }
        let mut t = egui::RichText::new(l.text.as_str());
        t = match (l.role, l.emphasis) {
            (TooltipRole::Title, Emphasis::Strong) => t.strong().size(15.0),
            (TooltipRole::Title, _) => t.color(rarity_color(tier)).size(12.0),
            (TooltipRole::Lore, _) => t.italics().weak(),
            (TooltipRole::Flavor, _) => t.weak().size(11.0),
            (TooltipRole::Affix, _) => t,
        };
        ui.label(t);
    }
}

/// r330-items wiring: build the composed tooltip from the bin's canonical tip
/// string ("Name\nEffect…\nLore" — `tip_for`) until items carry structured
/// affix/rarity data (item_modifier is still unwired; rarity arrives with it).
/// First line = title; last line (of ≥3) = lore; the middle = effect lines.
pub fn item_tooltip_from_tip(ui: &mut egui::Ui, tip: &str) {
    use crate::sim::item_tooltip::{compose, AffixView, ItemTooltipView};
    let mut it = tip.split('\n');
    let name = it.next().unwrap_or("");
    let rest: Vec<&str> = it.collect();
    let (effects, lore) = if rest.len() >= 2 {
        (&rest[..rest.len() - 1], rest[rest.len() - 1])
    } else {
        (&rest[..], "")
    };
    let affixes: Vec<AffixView> =
        effects.iter().map(|s| AffixView { label: s, magnitude: 0.0 }).collect();
    let tier = 0u8; // crafted items carry no rarity yet — see item_modifier
    let view = ItemTooltipView {
        name,
        rarity: tier,
        kind_label: None,
        slot_label: None,
        affixes: &affixes,
        lore,
        attribution: None,
        lore_cols: 34,
    };
    item_tooltip_ui(ui, &compose(&view), tier);
}



/// Inputs the new-game WIZARD body draws from. The two text buffers are
/// caller-owned `&mut String`s (egui edits in place); the seed preview is a
/// caller-uploaded texture (the renderer owns the handle + dirty flags —
/// this body only draws what it's given).
pub struct WizardState<'a> {
    pub title: &'a str,
    pub summary: &'a str,
    pub can_next: bool,
    pub at_confirm: bool,
    /// r205: focus the name box this frame (first frame open).
    pub focus_name: bool,
    pub name_buf: &'a mut String,
    pub seed_buf: &'a mut String,
    /// Pre-uploaded seed-preview texture + draw size, if ready.
    pub preview: Option<(egui::TextureId, egui::Vec2)>,
}

/// What the wizard decided this frame. `pick` uses the bin's action codes:
/// 1 Back, 2 Next, 3 Create, 4 Cancel, 5 Mode, 6 Seed, 7 set-name,
/// 8 apply-seed, 9 one-click Create World. `preview_click` = (u, v,
/// secondary) on the birds-eye strip (r185 seek/avoid).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WizardOut {
    pub pick: u8,
    pub preview_click: Option<(f32, f32, bool)>,
}

/// The wizard's flexible-width button — the original `tbtn` art.
fn wiz_button(ui: &mut egui::Ui, label: &str) -> bool {
    #[cfg(feature = "unendlich-ui")]
    {
        let accent = unendlich_ui::current_theme(ui.ctx()).accent;
        unendlich_ui::button(ui, label, accent).clicked()
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.button(label).clicked()
    }
}

/// The new-game WIZARD body (r179-r205), extracted from the renderer window
/// with the live_menu contract: headless-drawable, kit-optional, tested,
/// photographed. The caller owns the window chrome, the text buffers, the
/// preview texture lifecycle, and applies the picks.
pub fn wizard_body(ui: &mut egui::Ui, s: &mut WizardState<'_>) -> WizardOut {
    let mut out = WizardOut::default();
    ui.vertical_centered(|ui| {
        #[cfg(feature = "unendlich-ui")]
        ui.label(
            egui::RichText::new(s.title.to_uppercase())
                .font(unendlich_ui::menu::blocks_font(24.0))
                .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                .extra_letter_spacing(4.0),
        );
        #[cfg(not(feature = "unendlich-ui"))]
        ui.label(egui::RichText::new(s.title.to_uppercase()).size(24.0).strong());
        ui.add_space(8.0);
        ui.label(s.summary);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if wiz_button(ui, " Mode ") {
                out.pick = 5;
            }
            if wiz_button(ui, " Seed ") {
                out.pick = 6;
            }
        });
        // r179: name the world (action 7 applies).
        ui.horizontal(|ui| {
            ui.label("name:");
            let resp = ui.add(egui::TextEdit::singleline(s.name_buf).id_salt("live_menu-name_buf"));
            // r205: focus the name box the first frame the wizard opens, so
            // typing works without a precise click on the field.
            if s.focus_name {
                resp.request_focus();
            }
            if resp.lost_focus() || wiz_button(ui, "set") {
                out.pick = 7;
            }
        });
        // r181/r185: birds-eye equirect strip; click seeks matching seeds,
        // right-click avoids.
        if let Some((tex_id, size)) = s.preview {
            ui.add_space(8.0);
            let img = ui.add(egui::Image::new((tex_id, size)).sense(egui::Sense::click()));
            if img.clicked() || img.secondary_clicked() {
                if let Some(pos) = img.interact_pointer_pos() {
                    let r = img.rect;
                    let u = ((pos.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                    let v = ((pos.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                    out.preview_click = Some((u, v, img.secondary_clicked()));
                }
            }
            ui.weak("birds-eye \u{00b7} equirect strip");
            ui.add_space(4.0);
        }
        // r181: type a seed directly (Enter/apply).
        ui.horizontal(|ui| {
            ui.label("seed:");
            let resp = ui.add(egui::TextEdit::singleline(s.seed_buf).id_salt("live_menu-seed_buf"));
            if resp.lost_focus() || wiz_button(ui, "apply") {
                out.pick = 8;
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if wiz_button(ui, " Back ") {
                out.pick = 1;
            }
            if s.at_confirm {
                if wiz_button(ui, " Create ") {
                    out.pick = 3;
                }
            } else if s.can_next && wiz_button(ui, " Next ") {
                out.pick = 2;
            }
            // r185: one-click create from any step — blank name gets a
            // default bin-side.
            if wiz_button(ui, " Create World ") {
                out.pick = 9;
            }
        });
        ui.add_space(6.0);
        if wiz_button(ui, " Cancel ") {
            out.pick = 4;
        }
    });
    out
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
    // Height 46px (was 32px) — see dest_button_sized's doc for why.
    #[cfg(feature = "unendlich-ui")]
    {
        let accent = unendlich_ui::current_theme(ui.ctx()).accent;
        unendlich_ui::button_sized(ui, label, accent, egui::vec2(220.0, 46.0)).clicked()
    }
    #[cfg(not(feature = "unendlich-ui"))]
    {
        ui.add_sized([220.0, 46.0], egui::Button::new(label)).clicked()
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
        ui.add_space(10.0); // was 6.0 -- more separation between touch targets
        if pause_button(ui, "Settings") {
            out.pick = 2;
        }
        ui.add_space(10.0); // was 6.0
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
                ui.add_space(10.0); // was 4.0
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
                ui.add_space(10.0); // was 4.0
                if dest_button(ui, "Join Server") {
                    out.menu_pick = 3;
                }
                ui.add_space(10.0);
                // Multi-device: this machine as the render box for its owner's phones and
                // second screens, found by a six-character code over the DHT.
                if dest_button(ui, "Devices") {
                    out.menu_pick = 10;
                }
                if let Some(d) = s.devices {
                    ui.weak(d.status());
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
            5 => {
                devices_screen(ui, s.devices, &mut out);
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
                            "World-history points hold a copy of your world so you can step back.\n\
                             They're created on a timer, after long travel, and on crafting;\n\
                             the newest twelve are kept. In dev mode (UNENDLICH_DEV=1) they're\n\
                             created far more often, and you can force one with the button below.\n\
                             Restore loads a point the next time this world is opened -- what it\n\
                             replaces is kept as a point of its own, so a restore can be undone.\n\
                             Delete prunes a bad one.",
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
                ui.add_space(10.0); // was 4.0 -- more separation between touch targets
                if dest_button(ui, "Multiplayer") {
                    out.menu_pick = 2;
                }
                ui.add_space(10.0); // was 4.0
                // r203: Settings reachable from the MAIN MENU.
                if dest_button(ui, "Settings") {
                    out.open_settings = true;
                }
                ui.add_space(10.0); // was 4.0
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

/// The DEVICES screen — the multi-device session over the DHT
/// (`sim::net::device_session`). On the render box: the code to read out or scan, the joined
/// devices with their slot, capabilities and liveness, a kick per row, and a stop button. On
/// a phone: which discovery step is running or succeeded. With nothing hosting or searching
/// it offers to start.
fn devices_screen(ui: &mut egui::Ui, view: Option<&crate::sim::net::device_session::DevicesView>, out: &mut LiveMenuOut) {
    use crate::sim::net::device_session::DeviceCaps;
    ui.label("devices");
    ui.add_space(6.0);
    match view {
        None => {
            ui.weak("host this machine for your phones and second screens");
            ui.add_space(8.0);
            if dest_button(ui, "Start Hosting") {
                out.menu_pick = 10;
            }
        }
        Some(d) if !d.code.is_empty() => {
            // the code, big — it is read across a room
            ui.label(egui::RichText::new(&d.code).size(34.0).strong().monospace());
            ui.weak("type this on the phone, or scan:");
            ui.add(egui::Label::new(egui::RichText::new(&d.qr).small().monospace()).wrap());
            ui.add_space(6.0);
            ui.separator();
            ui.weak(d.status());
            egui::ScrollArea::vertical().id_salt("devices_scroll").max_height(180.0).show(ui, |ui| {
                for row in &d.rows {
                    ui.horizontal(|ui| {
                        let dot = if !row.connected { "\u{25cb}" } else if row.fresh { "\u{25cf}" } else { "\u{25d0}" };
                        ui.label(format!("{dot} {}  {}", row.slot, row.name));
                        let what = if row.slot == 0 {
                            "render box".to_string()
                        } else if row.caps.is_controller() {
                            format!("controller: {}", row.caps.label())
                        } else if row.caps.has(DeviceCaps::DISPLAY) {
                            format!("screen: {}", row.caps.label())
                        } else {
                            row.caps.label()
                        };
                        ui.weak(what);
                        if row.slot != 0 && ui.small_button("kick").clicked() {
                            out.device_kick = row.slot as i32;
                        }
                    });
                }
            });
            ui.add_space(8.0);
            if dest_button(ui, "Stop Hosting") {
                out.menu_pick = 11;
            }
        }
        Some(d) => {
            // a phone looking for its render box
            ui.label(d.status());
            if let Some((step, found)) = d.discovery {
                ui.weak(if found { "connecting" } else { step.label() });
            }
        }
    }
    ui.add_space(10.0);
    if dest_button(ui, "Back") {
        out.menu_pick = 6;
    }
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
                    devices: None,
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
        for kind in [0u8, 1, 2, 3, 4, 5] {
            let out = run_screen(kind, (&mp, &worlds, &rb));
            assert!(
                !out.shapes.is_empty(),
                "screen kind {kind} produced no shapes — the menu did not draw"
            );
        }
    }

    /// The wizard body draws headless with editable buffers and a default
    /// (no-click) outcome — the extraction contract, on both feature paths.
    #[test]
    fn wizard_draws_headless_with_no_outputs() {
        let ctx = test_ctx();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 560.0),
        ));
        let mut name = String::from("Vale");
        let mut seed = String::from("1337");
        let out = ctx.run(input, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let mut st = WizardState {
                    title: "New World",
                    summary: "mode: survival · seed: 1337",
                    can_next: true,
                    at_confirm: false,
                    focus_name: false,
                    name_buf: &mut name,
                    seed_buf: &mut seed,
                    preview: None,
                };
                let w = wizard_body(ui, &mut st);
                assert_eq!(w, WizardOut::default(), "no clicks → no outputs");
            });
        });
        assert!(!out.shapes.is_empty(), "the wizard drew nothing");
        assert_eq!(name, "Vale", "buffers untouched without input");
    }

    /// The composed item tooltip paints every rarity tier headless — the
    /// r330 wiring's guarantee (and the tip-string parser handles the
    /// 2-line "no canonical entry" fallback without inventing lore).
    #[test]
    fn item_tooltips_paint_every_tier_and_the_short_tip() {
        use crate::sim::item_tooltip::{compose, AffixView, ItemTooltipView};
        let ctx = test_ctx();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(720.0, 520.0),
        ));
        let out = ctx.run(input, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                for tier in 0u8..=5 {
                    let affixes = [AffixView { label: "+1 test", magnitude: 1.0 }];
                    let v = ItemTooltipView {
                        name: "T",
                        rarity: tier,
                        kind_label: Some("Kind"),
                        slot_label: None,
                        affixes: &affixes,
                        lore: "lore line",
                        attribution: Some("— attr"),
                        lore_cols: 20,
                    };
                    item_tooltip_ui(ui, &compose(&v), tier);
                }
                item_tooltip_from_tip(ui, "Greatsword\nHeavy two-hander\n3 ingots");
                item_tooltip_from_tip(ui, "Crafted item\n(no canonical entry yet)");
            });
        });
        assert!(!out.shapes.is_empty());
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
                        devices: None,
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
                    devices: None,
                };
                let _ = live_menu_body(ui, &s);
            });
        });
    }

    /// The Devices screen draws every view it has — a render box hosting two devices (one
    /// stale), a phone mid-ladder, and the idle offer — headless, with no output without a
    /// click. The screen exists so the multi-device session has a face; this is the test that
    /// the face is wired to the model.
    #[test]
    fn devices_screen_draws_every_view_headless() {
        use crate::sim::net::device_session::{DeviceCaps, DeviceRow, DevicesView, DiscoveryLadder, Step};
        let hosting = DevicesView {
            code: "ABC-DEF".into(),
            qr: "unendlich://join?code=ABCDEF&host=HOSTHOSTHOST".into(),
            rows: vec![
                DeviceRow { slot: 0, name: "kessler".into(), caps: DeviceCaps::DISPLAY, fresh: true, connected: true },
                DeviceRow { slot: 1, name: "Phone".into(), caps: DeviceCaps::TOUCH.with(DeviceCaps::IMU), fresh: true, connected: true },
                DeviceRow { slot: 2, name: "Tablet".into(), caps: DeviceCaps::DISPLAY.with(DeviceCaps::TOUCH), fresh: false, connected: false },
            ],
            max: 4,
            discovery: None,
        };
        let mut ladder = DiscoveryLadder::start(0);
        ladder.report(Step::Mdns, None, 1);
        let searching = DevicesView::searching(&ladder);
        for (label, view) in [("hosting", Some(&hosting)), ("searching", Some(&searching)), ("idle", None)] {
            let ctx = test_ctx();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(720.0, 560.0)));
            let out = ctx.run(input, |c| {
                egui::CentralPanel::default().show(c, |ui| {
                    let s = LiveMenuState {
                        screen_kind: 5,
                        mp_list: &[],
                        world_list: &[],
                        rollback_list: &[],
                        show_rollbacks: false,
                        world_delete_ask: -1,
                        dev_mode: false,
                        devices: view,
                    };
                    let o = live_menu_body(ui, &s);
                    assert_eq!(o, LiveMenuOut::default(), "{label}: no clicks → no outputs");
                });
            });
            assert!(!out.shapes.is_empty(), "{label}: the devices screen drew nothing");
        }
    }
}
