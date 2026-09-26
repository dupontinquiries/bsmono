// egui overlay UI: menus, god select, HUD, minimap, scoreboard, end screen.

use std::collections::HashMap;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use glam::{Mat4, Vec3};

use crate::consts::{self, Team, ARENA_RADIUS, LANES};
use crate::gods::ROSTER;
use crate::gridpick::GridPicker;
use crate::items;
use crate::sim::{AiState, Game};

// --- shared animation helpers -------------------------------------------------
// egui's `animate_bool` is already framerate-independent (it reads the
// frame's own dt internally), so it's the whole mechanism: no timers of our
// own to drift or forget to advance. `pulse_*` layers a one-shot decaying
// flash on top (purchases, ability-ready, capture toggling) using egui's
// persistent widget memory as the only place that state lives, so nothing
// here needs to be threaded through `UiState`.

/// Smoothed 0..1 hover amount for `id` this frame.
pub(crate) fn hover_t(ctx: &egui::Context, id: egui::Id, hovered: bool) -> f32 {
    ctx.animate_bool(id, hovered)
}

/// Draw a soft glow around `resp`'s rect that grows in as it's hovered.
pub(crate) fn glow_hover(ui: &mut egui::Ui, resp: &egui::Response, color: Color32, corner: f32) {
    let t = hover_t(ui.ctx(), resp.id, resp.hovered());
    if t > 0.002 {
        let r = resp.rect.expand(1.0 + t * 3.0);
        ui.painter().rect_stroke(r, corner, Stroke::new(1.0 + t * 1.8, color.linear_multiply(t)));
    }
}

/// Trigger a one-shot flash under `id` (call once, on the event).
pub(crate) fn pulse_set(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.insert_temp(id, 1.0f32));
}

/// Read + advance the flash under `id`: 1.0 the frame it's set, decaying to
/// 0 over roughly `1/decay` seconds. Call at most once per id per frame.
pub(crate) fn pulse_get(ctx: &egui::Context, id: egui::Id, decay: f32) -> f32 {
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    let v = ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    if v > 0.0 {
        ctx.data_mut(|d| d.insert_temp(id, (v - dt * decay).max(0.0)));
    }
    v
}

pub const GOLD: Color32 = Color32::from_rgb(222, 186, 96);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(12, 12, 18, 225);
pub const PANEL2: Color32 = Color32::from_rgba_premultiplied(20, 20, 28, 240);

#[derive(Clone)]
pub struct Floater {
    pub pos: Vec3,
    pub text: String,
    pub ttl: f32,
    pub age: f32,
    pub color: Color32,
}

#[derive(Default)]
pub struct UiState {
    /// The map the next match is played on (the select screen's picker).
    pub map_choice: crate::map::MapId,
    /// the player's display name from the game centre, if launched from it
    pub player_label: Option<String>,
    pub shop_open: bool,
    pub shop_tab: items::Cat,
    pub selected: usize,
    pub portraits: HashMap<u32, egui::TextureId>,
    pub floaters: Vec<Floater>,
    pub announce: Option<(String, f32, f32)>,
    pub show_controls: bool,
    /// ancestor-select grid: search box + cursor, shared logic with the shop
    /// (gridpick.rs, adapted from games/clawbound's lobby picker)
    pub pick: GridPicker,
    /// shop item grid: same picker, scoped to the active category
    pub shop_pick: GridPicker,
    /// slot pending a sell confirmation click (click once to arm, again to sell)
    pub sell_armed: Option<usize>,
    /// On a phone the shop shows its shelves OR the build; which one.
    pub shop_phone_tab: crate::shopui::PhoneTab,
}

impl UiState {
    pub fn new() -> UiState {
        UiState {
            map_choice: crate::map::active(),
            shop_tab: items::Cat::Strength,
            selected: 0,
            pick: GridPicker::new(3),
            shop_pick: GridPicker::new(2),
            ..Default::default()
        }
    }

    pub fn push_events(&mut self, events: &[Event2]) {
        for e in events {
            match e {
                Event2::Damage { pos, amount, big } => {
                    if *amount >= 1.0 {
                        self.floaters.push(Floater {
                            pos: *pos,
                            text: format!("{}", amount.round() as i32),
                            ttl: 0.8,
                            age: 0.0,
                            color: if *big { Color32::from_rgb(255, 210, 90) } else { Color32::from_rgb(255, 240, 220) },
                        });
                    }
                }
                Event2::Announce { text, dur } => {
                    self.announce = Some((text.clone(), 0.0, *dur));
                }
            }
        }
    }

    pub fn tick(&mut self, dt: f32) {
        for f in self.floaters.iter_mut() {
            f.age += dt;
        }
        self.floaters.retain(|f| f.age < f.ttl);
        if let Some((_, age, dur)) = &mut self.announce {
            *age += dt;
            if *age > *dur {
                self.announce = None;
            }
        }
    }
}

pub enum Event2 {
    Damage { pos: Vec3, amount: f32, big: bool },
    Announce { text: String, dur: f32 },
}

// ---------------------------------------------------------------------------

fn col(team: Team) -> Color32 {
    match team {
        Team::Blue => Color32::from_rgb(90, 150, 255),
        Team::Red => Color32::from_rgb(255, 96, 84),
    }
}

pub fn project(vp: Mat4, p: Vec3, w: f32, h: f32) -> Option<(Pos2, f32)> {
    let v = vp * Vec3::new(p.x, p.y, p.z).extend(1.0);
    if v.w <= 0.05 {
        return None;
    }
    let ndc = v.truncate() / v.w;
    if ndc.x < -1.1 || ndc.x > 1.1 || ndc.y < -1.1 || ndc.y > 1.1 {
        return None;
    }
    Some((Pos2::new((ndc.x * 0.5 + 0.5) * w, (0.5 - ndc.y * 0.5) * h), v.w))
}

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL2;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(40, 38, 50, 220);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(58, 55, 70, 230);
    style.visuals.selection.bg_fill = GOLD.linear_multiply(0.4);
    style.visuals.override_text_color = Some(Color32::from_rgb(235, 230, 220));
    ctx.set_style(style);
}

// --- main menu ---------------------------------------------------------------

/// The LAN screen. Paints `LanLobby` and reports clicks; decides nothing.
pub fn draw_lan(ctx: &egui::Context, lobby: &mut crate::lanlobby::LanLobby) -> crate::lanlobby::Action {
    use crate::lanlobby::Action;
    setup_style(ctx);
    let mut action = Action::None;
    egui::Area::new(egui::Id::new("lan")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.heading(egui::RichText::new("LOCAL NETWORK").size(40.0).color(GOLD).strong());
            ui.add_space(14.0);
            egui::Frame::none().fill(Color32::from_black_alpha(170)).rounding(8.0).inner_margin(14.0).show(ui, |ui| {
                ui.set_width(560.0);
                if lobby.rows.is_empty() {
                    ui.add_space(26.0);
                    ui.label(egui::RichText::new("No dens found yet.").size(18.0).color(Color32::from_rgb(170, 168, 160)));
                    ui.add_space(26.0);
                }
                for i in 0..lobby.rows.len() {
                    let r = lobby.rows[i].clone();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&r.name).size(19.0).color(Color32::WHITE));
                        ui.label(egui::RichText::new(format!("{}/{}", r.players, r.slots)).size(16.0).color(Color32::from_rgb(170, 168, 160)));
                        if !r.note.is_empty() {
                            ui.label(egui::RichText::new(r.note).size(14.0).color(Color32::from_rgb(230, 140, 90)));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 44 px tall: a thumb, not a cursor.
                            let b = ui.add_enabled(r.joinable, egui::Button::new(egui::RichText::new("JOIN").size(17.0).strong()).min_size(egui::vec2(96.0, 44.0)));
                            if b.clicked() {
                                action = lobby.click_join(i);
                            }
                        });
                    });
                    ui.separator();
                }
            });
            ui.add_space(12.0);
            // A fixed-size block, so `vertical_centered` can centre it. This was
            // `ui.horizontal` with a 120 px spacer, which centres nothing: the
            // row sat at the left edge of an area the long footer line had
            // stretched. Found by running the real binary under Xvfb.
            ui.allocate_ui_with_layout(egui::vec2(230.0 + 150.0 + 8.0, 48.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let label = if lobby.hosting { "■  STOP HOSTING" } else { "▶  HOST A DEN" };
                let host = ui.add_sized([230.0, 48.0], egui::Button::new(egui::RichText::new(label).size(19.0).color(Color32::BLACK).strong()).fill(GOLD));
                if host.clicked() {
                    let on = !lobby.hosting;
                    lobby.set_hosting(on);
                }
                if ui.add_sized([150.0, 48.0], egui::Button::new(egui::RichText::new("Back").size(17.0))).clicked() {
                    action = Action::Back;
                }
            });
            ui.add_space(10.0);
            if let Some(note) = &lobby.handoff_note {
                ui.label(egui::RichText::new(note).size(16.0).color(GOLD));
            }
            ui.label(egui::RichText::new(&lobby.status).size(15.0).color(Color32::from_rgb(190, 188, 178)));
            ui.label(egui::RichText::new("Hosting, finding and joining a den work, and the den counts who is in it. Playing a match across machines does not yet.").size(13.0).color(Color32::from_rgb(230, 170, 110)));
        });
    });
    action
}

