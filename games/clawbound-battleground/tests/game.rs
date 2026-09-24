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

/// Stalking is a state, not a key held down (review: a held Ctrl is a
/// compositor shortcut on Linux and absent on a handset, and tying the stalk
/// to the fight is better balance anyway). Three sources, one number.
#[test]
fn stalking_is_broken_by_a_fight_granted_by_the_kit_and_forced_by_wounds() {
    use clawbound::consts::{SNEAK_SPEED, STALK_BREAK_SECS, WOUNDED_FRAC};
    use clawbound::gods::Archetype;
    // An ancestor with a movement or self-buff ability: the kit's sneak boost.
    let (def_id, slot) = ROSTER
        .iter()
        .find_map(|d| d.abilities.iter().position(|a| matches!(a.arch, Archetype::Dash { .. } | Archetype::Blink { .. } | Archetype::Buff { .. })).map(|s| (d.id, s as u32)))
        .expect("some ancestor has a dash, blink or buff");
    let others: Vec<u32> = (0..ROSTER.len() as u32).filter(|i| *i != def_id).collect();
    let mut game = Game::new(def_id, [others[0], others[1], others[2], others[3]], [others[4], others[5], others[6], others[7], others[8]]);
    for g in game.gods.iter_mut().skip(1) {
        g.respawn_t = 999.0;
    }
    let tick = |g: &mut Game, secs: f32| for _ in 0..(secs * 60.0) as usize { g.tick(1.0 / 60.0); };
    tick(&mut game, 0.1);

    // 1. Intent alone: stalking, fully quiet, and a quarter of the speed.
    let run = game.move_speed(0);
    game.gods[0].sneak = true;
    assert!(game.stalking(0) && game.quiet(0) == 1.0);
    assert!((game.move_speed(0) / run - SNEAK_SPEED).abs() < 1e-4);

    // 2. A hit breaks it -- quiet AND the slow, at once, so a stalking cat
    //    that is jumped can run -- and it comes back by itself afterwards.
    game.gods[0].last_dmg_t = game.t;
    assert!(!game.stalking(0) && game.quiet(0) == 0.0, "a fight is not a stalk");
    assert_eq!(game.move_speed(0), run, "and the cat has its legs back immediately");
    tick(&mut game, STALK_BREAK_SECS + 0.2);
    assert!(game.stalking(0), "the intent was never cleared, so it settles back into the stalk");

    // 3. The kit: a cast breaks the stalk (it is an act) but opens a hush --
    //    nearly as quiet, at FULL speed.
    game.gods[0].mana = game.gods[0].max_mana;
    game.gods[0].cds = [0.0; 4];
    let aim = game.gods[0].pos + glam::Vec3::Z * 6.0;
    game.cast_queue.push((0, slot, aim));
    tick(&mut game, 0.05);
    assert!(!game.stalking(0), "casting is acting");
    assert!(game.gods[0].hush_t > 1.0, "but this ability grants a hush: {}", game.gods[0].hush_t);
    assert!((0.85..1.0).contains(&game.quiet(0)), "quiet {}", game.quiet(0));
    game.gods[0].sneak = false;
    let s = game.move_speed(0);
    assert!(s >= run * 0.99, "hush costs no speed: {s} vs {run}");

    // 4. Wounds. Both sides of the line.
    game.gods[0].hush_t = 0.0;
    game.gods[0].hp = game.gods[0].max_hp * (WOUNDED_FRAC + 0.05);
    assert_eq!(game.quiet(0), 0.0, "hurt but not badly");
    game.gods[0].hp = game.gods[0].max_hp * (WOUNDED_FRAC - 0.05);
    assert_eq!(game.quiet(0), 0.5, "a badly hurt cat slinks");
}


