//! skyshots — Starwell Glacier, looked at.
//!
//! `CLAWBOUND_SHOTS=sky clawbound-battleground --screenshot --map starwell`.
//! `CLAWBOUND_SHEET=overview,well` limits which are taken. A real match:
//! bots on both sides against the scene's own blockers; the fight shot is
//! taken after `CLAWBOUND_SKY_SECS` (default 80) seconds of it.

use glam::Vec3;

use crate::headless::{make_device, save_shot};
use crate::render::{Camera, Renderer};
use crate::scene::Scene;
use crate::sim::{Game, Rng};
use crate::sky;
use crate::vfx::Vfx;

const W: u32 = 1280;
const H: u32 = 720;

pub fn run() -> Result<(), String> {
    if crate::map::active() != crate::map::MapId::Sky {
        return Err("run with --map starwell (or CLAWBOUND_MAP=starwell)".into());
    }
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky shot"),
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
    let mut rng = Rng::new(0x5C1E5);
    // the player's cat is Cometail, the newest ancestor
    let (blue, red) = crate::map::team_picks(crate::gods::ROSTER_LEN as u32 - 1, &mut rng);
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some(crate::sim::AiBrain::new(crate::consts::Lane::Top, 0.9));
    game.open_playbook(&mut Rng::new(0x5C1E6));
    // wake the Moonwyrm for the shots (it rises at 4:00 in a match)
    for b in game.bosses.list.iter_mut() {
        if b.kind == crate::bosses::BossKind::Moonwyrm {
            b.spawn_t = 0.0;
        }
    }
    game.tick(1.0 / 60.0);
    game.events.clear();
    let mut vfx = Vfx::default();
    let only: Vec<String> = std::env::var("CLAWBOUND_SHEET").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let want = |n: &str| only.is_empty() || only.iter().any(|o| n.contains(o.as_str()));
    let mut shoot = |renderer: &mut Renderer, game: &Game, vfx: &mut Vfx, name: &str, cam: Camera| -> Result<(), String> {
        if !want(name) {
            return Ok(());
        }
        renderer.clear_queues();
        vfx.update(1.0 / 60.0, &[], game);
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
        Ok(())
    };
    let cam = |eye: Vec3, target: Vec3, fov_y: f32| Camera { eye, target, fov_y };
    shoot(&mut renderer, &game, &mut vfx, "sky_overview.png", cam(Vec3::new(0.0, 330.0, -330.0), Vec3::new(0.0, 0.0, -40.0), 1.1))?;
    shoot(&mut renderer, &game, &mut vfx, "sky_centre.png", cam(Vec3::new(0.0, 6.0, -14.0), Vec3::new(0.0, 2.0, 0.0), 1.0))?;
    shoot(&mut renderer, &game, &mut vfx, "sky_well.png", cam(Vec3::new(-10.0, 22.0, -62.0), Vec3::new(0.0, 2.0, 0.0), 1.0))?;
    // down a lane from beside the outer tower
    let top = crate::consts::Lane::Top.polyline();
    let len: f32 = top.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let at = |f: f32| crate::pass::along(&top, f * len);
    shoot(&mut renderer, &game, &mut vfx, "sky_lane.png", cam(at(0.22) + Vec3::Y * 9.0, at(0.36) + Vec3::Y * 2.0, 1.0))?;
    // a chasm, from a glade's pier, looking across to the well
    let (a, b) = sky::ferry_routes()[0];
    shoot(&mut renderer, &game, &mut vfx, "sky_chasm.png", cam(a + (a - b).normalize() * 8.0 + Vec3::Y * 6.0, b + Vec3::Y * 1.0, 1.0))?;
    // the star-bridge, from the lane end
    let (s0, s1) = sky::BRIDGES[0];
    let (p0, p1) = (Vec3::new(s0.0, 0.0, s0.1), Vec3::new(s1.0, 0.0, s1.1));
    shoot(&mut renderer, &game, &mut vfx, "sky_bridge.png", cam(p0 + Vec3::new(-6.0, 7.0, 3.0), p0.lerp(p1, 0.6) + Vec3::Y * 2.0, 1.0))?;
    // from the edge of the world, looking out and down at the cloud sea
    shoot(&mut renderer, &game, &mut vfx, "sky_edge.png", cam(Vec3::new(-245.0, 5.0, -135.0), Vec3::new(-320.0, -30.0, -110.0), 1.05))?;
    // the game's own camera (app.rs: 8.5 u behind, pitched 0.38, fov 0.95)
    // at an island's edge, looking out: the horizon and the drop as played
    let play = |at: Vec3, yaw: f32| {
        let dir = Vec3::new(yaw.sin() * 0.38f32.cos(), -0.38f32.sin(), yaw.cos() * 0.38f32.cos());
        let target = at + Vec3::Y * 1.5;
        cam(target - dir * 8.5, target, 0.95)
    };
    {
        let glade = Vec3::new(-104.0, 0.0, -96.0);
        let out = (Vec3::new(0.0, 0.0, -150.0) - glade).normalize();
        let at = glade + out * 23.0;
        shoot(&mut renderer, &game, &mut vfx, "sky_play_edge.png", play(at, out.x.atan2(out.z)))?;
        // looking straight out past the map's rim, over nothing
        let out = Vec3::new(-0.8, 0.0, -0.6).normalize();
        let mut r = 0.0;
        for k in 0..400 {
            let p = out * k as f32;
            if sky::floor().sdf(p.x, p.z) < -3.0 {
                r = k as f32;
            }
        }
        shoot(&mut renderer, &game, &mut vfx, "sky_play_rim.png", play(out * r, out.x.atan2(out.z)))?;
    }
    // mist curling over the stone and ice, and spilling off an edge: the
    // player's cat at a glade's rim, four seconds of wisps gathered round it
    {
        let keep = game.gods[0].pos;
        let glade = Vec3::new(-104.0, 0.0, -96.0);
        let out = (Vec3::new(0.0, 0.0, -150.0) - glade).normalize();
        let at = glade + out * 22.0;
        game.gods[0].pos = at;
        for _ in 0..240 {
            vfx.update(1.0 / 60.0, &[], &game);
        }
        let side = Vec3::new(-out.z, 0.0, out.x);
        shoot(&mut renderer, &game, &mut vfx, "sky_mist.png", cam(at - out * 6.0 + side * 5.0 + Vec3::Y * 3.5, at + out * 4.0 - Vec3::Y * 1.0, 1.0))?;
        game.gods[0].pos = keep;
    }
    // a glade with its larches
    shoot(&mut renderer, &game, &mut vfx, "sky_glade.png", cam(Vec3::new(-80.0, 8.0, -120.0), Vec3::new(-104.0, 3.0, -96.0), 1.0))?;
    // Cometail, close, on the well island's rim
    {
        let keep = game.gods[0].pos;
        let at = Vec3::new(-6.0, 0.0, -36.0);
        game.gods[0].pos = at;
        game.gods[0].yaw = -2.4;
        shoot(&mut renderer, &game, &mut vfx, "sky_cometail.png", cam(at + Vec3::new(-3.0, 2.2, 4.5), at + Vec3::Y * 0.9, 0.8))?;
        game.gods[0].pos = keep;
    }
    // the Moonwyrm in its well, from the rim, its stars about it
    shoot(&mut renderer, &game, &mut vfx, "sky_wyrm.png", cam(Vec3::new(10.0, 9.0, 40.0), Vec3::new(0.0, 10.0, 0.0), 0.95))?;
    // ...and all of it: the coil climbing ninety units over the well
    shoot(&mut renderer, &game, &mut vfx, "sky_wyrmtower.png", cam(Vec3::new(70.0, 22.0, 120.0), Vec3::new(0.0, 42.0, 0.0), 1.1))?;
    // ...and throwing them: a cat on the rim, a star or two in flight
    {
        let keep = (game.gods[0].pos, game.t, game.gods[0].hp);
        let rim = Vec3::new(20.0, 0.0, 18.0);
        game.gods[0].pos = rim;
        let mut k = 0;
        while (game.star_bolts.is_empty() || game.star_bolts.iter().all(|s| s.pos.distance(rim) < 6.0)) && k < 600 {
            game.gods[0].pos = rim;
            game.tick(1.0 / 60.0);
            let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
            vfx.update(1.0 / 60.0, &ev, &game);
            k += 1;
        }
        for _ in 0..12 {
            game.gods[0].pos = rim;
            game.tick(1.0 / 60.0);
            let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
            vfx.update(1.0 / 60.0, &ev, &game);
        }
        shoot(&mut renderer, &game, &mut vfx, "sky_starthrow.png", cam(rim + Vec3::new(12.0, 6.0, 16.0), Vec3::new(4.0, 7.0, 4.0), 1.0))?;
        game.gods[0].pos = keep.0;
        game.gods[0].hp = keep.2;
        game.star_bolts.clear();
    }
    // the Moonwyrm aloft (wyrmflight.rs): send it off a third of the way
    // round its loop, let it fly a few seconds dropping stars, and look up
    // at it; then wait for a star to land and look at the burst
    {
        let w = game.bosses.list.iter().position(|b| b.kind == crate::bosses::BossKind::Moonwyrm).ok_or("no wyrm")?;
        let keep: Vec<f32> = game.gods.iter().map(|g| g.hp).collect();
        let secs = crate::wyrmflight::FLIGHT_SECS * 0.3;
        game.bosses.list[w].flight = Some(secs);
        game.bosses.list[w].drop_t = 0.0;
        let step = |game: &mut Game, vfx: &mut Vfx| -> Vec<crate::sim::Event> {
            game.tick(1.0 / 60.0);
            let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
            vfx.update(1.0 / 60.0, &ev, game);
            ev
        };
        for _ in 0..(5 * 60) {
            step(&mut game, &mut vfx);
        }
        let b = &game.bosses.list[w];
        let (p, dir, _) = crate::wyrmflight::pose(b.home, b.flight.unwrap_or(secs));
        let side = Vec3::new(-dir.z, 0.0, dir.x).normalize_or_zero();
        let ground = Vec3::new(p.x, 0.0, p.z);
        shoot(&mut renderer, &game, &mut vfx, "sky_wyrmflight.png", cam(ground + side * 120.0 - dir * 60.0 + Vec3::Y * 25.0, p - dir * 60.0, 1.15))?;
        let mut hit = None;
        for _ in 0..(6 * 60) {
            let ev = step(&mut game, &mut vfx);
            if let Some(at) = ev.iter().find_map(|e| if let crate::sim::Event::StarfallFx { at, .. } = e { Some(*at) } else { None }) {
                hit = Some(at);
                break;
            }
        }
        if let Some(at) = hit {
            for _ in 0..5 {
                step(&mut game, &mut vfx);
            }
            let wp = game.bosses.list[w].pos;
            let away = Vec3::new(at.x - wp.x, 0.0, at.z - wp.z).normalize_or(Vec3::X);
            shoot(&mut renderer, &game, &mut vfx, "sky_starfall.png", cam(at + away * 28.0 + Vec3::Y * 10.0, at + Vec3::Y * 6.0, 1.1))?;
        }
        let b = &mut game.bosses.list[w];
        b.flight = None;
        b.pos = b.home;
        game.starfalls.clear();
        for (g, hp) in game.gods.iter_mut().zip(keep) {
            g.hp = hp.max(g.hp);
        }
    }
    // a ferry mid-crossing with a cat aboard: run the clock until the first
    // cloud is halfway over, and put the player's cat on it
    {
        let f = game.ferries[0];
        let mut t = game.t;
        while f.at(t).docked.is_some() || (f.at(t).pos - f.a.lerp(f.b, 0.5)).length() > 3.0 {
            t += 0.05;
        }
        let keep = (game.t, game.gods[0].pos);
        game.t = t;
        let deck = f.at(t).pos;
        game.gods[0].pos = deck + Vec3::new(1.0, 0.0, 0.5);
        game.gods[0].riding = Some(0);
        let across = (f.b - f.a).normalize();
        let side = Vec3::new(-across.z, 0.0, across.x);
        shoot(&mut renderer, &game, &mut vfx, "sky_ferry.png", cam(deck + side * 16.0 + Vec3::Y * 6.0 - across * 6.0, deck + Vec3::Y * 0.5, 1.0))?;
        game.t = keep.0;
        game.gods[0].pos = keep.1;
        game.gods[0].riding = None;
    }
    // the fight
    let steps: usize = std::env::var("CLAWBOUND_SKY_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(80) * 60;
    let t0 = std::time::Instant::now();
    let mut peak = 0usize;
    for _ in 0..steps {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, &game);
        peak = peak.max(game.minions.len());
    }
    println!(
        "sim: {} s of 15v15 in {:.2} s wall ({:.2} ms/tick), {} minions now, {} at peak",
        steps / 60,
        t0.elapsed().as_secs_f32(),
        t0.elapsed().as_secs_f32() * 1000.0 / steps as f32,
        game.minions.len(),
        peak
    );
    // where the most minions are: the waves meeting
    let ms: Vec<Vec3> = game.minions.iter().map(|m| m.pos).collect();
    let clash = ms.iter().copied().max_by_key(|p| ms.iter().filter(|q| (**q - *p).length() < 12.0).count()).unwrap_or(Vec3::ZERO);
    let out = Vec3::new(clash.x, 0.0, clash.z).normalize_or(Vec3::Z);
    shoot(&mut renderer, &game, &mut vfx, "sky_clash.png", cam(clash - out * 34.0 + Vec3::Y * 30.0, clash, 1.0))?;
    Ok(())
}
