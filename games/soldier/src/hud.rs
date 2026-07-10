//! hud — the egui HUD painters, shared by the software (app) and GPU renderers.
//!
//! Immediate-mode `egui::Painter` shapes + text drawn over the finished frame — no eframe, no
//! interactive widgets — so the eframe `app` path and the wgpu `gpu` path stamp the *identical*
//! HUD. Extracted verbatim from app.rs (only `draw_hud` lost its `&Settings` arg; the keybind
//! hint that used it stays app-side as `draw_bind_hints`).
#![allow(clippy::too_many_arguments)]
use crate::camera::FpsCam;
use crate::game::Game;
use crate::level::Cell;
use crate::weapons::WeaponKind;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

/// A modern dynamic reticle: four ticks that bloom apart as your spread grows (moving, hip-fire), shrink
/// when you aim down sights, with a small centre dot. Turns faintly red over an enemy.
pub fn draw_reticle(p: &egui::Painter, rect: Rect, g: &Game, ads: f32) {
    let c = rect.center();
    let pl = &g.player;
    let spd = (pl.vel.x * pl.vel.x + pl.vel.z * pl.vel.z).sqrt();
    let bloom = (6.0 + spd * 0.7) * (1.0 - 0.6 * ads) + if pl.sprinting { 8.0 } else { 0.0 };
    let on_target = aim_on_enemy(g);
    let base = g.cosmetics.reticle_color.color();
    let col = if on_target { Color32::from_rgb(245, 90, 78) } else { base };
    let stroke = Stroke::new(1.6, col);
    let len = 7.0;
    let gap = bloom;
    use crate::cosmetics::Reticle;
    match g.cosmetics.reticle {
        Reticle::Cross => {
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], stroke);
            p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
        Reticle::Dot => {
            p.circle_filled(c, 2.2, col);
        }
        Reticle::CrossDot => {
            p.circle_filled(c, 1.6, col);
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], stroke);
            p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
        Reticle::Circle => {
            p.circle_stroke(c, gap + 4.0, stroke);
            p.circle_filled(c, 1.2, col);
        }
        Reticle::Chevron => {
            let y = c.y + gap + 2.0;
            p.line_segment([Pos2::new(c.x - 8.0, y + 6.0), Pos2::new(c.x, y)], stroke);
            p.line_segment([Pos2::new(c.x + 8.0, y + 6.0), Pos2::new(c.x, y)], stroke);
            p.circle_filled(c, 1.2, col);
        }
        Reticle::Brackets => {
            let g2 = gap + 4.0;
            for sx in [-1.0, 1.0] {
                let x = c.x + sx * g2;
                p.line_segment([Pos2::new(x, c.y - 6.0), Pos2::new(x, c.y + 6.0)], stroke);
                p.line_segment([Pos2::new(x, c.y - 6.0), Pos2::new(x - sx * 4.0, c.y - 6.0)], stroke);
                p.line_segment([Pos2::new(x, c.y + 6.0), Pos2::new(x - sx * 4.0, c.y + 6.0)], stroke);
            }
            p.circle_filled(c, 1.2, col);
        }
        Reticle::TBar => {
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
    }
}

/// Rough check: is the camera pointing at a live enemy? Used to tint the reticle.
pub fn aim_on_enemy(g: &Game) -> bool {
    let cam = &g.player.cam;
    let fwd = cam.forward();
    let eye = g.player.eye();
    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let to = e.center() - eye;
        let dist = to.len();
        if dist < 0.5 || dist > 120.0 {
            continue;
        }
        let dir = to * (1.0 / dist);
        if fwd.dot(dir) > 1.0 - (e.kind.def().radius * 1.6 / dist).min(0.05) {
            return true;
        }
    }
    false
}

