//! map_preview — the crossfading fly-around preview of a map, shared by BOTH frontends
//! (the software `app` shell and the `gpu` bin). Renders with the same software rasterizer
//! the app uses; the texture id is the egui font atlas (Managed(0)) exactly as the app's
//! world render does — vertex colors carry the look, so no dedicated atlas is required.


use egui::{Color32, Pos2, Rect, Stroke};
use crate::math::Vec3;

fn darker(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
}
use crate::camera::FpsCam;
use crate::camera::scale_for_fov;

pub struct MapPreview {
    pub key: u64,
    level: crate::level::Level,
    vantages: Vec<FpsCam>,
    pub light: crate::mesh::Light,
    pub fog: crate::render::Fog,
}

impl MapPreview {
    pub fn build(key: u64, cfg: crate::level::MapConfig) -> Self {
        let level = crate::level::Level::generate(cfg);
        let ts = level.ts();
        let center = Vec3::new(level.w as f32 * 0.5 * ts, level.floor_y, level.h as f32 * 0.5 * ts);
        // Aim at the centerpiece: the most-central room (where the landmark sits), lifted a little so the
        // structure and the ground around it are both in frame.
        let (mcx, mcz) = (center.x, center.z);
        let mut focus = center;
        let mut bd = f32::MAX;
        for r in &level.rooms {
            let d = (r.center.x - mcx).powi(2) + (r.center.z - mcz).powi(2);
            if d < bd {
                bd = d;
                focus = r.center;
            }
        }
        let target = Vec3::new(focus.x, focus.y + 3.0, focus.z);
        // Distance and height scale with the map so big maps pull back; always high enough to clear terrain
        // and look *down* on the layout at a cinematic angle.
        let reach = level.w as f32 * ts;
        let dist = (reach * 0.16).clamp(24.0, 60.0);
        let height = (reach * 0.12).clamp(16.0, 42.0);
        // Three establishing angles sweeping around the centerpiece (plus one lower approach from spawn).
        let mut vantages: Vec<FpsCam> = (0..3)
            .map(|i| {
                let ang = 0.6 + i as f32 * 2.25;
                let pos = target + Vec3::new(ang.cos() * dist, height, ang.sin() * dist);
                let f = (target - pos).normalized();
                FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() }
            })
            .collect();
        // A grounded approach shot from the spawn looking toward the centerpiece — sells the player's-eye scale.
        let sp = level.spawn(0);
        let spos = sp + Vec3::new(0.0, 6.0, 0.0);
        let sf = (Vec3::new(target.x, target.y, target.z) - spos).normalized();
        vantages.push(FpsCam { pos: spos, yaw: sf.x.atan2(-sf.z), pitch: sf.y.clamp(-1.4, 1.4).asin() });

        let pal = level.pal;
        let mut light = crate::mesh::Light::sun(Vec3::new(-0.4, 0.92, 0.42), Color32::from_rgb(255, 248, 230));
        light.sky = pal.sky;
        light.ground = Color32::from_rgb(52, 50, 56);
        light.ambient = 1.05; // brighter than in-game so the thumbnail reads clearly
        // Push the fog well back so the establishing distance doesn't grey the scene out.
        let fog = crate::render::Fog::new(pal.fog, 55.0, 280.0);
        Self { key, level, vantages, light, fog }
    }

    pub fn paint(&self, painter: &egui::Painter, rect: Rect, time: f32, atlas: egui::TextureId) {
        let n = self.vantages.len();
        if n == 0 {
            return;
        }
        let cycle = 4.5_f32; // seconds per vantage — a slow, readable sweep
        let phase = time / cycle;
        let i = (phase as usize) % n;
        let j = (i + 1) % n;
        let frac = phase.fract();
        // crossfade over the last 40%, eased (smoothstep) so the transition glides rather than snaps
        let raw = ((frac - 0.6) / 0.4).clamp(0.0, 1.0);
        let fade = raw * raw * (3.0 - 2.0 * raw);
        let scale = scale_for_fov(rect.height(), 52f32.to_radians());
        let wind = crate::math::Vec2::new(0.0, 0.0);
        // Collect the full renderable mesh set once: static geometry (from the LOD field if the map has one,
        // else the flat list) plus water surfaces and destructibles, so authored maps that live in the LOD
        // field or use water/props render in the thumbnail exactly as they do in-game — and textured, via the
        // real material atlas rather than the flat untextured fallback the preview used before.
        let mut meshes: Vec<crate::mesh::Mesh3> = Vec::new();
        if let Some(field) = &self.level.lod_field {
            for m in field.render_meshes() {
                meshes.push(m.clone());
            }
        } else {
            meshes.extend(self.level.meshes.iter().cloned());
        }
        for d in &self.level.destructibles {
            meshes.extend(d.mesh());
        }
        let paint_one = |cam: &FpsCam, a: f32| {
            if a <= 0.002 {
                return;
            }
            let mut mesh = crate::render::render(
                &meshes, cam, rect.center().x, rect.center().y, scale, &self.light, &self.fog, rect.width(), rect.height(), time, wind, atlas, 5, &[],
            );
            if a < 0.998 {
                for v in &mut mesh.vertices {
                    let c = v.color;
                    v.color = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * a) as u8);
                }
            }
            // Paint the rendered mesh directly. (Previously used `with_clip_rect(rect).add(...)`, which in this
            // egui version returned a detached painter whose shapes never reached the layer — so the preview
            // showed only the sky gradient. The mesh verts are already inside `rect`, so a direct add on the
            // same painter is correct and visible.)
            painter.add(egui::Shape::mesh(mesh));
        };
        paint_one(&self.vantages[i], 1.0);
        paint_one(&self.vantages[j], fade);
    }
}

pub fn paint_sky_gradient(p: &egui::Painter, rect: Rect, sky: Color32, fog: Color32) {
    let mid = rect.center().y;
    let lerp = |a: Color32, b: Color32, s: f32| {
        let s = s.clamp(0.0, 1.0);
        Color32::from_rgb(
            (a.r() as f32 * (1.0 - s) + b.r() as f32 * s) as u8,
            (a.g() as f32 * (1.0 - s) + b.g() as f32 * s) as u8,
            (a.b() as f32 * (1.0 - s) + b.b() as f32 * s) as u8,
        )
    };
    let sky_top = darker(sky, 0.55);
    let bands = 16;
    let bh = (mid - rect.min.y) / bands as f32;
    for i in 0..bands {
        let y0 = rect.min.y + i as f32 * bh;
        let s = i as f32 / (bands - 1) as f32;
        let strip = Rect::from_min_max(Pos2::new(rect.min.x, y0), Pos2::new(rect.max.x, y0 + bh + 1.0));
        p.rect_filled(strip, 0.0, lerp(sky_top, fog, s * s));
    }
    // below the horizon line: the fog tone, flat (the ground/floor mesh normally covers this, but on maps
    // with no floor directly underfoot at the edges — e.g. looking down past a cliff — this reads as haze
    // rather than a hard black cutoff).
    let ground_strip = Rect::from_min_max(Pos2::new(rect.min.x, mid), Pos2::new(rect.max.x, rect.max.y));
    p.rect_filled(ground_strip, 0.0, darker(fog, 0.8));
}