pub fn draw_menu(ctx: &egui::Context, play: &mut bool, quit: &mut bool, lan: &mut bool, spectate: &mut bool) {
    setup_style(ctx);
    egui::Area::new(egui::Id::new("menu")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.heading(egui::RichText::new("CLAWBOUND").size(74.0).color(GOLD).strong());
            ui.label(egui::RichText::new("B A T T L E G R O U N D").size(30.0).color(Color32::from_rgb(200, 195, 180)));
            ui.label(egui::RichText::new("Rust · wgpu · Microvoxels — the thirteen legendary ancestors").size(15.0).color(Color32::from_rgb(150, 148, 140)));
            ui.add_space(28.0);
            let play_btn = ui.add_sized([260.0, 52.0], egui::Button::new(egui::RichText::new("▶  PLAY").size(24.0).color(Color32::BLACK).strong()).fill(GOLD));
            let t = hover_t(ctx, play_btn.id, play_btn.hovered());
            glow_hover(ui, &play_btn, GOLD, 6.0);
            if t > 0.001 {
                ui.painter().rect_filled(play_btn.rect, 6.0, Color32::from_white_alpha((t * 40.0) as u8));
            }
            if play_btn.clicked() {
                *play = true;
            }
            ui.add_space(8.0);
            let lan_btn = ui.add_sized([260.0, 42.0], egui::Button::new(egui::RichText::new("LOCAL NETWORK").size(18.0)));
            glow_hover(ui, &lan_btn, GOLD, 5.0);
            if lan_btn.clicked() {
                *lan = true;
            }
            ui.add_space(8.0);
            let spec_btn = ui.add_sized([260.0, 42.0], egui::Button::new(egui::RichText::new("SPECTATE").size(18.0)));
            glow_hover(ui, &spec_btn, GOLD, 5.0);
            if spec_btn.clicked() {
                *spectate = true;
            }
            ui.add_space(8.0);
            let quit_btn = ui.add_sized([260.0, 34.0], egui::Button::new(egui::RichText::new("Quit").size(16.0)));
            glow_hover(ui, &quit_btn, Color32::from_rgb(200, 90, 80), 5.0);
            if quit_btn.clicked() {
                *quit = true;
            }
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Move · Mouse look · Attack · 1-4 abilities · Shop · Scoreboard — all rebindable in CONTROLS; gamepad and touch supported").size(13.0).color(Color32::from_rgb(160, 158, 150)));
            ui.label(egui::RichText::new("5v5 · three lanes · destroy towers to unshield the enemy Elder · watch the sky").size(13.0).color(Color32::from_rgb(160, 158, 150)));
        });
    });
}

// --- god select --------------------------------------------------------------

pub fn draw_select(ctx: &egui::Context, ui_state: &mut UiState, locked: &mut bool) {
    setup_style(ctx);
    // Ancestor tag list for the picker: (name, "Role Melee/Ranged") — typing
    // "guard" or "ranged" narrows the grid same as typing a name.
    let tags: Vec<(&'static str, String)> = ROSTER
        .iter()
        .map(|d| (d.name, format!("{} {}", d.role.name(), if d.auto_range > 6.0 { "ranged" } else { "melee" })))
        .collect();
    let text = ancestor_tag_text;

    // left: roster grid
    egui::SidePanel::left("roster").exact_width(560.0).resizable(false).show(ctx, |ui| {
        ui.heading(egui::RichText::new("CHOOSE YOUR ANCESTOR").size(24.0).color(GOLD));
        ui.add_space(4.0);
        let search_id = egui::Id::new("select_search");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut ui_state.pick.search)
                .id(search_id)
                .hint_text("search by name or role (e.g. \"guardian\", \"ranged\")")
                .desired_width(f32::INFINITY),
        );
        if resp.changed() {
            let s = ui_state.pick.search.clone();
            ui_state.pick.set_search(s);
        }
        let search_focused = resp.has_focus();
        ui.add_space(6.0);

        let shown = ui_state.pick.shown(&tags, text);
        // keyboard grid nav — only when the search box isn't eating the keys
        if !search_focused {
            let (dx, dy, enter) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowRight) as i32 - i.key_pressed(egui::Key::ArrowLeft) as i32,
                    i.key_pressed(egui::Key::ArrowDown) as i32 - i.key_pressed(egui::Key::ArrowUp) as i32,
                    i.key_pressed(egui::Key::Enter),
                )
            });
            if dx != 0 || dy != 0 {
                ui_state.pick.move_cursor(dx, dy, shown.len());
            }
            if enter {
                if let Some(i) = ui_state.pick.hovered(&tags, text) {
                    ui_state.selected = i;
                    *locked = true;
                }
            }
        }
        if let Some(i) = ui_state.pick.hovered(&tags, text) {
            ui_state.selected = i;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            if shown.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("no ancestor matches that search").size(14.0).color(Color32::from_rgb(150, 148, 140)));
                });
            }
            for (cell, chunk) in shown.chunks(3).enumerate() {
                ui.horizontal(|ui| {
                    for (col, &i) in chunk.iter().enumerate() {
                        let def = &ROSTER[i];
                        let cursor_here = ui_state.pick.cursor == cell * 3 + col;
                        let id = egui::Id::new("ancestor_card").with(i);
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(172.0, 210.0), Sense::click());
                        let t = hover_t(ctx, id, resp.hovered() || cursor_here);
                        // a light lift on hover: the card grows a couple of
                        // pixels around its own centre rather than jumping
                        let r = rect.expand(t * 2.5);
                        let bg = if cursor_here { GOLD.linear_multiply(0.35) } else { PANEL2.linear_multiply(1.0 + t * 0.15) };
                        ui.painter().rect_filled(r, 8.0, bg);
                        let border = if cursor_here {
                            Stroke::new(3.0_f32, GOLD)
                        } else {
                            Stroke::new(1.0 + t * 1.5, Color32::from_rgb(70 + (t * 90.0) as u8, 68 + (t * 85.0) as u8, 80 + (t * 90.0) as u8))
                        };
                        ui.painter().rect_stroke(r, 8.0, border);
                        if let Some(tid) = ui_state.portraits.get(&(i as u32)) {
                            let pr = Rect::from_center_size(r.center() + Vec2::new(0.0, -45.0 - t * 2.0), Vec2::splat(120.0 + t * 3.0));
                            ui.painter().image(*tid, pr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        }
                        ui.painter().text(r.center_bottom() + Vec2::new(0.0, -46.0), Align2::CENTER_CENTER, def.name, FontId::proportional(19.0), Color32::from_rgb(240, 235, 220));
                        ui.painter().text(
                            r.center_bottom() + Vec2::new(0.0, -26.0),
                            Align2::CENTER_CENTER,
                            format!("{} · {}", def.role.name(), if def.auto_range > 6.0 { "Ranged" } else { "Melee" }),
                            FontId::proportional(12.0),
                            Color32::from_rgb(160, 158, 150),
                        );
                        if resp.clicked() {
                            ui_state.pick.point(cell * 3 + col, shown.len());
                            ui_state.selected = i;
                        }
                    }
                });
            }
        });
    });

    // right: details
    egui::CentralPanel::default().show(ctx, |ui| {
        let def = &ROSTER[ui_state.selected];
        ui.heading(egui::RichText::new(def.name).size(40.0).color(GOLD).strong());
        ui.label(egui::RichText::new(def.title).size(18.0).color(Color32::from_rgb(190, 185, 170)));
        ui.label(egui::RichText::new(format!("\u{201c}{}\u{201d}", def.philosophy)).size(13.5).italics().color(Color32::from_rgb(165, 160, 150)));
        ui.add_space(10.0);

        // stat bars
        let bars = [
            ("Health", def.hp, 750.0),
            ("Attack", def.auto_dmg, 62.0),
            ("Power", def.power, 14.0),
            ("Protection", def.protection, 32.0),
            ("Speed", def.move_speed, 14.0),
        ];
        for (name, v, max) in bars {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{:<11}", name)).size(13.0).color(Color32::from_rgb(170, 168, 158)));
                let (rect, _) = ui.allocate_exact_size(Vec2::new(220.0, 12.0), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(35, 34, 42));
                ui.painter().rect_filled(Rect::from_min_size(rect.left_top(), Vec2::new(rect.width() * (v / max).clamp(0.05, 1.0), 12.0)), 4.0, GOLD);
                ui.label(egui::RichText::new(format!("{:.0}", v)).size(12.0));
            });
        }
        ui.add_space(12.0);
        ui.label(egui::RichText::new("ABILITIES").size(16.0).color(GOLD));
        ui.add_space(4.0);
        for (slot, ab) in def.abilities.iter().enumerate() {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 34.0), Sense::hover());
                let c = Color32::from_rgb((ab.color[0] * 255.0) as u8, (ab.color[1] * 255.0) as u8, (ab.color[2] * 255.0) as u8);
                ui.painter().rect_filled(rect, 6.0, c.linear_multiply(0.55));
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5_f32, c));
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("{}", slot + 1), FontId::proportional(15.0), Color32::WHITE);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(ab.name).size(14.5).strong());
                    ui.label(egui::RichText::new(ab.desc).size(11.5).color(Color32::from_rgb(175, 172, 162)));
                });
            });
            ui.add_space(2.0);
        }
        ui.add_space(12.0);
        map_picker(ui, &mut ui_state.map_choice);
        ui.add_space(10.0);
        let lock = ui.add_sized([300.0, 48.0], egui::Button::new(egui::RichText::new("LOCK IN").size(22.0).color(Color32::BLACK).strong()).fill(GOLD));
        glow_hover(ui, &lock, GOLD, 8.0);
        ui.label(egui::RichText::new("Enter also locks in · type to search · ← → ↑ ↓ to browse").size(11.0).color(Color32::from_rgb(140, 138, 130)));
        if lock.clicked() {
            *locked = true;
        }
    });
}