/// The sniper scope: a black surround with a thin circular cutout and crosshair lines.
pub fn draw_scope(p: &egui::Painter, rect: Rect, ads: f32) {
    let c = rect.center();
    let r = (rect.height() * 0.42) * (0.6 + 0.4 * ads);
    // black surround: four big rects leaving a square hole, then a ring to fake the circle edge
    let dark = Color32::from_rgba_unmultiplied(4, 5, 7, 255);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, c.y - r)), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, c.y + r), rect.max), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, c.y - r), Pos2::new(c.x - r, c.y + r)), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(c.x + r, c.y - r), Pos2::new(rect.max.x, c.y + r)), 0.0, dark);
    p.circle_stroke(c, r, Stroke::new(2.0, Color32::from_rgb(10, 10, 12)));
    p.circle_stroke(c, r - 2.0, Stroke::new(1.0, Color32::from_rgb(40, 44, 50)));
    // crosshair
    let line = Stroke::new(1.0, Color32::from_rgb(20, 22, 26));
    p.line_segment([Pos2::new(c.x - r, c.y), Pos2::new(c.x + r, c.y)], line);
    p.line_segment([Pos2::new(c.x, c.y - r), Pos2::new(c.x, c.y + r)], line);
    p.line_segment([Pos2::new(c.x - 16.0, c.y), Pos2::new(c.x + 16.0, c.y)], Stroke::new(1.6, Color32::from_rgb(180, 30, 30)));
    p.line_segment([Pos2::new(c.x, c.y - 16.0), Pos2::new(c.x, c.y + 16.0)], Stroke::new(1.6, Color32::from_rgb(180, 30, 30)));
    p.circle_filled(c, 1.4, Color32::from_rgb(200, 40, 40));
}

/// The hitmarker: four short diagonal ticks around the centre when a shot connects (red on a kill).
pub fn draw_hitmarker(p: &egui::Painter, rect: Rect, g: &Game) {
    if g.hitmarker <= 0.0 {
        return;
    }
    let c = rect.center();
    let k = (g.hitmarker / 0.18).clamp(0.0, 1.0);
    let col = if g.hitmarker_kill {
        Color32::from_rgba_unmultiplied(235, 60, 50, (k * 255.0) as u8)
    } else {
        Color32::from_rgba_unmultiplied(235, 238, 242, (k * 230.0) as u8)
    };
    let stroke = Stroke::new(2.0, col);
    let r0 = 7.0;
    let r1 = 12.0;
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        p.line_segment([Pos2::new(c.x + dx * r0, c.y + dy * r0), Pos2::new(c.x + dx * r1, c.y + dy * r1)], stroke);
    }
}

pub fn draw_hit_flash(p: &egui::Painter, rect: Rect, g: &Game) {
    let f = g.player.hurt_flash;
    if f > 0.01 {
        let a = (f * 120.0) as u8;
        p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(150, 12, 12, a));
    }
}

/// Underwater screen treatment: a blue-green wash that deepens with submersion, going strongest and adding a
/// caustic shimmer band once the head goes under. Reads immediately as "I'm in the water" without any
/// renderer changes — a flat overlay keyed off the player's submersion state.
pub fn draw_underwater_tint(p: &egui::Painter, rect: Rect, g: &Game) {
    let sub = g.player.submersion;
    if sub <= 0.05 {
        return;
    }
    // wading (feet wet) is a faint tint; head-under is a strong immersive wash.
    let depth = (sub / 1.6).clamp(0.0, 1.0);
    let base_a = if g.player.underwater { 90.0 } else { 26.0 * depth };
    let a = base_a.clamp(0.0, 110.0) as u8;
    p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(28, 92, 116, a));
    if g.player.underwater {
        // a slow caustic shimmer — two offset sine bands of brightness drifting across the screen
        let t = g.time;
        let bands = 5;
        for i in 0..bands {
            let ph = t * 0.6 + i as f32 * 1.3;
            let y = rect.top() + rect.height() * ((ph.sin() * 0.5 + 0.5) * 0.9 + 0.05);
            let h = rect.height() * 0.06;
            let sa = (18.0 + 10.0 * (ph * 1.7).cos()) as u8;
            p.rect_filled(
                Rect::from_min_max(egui::pos2(rect.left(), y), egui::pos2(rect.right(), y + h)),
                0.0,
                Color32::from_rgba_unmultiplied(120, 190, 210, sa),
            );
        }
    }
}

