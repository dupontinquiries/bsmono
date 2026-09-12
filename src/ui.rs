// egui overlay UI: menus, god select, HUD, minimap, scoreboard, end screen.

use std::collections::HashMap;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use glam::{Mat4, Vec3};

use crate::consts::{self, Team, ARENA_RADIUS, LANES};
use crate::gods::ROSTER;
use crate::items;
use crate::sim::{AiState, Game};

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
    pub shop_open: bool,
    pub shop_tab: items::Cat,
    pub selected: usize,
    pub portraits: HashMap<u32, egui::TextureId>,
    pub floaters: Vec<Floater>,
    pub announce: Option<(String, f32, f32)>,
    pub show_controls: bool,
}

impl UiState {
    pub fn new() -> UiState {
        UiState {
            shop_tab: items::Cat::Strength,
            selected: 0,
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

pub fn draw_menu(ctx: &egui::Context, play: &mut bool, quit: &mut bool) {
    setup_style(ctx);
    egui::Area::new(egui::Id::new("menu")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.heading(egui::RichText::new("CLAWBOUND").size(74.0).color(GOLD).strong());
            ui.label(egui::RichText::new("B A T T L E G R O U N D").size(30.0).color(Color32::from_rgb(200, 195, 180)));
            ui.label(egui::RichText::new("Rust · wgpu · Microvoxels — the thirteen legendary ancestors").size(15.0).color(Color32::from_rgb(150, 148, 140)));
            ui.add_space(28.0);
            let play_btn = ui.add_sized([260.0, 52.0], egui::Button::new(egui::RichText::new("▶  PLAY").size(24.0).color(Color32::BLACK).strong()).fill(GOLD));
            if play_btn.clicked() {
                *play = true;
            }
            ui.add_space(8.0);
            if ui.add_sized([260.0, 34.0], egui::Button::new(egui::RichText::new("Quit").size(16.0))).clicked() {
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
    // left: roster grid
    egui::SidePanel::left("roster").exact_width(560.0).resizable(false).show(ctx, |ui| {
        ui.heading(egui::RichText::new("CHOOSE YOUR ANCESTOR").size(24.0).color(GOLD));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for row in 0..(ROSTER.len() + 2) / 3 {
                ui.horizontal(|ui| {
                    for colx in 0..3 {
                        let i = row * 3 + colx;
                        if i >= ROSTER.len() {
                            continue;
                        }
                        let def = &ROSTER[i];
                        let selected = ui_state.selected == i;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(172.0, 210.0), Sense::click());
                        let bg = if selected { GOLD.linear_multiply(0.35) } else { PANEL2 };
                        ui.painter().rect_filled(rect, 8.0, bg);
                        let border = if selected { Stroke::new(3.0_f32, GOLD) } else { Stroke::new(1.0_f32, Color32::from_rgb(70, 68, 80)) };
                        ui.painter().rect_stroke(rect, 8.0, border);
                        // portrait
                        if let Some(tid) = ui_state.portraits.get(&(i as u32)) {
                            let pr = Rect::from_min_size(rect.left_top() + Vec2::new(26.0, 10.0), Vec2::new(120.0, 120.0));
                            ui.painter().image(*tid, pr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        }
                        ui.painter().text(
                            rect.center_bottom() + Vec2::new(0.0, -46.0),
                            Align2::CENTER_CENTER,
                            def.name,
                            FontId::proportional(19.0),
                            Color32::from_rgb(240, 235, 220),
                        );
                        ui.painter().text(
                            rect.center_bottom() + Vec2::new(0.0, -26.0),
                            Align2::CENTER_CENTER,
                            format!("{} · {}", def.role.name(), if def.auto_range > 6.0 { "Ranged" } else { "Melee" }),
                            FontId::proportional(12.0),
                            Color32::from_rgb(160, 158, 150),
                        );
                        if resp.clicked() {
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
        ui.add_space(14.0);
        let lock = ui.add_sized([300.0, 48.0], egui::Button::new(egui::RichText::new("LOCK IN").size(22.0).color(Color32::BLACK).strong()).fill(GOLD));
        if lock.clicked() {
            *locked = true;
        }
    });
}

// --- HUD ----------------------------------------------------------------------

pub fn draw_hud(ctx: &egui::Context, game: &Game, player: usize, vp: Mat4, w: f32, h: f32, ui_state: &mut UiState) {
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
    // gods hp bars
    for i in 0..game.gods.len() {
        let gg = &game.gods[i];
        if gg.respawn_t > 0.0 {
            continue;
        }
        if let Some((sp, _depth)) = project(vp, gg.pos + Vec3::Y * 2.6, w, h) {
            let bw = if i == player { 64.0 } else { 52.0 };
            let r = Rect::from_min_size(sp - Vec2::new(bw / 2.0, 10.0), Vec2::new(bw, 7.0));
            painter.rect_filled(r, 2.0, Color32::from_rgb(15, 14, 20));
            painter.rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (gg.hp / gg.max_hp).clamp(0.0, 1.0), 7.0)), 2.0, col(gg.team));
            if gg.shield > 0.0 {
                painter.rect_filled(Rect::from_min_size(r.left_top(), Vec2::new(r.width() * (gg.shield / gg.max_hp).clamp(0.0, 1.0), 3.5)), 2.0, Color32::from_rgb(160, 220, 255));
            }
            let name = game.god_name(i);
            painter.text(sp + Vec2::new(0.0, -16.0), Align2::CENTER_CENTER, name, FontId::proportional(11.0), col(gg.team));
            if i == player {
                painter.circle_stroke(sp + Vec2::new(0.0, 8.0), 3.0, Stroke::new(1.5_f32, GOLD));
            }
        }
    }
    // minions hp bars (only when damaged)
    for m in &game.minions {
        if m.hp < m.max_hp {
            if let Some((sp, _)) = project(vp, m.pos + Vec3::Y * 1.5, w, h) {
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
        let scale = size * 0.5 / (ARENA_RADIUS + 8.0);
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

pub fn draw_shop(ctx: &egui::Context, game: &mut Game, player: usize, ui_state: &mut UiState) {
    setup_style(ctx);
    egui::Window::new(egui::RichText::new("ARMORY").size(22.0).color(GOLD))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size(Vec2::new(940.0, 620.0))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let g = &game.gods[player];
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Gold: {:.0}", g.gold)).size(17.0).color(GOLD).strong());
                ui.separator();
                for c in items::CATS {
                    let label = c.name();
                    let sel = ui_state.shop_tab == c;
                    let btn = egui::Button::new(egui::RichText::new(label).size(12.5));
                    let btn = if sel { btn.fill(GOLD.linear_multiply(0.55)) } else { btn };
                    if ui.add(btn).clicked() {
                        ui_state.shop_tab = c;
                    }
                }
            });
            ui.separator();
            ui.columns(2, |cols| {
                // shop grid
                cols[0].vertical(|ui| {
                    egui::ScrollArea::vertical().id_source("shop_items").show(ui, |ui| {
                        let cat_items: Vec<&items::Item> = items::all().iter().filter(|it| it.cat == ui_state.shop_tab).collect();
                        for chunk in cat_items.chunks(2) {
                            ui.horizontal(|ui| {
                                for item in chunk {
                                    draw_item_card(ui, game, player, item);
                                }
                            });
                        }
                    });
                });
                // build + owned
                cols[1].vertical(|ui| {
                    ui.heading(egui::RichText::new("YOUR BUILD").size(15.0).color(GOLD));
                    let g = &game.gods[player];
                    let st = Game::item_stats(g);
                    ui.monospace(format!("STR {:.0}   INT {:.0}   PWR {:.0}", st.strength, st.intelligence, st.power));
                    ui.monospace(format!("HP {:.0}   PROT {:.0}   AS +{:.0}%", st.health, st.protection, st.attack_speed * 100.0));
                    ui.monospace(format!("PEN {}% / {:.0} flat   CDR {:.0}%", (st.pen_pct * 100.0) as i32, st.pen_flat, st.cdr * 100.0));
                    ui.separator();
                    for i in 0..6 {
                        if let Some(&iid) = g.items.get(i) {
                            let item = &items::all()[iid as usize];
                            let stacks = g.stacks.get(i).copied().unwrap_or(0.0);
                            let stack_txt = if item.stack.is_some() { format!("  [{} stacks]", stacks) } else { String::new() };
                            ui.label(egui::RichText::new(format!("{}. {} — {}g{}", i + 1, item.name, item.cost, stack_txt)).size(13.0));
                            if let Some(stk) = &item.stack {
                                ui.label(egui::RichText::new(format!("   {} (max {})", stack_kind_name(stk.kind), stk.max)).size(10.5).color(Color32::from_rgb(140, 220, 140)));
                            }
                        } else {
                            ui.label(egui::RichText::new(format!("{}. — empty —", i + 1)).size(13.0).color(Color32::from_rgb(90, 88, 96)));
                        }
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("Click items on the left to purchase.").size(11.5).color(Color32::from_rgb(150, 148, 140)));
                });
            });
        });
}

fn stack_kind_name(k: items::StackKind) -> &'static str {
    match k {
        items::StackKind::MinionKill => "stacks per minion kill",
        items::StackKind::GodKill => "stacks per ancestor takedown",
        items::StackKind::TowerKill => "stacks per tower",
        items::StackKind::Time => "stacks over time",
    }
}

fn draw_item_card(ui: &mut egui::Ui, game: &mut Game, player: usize, item: &items::Item) {
    let g = &game.gods[player];
    let affordable = g.gold >= item.cost && g.items.len() < 6;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(300.0, 96.0), Sense::click());
    let c = item.cat.color();
    let cc = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
    ui.painter().rect_filled(rect, 6.0, PANEL2);
    ui.painter().rect_filled(Rect::from_min_size(rect.left_top(), Vec2::new(6.0, rect.height())), 3.0, cc);
    ui.painter().text(rect.left_top() + Vec2::new(14.0, 14.0), Align2::LEFT_TOP, item.name, FontId::proportional(14.0), if affordable { Color32::WHITE } else { Color32::from_rgb(140, 138, 148) });
    ui.painter().text(rect.right_top() + Vec2::new(-8.0, 12.0), Align2::RIGHT_TOP, format!("{}g", item.cost), FontId::proportional(13.0), GOLD);
    // tier pips
    for t in 0..3 {
        let filled = t < item.tier as i32;
        ui.painter().circle_filled(rect.left_top() + Vec2::new(20.0 + t as f32 * 10.0, 34.0), 3.0, if filled { cc } else { Color32::from_rgb(60, 58, 68) });
    }
    // stat line
    let s = &item.stats;
    let mut parts: Vec<String> = Vec::new();
    if s.strength > 0.0 { parts.push(format!("+{:.0} STR", s.strength)); }
    if s.intelligence > 0.0 { parts.push(format!("+{:.0} INT", s.intelligence)); }
    if s.power > 0.0 { parts.push(format!("+{:.0} PWR", s.power)); }
    if s.protection > 0.0 { parts.push(format!("+{:.0} PROT", s.protection)); }
    if s.pen_pct > 0.0 { parts.push(format!("+{:.0}% PEN", s.pen_pct * 100.0)); }
    if s.pen_flat > 0.0 { parts.push(format!("+{:.0} flat PEN", s.pen_flat)); }
    if s.attack_speed > 0.0 { parts.push(format!("+{:.0}% AS", s.attack_speed * 100.0)); }
    if s.health > 0.0 { parts.push(format!("+{:.0} HP", s.health)); }
    if s.mana > 0.0 { parts.push(format!("+{:.0} MP", s.mana)); }
    if s.cdr > 0.0 { parts.push(format!("+{:.0}% CDR", s.cdr * 100.0)); }
    if s.move_speed > 0.0 { parts.push(format!("+{:.0}% MS", s.move_speed * 100.0)); }
    ui.painter().text(rect.left_top() + Vec2::new(14.0, 56.0), Align2::LEFT_TOP, parts.join("  "), FontId::proportional(11.0), Color32::from_rgb(190, 200, 210));
    ui.painter().text(rect.left_bottom() + Vec2::new(14.0, -6.0), Align2::LEFT_BOTTOM, item.desc, FontId::proportional(9.5), Color32::from_rgb(135, 133, 142));

    if resp.clicked() && affordable {
        let _ = game.buy_item(player, item.id);
    }
    if resp.hovered() {
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5_f32, cc));
    }
}

// --- scoreboard ----------------------------------------------------------------

pub fn draw_scoreboard(ctx: &egui::Context, game: &Game) {
    setup_style(ctx);
    egui::Area::new(egui::Id::new("board")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Frame::default().fill(PANEL).stroke(Stroke::new(1.5_f32, GOLD)).inner_margin(14.0).show(ui, |ui| {
            ui.heading(egui::RichText::new("SCOREBOARD").size(20.0).color(GOLD));
            for team in [Team::Blue, Team::Red] {
                ui.label(egui::RichText::new(if team == Team::Blue { "— ORDER (BLUE) —" } else { "— CHAOS (RED) —" }).size(15.0).color(col(team)));
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
                        ui.monospace(game.god_name(i));
                        ui.monospace(format!("{}", g.level));
                        ui.monospace(format!("{}/{}/{}", g.kills, g.deaths, g.assists));
                        ui.monospace(format!("{:.0}", g.gold));
                        ui.monospace(if names.is_empty() { "—".into() } else { names.join(", ") });
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
    egui::Area::new(egui::Id::new("click_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -150.0]).show(ctx, |ui| {
        ui.label(egui::RichText::new("click the world to take control · Esc for controls").size(14.0).color(Color32::from_rgb(200, 195, 180)));
    });
}

pub fn draw_controls(ctx: &egui::Context, cfg: &mut crate::input::Config, capturing: Option<crate::input::Action>, pad_connected: bool, out: &mut ControlsPanel) {
    use crate::input::{Action, Scheme, ACTIONS};
    setup_style(ctx);
    egui::Area::new(egui::Id::new("controls")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Frame::default().fill(PANEL).stroke(Stroke::new(1.5_f32, GOLD)).inner_margin(16.0).show(ui, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("CONTROLS").size(22.0).color(GOLD));
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
            if changed {
                cfg.save();
            }
            let _ = Action::Menu;
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

    // stick
    if let Some((origin, knob)) = pad.stick_draw() {
        painter.circle(to_px(origin), r_px(0.135), dim, Stroke::new(2.0, ring));
        painter.circle_filled(to_px(knob), r_px(0.05), Color32::from_rgba_premultiplied(230, 226, 216, 200));
    } else {
        // a faint hint where the thumb usually rests
        painter.circle_stroke(to_px(glam::Vec2::new(0.22, pad.extent.y - 0.30)), r_px(0.06), Stroke::new(1.0, Color32::from_rgba_premultiplied(230, 226, 216, 50)));
    }
    // attack
    let atk_c = if pad.attack_down() { Color32::from_rgba_premultiplied(200, 80, 60, 200) } else { dim };
    painter.circle(to_px(l.attack), r_px(crate::touch::ATK_R), atk_c, Stroke::new(2.0, ring));
    painter.text(to_px(l.attack), Align2::CENTER_CENTER, "ATK", FontId::proportional(r_px(0.06)), Color32::WHITE);
    // abilities with cooldown sweep
    for (i, c) in l.abilities.iter().enumerate() {
        let ab = &def.abilities[i];
        let col = Color32::from_rgb((ab.color[0] * 255.0) as u8, (ab.color[1] * 255.0) as u8, (ab.color[2] * 255.0) as u8);
        let cd = g.cds[i];
        let ready = cd <= 0.0 && g.mana >= ab.cost;
        let fill = if pad.ability_down(i) { col } else if ready { col.linear_multiply(0.55) } else { dim };
        painter.circle(to_px(*c), r_px(crate::touch::BTN_R), fill, Stroke::new(2.0, if ab.ult { GOLD } else { ring }));
        let label = if cd > 0.0 { format!("{:.0}", cd.ceil()) } else { format!("{}", i + 1) };
        painter.text(to_px(*c), Align2::CENTER_CENTER, label, FontId::proportional(r_px(0.05)), Color32::WHITE);
    }
    // shop / board / menu
    painter.circle(to_px(l.shop), r_px(crate::touch::BTN_R * 0.8), dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.shop), Align2::CENTER_CENTER, "$", FontId::proportional(r_px(0.05)), GOLD);
    painter.circle(to_px(l.board), r_px(crate::touch::BTN_R * 0.8) / pad.scale, dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.board), Align2::CENTER_CENTER, "≡", FontId::proportional(r_px(0.045) / pad.scale), Color32::WHITE);
    painter.circle(to_px(l.menu), r_px(crate::touch::BTN_R * 0.8) / pad.scale, dim, Stroke::new(1.5, ring));
    painter.text(to_px(l.menu), Align2::CENTER_CENTER, "⚙", FontId::proportional(r_px(0.045) / pad.scale), Color32::WHITE);
}
