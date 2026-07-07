// Benchmark the renderer across maps: measure per-frame render() time and triangle throughput.
use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::{Vec2, Vec3};
use std::time::Instant;
fn main() {
    let maps = ["Desert Town", "Reference Town", "Redwood Canyon", "Concrete Slab"];
    let (w, h) = (1280.0f32, 720.0f32);
    let scale = scale_for_fov(h, 70f32.to_radians());
    for name in maps {
        let Some(i) = PRESETS.iter().position(|p| p.name == name) else { continue };
        let game = Game::new_preset(7, Some(i), Loadout::default());
        let lvl = &game.level;
        let c = Vec3::new(lvl.w as f32*lvl.ts()*0.5, lvl.floor_y+2.0, lvl.h as f32*lvl.ts()*0.5);
        // spin the camera through 8 yaws, average the render time
        let mut total = 0.0f64; let mut tris = 0usize; let n = 16;
        for k in 0..n {
            let yaw = k as f32 / n as f32 * std::f32::consts::TAU;
            let cam = FpsCam { pos: c, yaw, pitch: -0.1 };
            let meshes = game.frame_meshes(&cam, false);
            let lights = game.dynamic_lights();
            let t0 = Instant::now();
            let m = early_fps::render::render(&meshes, &cam, w*0.5, h*0.5, scale, &game.light, &game.fog, w, h, game.time, Vec2::new(0.0,0.0), egui::TextureId::Managed(1), 5, &lights);
            total += t0.elapsed().as_secs_f64()*1000.0;
            tris += m.indices.len()/3;
        }
        println!("{:<16} render {:.2}ms/frame  out {} tris/frame", name, total/n as f64, tris/n);
    }
}