/// Low-health blood vignette around the screen edges — fades out as health regenerates.
pub fn draw_damage_vignette(p: &egui::Painter, rect: Rect, g: &Game) {
    if !g.player.alive {
        return;
    }
    let h = g.player.health;
    if h >= 60.0 {
        return;
    }
    let t = ((60.0 - h) / 60.0).clamp(0.0, 1.0);
    // a pulse so it reads as "critical"
    let pulse = 0.7 + 0.3 * (g.time * 4.0).sin();
    let band = (rect.height() * 0.16) * (0.5 + 0.5 * t);
    let alpha = (t * pulse * 150.0) as u8;
    let col = Color32::from_rgba_unmultiplied(120, 0, 0, alpha);
    // top, bottom, left, right soft bands
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + band)), 0.0, col);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - band), rect.max), 0.0, col);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.min.x + band, rect.max.y)), 0.0, col);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.max.x - band, rect.min.y), rect.max), 0.0, col);
}

pub fn draw_hud(p: &egui::Painter, rect: Rect, g: &Game) {
    let pl = &g.player;
    let pad = 26.0;
    let hy = rect.max.y - pad;

    // ── Bottom-right readout. In The Hunt there's no ammo — show the creature + prey remaining. ──────────
    if g.is_hunt() {
        if g.is_survivor_hunt() {
            // Survivor: escape timer + survivors remaining.
            let t = g.hunt_timer().max(0.0);
            let mm = (t as u32) / 60;
            let ss = (t as u32) % 60;
            let tcol = if g.hunt_escaped() { Color32::from_rgb(150, 230, 150) } else if t < 20.0 { Color32::from_rgb(235, 120, 90) } else { Color32::from_rgb(228, 232, 238) };
            let label = if g.hunt_escaped() { "DAWN".to_string() } else { format!("{}:{:02}", mm, ss) };
            p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, label, FontId::monospace(40.0), tcol);
            p.text(Pos2::new(rect.max.x - pad, hy - 46.0), Align2::RIGHT_BOTTOM, "UNTIL DAWN", FontId::monospace(14.0), dim());
            p.text(Pos2::new(rect.max.x - pad, hy - 68.0), Align2::RIGHT_BOTTOM, format!("SURVIVORS {}", g.survivors_left()), FontId::monospace(15.0), Color32::from_rgb(180, 200, 210));
            p.text(Pos2::new(rect.max.x - pad, hy - 90.0), Align2::RIGHT_BOTTOM, format!("HUNTED BY {}", g.creature().name().to_uppercase()), FontId::monospace(12.0), g.creature().color());
        } else {
            let alive = g.enemies_alive();
            let cr = g.creature();
            p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, format!("{}", alive), FontId::monospace(48.0), Color32::from_rgb(228, 120, 110));
            p.text(Pos2::new(rect.max.x - pad - 70.0, hy - 6.0), Align2::RIGHT_BOTTOM, "PREY", FontId::monospace(18.0), Color32::from_rgb(150, 156, 164));
            p.text(Pos2::new(rect.max.x - pad, hy - 52.0), Align2::RIGHT_BOTTOM, cr.name(), FontId::monospace(18.0), cr.color());
            p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "LMB  STRIKE", FontId::monospace(13.0), dim());
        }
    } else {
    let wpn = pl.weapon();
    let def = wpn.kind.def();
    let reloading = wpn.reloading > 0.0;
    let magcol = if reloading {
        Color32::from_rgb(230, 200, 90)
    } else if wpn.mag == 0 {
        Color32::from_rgb(225, 70, 60)
    } else {
        Color32::from_rgb(238, 240, 244)
    };
    p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, format!("{}", wpn.mag), FontId::monospace(48.0), magcol);
    let reserve_x = rect.max.x - pad - 64.0;
    p.text(
        Pos2::new(reserve_x, hy - 4.0),
        Align2::RIGHT_BOTTOM,
        format!("/ {}", wpn.reserve),
        FontId::monospace(22.0),
        Color32::from_rgb(150, 156, 164),
    );
    // Show the legendary's name (in its accent) when the legendary primary is up; else the archetype name.
    let (wlabel, wcol) = match g.player.loadout.legend() {
        Some(lg) if g.player.current == 0 || g.player.current == 2 => (lg.name, lg.accent.color()),
        _ => (def.name, weapon_color(def.kind)),
    };
    p.text(Pos2::new(rect.max.x - pad, hy - 52.0), Align2::RIGHT_BOTTOM, wlabel, FontId::monospace(18.0), wcol);
    // Class grenade count (T to throw): label reflects the equipped grenade kind.
    let frags = g.frags();
    let fcol = if frags > 0 { Color32::from_rgb(210, 150, 90) } else { dim() };
    let glabel = match pl.class.def().grenade {
        crate::class::GrenadeKind::Frag => "T FRAG",
        crate::class::GrenadeKind::Gravity => "T GRAV",
        crate::class::GrenadeKind::Acid => "T ACID",
    };
    p.text(Pos2::new(reserve_x, hy + 18.0), Align2::RIGHT_BOTTOM, format!("{} x{}", glabel, frags), FontId::monospace(15.0), fcol);
    if reloading {
        p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "RELOADING", FontId::monospace(13.0), Color32::from_rgb(230, 200, 90));
    } else if wpn.mag == 0 {
        p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "PRESS R", FontId::monospace(13.0), dim());
    }
    }

    // weapon slot pips (primary / sidearm) — not shown in The Hunt (claws only)
    if !g.is_hunt() {
        let mut x = rect.max.x - pad;
        for slot in (0..2).rev() {
            let k = pl.weapons[slot].kind;
            let sel = slot == pl.current;
            let col = if sel { weapon_color(k) } else { dim() };
            p.text(Pos2::new(x, hy - 100.0), Align2::RIGHT_BOTTOM, format!("{}", slot + 1), FontId::monospace(if sel { 18.0 } else { 14.0 }), col);
            x -= 24.0;
        }
    }

    // ── Health bar, bottom-left ───────────────────────────────────────────────────
    let bar_w = 188.0;
    let bar_h = 12.0;
    let bx = rect.min.x + pad;
    let bbottom = hy;
    let bar = Rect::from_min_max(Pos2::new(bx, bbottom - bar_h), Pos2::new(bx + bar_w, bbottom));
    p.rect_filled(bar, 2.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
    let frac = (pl.health / pl.max_health).clamp(0.0, 1.0);
    let fill = Rect::from_min_max(bar.min, Pos2::new(bar.min.x + bar_w * frac, bar.max.y));
    p.rect_filled(fill, 2.0, health_color(pl.health));
    p.rect_stroke(bar, 2.0, Stroke::new(1.0, Color32::from_rgb(60, 68, 78)), egui::StrokeKind::Inside);
    p.text(Pos2::new(bx, bbottom - bar_h - 6.0), Align2::LEFT_BOTTOM, format!("HEALTH  {}", pl.class.name().to_uppercase()), FontId::monospace(12.0), dim());

    // ── Super-ability charge bar, just above the health bar (only for classes with a super) ──────────────
    if pl.class.def().super_kind != crate::class::SuperKind::None {
        let sc = g.super_charge().clamp(0.0, 1.0);
        let sb_bottom = bbottom - bar_h - 28.0;
        let sbar = Rect::from_min_max(Pos2::new(bx, sb_bottom - 8.0), Pos2::new(bx + bar_w, sb_bottom));
        p.rect_filled(sbar, 2.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
        let scol = if sc >= 1.0 { Color32::from_rgb(120, 220, 255) } else { Color32::from_rgb(70, 120, 160) };
        let sfill = Rect::from_min_max(sbar.min, Pos2::new(sbar.min.x + bar_w * sc, sbar.max.y));
        p.rect_filled(sfill, 2.0, scol);
        p.rect_stroke(sbar, 2.0, Stroke::new(1.0, Color32::from_rgb(50, 70, 84)), egui::StrokeKind::Inside);
        let slabel = if sc >= 1.0 { "SUPER READY [X]".to_string() } else { "SUPER".to_string() };
        let slcol = if sc >= 1.0 { Color32::from_rgb(140, 230, 255) } else { dim() };
        p.text(Pos2::new(bx, sb_bottom - 10.0), Align2::LEFT_BOTTOM, slabel, FontId::monospace(11.0), slcol);
    }
    // ── Jetpack charge bar (only when the loadout has a jetpack) — the ion-drive fuel gauge ───────────────
    if pl.jetpack {
        let jc = pl.jet_fuel.clamp(0.0, 1.0);
        // sit it just above the super bar (or the health bar if no super)
        let super_offset = if pl.class.def().super_kind != crate::class::SuperKind::None { 44.0 } else { 0.0 };
        let jb_bottom = bbottom - bar_h - 28.0 - super_offset;
        let jbar = Rect::from_min_max(Pos2::new(bx, jb_bottom - 8.0), Pos2::new(bx + bar_w, jb_bottom));
        p.rect_filled(jbar, 2.0, Color32::from_rgba_unmultiplied(8, 14, 22, 180));
        // colour shifts from dim blue (empty) to bright ion-cyan (full); pulse while actively thrusting
        let base = if jc >= 0.999 { Color32::from_rgb(120, 210, 255) } else { Color32::from_rgb(70, 130, 190) };
        let jcol = if pl.jet_active {
            let f = 0.7 + 0.3 * ((g.time * 30.0).sin() * 0.5 + 0.5);
            Color32::from_rgb((90.0 + 130.0 * f) as u8, (170.0 + 60.0 * f) as u8, 255)
        } else {
            base
        };
        let jfill = Rect::from_min_max(jbar.min, Pos2::new(jbar.min.x + bar_w * jc, jbar.max.y));
        p.rect_filled(jfill, 2.0, jcol);
        p.rect_stroke(jbar, 2.0, Stroke::new(1.0, Color32::from_rgb(48, 78, 104)), egui::StrokeKind::Inside);
        let jlabel = if jc < 0.15 { "ION CHARGING" } else { "ION THRUST" };
        p.text(Pos2::new(bx, jb_bottom - 10.0), Align2::LEFT_BOTTOM, jlabel, FontId::monospace(11.0), Color32::from_rgb(120, 200, 255));
    }
    if g.streak >= 2 {
        let sy = if pl.class.def().super_kind != crate::class::SuperKind::None { bbottom - bar_h - 48.0 } else { bbottom - bar_h - 24.0 };
        p.text(
            Pos2::new(bx, sy),
            Align2::LEFT_BOTTOM,
            format!("STREAK x{}", g.streak),
            FontId::monospace(13.0),
            Color32::from_rgb(255, 190, 80),
        );
    }

    // ── Score / wave, top-left ────────────────────────────────────────────────────
    p.text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad),
        Align2::LEFT_TOP,
        format!("{:06}", g.score),
        FontId::monospace(26.0),
        Color32::from_rgb(235, 238, 244),
    );
    p.text(Pos2::new(rect.min.x + pad, rect.min.y + pad + 30.0), Align2::LEFT_TOP, "SCORE", FontId::monospace(12.0), dim());
    p.text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad + 54.0),
        Align2::LEFT_TOP,
        format!("WAVE {}   ENEMIES {}", g.wave, g.enemies_alive()),
        FontId::monospace(15.0),
        Color32::from_rgb(210, 150, 120),
    );
}

