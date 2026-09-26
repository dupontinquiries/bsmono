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
    // ultra's snow (CLAWBOUND_QUALITY=ultra; skipped otherwise): a cat
    // bounding through a drift, one padding over blown powder, and a
    // blast's crater -- a few seconds of each, drawn every frame so the
    // paws really land
    if vfx.snow.is_some() && (want("snow_deep") || want("snow_thin") || want("snow_blast")) {
        let keep = (game.gods[0].pos, game.gods[0].yaw, game.gods[0].move_dir);
        let find = |vfx: &Vfx, deep: bool| -> Option<Vec3> {
            let s = vfx.snow.as_ref()?;
            let mut best = None;
            for k in 0..4000 {
                let a = k as f32 * 2.399;
                let r = 30.0 + (k as f32).sqrt() * 4.0;
                let p = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                let ok = (0..8).all(|q| {
                    let o = p + Vec3::X * q as f32;
                    let d = s.depth_at(o.x, o.z);
                    sky::floor().sdf(o.x, o.z) < -3.0 && if deep { d > 0.75 } else { (0.04..0.2).contains(&d) && s.blow_at(o.x, o.z) > 0.6 }
                });
                if ok {
                    best = Some(p);
                    break;
                }
            }
            best
        };
        let mut walk = |renderer: &mut Renderer, game: &mut Game, vfx: &mut Vfx, from: Vec3, secs: f32, events: Vec<crate::sim::Event>| {
            game.gods[0].yaw = 0.0;
            game.gods[0].move_dir = Vec3::X;
            let n = (secs * 60.0) as usize;
            for k in 0..n {
                game.gods[0].pos = from + Vec3::X * (k as f32 / 60.0 * 4.5);
                game.t += 1.0 / 60.0;
                let ev = if k == n / 2 { events.clone() } else { Vec::new() };
                vfx.update(1.0 / 60.0, &ev, game);
                renderer.clear_queues();
                scene.draw(renderer, game, vfx, cam(Vec3::ZERO + Vec3::new(0.0, 30.0, 0.0), Vec3::ZERO, 1.0));
            }
        };
        if let Some(p) = find(&vfx, true) {
            walk(&mut renderer, &mut game, &mut vfx, p, 3.0, Vec::new());
            let at = game.gods[0].pos;
            shoot(&mut renderer, &game, &mut vfx, "sky_snow_deep.png", cam(at + Vec3::new(-5.5, 3.2, 4.0), at + Vec3::new(-2.5, 0.2, 0.0), 0.9))?;
        }
        if let Some(p) = find(&vfx, false) {
            walk(&mut renderer, &mut game, &mut vfx, p, 3.0, Vec::new());
            let at = game.gods[0].pos;
            shoot(&mut renderer, &game, &mut vfx, "sky_snow_thin.png", cam(at + Vec3::new(-5.5, 3.2, 4.0), at + Vec3::new(-2.5, 0.2, 0.0), 0.9))?;
        }
        if let Some(p) = find(&vfx, true) {
            let blast = p + Vec3::new(5.0, 0.0, 0.0);
            let ev = vec![crate::sim::Event::Explosion { pos: blast, radius: 4.0, color: [0.8, 0.8, 0.9] }];
            walk(&mut renderer, &mut game, &mut vfx, p, 1.4, ev);
            shoot(&mut renderer, &game, &mut vfx, "sky_snow_blast.png", cam(blast + Vec3::new(-3.0, 15.0, 9.0), blast, 1.0))?;
        }
        game.gods[0].pos = keep.0;
        game.gods[0].yaw = keep.1;
        game.gods[0].move_dir = keep.2;
    }
    // a nook in a cliff face, from out in the air before it
    if let Some(crate::skyworld::Item::Nook { at, facing, .. }) = crate::skyworld::placements().into_iter().find(|i| matches!(i, crate::skyworld::Item::Nook { kind: 0, .. })) {
        shoot(&mut renderer, &game, &mut vfx, "sky_nook.png", cam(at + facing * 8.0 + Vec3::Y * 2.5 + Vec3::new(facing.z, 0.0, -facing.x) * 3.0, at + Vec3::Y * 1.0, 0.9))?;
    }
    // the blue clan's camp behind its base (clancamp.rs)
    {
        let f = crate::consts::Team::Blue.fountain();
        shoot(&mut renderer, &game, &mut vfx, "sky_camp.png", cam(f + Vec3::new(26.0, 20.0, -52.0), f + Vec3::new(0.0, 1.0, -16.0), 1.0))?;
    }
    // the Elder on its ground, the push lane marked out before it
    {
        let h = crate::consts::Team::Blue.titan_pos();
        shoot(&mut renderer, &game, &mut vfx, "sky_elder.png", cam(h + Vec3::new(-24.0, 16.0, 40.0), h + Vec3::new(0.0, 3.0, 8.0), 1.0))?;
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
    // the clouds that hide cats (skycloud.rs): run on until a bank is well
    // over the islands and a wisp is thick, and look at each
    if want("cloudbank") || want("wisp") {
        let over_floor = |c: &crate::skycloud::Cloud| sky::floor().sdf(c.pos.x, c.pos.z) < 0.0 && c.thick > 0.8;
        let mut k = 0;
        while k < 60 * 240 && !crate::skycloud::clouds(game.cloud_seed, game.t).iter().any(|c| c.kind == crate::skycloud::Kind::Bank && over_floor(c)) {
            game.tick(1.0 / 60.0);
            let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
            vfx.update(1.0 / 60.0, &ev, &game);
            k += 1;
        }
        let cs = crate::skycloud::clouds(game.cloud_seed, game.t);
        if let Some(b) = cs.iter().find(|c| c.kind == crate::skycloud::Kind::Bank && over_floor(c)).copied() {
            shoot(&mut renderer, &game, &mut vfx, "sky_cloudbank.png", cam(b.pos + Vec3::new(0.0, 70.0, 95.0), b.pos, 1.05))?;
            // three dashes through it, a moment apart: the cloud parts in
            // their wake (vfx::Gust)
            for (k, z) in [-10.0f32, 0.0, 10.0].into_iter().enumerate() {
                let ev = vec![crate::sim::Event::DashFx { pos: b.pos + Vec3::new(8.0, 0.0, z), dir: Vec3::X, color: [0.9, 0.9, 1.0] }];
                vfx.update(1.0 / 60.0, &ev, &game);
                for _ in 0..(8 + k * 4) {
                    vfx.update(1.0 / 60.0, &[], &game);
                }
            }
            shoot(&mut renderer, &game, &mut vfx, "sky_cloudpart.png", cam(b.pos + Vec3::new(0.0, 45.0, 60.0), b.pos, 1.05))?;
        }
        if let Some(w) = cs.iter().filter(|c| c.kind == crate::skycloud::Kind::Wisp && over_floor(c)).max_by(|a, b| a.thick.total_cmp(&b.thick)) {
            let side = Vec3::new(-w.dir.z, 0.0, w.dir.x).normalize_or_zero();
            shoot(&mut renderer, &game, &mut vfx, "sky_wisp.png", cam(w.pos + side * 30.0 + Vec3::Y * 9.0, w.pos + Vec3::Y * 2.0, 1.1))?;
        }
        println!("clouds at {:.0} s: {} banks, {} wisps", game.t, cs.iter().filter(|c| c.kind == crate::skycloud::Kind::Bank).count(), cs.iter().filter(|c| c.kind == crate::skycloud::Kind::Wisp).count());
    }
    Ok(())
}