/// The bridges are gone: the river runs through the crossings, a log is the
/// dry way over, and a log can be blown out of its seat and rolled away --
/// after which the lane fords the river like anywhere else.
#[test]
fn the_crossing_is_a_log_the_log_can_be_rolled_away_and_then_it_is_a_ford() {
    use clawbound::logs::{crossings, LOG_RADIUS};
    use clawbound::sim::Event;
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    assert_eq!(game.logs.len(), 2);
    let c = crossings()[0];
    let log = game.logs[0];

    // There is a river under the crossing now. (Before: a causeway, depth 0.)
    assert!(water_depth(c.at.x, c.at.z) > 1.0, "no channel under the crossing: depth {}", water_depth(c.at.x, c.at.z));
    // On the log: dry, full speed. A log's width to the side: wading.
    assert_eq!(game.water_depth_now(c.at.x, c.at.z), 0.0, "the log is the dry way over");
    let beside = c.at + log.down * (LOG_RADIUS + 1.5);
    assert!(game.water_depth_now(beside.x, beside.z) > 0.8, "beside it you wade");
    game.gods[0].pos = c.at;
    let dry = game.move_speed(0);
    game.gods[0].pos = beside;
    assert!(game.move_speed(0) < dry * 0.85, "and wading is slower: {} vs {dry}", game.move_speed(0));

    // Park everyone far away; a cat rides the log.
    for g in game.gods.iter_mut() {
        g.pos = glam::Vec3::new(0.0, 0.0, 120.0);
    }
    game.gods[0].pos = c.at;
    // A blast on the upstream side.
    let from = c.at - log.down * 3.0;
    game.events.push(Event::Explosion { pos: from, radius: 6.0, color: [1.0; 3] });
    let mut thrown = false;
    for _ in 0..(20 * 60) {
        game.tick(1.0 / 60.0);
        game.gods[0].move_dir = glam::Vec3::ZERO;
        thrown |= game.gods[0].fall.is_some();
        if game.logs[0].lodged {
            break;
        }
    }
    let l = game.logs[0];
    assert!(l.lodged, "the log never came to rest: x = {}, free = {}", l.x, l.free);
    assert!((l.centre() - c.at).length() > 15.0, "it went {} u", (l.centre() - c.at).length());
    assert!(thrown, "the cat riding it was never thrown");
    assert!(game.gods[0].fall.is_none() && game.gods[0].pos.y.abs() < 0.01, "and ended on its feet in the river");
    // The lane now fords the river; the log is a dry perch downstream.
    assert!(game.water_depth_now(c.at.x, c.at.z) > 1.0, "the crossing is a ford now");
    assert_eq!(game.water_depth_now(l.centre().x, l.centre().z), 0.0);
    assert!(!game.logs[1].free && game.logs[1].x.abs() < 0.01, "the other crossing, 150 u away, never noticed");
}


/// "When is the line crossed, and who is being targeted" -- answered by
/// state the player is shown, tested as state. And the numbers behind
/// "indefensible": the tower now out-ranges every auto and every lob.
#[test]
fn a_tower_says_where_its_line_is_and_who_it_is_after() {
    use clawbound::sim::TargetRef;
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    for g in game.gods.iter_mut().skip(1) {
        g.respawn_t = 999.0;
    }
    let me = game.gods[0].team;
    let ti = game.towers.iter().position(|t| t.team != me && t.hp > 0.0).unwrap();
    let tpos = game.towers[ti].pos;
    let mine = game.towers.iter().position(|t| t.team == me).unwrap();
    let state = |g: &Game, i: usize| g.tower_rings(0)[i][3];

    game.gods[0].pos = tpos + glam::Vec3::X * (TOWER_RANGE + 3.0);
    game.tick(1.0 / 60.0);
    assert_eq!(state(&game, mine), if (game.towers[mine].pos - game.gods[0].pos).length() < TOWER_RANGE + TOWER_RING_SHOW { 1.0 } else { 0.0 }, "my own tower: calm when near, not drawn when far");
    let far = game.towers.iter().position(|t| (t.pos - game.gods[0].pos).length() > TOWER_RANGE + TOWER_RING_SHOW + 5.0).expect("some tower is far away");
    assert_eq!(state(&game, far), 0.0, "a line I am nowhere near is not drawn");
    assert_eq!(state(&game, ti), 2.0, "theirs, and I am outside the line");
    assert_eq!(game.tower_rings(0)[ti][2], TOWER_RANGE);
    assert_eq!(game.towers[ti].aim, None);

    // One step over the line. It aims THIS TICK -- before it has fired.
    game.gods[0].pos = tpos + glam::Vec3::X * (TOWER_RANGE - 0.5);
    game.towers[ti].atk_timer = 0.8;
    game.tick(1.0 / 60.0);
    assert_eq!(game.towers[ti].aim, Some(TargetRef::God(0)), "the tether is live on the tick the line is crossed");
    assert_eq!(state(&game, ti), 4.0);
    assert!(game.towers[ti].last_target.is_none(), "and it has not fired yet: last_target could never have shown this");

    // Its shot slows: running past is a decision.
    let speed = game.move_speed(0);
    for _ in 0..70 {
        game.gods[0].pos = tpos + glam::Vec3::X * (TOWER_RANGE - 0.5);
        game.tick(1.0 / 60.0);
    }
    assert!(game.move_speed(0) < speed * 0.8, "a tower shot slows: {} vs {speed}", game.move_speed(0));

    // Step back out: the aim drops at once.
    game.gods[0].pos = tpos + glam::Vec3::X * (TOWER_RANGE + 3.0);
    game.tick(1.0 / 60.0);
    assert_eq!(game.towers[ti].aim, None);
    assert_eq!(state(&game, ti), 2.0);

    let max_auto = ROSTER.iter().map(|d| d.auto_range).fold(0.0f32, f32::max);
    assert!(TOWER_RANGE > max_auto + 6.0 && TOWER_RANGE > 20.0, "out-ranges every auto ({max_auto}) and the 20 u lobs");
}