/// UAV: a red diamond marker over each live enemy (on-screen), or an edge chevron pointing toward
/// off-screen ones — the recon sweep that lets you push with information.
pub fn draw_uav_markers(p: &egui::Painter, rect: Rect, g: &Game, cam: &FpsCam, cx: f32, cy: f32, scale: f32) {
    let c = rect.center();
    let col = Color32::from_rgb(235, 80, 70);
    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let head = e.center() + crate::math::Vec3::new(0.0, e.kind.def().height * 0.5 + 0.4, 0.0);
        match cam.project(head, cx, cy, scale) {
            Some((sx, sy, depth)) if depth > 0.0 => {
                let at = Pos2::new(sx, sy);
                // small downward diamond
                let s = 6.0;
                p.add(egui::Shape::convex_polygon(
                    vec![Pos2::new(at.x, at.y - s), Pos2::new(at.x + s, at.y), Pos2::new(at.x, at.y + s), Pos2::new(at.x - s, at.y)],
                    Color32::from_rgba_unmultiplied(235, 80, 70, 180),
                    Stroke::new(1.0, col),
                ));
            }
            _ => {
                // off-screen: a chevron at the edge in the enemy's screen direction
                let to = e.center() - cam.pos;
                let rx = to.dot(cam.right());
                let fz = to.dot(cam.forward());
                let ang = rx.atan2(fz.max(0.001)) * if fz < 0.0 { 3.0 } else { 1.0 };
                let dir = Vec2::new(ang.sin(), -0.2).normalized();
                let edge = c + dir * (rect.width().min(rect.height()) * 0.4);
                p.circle_filled(edge, 3.0, Color32::from_rgba_unmultiplied(235, 80, 70, 150));
            }
        }
    }
}