// --- HUD ----------------------------------------------------------------------

pub fn draw_hud(ctx: &egui::Context, game: &Game, player: usize, vp: Mat4, w: f32, h: f32, ui_state: &mut UiState) {
    // Project into egui's OWN screen, in logical points. The caller passes
    // the render surface's size, in physical pixels; on a display scaled
    // 1.25x, 2x (Retina), 3x (phones) every world-anchored bar, name and
    // floater landed that factor too far down and right -- following its
    // cat, below it ("healthbars underground, but attached"). The headless
    // renders run at 1x, which is why they never showed it.
    let _ = (w, h);
    let (w, h) = {
        let r = ctx.screen_rect();
        (r.width(), r.height())
    };
    setup_style(ctx);
    let g = &game.gods[player];
    let def = game.def(player);

    // health / mana / xp bottom-left
    egui::Area::new(egui::Id::new("vitals")).anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            // level circle
            let (r, _) = ui.allocate_exact_size(Vec2::new(54.0, 54.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 26.0, Color32::from_rgb(30, 28, 38));
            ui.painter().circle_stroke(r.center(), 26.0, Stroke::new(3.0_f32, GOLD));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", g.level), FontId::proportional(24.0), GOLD);
            ui.vertical(|ui| {
                let bar = |ui: &mut egui::Ui, frac: f32, c: Color32, label: String, height: f32| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(280.0, height), Sense::hover());
                    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(28, 26, 34));
                    ui.painter().rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * frac.clamp(0.0, 1.0), height)), 3.0, c);
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), Color32::WHITE);
                };
                bar(ui, g.hp / g.max_hp, Color32::from_rgb(70, 200, 90), format!("{:.0} / {:.0}", g.hp.ceil(), g.max_hp), 18.0);
                bar(ui, g.mana / g.max_mana, Color32::from_rgb(70, 130, 240), format!("{:.0} / {:.0}", g.mana, g.max_mana), 14.0);
                let need = consts::xp_for_level(g.level);
                let (r, _) = ui.allocate_exact_size(Vec2::new(280.0, 6.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, Color32::from_rgb(28, 26, 34));
                ui.painter().rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (g.xp / need).clamp(0.0, 1.0), 6.0)), 2.0, Color32::from_rgb(190, 160, 240));
            });
        });
        // shield indicator
        if g.shield > 0.0 {
            ui.label(egui::RichText::new(format!("🛡 Shield {:.0}", g.shield)).size(12.0).color(Color32::from_rgb(150, 210, 255)));
        }
    });

    // ability bar bottom-center
    egui::Area::new(egui::Id::new("abilities")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -14.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            for slot in 0..4u32 {
                let ab = &def.abilities[slot as usize];
                let cd_frac = (g.cds[slot as usize] / (ab.cooldown * (1.0 - g.cdr))).clamp(0.0, 1.0);
                let no_mana = g.mana < ab.cost;
                let (r, _) = ui.allocate_exact_size(Vec2::new(62.0, 62.0), Sense::hover());
                let c = Color32::from_rgb((ab.color[0] * 255.0) as u8, (ab.color[1] * 255.0) as u8, (ab.color[2] * 255.0) as u8);
                ui.painter().rect_filled(r, 8.0, c.linear_multiply(0.42));
                // cooldown sweep from top
                if cd_frac > 0.0 {
                    let h = r.height() * cd_frac;
                    ui.painter().rect_filled(Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + h)), 8.0, Color32::from_rgba_premultiplied(8, 8, 10, 190));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{:.1}", g.cds[slot as usize]), FontId::proportional(17.0), Color32::WHITE);
                } else {
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", slot + 1), FontId::proportional(24.0), Color32::WHITE);
                }
                let border = if ab.ult { Stroke::new(3.0_f32, GOLD) } else { Stroke::new(2.0_f32, c) };
                ui.painter().rect_stroke(r, 8.0, border);
                if no_mana {
                    ui.painter().rect_filled(r, 8.0, Color32::from_rgba_premultiplied(20, 20, 60, 90));
                }
                // mana cost
                ui.painter().text(r.left_bottom() + Vec2::new(4.0, -3.0), Align2::LEFT_BOTTOM, format!("{}", ab.cost), FontId::proportional(10.0), Color32::from_rgb(130, 170, 255));
            }
        });
    });

    // gold + items bottom-right (above minimap)
    egui::Area::new(egui::Id::new("items")).anchor(egui::Align2::RIGHT_BOTTOM, [190.0, -16.0]).show(ctx, |ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(format!("● {:.0} gold", g.gold)).size(16.0).color(GOLD).strong());
            ui.horizontal(|ui| {
                for i in 0..6 {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(44.0, 44.0), Sense::hover());
                    if let Some(&iid) = g.items.get(i) {
                        let item = &items::all()[iid as usize];
                        let c = item.cat.color();
                        let cc = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
                        ui.painter().rect_filled(r, 6.0, cc.linear_multiply(0.5));
                        ui.painter().rect_stroke(r, 6.0, Stroke::new(1.5_f32, cc));
                        let tier_mark = "●".repeat(item.tier as usize);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", item.name.chars().next().unwrap_or('?')), FontId::proportional(18.0), Color32::WHITE);
                        ui.painter().text(r.right_bottom() + Vec2::new(-2.0, -2.0), Align2::RIGHT_BOTTOM, tier_mark, FontId::proportional(7.0), GOLD);
                        let stack_count = g.stacks.get(i).copied().unwrap_or(0.0);
                        if stack_count > 0.0 {
                            ui.painter().text(r.left_top() + Vec2::new(3.0, 2.0), Align2::LEFT_TOP, format!("{:.0}", stack_count), FontId::proportional(10.0), Color32::from_rgb(120, 255, 140));
                        }
                    } else {
                        ui.painter().rect_filled(r, 6.0, Color32::from_rgb(26, 25, 33));
                        ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0_f32, Color32::from_rgb(60, 58, 70)));
                    }
                }
            });
        });
    });

    // timer + tower score top-center
    egui::Area::new(egui::Id::new("score")).anchor(egui::Align2::CENTER_TOP, [0.0, 8.0]).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            let mins = (game.t / 60.0).floor() as i32;
            let secs = (game.t % 60.0).floor() as i32;
            ui.label(egui::RichText::new(format!("{:02}:{:02}", mins, secs)).size(20.0).color(Color32::WHITE).strong());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} towers", game.tower_alive[0])).size(14.0).color(col(Team::Blue)));
                ui.label(egui::RichText::new("·").size(14.0));
                ui.label(egui::RichText::new(format!("{} towers", game.tower_alive[1])).size(14.0).color(col(Team::Red)));
            });
        });
    });

    // killfeed top-right
    egui::Area::new(egui::Id::new("feed")).anchor(egui::Align2::RIGHT_TOP, [-12.0, 10.0]).show(ctx, |ui| {
        ui.vertical(|ui| {
            for k in game.killfeed.iter().rev() {
                let age = game.t - k.t;
                if age > 7.0 {
                    continue;
                }
                let a = (1.0 - age / 7.0).clamp(0.3, 1.0);
                let bg = Color32::from_rgba_premultiplied((14.0 * a) as u8, (14.0 * a) as u8, (20.0 * a) as u8, (200.0 * a) as u8);
                let (r, _) = ui.allocate_exact_size(Vec2::new(280.0, 22.0), Sense::hover());
                ui.painter().rect_filled(r, 4.0, bg);
                ui.painter().text(r.left_center() + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER, &k.text, FontId::proportional(12.5), Color32::from_rgb(230, 226, 216));
            }
        });
    });

    // announcements center
    if let Some((text, age, dur)) = &ui_state.announce {
        let fade = (1.0 - age / dur).clamp(0.0, 1.0);
        egui::Area::new(egui::Id::new("announce")).anchor(egui::Align2::CENTER_CENTER, [0.0, -140.0]).show(ctx, |ui| {
            ui.painter().text(
                ui.painter().clip_rect().center() + Vec2::new(0.0, -140.0),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(34.0),
                GOLD.gamma_multiply(fade),
            );
        });
    }

    // sky + map-event chip (top centre)
    {
        let wk = game.weather.kind.name();
        let night = game.weather.night > 0.5;
        let mut line = format!("{}{}", if night { "Night · " } else { "" }, wk);
        if let Some(a) = &game.map_events.active {
            let left = (a.until - game.t).max(0.0);
            line = format!("{}   ·   {} {:.0}s", line, a.kind.name(), left);
        }
        if g.blessing_t > 0.0 {
            line = format!("{}   ·   RIVERBLESSED {:.0}s", line, g.blessing_t);
        }
        for b in &game.bosses.list {
            let short = match b.kind { crate::bosses::BossKind::Boar => "BOAR", crate::bosses::BossKind::Drake => "DRAKE" };
            if b.alive {
                line = format!("{}   ·   {} {:.0}%", line, short, b.hp / b.max_hp * 100.0);
            } else {
                let left = (b.spawn_t - game.t).max(0.0);
                line = format!("{}   ·   {} {}:{:02}", line, short, (left / 60.0) as u32, (left % 60.0) as u32);
            }
        }
        let ti = g.team as usize;
        if game.bosses.bounty_t[ti] > 0.0 {
            line = format!("{}   ·   BOUNTY {:.0}s", line, game.bosses.bounty_t[ti]);
        }
        if game.bosses.stormfire_t[ti] > 0.0 {
            line = format!("{}   ·   STORMFIRE {:.0}s", line, game.bosses.stormfire_t[ti]);
        }
        if let Some(a) = &game.disasters.active {
            line = format!("{}   ·   ⚠ {} {:.0}s", line, a.kind.name(), (a.until - game.t).max(0.0));
        } else if let Some((k, at)) = game.disasters.scheduled {
            if game.disasters.warned {
                line = format!("{}   ·   ⚠ {} in {:.0}s", line, k.name(), (at - game.t).max(0.0));
            }
        }
        egui::Area::new(egui::Id::new("skychip")).anchor(egui::Align2::CENTER_TOP, [0.0, 8.0]).show(ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(line.len() as f32 * 7.4 + 24.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r, 6.0, Color32::from_rgba_premultiplied(12, 12, 18, 170));
            let c = match game.map_events.active.as_ref().map(|a| a.kind.color()) {
                Some(c) => Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8),
                None => Color32::from_rgb(190, 186, 176),
            };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, &line, FontId::proportional(12.5), c);
        });
    }

    // world-space: hp bars + damage floaters
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("world")));
    draw_god_bars(&painter, game, Some(player), vp, w, h);
    // minions hp bars (only when damaged)
    for m in &game.minions {
        if m.hp < m.max_hp {
            // 1.1 not 1.5: the minion is a ~1.0u mini cat, not the 1.26u
            // humanoid this offset was chosen for.
            if let Some((sp, _)) = project(vp, m.pos + Vec3::Y * 1.1, w, h) {
                let r = Rect::from_min_size(sp - Vec2::new(16.0, 6.0), Vec2::new(32.0, 4.0));
                painter.rect_filled(r, 1.5, Color32::from_rgb(15, 14, 20));
                painter.rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (m.hp / m.max_hp).clamp(0.0, 1.0), 4.0)), 1.5, col(m.team));
            }
        }
    }
    // floaters
    for f in &ui_state.floaters {
        if let Some((sp, _)) = project(vp, f.pos + Vec3::Y * (0.5 + f.age * 1.4), w, h) {
            painter.text(sp, Align2::CENTER_CENTER, &f.text, FontId::proportional(if f.color == Color32::from_rgb(255, 210, 90) { 20.0 } else { 15.0 }), f.color.gamma_multiply(1.0 - f.age / f.ttl));
        }
    }

    // minimap
    draw_minimap(ctx, game, player);

    // dead overlay
    if g.respawn_t > 0.0 {
        egui::Area::new(egui::Id::new("dead")).anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0]).show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("YOU HAVE FALLEN").size(30.0).color(Color32::from_rgb(255, 90, 80)).strong());
                ui.label(egui::RichText::new(format!("Respawning in {:.1}s", g.respawn_t)).size(18.0));
            });
        });
    }
}