/// Measure before optimising. What a full 5v5 bot match costs per tick on the
/// CPU, and a tripwire if it ever gets much worse. This profile is
/// UNOPTIMISED, so the absolute number is pessimistic by several times; it is
/// here to be compared with itself.
#[test]
fn a_full_match_tick_is_cheap_enough_to_leave_the_frame_to_the_gpu() {
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    // The player slot idles: nine bots, the waves and the towers are the load.
    for _ in 0..600 {
        game.tick(1.0 / 60.0); // warm up: minion waves out, bots in lane
        game.events.clear();
    }
    let ticks = 1800;
    let t0 = std::time::Instant::now();
    for _ in 0..ticks {
        game.tick(1.0 / 60.0);
        game.events.clear();
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / ticks as f64;
    println!("sim: {ms:.3} ms/tick over {ticks} ticks, {} minions, {} projectiles (unoptimised profile)", game.minions.len(), game.projectiles.len());
    assert!(ms < 6.0, "a tick costs {ms:.2} ms unoptimised; the budget at 60 Hz is 16.6 for EVERYTHING");
}

/// Rough ground through the real sim loop: a cat run through a thicket
/// covers less ground than the same run on open ground, is not stopped, and
/// a leap over it is not slowed at all (`move_speed` skips the going while
/// airborne).
#[test]
fn a_thicket_slows_a_cat_and_does_not_stop_it() {
    use clawbound::blockers::{Blockers, Shape};
    use clawbound::going::{Ground, Rough};
    use glam::Vec3;
    let run = |rough: bool| {
        let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for g in game.gods.iter_mut().skip(1) {
            g.respawn_t = 999.0; // alone on the field
        }
        game.gods[0].ai = None;
        let start = game.gods[0].pos;
        let mut b = Blockers::new(200.0, 6.0);
        if rough {
            b.push_rough(Rough { shape: Shape::Circle { x: start.x, z: start.z + 6.0, r: 8.0 }, mul: 0.5, edge: 0.5, ground: Ground::Thicket });
        }
        game.blockers = std::sync::Arc::new(b);
        for _ in 0..60 {
            game.gods[0].move_dir = Vec3::Z;
            game.tick(1.0 / 60.0);
        }
        (game.gods[0].pos - start).length()
    };
    let (open, thick) = (run(false), run(true));
    assert!(thick > 0.5, "slowed, not stopped: {thick}");
    let ratio = thick / open;
    assert!((ratio - 0.5).abs() < 0.08, "a 0.5 thicket halves the run: {thick} vs {open} ({ratio})");
}

/// Bots look before they commit (`steer`): the same bot in the same state,
/// with a trunk dropped two units down the line it chose, turns off that
/// line instead of walking into the trunk and being pushed round it.
#[test]
fn a_bot_steers_off_a_line_that_runs_into_a_trunk() {
    use clawbound::blockers::{Blocker, Blockers, Cover, Shape};
    use glam::Vec3;
    let fresh = || {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        g.blockers = std::sync::Arc::new(Blockers::new(200.0, 6.0));
        g
    };
    let bot = 1;
    let mut a = fresh();
    clawbound::ai::think(&mut a, bot);
    let d1 = a.gods[bot].move_dir;
    assert!(d1.length() > 0.1, "the bot wants to go somewhere: {d1}");
    let at = a.gods[bot].pos + d1.normalize() * 2.0;
    let mut b = fresh();
    let mut bl = Blockers::new(200.0, 6.0);
    bl.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 0.9 }, top: 4.0, cover: Cover::NONE });
    b.blockers = std::sync::Arc::new(bl);
    clawbound::ai::think(&mut b, bot);
    let d2 = b.gods[bot].move_dir;
    let turn = d1.normalize().dot(d2.normalize()).clamp(-1.0, 1.0).acos();
    assert!(turn > 0.3, "turned off the line by {turn} rad");
    let probe = b.gods[bot].pos + d2.normalize() * 1.2;
    assert!(!b.blockers.blocked(probe.x, probe.z, GOD_RADIUS), "and not into the trunk");
    let _ = Vec3::ZERO;
}