/// Directional damage indicators: red arcs at the screen edge pointing toward where fire is coming from.
pub fn draw_damage_arrows(p: &egui::Painter, rect: Rect, g: &Game) {
    let c = rect.center();
    let yaw = g.player.cam.yaw;
    for (dir, life) in &g.damage_dirs {
        // angle of the shooter relative to where we're facing (0 = dead ahead)
        let world_ang = dir.x.atan2(-dir.z); // matches yaw convention (yaw 0 → -Z)
        let rel = world_ang - yaw;
        let a = (life.clamp(0.0, 1.0) * 200.0) as u8;
        let col = Color32::from_rgba_unmultiplied(220, 40, 40, a);
        // place an arc ~120px from centre at the relative bearing (screen: +x right, +y down)
        let rr = rect.height() * 0.18;
        let centre = c + Vec2::new(rel.sin() * rr, -rel.cos() * rr);
        // a short arc drawn as a thick chevron facing outward
        let tangent = Vec2::new(rel.cos(), rel.sin());
        let out = Vec2::new(rel.sin(), -rel.cos());
        let p0 = centre - tangent * 18.0;
        let p1 = centre + out * 12.0;
        let p2 = centre + tangent * 18.0;
        p.line_segment([p0, p1], Stroke::new(4.0, col));
        p.line_segment([p1, p2], Stroke::new(4.0, col));
    }
}

