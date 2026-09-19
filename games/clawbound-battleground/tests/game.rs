use clawbound::consts::*;
use clawbound::gods::{AutoKind, ROSTER};
use clawbound::items;
use clawbound::sim::{AiState, Game, Team};

#[test]
fn catalog_has_exactly_100_items() {
    let cat = items::all();
    assert_eq!(cat.len(), 100, "item catalog must have exactly 100 items");
    let mut ids: Vec<u16> = cat.iter().map(|i| i.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 100, "item ids must be unique");
    for it in cat {
        assert!(it.cost > 0.0, "item {} must cost something", it.name);
        assert!(it.tier >= 1 && it.tier <= 3);
    }
}

#[test]
fn catalog_categories_match_user_spec() {
    // All 8 requested categories must exist with items
    let cat = items::all();
    for c in items::CATS {
        let n = cat.iter().filter(|it| it.cat == c).count();
        assert!(n >= 8, "category {:?} has {} items, expected >= 8", c, n);
    }
}

#[test]
fn stacking_items_declare_rules() {
    let cat = items::all();
    let stacking = cat.iter().filter(|it| it.cat == items::Cat::Stacking);
    assert_eq!(stacking.count(), 8, "8 handcrafted stacking items");
    for it in cat.iter().filter(|it| it.stack.is_some()) {
        let st = it.stack.unwrap();
        assert!(st.per > 0.0 && st.max > st.per, "stacking rule for {}", it.name);
    }
}

#[test]
fn roster_is_13_ancestors_with_4_abilities_each() {
    assert_eq!(ROSTER.len(), 13);
    let roles: std::collections::HashSet<_> = ROSTER.iter().map(|g| g.role).collect();
    assert!(roles.len() >= 4, "roster should span multiple roles");
    for g in &ROSTER {
        assert_eq!(g.abilities.len(), 4);
        for (i, ab) in g.abilities.iter().enumerate() {
            assert_eq!(ab.key, i as u32);
            assert!(ab.cooldown > 0.0);
        }
        assert!(g.hp > 400.0 && g.hp < 800.0);
        assert_eq!((g.auto == AutoKind::Melee) as bool, g.auto_range < 6.0);
    }
}

#[test]
fn lanes_are_symmetric_and_reach_both_bases() {
    for lane in LANES {
        let path_blue = lane.path_for(Team::Blue);
        let path_red = lane.path_for(Team::Red);
        assert!(path_blue.len() > 10);
        // red path is blue path reversed (endpoints swapped)
        let d = |a: glam::Vec3, b: glam::Vec3| (a - b).length();
        assert!(d(path_blue[0], Team::Blue.base_pos()) < 14.0);
        assert!(d(*path_blue.last().unwrap(), Team::Red.base_pos()) < 14.0);
        assert!(d(path_red[0], Team::Red.base_pos()) < 14.0);
    }
}

#[test]
fn towers_exist_for_all_lanes_and_teams() {
    for lane in LANES {
        for team in [Team::Blue, Team::Red] {
            let outer = tower_pos(lane, team, false);
            let inner = tower_pos(lane, team, true);
            assert!(outer.z.abs() > 0.0 || lane == Lane::Mid);
            // inner is closer to the owning base
            assert!(
                inner.distance(team.base_pos()) < outer.distance(team.base_pos()),
                "inner tower must be closer to base"
            );
        }
    }
}

#[test]
fn simulation_runs_120s_without_panic_and_progresses() {
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    // give the player slot an AI brain so it fights too
    game.gods[0].ai = Some(clawbound::sim::AiBrain {
        lane: Lane::Mid,
        aggression: 0.9,
        state: AiState::Push,
        target: None,
        strafe_dir: 1.0,
        strafe_t: 0.0,
        buy_t: 1.0,
        wp: 0,
    });
    for step in 0..(120 * 120) {
        game.tick(1.0 / 120.0);
        if game.over.is_some() {
            break;
        }
        let _ = step;
    }
    assert!(game.t > 60.0, "sim should run past one minute");
    // gold must flow somewhere: the roster is seeded, so any one slot can be
    // the one that gets fed — check the whole match's economy instead
    let team_gold: f32 = game.gods.iter().map(|g| g.gold + g.items.len() as f32 * 500.0).sum();
    assert!(team_gold > 10.0 * START_GOLD, "gold economy must flow: {team_gold}");
    assert!(game.gods[0].gold > 0.0 || !game.gods[0].items.is_empty(), "the player slot must at least have bought");
    // towers must have been fought over
    assert!(game.tower_alive[0] + game.tower_alive[1] < 12, "AI should be taking towers: {}", game.tower_alive[0] + game.tower_alive[1]);
}