fn draw_minimap(ctx: &egui::Context, game: &Game, player: usize) {
    egui::Area::new(egui::Id::new("minimap")).anchor(egui::Align2::RIGHT_BOTTOM, [-14.0, -14.0]).show(ctx, |ui| {
        let size = 170.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), Sense::hover());
        let p = ui.painter();
        let scale = size * 0.5 / (consts::arena_radius() + 8.0);
        let to_map = |pos: Vec3| -> Pos2 {
            let c = rect.center();
            Pos2::new(c.x + pos.x * scale, c.y - pos.z * scale)
        };
        // arena disc
        p.circle_filled(rect.center(), size * 0.5, Color32::from_rgba_premultiplied(16, 20, 16, 215));
        p.circle_stroke(rect.center(), size * 0.5, Stroke::new(2.0_f32, Color32::from_rgb(90, 85, 70)));
        // lanes
        for lane in LANES {
            let pts = lane.polyline();
            for w in pts.windows(2) {
                p.line_segment([to_map(w[0]), to_map(w[1])], Stroke::new(2.5_f32, Color32::from_rgb(70, 66, 58)));
            }
        }
        // towers
        for tw in &game.towers {
            let s = to_map(tw.pos);
            p.rect_filled(Rect::from_center_size(s, Vec2::new(7.0, 7.0)), 1.5, col(tw.team));
        }
        // titans
        for t in 0..2 {
            let tt = &game.titans[t];
            if tt.hp > 0.0 {
                let s = to_map(tt.pos);
                p.circle_filled(s, 6.5, col(tt.team));
                p.circle_stroke(s, 6.5, Stroke::new(2.0_f32, GOLD));
            }
        }
        // objectives: a diamond at each pit, filled while the beast is up
        for b in &game.bosses.list {
            let s = to_map(b.home);
            let c = b.kind.color();
            let cc = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
            let pts = vec![s + Vec2::new(0.0, -7.0), s + Vec2::new(7.0, 0.0), s + Vec2::new(0.0, 7.0), s + Vec2::new(-7.0, 0.0)];
            if b.alive {
                p.add(egui::Shape::convex_polygon(pts, cc, Stroke::new(1.5_f32, Color32::BLACK)));
            } else {
                p.add(egui::Shape::closed_line(pts, Stroke::new(1.5_f32, cc.gamma_multiply(0.6))));
            }
        }
        // catastrophes: fire cells, fissures, the funnel
        for f in &game.disasters.fire {
            p.circle_filled(to_map(f.pos), 3.0, Color32::from_rgb(255, 120, 30));
        }
        for c in &game.disasters.charred {
            p.circle_filled(to_map(*c), 2.5, Color32::from_rgb(40, 32, 28));
        }
        for f in &game.disasters.fissures {
            let c = if f.heat > 0.5 { Color32::from_rgb(255, 90, 40) } else { Color32::from_rgb(50, 40, 40) };
            p.line_segment([to_map(f.a), to_map(f.b)], Stroke::new(2.0_f32, c));
        }
        if let Some(t) = &game.disasters.tornado {
            p.circle_stroke(to_map(t.pos), 6.0, Stroke::new(2.0_f32, Color32::from_rgb(200, 200, 210)));
        }
        // minions
        for m in &game.minions {
            p.circle_filled(to_map(m.pos), 1.8, col(m.team).gamma_multiply(0.75));
        }
        // gods
        for i in 0..game.gods.len() {
            let gg = &game.gods[i];
            if gg.respawn_t > 0.0 {
                continue;
            }
            let s = to_map(gg.pos);
            p.circle_filled(s, if i == player { 5.0 } else { 4.2 }, col(gg.team));
            if i == player {
                p.circle_stroke(s, 6.2, Stroke::new(2.0_f32, Color32::WHITE));
            }
        }
        // AI state markers for enemies
        for i in 0..game.gods.len() {
            if i == player {
                continue;
            }
            if let Some(b) = &game.gods[i].ai {
                if b.state == AiState::Retreat {
                    let s = to_map(game.gods[i].pos);
                    p.text(s + Vec2::new(0.0, -9.0), Align2::CENTER_CENTER, "↩", FontId::proportional(10.0), Color32::from_rgb(200, 200, 120));
                }
            }
        }
    });
}