/// Scorestreak readout: Q-UAV and F-STRIKE with a fill bar that completes as you score, and a READY tag.
pub fn draw_scorestreaks(p: &egui::Painter, rect: Rect, g: &Game) {
    let x = rect.min.x + 26.0;
    let mut y = rect.min.y + 26.0 + 84.0;
    let rows = [
        ("G  UAV", g.uav_progress(), g.uav_ready(), g.uav_timer > 0.0),
        ("H  SENTRY", g.sentry_progress(), g.sentry_ready(), false),
        ("F  STRIKE", g.strike_progress(), g.strike_ready(), false),
    ];
    for (label, prog, ready, active) in rows {
        let key_col = if active {
            Color32::from_rgb(120, 230, 140)
        } else if ready {
            Color32::from_rgb(255, 210, 90)
        } else {
            Color32::from_rgb(150, 156, 164)
        };
        p.text(Pos2::new(x, y), Align2::LEFT_TOP, label, FontId::monospace(13.0), key_col);
        // progress bar
        let bw = 96.0;
        let bh = 5.0;
        let bar = Rect::from_min_size(Pos2::new(x + 86.0, y + 3.0), Vec2::new(bw, bh));
        p.rect_filled(bar, 1.0, Color32::from_rgba_unmultiplied(10, 12, 16, 160));
        let fillw = bw * prog.clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(fillw, bh)), 1.0, key_col);
        if active {
            p.text(Pos2::new(x + 86.0 + bw + 8.0, y, ), Align2::LEFT_TOP, "ONLINE", FontId::monospace(11.0), key_col);
        } else if ready {
            p.text(Pos2::new(x + 86.0 + bw + 8.0, y), Align2::LEFT_TOP, "READY", FontId::monospace(11.0), key_col);
        }
        y += 18.0;
    }
}