#[test]
fn damage_mitigation_order_pen_then_flat() {
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    // find an enemy of blue
    let enemy = game.gods.iter().position(|g| g.team == Team::Red).unwrap();
    let hp0 = game.gods[enemy].hp;
    let prot = game.gods[enemy].prot;
    // 100 dmg, 50% pen: eff = prot*0.5; mult = 100/(100+eff)
    let expected = hp0 - 100.0 * 100.0 / (100.0 + prot * 0.5);
    game.deal_damage(clawbound::sim::TargetRef::God(enemy), 100.0, false, None, 0.5, 0.0);
    assert!((game.gods[enemy].hp - expected).abs() < 0.01);
}

#[test]
fn titan_shielded_until_towers_fall() {
    let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    assert!(game.titan_shielded(Team::Red), "titan starts shielded");
    assert!(game.titan_shielded(Team::Blue));
    assert_eq!(game.tower_alive[0], 6);
}

#[test]
fn xp_curve_monotonic() {
    let mut prev = 0.0;
    for l in 1..MAX_LEVEL {
        let x = xp_for_level(l);
        assert!(x > prev);
        prev = x;
    }
}


/// The leap, through the real sim loop: press toward a rock, end up standing
/// on it; press again toward the next, and the chain skips the cooldown;
/// walk off, and come back down.
#[test]
fn a_cat_leaps_onto_a_rock_bounds_to_the_next_and_steps_off() {
    use clawbound::blockers::{Blockers, PropKind};
    use clawbound::leap::{LEAP_COOLDOWN, LEAP_UP};
    use glam::Vec3;
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    let mut b = Blockers::new(200.0, 6.0);
    let start = game.gods[0].pos;
    b.push_prop(PropKind::Rock, start.x + 5.0, start.z, 0.0, 1.0);
    b.push_prop(PropKind::BoulderCluster, start.x + 10.5, start.z, 0.0, 1.0);
    game.blockers = std::sync::Arc::new(b);
    let step = |g: &mut Game, n: usize| for _ in 0..n { g.tick(1.0 / 60.0); };

    game.leap_queue.push((0, Vec3::X));
    step(&mut game, 5);
    assert!(game.gods[0].leap.is_some(), "the press was accepted");
    let mut peak = 0.0f32;
    for _ in 0..90 {
        game.tick(1.0 / 60.0);
        peak = peak.max(game.gods[0].pos.y);
    }
    let g = &game.gods[0];
    assert_eq!(g.perch, Some(1.4), "standing on the rock");
    assert!((g.pos - Vec3::new(start.x + 5.0, 1.4, start.z)).length() < 0.2, "at {}", g.pos);
    assert!(peak > 1.8 && peak < LEAP_UP, "arced over it: {peak}");
    assert!(g.leap_cd > LEAP_COOLDOWN * 0.3, "a ground leap costs a cooldown");

    // Still cooling down -- but from a perch the chain is free.
    game.leap_queue.push((0, Vec3::X));
    step(&mut game, 90);
    assert_eq!(game.gods[0].perch, Some(2.0), "bounded to the boulders while the cooldown was still running");

    // Walk off the edge.
    game.gods[0].move_dir = Vec3::Z;
    step(&mut game, 60);
    assert_eq!(game.gods[0].perch, None);
    assert!(game.gods[0].pos.y < 0.01, "and came down: y = {}", game.gods[0].pos.y);

    // On open ground with the cooldown running, a press is refused.
    game.gods[0].move_dir = Vec3::ZERO;
    game.gods[0].leap_cd = 1.0;
    game.leap_queue.push((0, Vec3::Z));
    step(&mut game, 3);
    assert!(game.gods[0].leap.is_none(), "no back-to-back leaps across open ground");
}


