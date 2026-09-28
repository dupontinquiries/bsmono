//! mazeshots — the Quarry, looked at.
//!
//! `CLAWBOUND_SHOTS=maze clawbound-battleground --screenshot --map quarry`.
//! `CLAWBOUND_SHEET=top,heart` limits which are taken. The whole labyrinth
//! from above (and again after it has turned), a room at play height, the
//! Heart, gates rising and warning, a section stirring and biting with a
//! cat alone in it; then bots on both sides play for
//! `CLAWBOUND_MAZE_SECS` (default 150) seconds and the fight is shot.

use glam::Vec3;

use crate::headless::{make_device, save_shot};
use crate::maze as mz;
use crate::mazeworld::Item;
use crate::render::{Camera, Renderer};
use crate::scene::Scene;
use crate::sim::{Game, Rng};
use crate::vfx::Vfx;

const W: u32 = 1280;
const H: u32 = 720;

pub fn run() -> Result<(), String> {
    if crate::map::active() != crate::map::MapId::Maze {
        return Err("run with --map quarry (or CLAWBOUND_MAP=quarry)".into());
    }
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    if let Some(q) = crate::quality::Quality::requested() {
        renderer.quality = q;
        renderer.set_shadow_size(q.shadow_size());
    }
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("maze shot"),
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
    let mut rng = Rng::new(0x7A0E);
    let (blue, red) = crate::map::team_picks(0, &mut rng);
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some(crate::sim::AiBrain::new(crate::consts::Lane::Top, 0.9));
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
    // the game's own camera, as app.rs builds it: 8.5 u behind at this
    // map's pitch (`MapId::cam_pitch`), the boom brought in off the walls
    // (`map::camera_boom`), fov 0.95
    let play = |at: Vec3, yaw: f32| {
        let pitch = crate::map::active().cam_pitch();
        let dir = Vec3::new(yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos());
        let target = at + Vec3::Y * 1.5;
        cam(target - dir * crate::map::camera_boom(target, dir, 8.5), target, 0.95)
    };
    shoot(&mut renderer, &game, &mut vfx, "maze_top.png", cam(Vec3::new(0.0, 420.0, -70.0), Vec3::new(0.0, 0.0, -4.0), 1.0))?;
    shoot(&mut renderer, &game, &mut vfx, "maze_overview.png", cam(Vec3::new(-160.0, 150.0, -230.0), Vec3::new(0.0, 0.0, -30.0), 1.0))?;
    // a room at play height: the blue Top lane's first room
    shoot(&mut renderer, &game, &mut vfx, "maze_room_play.png", play(mz::room_pos((0, -2)), 0.4))?;
    // the Heart
    shoot(&mut renderer, &game, &mut vfx, "maze_heart.png", cam(Vec3::new(-18.0, 26.0, -34.0), Vec3::new(0.0, 2.0, 0.0), 1.0))?;
    // the labyrinth turning: a gate half up, and the gates of a later turn
    game.t = mz::FIRST_TURN + mz::GATE_MOVE * 0.5;
    shoot(&mut renderer, &game, &mut vfx, "maze_gates_rising.png", cam(Vec3::new(-40.0, 60.0, -120.0), Vec3::new(-10.0, 0.0, -40.0), 1.0))?;
    game.t = mz::FIRST_TURN + mz::TURN * 2.0 - 1.0;
    shoot(&mut renderer, &game, &mut vfx, "maze_gates_warn.png", cam(Vec3::new(-40.0, 60.0, -120.0), Vec3::new(-10.0, 0.0, -40.0), 1.0))?;
    game.t = mz::FIRST_TURN + mz::TURN * 2.0 + mz::GATE_MOVE + 1.0;
    shoot(&mut renderer, &game, &mut vfx, "maze_top_turned.png", cam(Vec3::new(0.0, 420.0, -70.0), Vec3::new(0.0, 0.0, -4.0), 1.0))?;
    // the machine (`machine`): a stone winding up out of its hopper in the
    // hills, coming down its chute, then rolling a sealed lock with the pads
    // dropping open behind it; the gear trains on the walls from close
    let t2 = mz::FIRST_TURN + mz::TURN; // turn 2
    vfx.machine_turn = 3.0;
    let p2 = crate::machine::pair_for(game.wake.seed, 2).unwrap_or(0);
    let tr = crate::machine::pairs()[p2][0].clone();
    let (h, l) = (tr.hopper(), tr.lip());
    let d = Vec3::new(l.x - h.x, 0.0, l.z - h.z).normalize_or_zero();
    let side = Vec3::new(-d.z, 0.0, d.x);
    game.t = t2 + crate::machine::ROLL_AT - 1.5;
    shoot(&mut renderer, &game, &mut vfx, "maze_machine_winding.png", cam(l + d * 14.0 + side * 12.0 + Vec3::Y * 14.0, (h + l) * 0.5, 1.0))?;
    game.t = t2 + crate::machine::ROLL_AT + 0.9;
    shoot(&mut renderer, &game, &mut vfx, "maze_machine_chute.png", cam((h + l) * 0.5 + side * 30.0 + Vec3::Y * 6.0, (h + l) * 0.5, 1.0))?;
    game.t = t2 + crate::machine::ROLL_AT + tr.floor_from / crate::machine::SPEED + 3.0;
    if let Some(b) = crate::machine::boulders_at(game.wake.seed, game.t).first().copied() {
        let p = Vec3::new(b.pos.x, 0.0, b.pos.z);
        shoot(&mut renderer, &game, &mut vfx, "maze_machine_rolling.png", cam(p - b.dir * 34.0 + side * 18.0 + Vec3::Y * 26.0, p - b.dir * 10.0, 1.0))?;
        shoot(&mut renderer, &game, &mut vfx, "maze_machine_play.png", play(p + b.dir * 4.0 + side * 12.0, (-b.dir.x).atan2(-b.dir.z)))?;
    }
    // a lock armed: its pads' rims lit, before the stone is let go
    game.t = t2 + 3.0;
    let rm = mz::room_pos(tr.rooms[1]);
    shoot(&mut renderer, &game, &mut vfx, "maze_machine_armed.png", cam(rm + Vec3::new(-20.0, 34.0, -30.0), rm, 1.0))?;
    // a Geartooth tablet (`geartooth`), close, from the floor in front of it
    if let Some(Item::Tablet { at, face, .. }) = crate::mazeworld::placements().into_iter().find(|i| matches!(i, Item::Tablet { k: 2, .. })) {
        shoot(&mut renderer, &game, &mut vfx, "maze_tablet.png", cam(at + face * 5.5 + Vec3::Y * 2.2, at + Vec3::Y * 1.3, 0.8))?;
    }
    shoot(&mut renderer, &game, &mut vfx, "maze_gears.png", cam(Vec3::new(-70.0, 55.0, -95.0), Vec3::new(-40.0, 8.0, -55.0), 1.0))?;
    // a section stirring, then biting, the player's cat alone in it
    let class = game.wake.classes(0)[0];
    let spot = mz::rooms()
        .map(mz::room_pos)
        .find(|p| mz::section_at(p.x, p.z).is_some_and(|s| s.class() == class))
        .unwrap_or(Vec3::ZERO)
        + Vec3::new(6.0, 0.0, 4.0);
    for g in game.gods.iter_mut() {
        g.ai = None;
        g.move_dir = Vec3::ZERO;
    }
    game.gods[0].pos = spot;
    let near = cam(spot + Vec3::new(-12.0, 24.0, -18.0), spot + Vec3::Y, 1.0);
    game.t = mz::FIRST_WAKE + 4.0;
    shoot(&mut renderer, &game, &mut vfx, "maze_stirring.png", near)?;
    game.t = mz::FIRST_WAKE + mz::WARN + 6.0;
    shoot(&mut renderer, &game, &mut vfx, "maze_hostile.png", near)?;
    shoot(&mut renderer, &game, &mut vfx, "maze_hostile_play.png", play(spot, 0.6))?;
    println!("woke {} at {spot}", mz::class_name(class));
    // the fight: a fresh match, bots on both sides
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some(crate::sim::AiBrain::new(crate::consts::Lane::Top, 0.9));
    let secs: f32 = std::env::var("CLAWBOUND_MAZE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(150.0);
    let mut bites = 0;
    for _ in 0..(secs * 60.0) as usize {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        bites += ev.iter().filter(|e| matches!(e, crate::sim::Event::ThornBite { .. })).count();
        vfx.update(1.0 / 60.0, &ev, &game);
    }
    let alive: Vec<Vec3> = game.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
    let knot = alive.iter().copied().max_by_key(|p| alive.iter().filter(|q| (**q - *p).length() < 14.0).count()).unwrap_or(Vec3::ZERO);
    shoot(&mut renderer, &game, &mut vfx, "maze_fight.png", play(knot, 0.5))?;
    println!("after {secs:.0} s: towers {:?}, kills {}, thorn bites {bites}", game.tower_alive, game.gods.iter().map(|g| g.kills).sum::<u32>());
    Ok(())
}