/// A thin top-centre compass strip with cardinal letters that scrolls as you turn (a BO2 HUD staple).
pub fn draw_compass(p: &egui::Painter, rect: Rect, g: &Game) {
    let yaw = g.player.cam.yaw;
    let cx = rect.center().x;
    let top = rect.min.y + 16.0;
    let halfw = 150.0;
    let strip = Rect::from_min_max(Pos2::new(cx - halfw, top), Pos2::new(cx + halfw, top + 20.0));
    p.rect_filled(strip, 3.0, Color32::from_rgba_unmultiplied(10, 12, 16, 130));
    // yaw=0 looks -Z which we call North. Map heading to pixels: 90° across ≈ halfw.
    let px_per_rad = halfw / (std::f32::consts::PI * 0.5);
    let marks = [("N", 0.0), ("E", std::f32::consts::FRAC_PI_2), ("S", std::f32::consts::PI), ("W", -std::f32::consts::FRAC_PI_2)];
    for (lbl, ang) in marks {
        // signed smallest difference between facing yaw and this mark
        let mut d = ang - yaw;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        let xx = cx + d * px_per_rad;
        if xx < strip.min.x + 6.0 || xx > strip.max.x - 6.0 {
            continue;
        }
        p.text(Pos2::new(xx, top + 10.0), Align2::CENTER_CENTER, lbl, FontId::monospace(13.0), Color32::from_rgb(220, 226, 232));
    }
    // centre tick
    p.line_segment([Pos2::new(cx, top), Pos2::new(cx, top + 20.0)], Stroke::new(1.5, Color32::from_rgb(255, 200, 90)));
}

/// Kill feed: recent elimination lines fading out in the top-right, under the minimap.
pub fn draw_killfeed(p: &egui::Painter, rect: Rect, g: &Game) {
    let x = rect.max.x - 16.0;
    let mut y = rect.min.y + 170.0;
    for (text, life) in g.feed.iter().take(5) {
        let a = (life.clamp(0.0, 1.0) * 230.0) as u8;
        let col = if text.starts_with("WAVE") {
            Color32::from_rgba_unmultiplied(255, 200, 90, a)
        } else {
            Color32::from_rgba_unmultiplied(225, 235, 245, a)
        };
        p.text(Pos2::new(x, y), Align2::RIGHT_TOP, text, FontId::monospace(14.0), col);
        y += 19.0;
    }
}