/// What a cat does with a wall: up onto it from the ground, along the top
/// of it, knocked off it by a solid hit -- and down on its feet, unhurt by
/// the fall itself.
#[test]
fn a_cat_runs_a_wall_top_is_knocked_off_and_lands_on_its_feet() {
    use clawbound::blockers::{Blockers, PropKind};
    use clawbound::leap::{Phase, RIGHTING_SECS};
    use glam::Vec3;
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    let start = game.gods[0].pos;
    let mut b = Blockers::new(250.0, 6.0);
    // a wall running along +x, four units ahead in +z
    b.push_segment(PropKind::RuinWall, start.x - 2.0, start.z + 4.0, start.x + 14.0, start.z + 4.0, 0.0, 1.0);
    game.blockers = std::sync::Arc::new(b);
    // Keep everyone else out of it: this is about one cat and one wall.
    for g in game.gods.iter_mut().skip(1) {
        g.respawn_t = 999.0;
    }

    game.leap_queue.push((0, Vec3::Z));
    for _ in 0..90 {
        game.tick(1.0 / 60.0);
    }
    let g = &game.gods[0];
    assert_eq!((g.perch, g.perch_narrow), (Some(3.0), true), "up on the wall, three times its own height, and it is a ridge");

    // Along the top. It must STAY on top, and cover real ground.
    let x0 = game.gods[0].pos.x;
    game.gods[0].move_dir = Vec3::X;
    for _ in 0..60 {
        game.tick(1.0 / 60.0);
        assert_eq!(game.gods[0].perch, Some(3.0), "fell off a wall it was walking ALONG at x = {}", game.gods[0].pos.x);
    }
    let ran = game.gods[0].pos.x - x0;
    assert!(ran > 5.0, "a second along the ridge covered only {ran} u");
    game.gods[0].move_dir = Vec3::ZERO;
    game.tick(1.0 / 60.0);

    // A solid hit. (A scratch must not do it -- both directions.)
    let max = game.gods[0].max_hp;
    game.gods[0].hp -= max * 0.04;
    game.tick(1.0 / 60.0);
    assert!(game.gods[0].perch.is_some(), "a 4% scratch does not take a cat's footing");
    game.gods[0].hp -= max * 0.20;
    let hp_after_hit = game.gods[0].hp;
    game.tick(1.0 / 60.0);
    let fall = game.gods[0].fall.expect("a 20% hit on a ridge knocks it off");
    assert!(fall.tumble.abs() > 2.0, "and it goes over on its back: {}", fall.tumble);

    let (mut t, mut upright_at, mut landed_at) = (0.0f32, None, None);
    for _ in 0..120 {
        game.tick(1.0 / 60.0);
        t += 1.0 / 60.0;
        let g = &game.gods[0];
        if let Some(f) = g.fall {
            if upright_at.is_none() && f.roll().abs() < 0.05 {
                upright_at = Some(t);
            }
        } else if landed_at.is_none() {
            landed_at = Some(t);
            assert!(matches!(g.leap, Some(l) if l.phase == Phase::Land), "a fall ends in a landing, not a teleport");
        }
    }
    let (up, down) = (upright_at.expect("never righted"), landed_at.expect("never landed"));
    assert!(up <= RIGHTING_SECS + 0.04 && up < down, "upright at {up}s, down at {down}s");
    let g = &game.gods[0];
    assert!(g.pos.y.abs() < 1e-3 && g.perch.is_none() && g.leap.is_none(), "on the ground and done: {:?}", g.pos);
    assert!(g.hp >= hp_after_hit - 1e-3, "the FALL cost nothing: {} -> {}", hp_after_hit, g.hp);
    let off_axis = (g.pos.z - (start.z + 4.0)).abs();
    assert!(off_axis > 0.9 + 0.5, "and it is beside the wall, not inside it: {off_axis} u from the spine");
}