// --- shop ----------------------------------------------------------------------

/// The shop. The Hoard (`shopui`) draws it; `shopview` decides it.
pub fn draw_shop(ctx: &egui::Context, game: &mut Game, player: usize, ui_state: &mut UiState) {
    setup_style(ctx);
    crate::shopui::draw(ctx, game, player, ui_state);
}


// --- scoreboard ----------------------------------------------------------------

pub fn draw_scoreboard(ctx: &egui::Context, game: &Game) {
    draw_scoreboard_labelled(ctx, game, None)
}

pub fn draw_scoreboard_labelled(ctx: &egui::Context, game: &Game, ui_label: Option<&str>) {
    let ui_label = ui_label.map(|s| s.to_string());
    setup_style(ctx);
    // Fifteen a side does not fit a screen, let alone a phone held
    // landscape: the board scrolls, and with more than five a side the item
    // names (the widest column by far) become a count.
    // Anchored to the top with an explicit budget: egui centres an area by
    // LAST frame's size, so a centred board that grows runs off the bottom.
    let max_h = (ctx.screen_rect().height() - 24.0).max(160.0);
    let big = game.gods.iter().filter(|g| g.team == Team::Blue).count() > 5;
    egui::Area::new(egui::Id::new("board")).anchor(egui::Align2::CENTER_TOP, [0.0, 12.0]).show(ctx, |ui| {
        egui::Frame::default().fill(PANEL).stroke(Stroke::new(1.5_f32, GOLD)).inner_margin(14.0).show(ui, |ui| {
            ui.heading(egui::RichText::new("SCOREBOARD").size(20.0).color(GOLD));
            // heading ~28 + frame margins 28 + stroke
            egui::ScrollArea::vertical().id_source("board_scroll").max_height(max_h - 64.0).show(ui, |ui| {
            for team in [Team::Blue, Team::Red] {
                ui.label(egui::RichText::new(format!("— {} —", team.name())).size(15.0).color(col(team)));
                ui.label(egui::RichText::new(format!("hunt: {} animals caught", game.wild.caught[team as usize])).size(12.0).color(Color32::from_rgb(170, 168, 158)));
                egui::Grid::new(format!("grid{}", team as usize)).num_columns(7).spacing([14.0, 3.0]).show(ui, |ui| {
                    ui.monospace("Ancestor");
                    ui.monospace("Lv");
                    ui.monospace("K/D/A");
                    ui.monospace("Gold");
                    ui.monospace("Items");
                    ui.monospace("Lane");
                    ui.monospace("State");
                    ui.end_row();
                    for i in 0..game.gods.len() {
                        let g = &game.gods[i];
                        if g.team != team {
                            continue;
                        }
                        let names: Vec<String> = g.items.iter().map(|id| items::all()[*id as usize].name.to_string()).collect();
                        match (&ui_label, g.is_player) {
                            (Some(n), true) => ui.monospace(format!("{} ({})", game.god_name(i), n)),
                            _ => ui.monospace(game.god_name(i)),
                        };
                        ui.monospace(format!("{}", g.level));
                        ui.monospace(format!("{}/{}/{}", g.kills, g.deaths, g.assists));
                        ui.monospace(format!("{:.0}", g.gold));
                        ui.monospace(if names.is_empty() {
                            "—".into()
                        } else if big {
                            format!("{} item{}", names.len(), if names.len() == 1 { "" } else { "s" })
                        } else {
                            names.join(", ")
                        });
                        ui.monospace(g.ai.as_ref().map_or("YOU".to_string(), |b| b.lane.name().to_string()));
                        ui.monospace(match &g.ai {
                            Some(b) => match b.state {
                                AiState::Push => "pushing".to_string(),
                                AiState::Fight => "fighting".to_string(),
                                AiState::Retreat => "retreating".to_string(),
                            },
                            None => String::new(),
                        });
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
            });
        });
    });
}

// --- end screen ----------------------------------------------------------------

pub fn draw_end(ctx: &egui::Context, game: &Game, again: &mut bool, menu: &mut bool) {
    setup_style(ctx);
    let won = game.over == Some(Team::Blue);
    egui::Area::new(egui::Id::new("end")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(egui::RichText::new(if won { "VICTORY" } else { "DEFEAT" }).size(72.0).color(if won { GOLD } else { Color32::from_rgb(220, 80, 70) }).strong());
            ui.label(egui::RichText::new(format!("Match time {:.0}:{:02}", (game.t / 60.0).floor() as i32, (game.t % 60.0) as i32)).size(16.0));
            ui.add_space(16.0);
            // player stats
            let g = &game.gods[0];
            ui.label(egui::RichText::new(format!("{} — {} / {} / {}   Lv {}   {:.0} gold", game.god_name(0), g.kills, g.deaths, g.assists, g.level, g.gold)).size(18.0));
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                if ui.add_sized([200.0, 42.0], egui::Button::new(egui::RichText::new("Play Again").size(18.0).color(Color32::BLACK).strong()).fill(GOLD)).clicked() {
                    *again = true;
                }
                if ui.add_sized([160.0, 42.0], egui::Button::new(egui::RichText::new("Main Menu").size(18.0))).clicked() {
                    *menu = true;
                }
            });
        });
    });
}

pub fn roster_len() -> usize {
    ROSTER.len()
}


// --- controls panel (rebind by pressing), touch HUD, hints ----------------

/// What the controls panel asked for this frame; app.rs applies it.
pub struct ControlsPanel {
    pub close: bool,
    pub capture: Option<crate::input::Action>,
    pub clear: Option<crate::input::Action>,
    pub scheme: Option<crate::input::Scheme>,
    pub reset: bool,
}

pub fn draw_controls_button(ctx: &egui::Context, open: &mut bool) {
    egui::Area::new(egui::Id::new("controls_btn")).anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0]).show(ctx, |ui| {
        if ui.add_sized([150.0, 32.0], egui::Button::new(egui::RichText::new("⚙  CONTROLS").size(15.0))).clicked() {
            *open = true;
        }
    });
}

pub fn draw_click_to_play(ctx: &egui::Context) {
    // a slow breathing fade, so the prompt reads as "waiting for you" rather
    // than sitting there as flat, static text
    let t = ctx.input(|i| i.time) as f32;
    let breathe = 0.75 + 0.25 * (t * 1.6).sin();
    egui::Area::new(egui::Id::new("click_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -150.0]).show(ctx, |ui| {
        ui.label(egui::RichText::new("click the world to take control · Esc for controls").size(14.0).color(Color32::from_rgba_premultiplied(200, 195, 180, (breathe * 255.0) as u8)));
    });
}