pub fn draw_minimap(p: &egui::Painter, rect: Rect, g: &Game) {
    let size = 132.0;
    let map_rect = Rect::from_min_size(Pos2::new(rect.max.x - size - 16.0, rect.min.y + 16.0), Vec2::splat(size));
    p.rect_filled(map_rect, 4.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
    p.rect_stroke(map_rect, 4.0, Stroke::new(1.0, Color32::from_rgb(60, 70, 80)), egui::StrokeKind::Inside);

    let lvl = &g.level;
    let ts = lvl.ts();
    let radius_tiles = 13.0;
    let ppt = (size * 0.5) / radius_tiles;
    let center = map_rect.center();
    let px = g.player.pos.x / ts;
    let pz = g.player.pos.z / ts;

    let to_screen = |wx_tiles: f32, wz_tiles: f32| -> Pos2 { Pos2::new(center.x + (wx_tiles - px) * ppt, center.y + (wz_tiles - pz) * ppt) };

    let r = radius_tiles.ceil() as i32;
    let (ptx, ptz) = (px.floor() as i32, pz.floor() as i32);
    for tz in (ptz - r)..=(ptz + r) {
        for tx in (ptx - r)..=(ptx + r) {
            let cell = lvl.cell(tx, tz);
            let col = match cell {
                Cell::Wall | Cell::Solid => Color32::from_rgb(90, 110, 130),
                Cell::Floor => Color32::from_rgba_unmultiplied(40, 50, 60, 130),
                _ => continue,
            };
            let sc = to_screen(tx as f32 + 0.5, tz as f32 + 0.5);
            if !map_rect.contains(sc) {
                continue;
            }
            let half = ppt * 0.5;
            p.rect_filled(Rect::from_center_size(sc, Vec2::splat(half * 2.0 - 1.0)), 0.0, col);
        }
    }

    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let sc = to_screen(e.pos.x / ts, e.pos.z / ts);
        if map_rect.contains(sc) {
            p.circle_filled(sc, 2.6, Color32::from_rgb(235, 70, 60));
        }
    }

    p.circle_filled(center, 3.0, Color32::from_rgb(120, 255, 140));
    let yaw = g.player.cam.yaw;
    let dir = Vec2::new(yaw.sin(), -yaw.cos());
    p.line_segment([center, center + dir * 11.0], Stroke::new(1.5, Color32::from_rgb(120, 255, 140)));
}

pub fn draw_center_banner(p: &egui::Painter, rect: Rect, text: &str, col: Color32) {
    let c = rect.center();
    p.text(c + Vec2::new(2.0, 2.0), Align2::CENTER_CENTER, text, FontId::monospace(40.0), Color32::from_black_alpha(180));
    p.text(c, Align2::CENTER_CENTER, text, FontId::monospace(40.0), col);
}

// ── small colour helpers ────────────────────────────────────────────────────────

pub fn dim() -> Color32 {
    Color32::from_rgb(150, 156, 162)
}
pub fn health_color(h: f32) -> Color32 {
    if h > 60.0 {
        Color32::from_rgb(120, 230, 130)
    } else if h > 30.0 {
        Color32::from_rgb(235, 200, 90)
    } else {
        Color32::from_rgb(230, 70, 60)
    }
}
pub fn weapon_color(k: WeaponKind) -> Color32 {
    match k {
        WeaponKind::Ar => Color32::from_rgb(210, 214, 222),
        WeaponKind::Smg => Color32::from_rgb(200, 210, 220),
        WeaponKind::Shotgun => Color32::from_rgb(230, 170, 110),
        WeaponKind::Sniper => Color32::from_rgb(150, 200, 255),
        WeaponKind::Lmg => Color32::from_rgb(220, 200, 150),
        WeaponKind::Dmr => Color32::from_rgb(210, 220, 170),
        WeaponKind::Pistol => Color32::from_rgb(200, 205, 215),
        WeaponKind::EmSword => Color32::from_rgb(120, 200, 255),
        WeaponKind::Launcher => Color32::from_rgb(220, 190, 130),
        WeaponKind::ArcCoil => Color32::from_rgb(150, 220, 255),
        WeaponKind::CryoLance => Color32::from_rgb(190, 240, 255),
        WeaponKind::VoidMaul => Color32::from_rgb(180, 130, 220),
    }
}
