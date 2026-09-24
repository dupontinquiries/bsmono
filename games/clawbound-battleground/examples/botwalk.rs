// How well bots MOVE in the real world: an AI-only match against the scene's
// own blockers and going (simdbg runs with none, so it cannot see any of
// this). `cargo run --example botwalk [seed] [minutes]`.
//
// Per bot, per one-second window in which it WANTED to move: how far it got
// against how far its own speed would carry it on open ground. A window under
// 25% is a stall -- pinned on a trunk, a corner, a wall. Also: seconds spent
// on rough ground, and the mean going under the bots' feet.
use clawbound::going::Carriage;
use clawbound::sim::*;
use glam::Vec3;

fn main() {
    let seed: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let minutes: f32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6.0);
    // CLAWBOUND_MAP=pass for Twinfang Pass (15 a side).
    let map = std::env::var("CLAWBOUND_MAP").ok().and_then(|k| clawbound::map::MapId::from_key(&k)).unwrap_or_default();
    clawbound::map::set_active(map);
    eprintln!("building the world's blockers ({})...", map.name());
    let blockers = std::sync::Arc::new(clawbound::scene::world_blockers());
    let mut game = if map == clawbound::map::MapId::Jungle {
        Game::new((seed % 13) as u32, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    } else {
        let (b, r) = clawbound::map::team_picks((seed % 13) as u32, &mut Rng::new(seed));
        Game::new_teams(&b, &r)
    };
    game.rng = Rng::new(0xC0FFEE ^ seed.wrapping_mul(7919));
    game.blockers = blockers.clone();
    game.gods[0].ai = Some(AiBrain { lane: clawbound::consts::Lane::Mid, aggression: 0.9, state: AiState::Push, target: None, strafe_dir: 1.0, strafe_t: 0.0, buy_t: 1.0, wp: 0 });
    let n = game.gods.len();
    let (mut windows, mut stalls) = (0usize, 0usize);
    let mut stall_at: Vec<Vec3> = Vec::new();
    let mut rough_secs = 0.0f32;
    let mut going_sum = 0.0f64;
    let mut going_n = 0usize;
    let mut start: Vec<Vec3> = game.gods.iter().map(|g| g.pos).collect();
    let mut wanted = vec![0.0f32; n];
    let mut expect = vec![0.0f32; n];
    let dt = 1.0 / 60.0;
    for step in 0..((minutes * 60.0 * 60.0) as usize) {
        for i in 0..n {
            let g = &game.gods[i];
            if g.respawn_t <= 0.0 && g.move_dir.length_squared() > 0.01 && g.leap.is_none() && g.dash.is_none() {
                wanted[i] += dt;
                expect[i] += clawbound::gods::god(g.def_id).move_speed * dt;
                let go = blockers.going_at(g.pos.x, g.pos.z, Carriage { stalking: game.stalking(i) });
                going_sum += go as f64;
                going_n += 1;
                if go < 0.99 {
                    rough_secs += dt;
                }
            }
        }
        game.tick(dt);
        game.events.clear();
        if step % 60 == 59 {
            for i in 0..n {
                let g = &game.gods[i];
                if wanted[i] > 0.9 && g.respawn_t <= 0.0 {
                    windows += 1;
                    let got = Vec3::new(g.pos.x - start[i].x, 0.0, g.pos.z - start[i].z).length();
                    if got < 0.25 * expect[i] {
                        stalls += 1;
                        stall_at.push(g.pos);
                    }
                }
                start[i] = g.pos;
                wanted[i] = 0.0;
                expect[i] = 0.0;
            }
        }
        if game.over.is_some() {
            break;
        }
    }
    // Living cats only: a dead cat's position is where it fell, and is
    // not moved again until it respawns at its fountain.
    for (i, g) in game.gods.iter().enumerate() {
        if !clawbound::map::open_ground(g.pos.x, g.pos.z, 0.0) {
            println!("  off the floor: god {i} at ({:.1},{:.1}) respawn_t {:.1} leap {} dash {} fall {}", g.pos.x, g.pos.z, g.respawn_t, g.leap.is_some(), g.dash.is_some(), g.fall.is_some());
        }
    }
    let off_floor = game.gods.iter().filter(|g| g.respawn_t <= 0.0 && !clawbound::map::open_ground(g.pos.x, g.pos.z, 0.0)).count();
    println!("{} cats, {} living ended off the floor", game.gods.len(), off_floor);
    println!(
        "seed {seed}: {windows} moving bot-seconds, {stalls} stalled ({:.1}%), {:.0} s on rough ground, mean going underfoot {:.3}",
        100.0 * stalls as f32 / windows.max(1) as f32,
        rough_secs,
        going_sum / going_n.max(1) as f64
    );
    // Where the stalls cluster, to the nearest 10 u: the places to look at.
    let mut cells: std::collections::HashMap<(i32, i32), usize> = Default::default();
    for p in &stall_at {
        *cells.entry(((p.x / 10.0).round() as i32 * 10, (p.z / 10.0).round() as i32 * 10)).or_default() += 1;
    }
    let mut v: Vec<_> = cells.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for ((x, z), c) in v.iter().take(6) {
        println!("  stalls near ({x:>4}, {z:>4}): {c}");
    }
}