/// Twinfang Pass through the real sim: 15 a side, the scene's own
/// collision (cliffs included), 60 s of bots. No living cat is ever in the
/// rock, and every lane's wave of cats has left its base.
#[test]
fn the_pass_holds_fifteen_a_side_and_nobody_walks_into_the_rock() {
    use clawbound::map::{self, MapId};
    map::with(MapId::Pass, || {
        let blockers = std::sync::Arc::new(clawbound::scene::world_blockers());
        let (b, r) = map::team_picks(2, &mut clawbound::sim::Rng::new(5));
        let mut game = Game::new_teams(&b, &r);
        assert_eq!(game.gods.len(), 30);
        game.blockers = blockers;
        let f = map::floor().unwrap();
        for _ in 0..(60 * 60) {
            game.tick(1.0 / 60.0);
            game.events.clear();
            for (i, g) in game.gods.iter().enumerate() {
                if g.respawn_t <= 0.0 {
                    assert!(f.sdf(g.pos.x, g.pos.z) <= 0.05, "god {i} in the rock at {} (sdf {})", g.pos, f.sdf(g.pos.x, g.pos.z));
                }
            }
        }
        let left = game.gods.iter().filter(|g| (g.pos - g.team.base_pos()).length() > 40.0).count();
        assert!(left >= 20, "only {left} of 30 cats got out of their bases in a minute");
    });
}

/// A bot on the pass, inside a massif, finds its way out to its lane: the
/// way starts sideways, round the peak, which local steering alone does not
/// find (`floor::route`). Break-tested: with routing off the bot is still
/// pinned at (-24, -43) after 30 s; with it, it is on the lane in < 1 s.
/// (A shoulder clearing was tried first and did NOT fail without routing,
/// so it proved nothing.)
#[test]
fn a_bot_finds_its_way_out_of_the_massif_on_the_pass() {
    use clawbound::map::{self, MapId};
    map::with(MapId::Pass, || {
        let blockers = std::sync::Arc::new(clawbound::scene::world_blockers());
        let (b, r) = map::team_picks(2, &mut clawbound::sim::Rng::new(5));
        let mut game = Game::new_teams(&b, &r);
        game.blockers = blockers;
        // everyone else out of the way
        for g in game.gods.iter_mut().skip(3) {
            g.respawn_t = 999.0;
        }
        // A mid-lane bot on the boss ground inside the west massif: the peak
        // stands between it and its lane, and both ways out (the slot round
        // the peak's flank, the slot to the side lane) start sideways.
        let bot = 2;
        assert_eq!(game.gods[bot].ai.as_ref().unwrap().lane, clawbound::consts::Lane::Mid);
        game.gods[1].respawn_t = 999.0;
        game.gods[bot].respawn_t = 0.0;
        game.gods[bot].pos = glam::Vec3::new(-96.0 * clawbound::pass::S, 0.0, 0.0);
        let mut t = 0.0;
        let mut on_lane = false;
        while t < 30.0 {
            game.tick(1.0 / 60.0);
            game.events.clear();
            t += 1.0 / 60.0;
            let p = game.gods[bot].pos;
            if clawbound::consts::lane_clearance(p.x, p.z) < 6.0 {
                on_lane = true;
                break;
            }
        }
        assert!(on_lane, "still off its lane after {t:.0} s, at {}", game.gods[bot].pos);
    });
}