/// A small reticle at screen centre, fading in while the mouse is captured
/// and a touch flourish (a brief expanding ring) whenever capture toggles —
/// it doubles as the clearest possible signal of "you have control now".
pub fn draw_crosshair(ctx: &egui::Context, locked: bool, w: f32, h: f32) {
    let id = egui::Id::new("crosshair_captured");
    let t = ctx.animate_bool(id, locked);
    // fire the toggle ring exactly once per transition, using egui memory to
    // remember which state we last drew (see pulse_set/get above)
    let prev_id = egui::Id::new("crosshair_prev_locked");
    let prev = ctx.data(|d| d.get_temp::<bool>(prev_id)).unwrap_or(locked);
    if prev != locked {
        pulse_set(ctx, egui::Id::new("crosshair_toggle"));
    }
    ctx.data_mut(|d| d.insert_temp(prev_id, locked));
    let flash = pulse_get(ctx, egui::Id::new("crosshair_toggle"), 2.5);
    if t < 0.01 && flash < 0.01 {
        return;
    }
    egui::Area::new(egui::Id::new("crosshair")).fixed_pos(Pos2::ZERO).interactable(false).show(ctx, |ui| {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("crosshair_layer")));
        // the centre of egui's screen, in logical points: w/h are the
        // surface's physical pixels, and at 2x their half is the far corner
        let _ = (w, h);
        let c = ui.ctx().screen_rect().center();
        let a = (t * 220.0) as u8;
        let col = Color32::from_rgba_premultiplied(230, 226, 216, a);
        painter.line_segment([c + Vec2::new(-6.0, 0.0), c + Vec2::new(-2.0, 0.0)], Stroke::new(1.4_f32, col));
        painter.line_segment([c + Vec2::new(2.0, 0.0), c + Vec2::new(6.0, 0.0)], Stroke::new(1.4_f32, col));
        painter.line_segment([c + Vec2::new(0.0, -6.0), c + Vec2::new(0.0, -2.0)], Stroke::new(1.4_f32, col));
        painter.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(0.0, 6.0)], Stroke::new(1.4_f32, col));
        if flash > 0.01 {
            let r = 4.0 + (1.0 - flash) * 22.0;
            painter.circle_stroke(c, r, Stroke::new(1.5_f32, col.linear_multiply(flash)));
        }
    });
}

pub fn draw_controls(ctx: &egui::Context, cfg: &mut crate::input::Config, capturing: Option<crate::input::Action>, pad_connected: bool, out: &mut ControlsPanel) {
    use crate::input::{Action, Scheme, ACTIONS};
    setup_style(ctx);
    egui::Area::new(egui::Id::new("controls")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Frame::default().fill(PANEL).stroke(Stroke::new(1.5_f32, GOLD)).inner_margin(16.0).show(ui, |ui| {
            // scrolls when the screen is short (a phone in landscape): the
            // graphics and audio settings made it taller than 720 px
            let max_h = (ctx.screen_rect().height() - 48.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("CONTROLS & SETTINGS").size(22.0).color(GOLD));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("  ✕  ").clicked() {
                        out.close = true;
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Scheme").size(13.0).color(Color32::from_rgb(170, 168, 158)));
                for sc in [Scheme::Classic, Scheme::Dvorak] {
                    let sel = cfg.bindings.scheme == sc;
                    if ui.selectable_label(sel, sc.name()).clicked() && !sel {
                        out.scheme = Some(sc);
                    }
                }
                if cfg.bindings.scheme == Scheme::Custom {
                    ui.label(egui::RichText::new("CUSTOM").color(GOLD));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("reset all").clicked() {
                        out.reset = true;
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(if pad_connected { "controller connected — press a pad button on a row to bind it" } else { "click a row, then press the key / mouse button you want (Esc cancels)" }).size(12.0).color(Color32::from_rgb(150, 148, 140)));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                egui::Grid::new("bindgrid").num_columns(3).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                    for a in ACTIONS {
                        let cap = capturing == Some(a);
                        ui.label(egui::RichText::new(a.label()).size(13.0));
                        let txt = if cap { "press a key…".to_string() } else { cfg.bindings.labels(a) };
                        let b = ui.add_sized([260.0, 22.0], egui::Button::new(egui::RichText::new(txt).size(13.0).color(if cap { GOLD } else { Color32::WHITE })));
                        if b.clicked() {
                            out.capture = Some(a);
                        }
                        if ui.small_button("clear").clicked() {
                            out.clear = Some(a);
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(8.0);
            ui.separator();
            let st = &mut cfg.settings;
            let mut changed = false;
            changed |= ui.add(egui::Slider::new(&mut st.mouse_sens, 0.2..=3.0).text("mouse sensitivity")).changed();
            changed |= ui.add(egui::Slider::new(&mut st.pad_sens, 0.2..=3.0).text("stick sensitivity")).changed();
            changed |= ui.checkbox(&mut st.invert_y, "invert look Y").changed();
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut st.touch_enabled, "on-screen touch controls").changed();
                changed |= ui.add(egui::Slider::new(&mut st.touch_scale, 0.6..=1.8).text("size")).changed();
            });
            // graphics and audio
            ui.add_space(8.0);
            ui.separator();
            ui.label(egui::RichText::new("GRAPHICS & AUDIO").size(15.0).color(GOLD));
            ui.label(egui::RichText::new("Esc opens this panel during a match too.").size(11.0).color(Color32::from_rgb(170, 166, 156)));
            egui::ComboBox::from_label("graphics").selected_text(st.quality.label()).show_ui(ui, |ui| {
                for q in crate::quality::Quality::ALL {
                    changed |= ui.selectable_value(&mut st.quality, q, q.label()).changed();
                }
            });
            if st.quality == crate::quality::Quality::Ultra {
                ui.horizontal(|ui| {
                    ui.label("fur");
                    changed |= ui.radio_value(&mut st.fur, crate::render::FurStyle::Tape, "tape").changed();
                    changed |= ui.radio_value(&mut st.fur, crate::render::FurStyle::Locks, "locks").changed();
                });
                ui.label(egui::RichText::new("Ultra is a preview: smooth, furred cats, coloured light, traced shadows, wet reflections. Applies from the next match.").size(11.0).color(Color32::from_rgb(170, 166, 156)));
            }
            changed |= ui.add(egui::Slider::new(&mut st.vol_master, 0.0..=1.0).text("master volume")).changed();
            changed |= ui.add(egui::Slider::new(&mut st.vol_music, 0.0..=1.0).text("music")).changed();
            changed |= ui.add(egui::Slider::new(&mut st.vol_sfx, 0.0..=1.0).text("effects")).changed();
            changed |= ui.add(egui::Slider::new(&mut st.vol_voice, 0.0..=1.0).text("voices")).changed();
            changed |= ui.checkbox(&mut st.show_fps, "show frame rate").changed();
            changed |= ui.checkbox(&mut st.vsync, "vsync (off: uncapped, to measure the machine)").changed();
            if changed {
                cfg.save();
            }
            let _ = Action::Menu;
            });
        });
    });
}

