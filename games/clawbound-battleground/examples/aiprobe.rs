// Bot-behaviour probe: the three complaints about the old fight brain,
// counted over AI-only matches.
//
//   cargo run --release --example aiprobe -- [matches] [minutes]
//
//   stalemate s  -- seconds a bot spends in a fight with an enemy god close
//                   by while NEITHER has taken damage for 3 s (circling)
//   tower deaths -- gods that died inside an enemy tower's range with no
//                   minion of theirs under it (going too deep)
//   retreats     -- trips home started, and the mean health they started at
//                   (fleeing)
//   kills        -- total, for scale
//
// Uses only API the pre-tactics brain also had (bar the one AiBrain
// constructor line), so the same file measures both.
use clawbound::consts::{Lane, Team, TOWER_RANGE};
use clawbound::sim::*;

fn main() {
    // CLAWBOUND_MAP=starwell (or pass): the probe on another map, with its
    // floor as collision (what keeps cats off the sky map's void)
    if let Some(m) = std::env::var("CLAWBOUND_MAP").ok().as_deref().and_then(clawbound::map::MapId::from_key) {
        clawbound::map::set_active(m);
    }
    let args: Vec<f32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let matches = args.first().copied().unwrap_or(6.0) as u64;
    let max_t = args.get(1).copied().unwrap_or(8.0) * 60.0;
    let (mut stale, mut tower_deaths, mut deaths, mut retreats, mut retreat_hp, mut kills, mut wins) = (0.0f32, 0u32, 0u32, 0u32, 0.0f32, 0u32, [0u32; 3]);
    let t0 = std::time::Instant::now();
    let mut why: std::collections::BTreeMap<String, u32> = Default::default();
    for m in 0..matches {
        let (blue, red) = if clawbound::map::active() == clawbound::map::MapId::Jungle {
            rosters(m * 17 + 3)
        } else {
            clawbound::map::team_picks((m % 13) as u32, &mut Rng::new(m * 17 + 3))
        };
        let mut game = Game::new_teams(&blue, &red);
        if let Some(f) = clawbound::map::floor() {
            let mut b = clawbound::blockers::Blockers::new(clawbound::consts::arena_radius() + 20.0, 6.0);
            b.set_floor(f);
            game.blockers = std::sync::Arc::new(b);
        }
        game.rng = Rng::new(0xB0B ^ m.wrapping_mul(7919));
        game.gods[0].is_player = false;
        game.gods[0].ai = Some(AiBrain::new(Lane::Mid, 0.7));
        game.auto_buy(0);
        let n = game.gods.len();
        let mut was_dead = vec![false; n];
        let mut was_state = vec![AiState::Push; n];
        let mut last_target: Vec<Option<TargetRef>> = vec![None; n];
        let dt = 1.0 / 60.0;
        let mut first_tower = None;
        let towers0 = game.tower_alive[0] + game.tower_alive[1];
        while game.over.is_none() && game.t < max_t {
            game.tick(dt);
            game.events.clear();
            let t = game.t;
            if first_tower.is_none() && game.tower_alive[0] + game.tower_alive[1] < towers0 {
                first_tower = Some(t);
            }
            for i in 0..n {
                let g = &game.gods[i];
                let dead = g.respawn_t > 0.0;
                if dead && !was_dead[i] {
                    deaths += 1;
                    let enemy_tower = game.towers.iter().find(|tw| tw.team != g.team && tw.hp > 0.0 && tw.pos.distance(g.pos) < TOWER_RANGE);
                    if let Some(tw) = enemy_tower {
                        let covered = game.minions.iter().any(|mn| mn.team == g.team && mn.hp > 0.0 && mn.pos.distance(tw.pos) < TOWER_RANGE * 0.8);
                        if !covered {
                            tower_deaths += 1;
                            let kind = match last_target[i] {
                                Some(TargetRef::God(_)) => "god",
                                Some(TargetRef::Tower(_)) => "tower",
                                Some(TargetRef::Titan(_)) => "titan",
                                Some(TargetRef::Minion(_)) => "minion",
                                Some(TargetRef::Boss(_)) => "boss",
                                None => "none",
                            };
                            *why.entry(format!("{:?}/{kind}", was_state[i])).or_insert(0u32) += 1;
                        }
                    }
                }
                was_dead[i] = dead;
                let Some(b) = &g.ai else { continue };
                if b.state == AiState::Retreat && was_state[i] != AiState::Retreat {
                    retreats += 1;
                    retreat_hp += g.hp / g.max_hp;
                }
                was_state[i] = b.state;
                last_target[i] = b.target;
                if dead || b.state != AiState::Fight {
                    continue;
                }
                if let Some(TargetRef::God(j)) = b.target {
                    let e = &game.gods[j];
                    if e.pos.distance(g.pos) < 14.0 && t - g.last_dmg_t > 3.0 && t - e.last_dmg_t > 3.0 {
                        stale += dt;
                    }
                }
            }
        }
        kills += game.gods.iter().map(|g| g.kills).sum::<u32>();
        wins[match game.over {
            Some(Team::Blue) => 0,
            Some(Team::Red) => 1,
            None => 2,
        }] += 1;
        eprintln!(
            "  match {m}: {:.0}s over={:?} towers={:?} first tower at {:?} minions {} boss kills {:?} titans {:?}",
            game.t,
            game.over,
            game.tower_alive,
            first_tower.map(|t| t as i32),
            game.minions.len(),
            game.bosses.list.iter().map(|b| (b.kind, b.kills)).collect::<Vec<_>>(),
            game.titans.iter().map(|t| t.hp as i32).collect::<Vec<_>>()
        );
    }
    let per = |x: f32| x / matches as f32;
    println!(
        "{matches} matches x {:.0} min: stalemate {:.0} s/match  tower deaths {:.1}/match of {:.1} deaths  retreats {:.1}/match at {:.0}% hp  kills {:.1}/match  blue/red/capped {:?}  ({:.0}s)",
        max_t / 60.0,
        per(stale),
        per(tower_deaths as f32),
        per(deaths as f32),
        per(retreats as f32),
        if retreats > 0 { retreat_hp / retreats as f32 * 100.0 } else { 0.0 },
        per(kills as f32),
        wins,
        t0.elapsed().as_secs_f32()
    );
    println!("  tower deaths by (state/target) just before: {why:?}");
}

/// Two rosters of five, no repeats (a copy of arena::rosters, which the
/// old brain's tree does not have).
fn rosters(seed: u64) -> (Vec<u32>, Vec<u32>) {
    let mut rng = Rng::new(seed ^ 0x2057E2);
    let mut pool: Vec<u32> = (0..clawbound::gods::ROSTER_LEN as u32).collect();
    let mut side = |rng: &mut Rng| -> Vec<u32> { (0..5).map(|_| pool.remove((rng.next_u() % pool.len() as u64) as usize)).collect() };
    let a = side(&mut rng);
    let b = side(&mut rng);
    (a, b)
}