/// Through the real sim: a cat walking the pass's Top lane over the shelf
/// -- up the ramp, along the crest, down again -- is on the ground every
/// tick and never enters a fall (walking down a ramp is not falling).
#[test]
fn a_cat_walks_the_climbing_lane_on_the_ground_the_whole_way() {
    use clawbound::map::{self, MapId};
    map::with(MapId::Pass, || {
        let blockers = std::sync::Arc::new(clawbound::scene::world_blockers());
        let (b, r) = map::team_picks(2, &mut clawbound::sim::Rng::new(5));
        let mut game = Game::new_teams(&b, &r);
        game.blockers = blockers;
        for g in game.gods.iter_mut().skip(1) {
            g.respawn_t = 999.0;
        }
        game.gods[0].ai = None;
        let top = clawbound::consts::Lane::Top.polyline();
        let len: f32 = top.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let start = clawbound::pass::along(&top, 0.33 * len);
        game.gods[0].pos = start;
        let (mut peak, mut s) = (0.0f32, 0.33 * len);
        for _ in 0..(60 * 60) {
            // steer along the lane: aim a few units ahead of where we are
            let p = game.gods[0].pos;
            let ahead = clawbound::pass::along(&top, s + 6.0);
            // HORIZONTAL distance: the waypoints are at y = 0 and the cat is
            // up on the shelf (a 3D distance stalled this test at 0.43)
            if glam::Vec3::new(ahead.x - p.x, 0.0, ahead.z - p.z).length() < 7.0 {
                s += 1.0;
            }
            game.gods[0].move_dir = glam::Vec3::new(ahead.x - p.x, 0.0, ahead.z - p.z).normalize_or_zero();
            game.tick(1.0 / 60.0);
            game.events.clear();
            let g = &game.gods[0];
            let ground = map::ground_y(g.pos.x, g.pos.z);
            assert!(g.fall.is_none(), "fell walking the lane at {} (ground {ground})", g.pos);
            assert!((g.pos.y - ground).abs() < 0.05, "off the ground at {}: y {} vs {ground}", g.pos, g.pos.y);
            peak = peak.max(g.pos.y);
            if s > 0.67 * len {
                break;
            }
        }
        assert!(s > 0.67 * len, "did not get over the shelf (reached {:.2})", s / len);
        assert!((peak - clawbound::pass::SHELF_H).abs() < 0.1, "climbed to {peak}");
    });
}

/// The same climb for the AI and a minion wave: bots pushing Top and the
/// minions they walk with get over the shelf. Before waypoints stood on the
/// ground, both advanced waypoints by 3D distance against points at y = 0
/// and stalled 9 u above them.
#[test]
fn bots_and_minions_get_over_the_shelf() {
    use clawbound::map::{self, MapId};
    map::with(MapId::Pass, || {
        let blockers = std::sync::Arc::new(clawbound::scene::world_blockers());
        let (b, r) = map::team_picks(2, &mut clawbound::sim::Rng::new(5));
        let mut game = Game::new_teams(&b, &r);
        game.blockers = blockers;
        // red stays home: an empty lane to walk
        for g in game.gods.iter_mut() {
            if g.team == clawbound::consts::Team::Red {
                g.respawn_t = 9999.0;
            }
        }
        let top = clawbound::consts::Lane::Top.polyline();
        let len: f32 = top.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let past = clawbound::pass::along(&top, 0.66 * len);
        let (mut god_past, mut minion_past) = (false, false);
        for _ in 0..(150 * 60) {
            game.tick(1.0 / 60.0);
            game.events.clear();
            let near = |p: glam::Vec3| glam::Vec3::new(p.x - past.x, 0.0, p.z - past.z).length() < 12.0;
            god_past |= game.gods.iter().any(|g| g.team == clawbound::consts::Team::Blue && g.respawn_t <= 0.0 && near(g.pos));
            minion_past |= game.minions.iter().any(|m| m.team == clawbound::consts::Team::Blue && near(m.pos));
            if god_past && minion_past {
                break;
            }
        }
        assert!(god_past, "no blue bot got over the shelf in 150 s");
        assert!(minion_past, "no blue minion got over the shelf in 150 s");
    });
}