/// Touch overlay: floating stick, attack + ability buttons (with cooldown
/// sweeps), shop / board / menu. Geometry from `touch::layout`, drawn in
/// the same shorter-side units the pad uses for hit-testing.
pub fn draw_touch_pad(ctx: &egui::Context, pad: &crate::touch::TouchPad, game: &Game, w: f32, h: f32) {
    let short = w.min(h).max(1.0);
    let ppp = ctx.pixels_per_point();
    let to_px = |v: glam::Vec2| Pos2::new(v.x * short / ppp, v.y * short / ppp);
    let r_px = |r: f32| r * pad.scale * short / ppp;
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("touchpad")));
    let l = crate::touch::layout(pad.extent, pad.scale);
    let g = &game.gods[0];
    let def = game.def(0);
    let dim = Color32::from_rgba_premultiplied(20, 20, 28, 120);
    let ring = Color32::from_rgba_premultiplied(230, 226, 216, 140);

    let t = ctx.input(|i| i.time) as f32;

    // stick
    if let Some((origin, knob)) = pad.stick_draw() {
        painter.circle(to_px(origin), r_px(0.135), dim, Stroke::new(2.0, ring));
        painter.circle_filled(to_px(knob), r_px(0.05), Color32::from_rgba_premultiplied(230, 226, 216, 200));
    } else {
        // a faint hint where the thumb usually rests, breathing gently so
        // it reads as "tap here" rather than a static leftover mark
        let breathe = 0.5 + 0.5 * (t * 1.3).sin();
        let a = (50.0 + breathe * 35.0) as u8;
        painter.circle_stroke(to_px(glam::Vec2::new(0.22, pad.extent.y - 0.30)), r_px(0.055 + breathe * 0.01), Stroke::new(1.0, Color32::from_rgba_premultiplied(230, 226, 216, a)));
    }
    // attack: a quick press-scale via animate_bool, keyed to the button id
    let atk_t = ctx.animate_bool(egui::Id::new("touch_atk"), pad.attack_down());
    let atk_c = Color32::from_rgba_premultiplied((20.0 + atk_t * 180.0) as u8, (20.0 + atk_t * 60.0) as u8, (28.0 + atk_t * 32.0) as u8, (120.0 + atk_t * 80.0) as u8);
    painter.circle(to_px(l.attack), r_px(crate::touch::ATK_R) * (1.0 + atk_t * 0.08), atk_c, Stroke::new(2.0, ring));
    painter.text(to_px(l.attack), Align2::CENTER_CENTER, "ATK", FontId::proportional(r_px(0.06)), Color32::WHITE);
    // abilities: press-scale, a bright flash the instant one comes off
    // cooldown (so you notice without staring at the countdown), cooldown sweep
    for (i, c) in l.abilities.iter().enumerate() {
        let ab = &def.abilities[i];
        let col = Color32::from_rgb((ab.color[0] * 255.0) as u8, (ab.color[1] * 255.0) as u8, (ab.color[2] * 255.0) as u8);
        let cd = g.cds[i];
        let ready = cd <= 0.0 && g.mana >= ab.cost;
        let ready_id = egui::Id::new("touch_ability_ready").with(i);
        let was_ready = ctx.data(|d| d.get_temp::<bool>(ready_id)).unwrap_or(ready);
        if ready && !was_ready {
            pulse_set(ctx, egui::Id::new("touch_ability_flash").with(i));
        }
        ctx.data_mut(|d| d.insert_temp(ready_id, ready));
        let flash = pulse_get(ctx, egui::Id::new("touch_ability_flash").with(i), 2.0);
        let press_t = ctx.animate_bool(egui::Id::new("touch_ability_press").with(i), pad.ability_down(i));
        let fill = if pad.ability_down(i) {
            col
        } else if flash > 0.01 {
            let k = flash * 0.6;
            let [r, g, b, a] = col.to_array();
            Color32::from_rgba_premultiplied((r as f32 * (1.0 - k) + 255.0 * k) as u8, (g as f32 * (1.0 - k) + 255.0 * k) as u8, (b as f32 * (1.0 - k) + 255.0 * k) as u8, a)
        } else if ready {
            col.linear_multiply(0.55)
        } else {
            dim
        };
        let r = r_px(crate::touch::BTN_R) * (1.0 + press_t * 0.1 + flash * 0.12);
        painter.circle(to_px(*c), r, fill, Stroke::new(2.0 + flash * 1.5, if ab.ult { GOLD } else { ring }));
        let label = if cd > 0.0 { format!("{:.0}", cd.ceil()) } else { format!("{}", i + 1) };
        painter.text(to_px(*c), Align2::CENTER_CENTER, label, FontId::proportional(r_px(0.05)), Color32::WHITE);
        // drag-to-aim: only ever drawn for a mobility ability (never an
        // ultimate — see AbilityDef::quick_aimable), and only once the
        // finger has actually committed past the deadzone, so a settling
        // touch never flashes a direction it isn't going to fire
        if let Some(drag) = pad.ability_drag_live(i) {
            let conf = pad.ability_drag_confidence(i);
            // the arrow points where the finger actually is — screen space,
            // y down, exactly like `drag` already is; no convention flip
            // needed here (that only matters for the stored release vector)
            let d = drag.normalize_or(glam::Vec2::new(0.0, -1.0));
            let dir_px = Vec2::new(d.x, d.y);
            let tip = to_px(*c) + dir_px * r_px(crate::touch::BTN_R) * (1.6 + conf * 0.8);
            let aim_col = Color32::from_rgba_premultiplied(255, 255, 255, (140.0 + conf * 100.0) as u8);
            painter.line_segment([to_px(*c), tip], Stroke::new(2.0 + conf * 2.0, aim_col));
            painter.circle_filled(tip, 3.0 + conf * 3.0, aim_col);
            // confidence ring: fills in around the button as the drag
            // commits, so "will this actually fire in that direction" is
            // never a guess
            painter.circle_stroke(to_px(*c), r + 5.0, Stroke::new(2.0 + conf * 2.0, col.linear_multiply(0.4 + conf * 0.6)));
        }
    }
    // shop / board / menu — press feedback so taps feel acknowledged
    painter.circle(to_px(l.shop), r_px(crate::touch::BTN_R * 0.8), dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.shop), Align2::CENTER_CENTER, "$", FontId::proportional(r_px(0.05)), GOLD);
    painter.circle(to_px(l.leap), r_px(crate::touch::BTN_R * 0.8), dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.leap), Align2::CENTER_CENTER, "^", FontId::proportional(r_px(0.06)), GOLD);
    let board_t = ctx.animate_bool(egui::Id::new("touch_board_held"), pad.board_down());
    let board_fill = Color32::from_rgba_premultiplied((20.0 + board_t * 40.0) as u8, 20, 28, (120.0 + board_t * 60.0) as u8);
    painter.circle(to_px(l.board), r_px(crate::touch::BTN_R * 0.8) / pad.scale * (1.0 + board_t * 0.08), board_fill, Stroke::new(1.5, ring));
    painter.text(to_px(l.board), Align2::CENTER_CENTER, "≡", FontId::proportional(r_px(0.045) / pad.scale), Color32::WHITE);
    painter.circle(to_px(l.menu), r_px(crate::touch::BTN_R * 0.8) / pad.scale, dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.menu), Align2::CENTER_CENTER, "⚙", FontId::proportional(r_px(0.045) / pad.scale), Color32::WHITE);
}

/// Named (not closure) so it's automatically higher-ranked over the
/// reference's lifetime — see the note on `GridPicker`'s `text` parameter.
fn ancestor_tag_text<'a>(i: &'a (&'static str, String)) -> (&'a str, &'a str) {
    (i.0, i.1.as_str())
}

/// The match intro's overlay (`intro::Overlay`): cinema bars, the lane
/// caption, the team-versus-team card, the announcer's line, a skip hint.
/// Draws only; every value comes from the intro timeline.
pub fn draw_intro_overlay(ctx: &egui::Context, o: &crate::intro::Overlay) {
    let screen = ctx.screen_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("intro")));
    let (w, h) = (screen.width(), screen.height());
    let a = |x: f32| (x.clamp(0.0, 1.0) * 255.0) as u8;
    // bars
    let bar = h * 0.11 * o.bars;
    if bar > 0.5 {
        p.rect_filled(Rect::from_min_size(screen.min, Vec2::new(w, bar)), 0.0, Color32::BLACK);
        p.rect_filled(Rect::from_min_size(Pos2::new(screen.min.x, screen.max.y - bar), Vec2::new(w, bar)), 0.0, Color32::BLACK);
    }
    if let Some(c) = o.caption {
        p.text(Pos2::new(screen.min.x + 28.0, screen.min.y + bar + 20.0), Align2::LEFT_TOP, c, FontId::proportional(22.0), Color32::from_rgba_unmultiplied(235, 230, 220, 220));
    }
    if o.versus > 0.01 {
        let v = o.versus;
        let cy = screen.center().y - h * 0.04;
        let slide = (1.0 - v) * w * 0.12;
        // a dark band behind the names
        p.rect_filled(Rect::from_center_size(Pos2::new(screen.center().x, cy), Vec2::new(w, h * 0.22)), 0.0, Color32::from_black_alpha(a(v * 0.55)));
        let size = (h * 0.075).clamp(26.0, 64.0);
        for (team, sx, align) in [(Team::Blue, -1.0f32, Align2::RIGHT_CENTER), (Team::Red, 1.0, Align2::LEFT_CENTER)] {
            let c = team.rgb();
            let col = Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, a(v));
            let x = screen.center().x + sx * (w * 0.07 + slide);
            // a soft glow: the name at the SAME size, faint, offset round
            // itself (a bigger font from the same anchor misaligns -- it
            // read as ghost letters)
            for k in 0..8 {
                let ang = k as f32 / 8.0 * std::f32::consts::TAU;
                p.text(Pos2::new(x + ang.cos() * 2.5, cy - size * 0.18 + ang.sin() * 2.5), align, team.name(), FontId::proportional(size), col.linear_multiply(0.12));
            }
            p.text(Pos2::new(x, cy - size * 0.18), align, team.name(), FontId::proportional(size), col);
            p.text(Pos2::new(x, cy + size * 0.62), align, team.motto(), FontId::proportional(size * 0.28), Color32::from_rgba_unmultiplied(220, 216, 205, a(v * 0.85)));
        }
        p.text(Pos2::new(screen.center().x, cy - size * 0.18), Align2::CENTER_CENTER, "VS", FontId::proportional(size * 0.8), Color32::from_rgba_unmultiplied(222, 186, 96, a(v)));
    }
    if let Some(line) = o.line {
        if o.line_alpha > 0.01 {
            // inside the bottom bar when it is there (legible over any
            // scene: pale subtitles over the pass's snow could not be read);
            // just above the edge when it is not
            let y = if bar > 30.0 { screen.max.y - bar * 0.5 + 12.0 } else { screen.max.y - h * 0.06 - 26.0 };
            p.text(Pos2::new(screen.center().x + 1.5, y + 1.5), Align2::CENTER_BOTTOM, line, FontId::proportional((h * 0.034).clamp(16.0, 30.0)), Color32::from_black_alpha(a(o.line_alpha * 0.7)));
            p.text(Pos2::new(screen.center().x, y), Align2::CENTER_BOTTOM, line, FontId::proportional((h * 0.034).clamp(16.0, 30.0)), Color32::from_rgba_unmultiplied(240, 232, 214, a(o.line_alpha)));
        }
    }
    if o.skip_hint > 0.01 {
        p.text(Pos2::new(screen.max.x - 20.0, screen.max.y - 14.0), Align2::RIGHT_BOTTOM, "tap or press any key to skip", FontId::proportional(13.0), Color32::from_rgba_unmultiplied(200, 196, 186, a(o.skip_hint * 0.7)));
    }
}

