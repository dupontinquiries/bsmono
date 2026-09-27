//! atlantisshots — the Drowned Halls, looked at.
//!
//! `CLAWBOUND_SHOTS=atlantis clawbound-battleground --screenshot --map atlantis`.
//! `CLAWBOUND_SHEET=hall,library` limits which are taken. A real 3v3: bots
//! on both sides against the scene's own blockers; the fight shots are taken
//! after `CLAWBOUND_ATL_SECS` (default 70) seconds of it.

use glam::Vec3;

use crate::atlantis as at;
use crate::headless::{make_device, save_shot};
use crate::render::{Camera, Renderer};
use crate::scene::Scene;
use crate::sim::{Game, Rng};
use crate::vfx::Vfx;

const W: u32 = 1280;
const H: u32 = 720;

pub fn run() -> Result<(), String> {
    if crate::map::active() != crate::map::MapId::Atlantis {
        return Err("run with --map atlantis (or CLAWBOUND_MAP=atlantis)".into());
    }
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    if let Some(q) = crate::quality::Quality::requested() {
        renderer.quality = q;
        renderer.set_shadow_size(q.shadow_size());
    }
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlantis shot"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let t0 = std::time::Instant::now();
    let scene = Scene::new(&mut renderer);
    println!("scene built in {:.1} s, {} voxel instances", t0.elapsed().as_secs_f32(), scene.total_voxels);
    let mut rng = Rng::new(0xA71A);
    let (blue, red) = crate::map::team_picks(0, &mut rng);
    assert_eq!((blue.len(), red.len()), (3, 3), "the Drowned Halls are 3 a side");
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some(crate::sim::AiBrain::new(crate::consts::Lane::Mid, 0.9));
    game.tick(1.0 / 60.0);
    game.events.clear();
    let mut vfx = Vfx::default();
    let only: Vec<String> = std::env::var("CLAWBOUND_SHEET").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let want = |n: &str| only.is_empty() || only.iter().any(|o| n.contains(o.as_str()));
    let shoot = |renderer: &mut Renderer, game: &Game, vfx: &mut Vfx, name: &str, cam: Camera| -> Result<(), String> {
        if !want(name) {
            return Ok(());
        }
        renderer.clear_queues();
        vfx.update(1.0 / 60.0, &[], game);
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t.max(4.0));
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
        Ok(())
    };
    let cam = |eye: Vec3, target: Vec3, fov_y: f32| Camera { eye, target, fov_y };
    // the game's own camera (app.rs: 8.5 u behind, pitched 0.38, fov 0.95)
    let play = |at: Vec3, yaw: f32| {
        let dir = Vec3::new(yaw.sin() * 0.38f32.cos(), -0.38f32.sin(), yaw.cos() * 0.38f32.cos());
        let target = at + Vec3::Y * 1.5;
        cam(target - dir * 8.5, target, 0.95)
    };
    shoot(&mut renderer, &game, &mut vfx, "atl_overview.png", cam(Vec3::new(-150.0, 170.0, -150.0), Vec3::new(20.0, 0.0, -10.0), 1.0))?;
    // down the lane hall from the blue base's door, at play height
    shoot(&mut renderer, &game, &mut vfx, "atl_hall_play.png", play(Vec3::new(0.0, 0.0, -64.0), 0.0))?;
    shoot(&mut renderer, &game, &mut vfx, "atl_hall_low.png", cam(Vec3::new(-3.0, 3.0, -50.0), Vec3::new(-6.0, 4.0, -20.0), 1.05))?;
    // up through the vault: the ribs, the glass, the sea overhead
    shoot(&mut renderer, &game, &mut vfx, "atl_hall_up.png", cam(Vec3::new(-4.0, 2.0, -40.0), Vec3::new(-6.0, 14.0, -22.0), 1.1))?;
    // the Drowned Library
    shoot(&mut renderer, &game, &mut vfx, "atl_library.png", cam(Vec3::new(at::LIBRARY_X - 22.0, 9.0, -14.0), Vec3::new(at::LIBRARY_X + 6.0, 1.0, 4.0), 1.05))?;
    shoot(&mut renderer, &game, &mut vfx, "atl_library_play.png", play(Vec3::new(at::LIBRARY_X - 14.0, 0.0, -10.0), 1.2))?;
    // the base under its dome
    shoot(&mut renderer, &game, &mut vfx, "atl_base.png", cam(Vec3::new(-14.0, 8.0, -76.0), Vec3::new(0.0, 2.0, -98.0), 1.05))?;
    // over the wall to the sea floor: kelp and coral
    shoot(&mut renderer, &game, &mut vfx, "atl_seafloor.png", cam(Vec3::new(-20.0, 14.0, -30.0), Vec3::new(-50.0, -6.0, -20.0), 1.0))?;
    // the fight: let the bots play
    let secs: f32 = std::env::var("CLAWBOUND_ATL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(70.0);
    for _ in 0..(secs * 60.0) as usize {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, &game);
    }
    let alive: Vec<Vec3> = game.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
    let knot = alive.iter().copied().max_by_key(|p| alive.iter().filter(|q| (**q - *p).length() < 14.0).count()).unwrap_or(Vec3::ZERO);
    shoot(&mut renderer, &game, &mut vfx, "atl_fight.png", cam(knot + Vec3::new(-10.0, 9.0, -12.0), knot + Vec3::Y, 1.0))?;
    let p = game.gods[0].pos;
    shoot(&mut renderer, &game, &mut vfx, "atl_fight_play.png", play(p, game.gods[0].yaw.cos().atan2(game.gods[0].yaw.sin())))?;
    println!("after {secs:.0} s: towers {:?}, kills {}", game.tower_alive, game.gods.iter().map(|g| g.kills).sum::<u32>());
    Ok(())
}