/// One line on each map, for the picker.
pub fn map_blurb(m: crate::map::MapId) -> &'static str {
    match m {
        crate::map::MapId::Jungle => "the valley: three lanes through deep jungle",
        crate::map::MapId::Pass => "the mountains: a canyon between twin peaks",
    }
}

/// The map picker on the select screen: a card per map, the chosen one lit.
pub fn map_picker(ui: &mut egui::Ui, choice: &mut crate::map::MapId) {
    ui.label(egui::RichText::new("BATTLEGROUND").size(16.0).color(GOLD));
    ui.horizontal(|ui| {
        for m in crate::map::MapId::ALL {
            let sel = *choice == m;
            // wide enough for the blurb (210 overflowed it)
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(236.0, 58.0), Sense::click());
            let t = hover_t(ui.ctx(), egui::Id::new("map_card").with(m.key()), resp.hovered());
            let p = ui.painter();
            p.rect_filled(rect, 8.0, if sel { Color32::from_rgb(58, 52, 36) } else { Color32::from_rgb(34, 33, 40).linear_multiply(1.0 + 0.3 * t) });
            p.rect_stroke(rect, 8.0, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { GOLD } else { Color32::from_rgb(80, 78, 70) }));
            let n = m.team_size();
            p.text(rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, m.name(), FontId::proportional(15.0), if sel { GOLD } else { Color32::from_rgb(220, 216, 205) });
            p.text(rect.right_top() + Vec2::new(-10.0, 9.0), Align2::RIGHT_TOP, format!("{n} v {n}"), FontId::proportional(12.0), Color32::from_rgb(180, 176, 165));
            p.text(rect.left_top() + Vec2::new(10.0, 32.0), Align2::LEFT_TOP, map_blurb(m), FontId::proportional(10.5), Color32::from_rgb(160, 156, 146));
            if resp.clicked() {
                *choice = m;
            }
            ui.add_space(6.0);
        }
    });
}

#[cfg(test)]
mod hidpi_tests {
    use super::*;

    /// At 2x (a Retina display), a cat in the middle of the view has its
    /// health bar in the middle of the screen -- in egui's logical points,
    /// whatever size in physical pixels the caller passes.
    #[test]
    fn health_bars_sit_over_their_cats_on_a_scaled_display() {
        let game = Game::new_teams(&[0], &[1]);
        let cat = game.gods[0].pos;
        let cam = crate::render::Camera { eye: cat + Vec3::new(0.0, 8.0, -12.0), target: cat + Vec3::Y * 2.6, fov_y: 1.0 };
        let (pw, ph) = (1280.0f32, 720.0f32); // physical pixels
        let vp = cam.view_proj(pw / ph);
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        let mut st = UiState::new();
        let mut shapes = Vec::new();
        for k in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(pw / 2.0, ph / 2.0))),
                time: Some(k as f64 * 0.016),
                ..Default::default()
            };
            shapes = ctx.run(raw, |c| draw_hud(c, &game, 0, vp, pw, ph, &mut st)).shapes;
        }
        // the player's bar: a 64-point-wide dark rect
        let bar = shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r) if (r.rect.width() - 64.0).abs() < 0.5 && (r.rect.height() - 7.0).abs() < 0.5 => Some(r.rect),
                _ => None,
            })
            .expect("the player's health bar was drawn");
        let centre = egui::pos2(pw / 4.0, ph / 4.0); // the middle of the LOGICAL screen
        assert!((bar.center().x - centre.x).abs() < 4.0, "bar at x {} -- the middle is {}", bar.center().x, centre.x);
        assert!((bar.center().y - centre.y).abs() < 12.0, "bar at y {} -- the middle is {}", bar.center().y, centre.y);
    }

    /// The crosshair marks the middle of the screen at 2x too.
    #[test]
    fn the_crosshair_is_centred_on_a_scaled_display() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        let (pw, ph) = (1280.0f32, 720.0f32);
        let mut shapes = Vec::new();
        for k in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(pw / 2.0, ph / 2.0))),
                time: Some(k as f64 * 0.016),
                ..Default::default()
            };
            shapes = ctx.run(raw, |c| draw_crosshair(c, true, pw, ph)).shapes;
        }
        let pts: Vec<egui::Pos2> = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, .. } => Some(points[0].lerp(points[1], 0.5)),
                _ => None,
            })
            .collect();
        assert!(!pts.is_empty(), "no crosshair drawn");
        let mid = pts.iter().fold(egui::Vec2::ZERO, |a, p| a + p.to_vec2()) / pts.len() as f32;
        assert!((mid.x - pw / 4.0).abs() < 2.0 && (mid.y - ph / 4.0).abs() < 2.0, "crosshair at {mid:?}, screen middle ({}, {})", pw / 4.0, ph / 4.0);
    }
}

/// Every living cat's health bar, shield and name, over its head; the
/// player (if any) marked. Shared by the player's HUD and the spectator's.
pub fn draw_god_bars(painter: &egui::Painter, game: &Game, player: Option<usize>, vp: Mat4, w: f32, h: f32) {
    for i in 0..game.gods.len() {
        let gg = &game.gods[i];
        if gg.respawn_t > 0.0 {
            continue;
        }
        if let Some((sp, _depth)) = project(vp, gg.pos + Vec3::Y * 2.6, w, h) {
            let bw = if Some(i) == player { 64.0 } else { 52.0 };
            let r = Rect::from_min_size(sp - Vec2::new(bw / 2.0, 10.0), Vec2::new(bw, 7.0));
            painter.rect_filled(r, 2.0, Color32::from_rgb(15, 14, 20));
            painter.rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (gg.hp / gg.max_hp).clamp(0.0, 1.0), 7.0)), 2.0, col(gg.team));
            if gg.shield > 0.0 {
                painter.rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (gg.shield / gg.max_hp).clamp(0.0, 1.0), 3.5)), 2.0, Color32::from_rgb(160, 220, 255));
            }
            let name = game.god_name(i);
            painter.text(sp + Vec2::new(0.0, -16.0), Align2::CENTER_CENTER, name, FontId::proportional(11.0), col(gg.team));
            if Some(i) == player {
                painter.circle_stroke(sp + Vec2::new(0.0, 8.0), 3.0, Stroke::new(1.5_f32, GOLD));
            }
        }
    }
}

/// The spectator's overlay: every cat's bar, the minimap, the mode and who
/// is watched, and the keys. Nothing of any one cat's own HUD.
pub fn draw_spectator_hud(ctx: &egui::Context, game: &Game, vp: Mat4, label: &str) {
    setup_style(ctx);
    let r = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("spectator_world")));
    draw_god_bars(&painter, game, None, vp, r.width(), r.height());
    draw_minimap(ctx, game, 0);
    let top = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("spectator")));
    top.text(egui::pos2(r.center().x, r.top() + 18.0), Align2::CENTER_TOP, label, FontId::proportional(20.0), GOLD);
    top.text(egui::pos2(r.center().x, r.top() + 44.0), Align2::CENTER_TOP, "Tab next cat  ·  Z director  ·  X free camera (WASD, mouse, wheel)  ·  Esc leave", FontId::proportional(12.5), Color32::from_rgb(190, 186, 176));
}

/// The frame-rate readout (a setting).
pub fn draw_fps(ctx: &egui::Context, fps: f32, ms: f32) {
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("fps")));
    let r = ctx.screen_rect();
    p.text(r.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, format!("{fps:.0} fps  {ms:.1} ms"), FontId::monospace(13.0), Color32::from_rgb(220, 230, 200));
}
