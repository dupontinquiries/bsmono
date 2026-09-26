use clawbound::consts::*;
use clawbound::gods::{AutoKind, ROSTER};
use clawbound::items;
use clawbound::sim::{Game, Team};

#[test]
fn catalog_has_exactly_102_items() {
    // 100 across the stat categories, and the two Siphon items (brawl)
    let cat = items::all();
    assert_eq!(cat.len(), 102, "item catalog must have exactly 102 items");
    let mut ids: Vec<u16> = cat.iter().map(|i| i.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 102, "item ids must be unique");
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
fn roster_is_14_ancestors_with_4_abilities_each() {
    assert_eq!(ROSTER.len(), 14);
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
fn simulation_runs_three_minutes_without_panic_and_progresses() {
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    // give the player slot an AI brain so it fights too
    game.gods[0].ai = Some({ let mut b = clawbound::sim::AiBrain::new(Lane::Mid, 0.9); b.buy_t = 1.0; b });
    // Three minutes, not two: since bots stopped sieging towers alone
    // (ai.rs, tower discipline) this roster's first tower falls at 2-3
    // minutes rather than 72 s (163 s measured at 041, 141 s at 043, with
    // the minion changes between) -- though the AI-only average across six
    // other rosters barely moved (examples/aiprobe: 116 s before, 120 s
    // after).
    for step in 0..(180 * 120) {
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
    assert!(!game.stalking(0), "casting is acting -- from the wind-up");
    // the hush comes with the ability itself, when it fires after its wind-up
    let wind = clawbound::abilities::cast_timing(&game.def(0).abilities[slot as usize]).0;
    tick(&mut game, wind + 0.02);
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

/// The firing range: every ability of every ancestor, cast by a cat facing
/// EAST at an aim to the NORTH (where the camera looks). Each must fire;
/// the cat must turn to face north; and every directional effect -- a
/// projectile's flight, a dash's path, a blink's landing, a beam, a
/// telegraph, a storm, a pull -- must go north, never along the cat's old
/// facing.
#[test]
fn firing_range_every_ability_goes_where_it_is_aimed_and_turns_the_cat() {
    use clawbound::gods::{Archetype, ROSTER};
    use clawbound::sim::Event;
    use glam::Vec3;
    let north = Vec3::Z;
    let mut report = Vec::new();
    for (god, def) in ROSTER.iter().enumerate() {
        for slot in 0..4u32 {
            let mut game = Game::new_teams(&[god as u32], &[(god as u32 + 1) % 13]);
            game.gods[1].respawn_t = 999.0; // an empty range
            game.gods[0].ai = None;
            let start = Vec3::new(0.0, 0.0, -40.0);
            game.gods[0].pos = start;
            game.gods[0].yaw = 0.0; // facing east: (cos 0, 0, sin 0)
            game.gods[0].mana = 10_000.0;
            let aim = start + north * 10.0;
            game.cast_queue.push((0, slot, aim));
            let mut events = Vec::new();
            let mut fired = false;
            let mut turned = false;
            let mut dash_dir = None;
            let mut shots: Vec<Vec3> = Vec::new();
            for _ in 0..90 {
                game.tick(1.0 / 60.0);
                let f = clawbound::abilities::facing_of(game.gods[0].yaw);
                turned |= f.dot(north) > 0.98;
                if let Some(d) = game.gods[0].dash.as_ref() {
                    dash_dir.get_or_insert((d.to - d.from).normalize_or_zero());
                }
                for p in &game.projectiles {
                    if p.owner == 0 && p.vel.length() > 0.1 {
                        shots.push(Vec3::new(p.vel.x, 0.0, p.vel.z).normalize());
                    }
                }
                for e in game.events.drain(..) {
                    fired |= matches!(e, Event::Cast { god: 0, .. });
                    events.push(e);
                }
                if fired && game.gods[0].dash.is_none() && game.gods[0].casting.is_none() {
                    break;
                }
            }
            let ab = def.abilities[slot as usize];
            assert!(fired, "{} slot {slot} ({}) never fired", def.name, ab.name);
            assert!(turned, "{} slot {slot} ({}) did not turn the cat to its aim", def.name, ab.name);
            // the evidence of direction, per archetype
            let flat = |p: Vec3| Vec3::new(p.x - start.x, 0.0, p.z - start.z).normalize_or_zero();
            let mut dirs: Vec<Vec3> = shots.clone();
            for e in &events {
                match *e {
                    Event::BeamFx { dir, length, .. } if length > 2.0 => dirs.push(Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero()),
                    Event::RingTelegraph { pos, .. } | Event::StormFx { pos, .. } | Event::PullFx { pos, .. } => {
                        if Vec3::new(pos.x - start.x, 0.0, pos.z - start.z).length() > 1.0 {
                            dirs.push(flat(pos));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(d) = dash_dir {
                dirs.push(d);
            }
            if matches!(ab.arch, Archetype::Blink { .. }) {
                dirs.push(flat(game.gods[0].pos));
            }
            let directional = !matches!(ab.arch, Archetype::Nova { .. } | Archetype::Shield { .. } | Archetype::Buff { .. } | Archetype::Heal { .. });
            if directional {
                assert!(!dirs.is_empty(), "{} slot {slot} ({:?}): no directional effect found", def.name, ab.arch);
                for d in &dirs {
                    assert!(d.dot(north) > 0.9, "{} slot {slot} ({}) went {d}, aimed north", def.name, ab.name);
                }
            }
            report.push(format!("{}/{}: {} dirs ok", def.name, ab.name, dirs.len()));
        }
    }
    assert_eq!(report.len(), ROSTER.len() * 4);
}

/// A cast aimed at the cat itself goes along its facing -- the cat's own
/// convention, (cos yaw, sin yaw). It went along (sin, cos), mirrored.
#[test]
fn a_cast_aimed_at_the_cat_goes_along_its_facing() {
    use clawbound::gods::{Archetype, ROSTER};
    use glam::Vec3;
    let (god, slot) = ROSTER
        .iter()
        .enumerate()
        .find_map(|(g, d)| d.abilities.iter().position(|a| matches!(a.arch, Archetype::Bolt { .. })).map(|s| (g, s as u32)))
        .expect("someone has a bolt");
    let mut game = Game::new_teams(&[god as u32], &[(god as u32 + 1) % 13]);
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    game.gods[0].mana = 10_000.0;
    let yaw = 0.4f32;
    game.gods[0].yaw = yaw;
    let p = game.gods[0].pos;
    game.cast_queue.push((0, slot, p));
    for _ in 0..60 {
        game.tick(1.0 / 60.0);
        if let Some(pr) = game.projectiles.iter().find(|pr| pr.owner == 0) {
            let d = Vec3::new(pr.vel.x, 0.0, pr.vel.z).normalize();
            let want = clawbound::abilities::facing_of(yaw);
            assert!(d.dot(want) > 0.99, "bolt went {d}, facing is {want}");
            return;
        }
    }
    panic!("no bolt fired");
}

/// Movement turns the cat only within a cone round the view: strafing
/// right, it turns part way; moving back toward the camera, it keeps
/// facing the view and backs up. A channel roots it; a wind-up slows it.
#[test]
fn movement_faces_within_the_view_cone_and_casts_hold_the_cat() {
    use clawbound::systems::{facing_yaw, BACKPEDAL, FACE_CONE};
    use glam::Vec3;
    let view = Vec3::Z;
    let v = clawbound::abilities::yaw_of(view);
    let east = facing_yaw(Vec3::X, view);
    assert!(((east - v).abs() - FACE_CONE).abs() < 1e-4, "strafing turns only to the cone's edge");
    let back = facing_yaw(-Vec3::Z, view);
    assert!((back - v).abs() < 1e-4, "backing up keeps facing the view");
    let ahead = facing_yaw(Vec3::new(0.3, 0.0, 1.0), view);
    assert!((clawbound::abilities::facing_of(ahead) - Vec3::new(0.3, 0.0, 1.0).normalize()).length() < 1e-4, "inside the cone: face the move");
    assert_eq!(facing_yaw(Vec3::X, Vec3::ZERO), 0.0, "no view (a bot): face the move");
    assert!(BACKPEDAL > FACE_CONE);
}

/// The audio engine end to end, everything but the device: a real match, a
/// cat casting; its events through the cues; a folder of real .opus/.wav
/// files loaded in the background; the mixer rendering. The cat's own voice
/// line plays; with no specific cast sound on disk the generic one fills
/// in; and a cat far from the listener is not heard speaking.
#[test]
fn the_audio_engine_voices_a_cast_end_to_end() {
    use clawbound::cues::{cues_for, place, VoiceGate};
    use clawbound::mixer::{self, Play};
    use clawbound::soundbank::{Bank, Loader};
    use glam::Vec3;
    let dir = std::env::temp_dir().join(format!("cb_e2e_audio_{}", std::process::id()));
    let mut game = Game::new_teams(&[0], &[1]);
    let a = clawbound::cues::ancestor_key(&game, 0);
    std::fs::create_dir_all(dir.join(format!("voice/{a}"))).unwrap();
    std::fs::create_dir_all(dir.join("sfx")).unwrap();
    // a voice line: a 440 Hz tone, as Opus; a generic cast sound, as WAV
    let tone: Vec<f32> = (0..24_000).map(|i| (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5).collect();
    {
        use opus_pure::{Application, OggOpusWriter, OpusEncoder, OpusHead, MAX_PACKET_BYTES};
        let mut enc = OpusEncoder::new(48_000, 1, Application::Voip).unwrap();
        let head = OpusHead::for_encoder(&enc, 48_000);
        let mut out = Vec::new();
        {
            let mut w = OggOpusWriter::new(&mut out, head).unwrap();
            let mut pk = vec![0u8; MAX_PACKET_BYTES];
            for block in tone.chunks_exact(960) {
                let n = enc.encode(block, 960, &mut pk).unwrap();
                w.write_packet(&pk[..n]).unwrap();
            }
            w.finish().unwrap();
        }
        std::fs::write(dir.join(format!("voice/{a}/cast.opus")), out).unwrap();
    }
    std::fs::write(dir.join("sfx/cast.wav"), clawbound::brass::wav_bytes(&tone[..4800], 48_000)).unwrap();

    let (mut h, mut m) = mixer::new();
    let mut bank = Bank::loading(Loader::start(dir.clone(), 48_000));
    let t0 = std::time::Instant::now();
    while bank.len() < 2 && t0.elapsed().as_secs() < 10 {
        bank.poll(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(bank.len(), 2, "errors: {:?}", bank.errors);

    // a cast with a wind-up, so there is a CastStart (voice) and a Cast (sfx)
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    game.gods[0].mana = 10_000.0;
    let slot = (0..4u32).find(|&s| clawbound::abilities::cast_timing(&game.def(0).abilities[s as usize]).0 > 0.0).unwrap();
    let p = game.gods[0].pos;
    game.cast_queue.push((0, slot, p + Vec3::Z * 8.0));
    let mut gate = VoiceGate::default();
    let mut played = Vec::new();
    for _ in 0..60 {
        game.tick(1.0 / 60.0);
        let evs: Vec<_> = game.events.drain(..).collect();
        for ev in evs {
            for cue in cues_for(&ev, &game) {
                let Some((gain, pan)) = place(&cue, p, Vec3::X) else { continue };
                if !gate.allow(&cue, game.t) {
                    continue;
                }
                if let Some(id) = bank.first(&cue.keys) {
                    played.push(cue.keys.clone());
                    h.play(Play { clip: id, bus: cue.bus, gain, pan, priority: cue.priority, looped: false, speaker: cue.speaker.map(|x| x as u16) });
                }
            }
        }
    }
    assert!(played.iter().any(|k| k.iter().any(|x| x.ends_with("/cast"))), "the voice line did not play: {played:?}");
    assert!(played.iter().any(|k| k.last().map(String::as_str) == Some("sfx/cast")), "the generic cast sound did not fill in: {played:?}");
    let mut out = vec![0.0f32; 2 * 4096];
    m.render_add(&mut out, 2);
    let energy: f32 = out.iter().map(|x| x * x).sum();
    assert!(energy > 1.0, "nothing audible came out of the mixer: {energy}");

    // far away: the same cat's line is not spoken
    let far = clawbound::cues::Cue { pos: Some(p + Vec3::X * 80.0), ..cues_for(&clawbound::sim::Event::CastStart { god: 0, slot, aim: p, dur: 0.3 }, &game)[0].clone() };
    assert!(place(&far, p, Vec3::X).is_none());
    std::fs::remove_dir_all(&dir).ok();
}

/// The netplay stack over REAL UDP sockets (loopback): two sessions, two
/// games, real time. They stay in sync, keep advancing, and settle on the
/// minimum input delay, as on a LAN.
#[test]
fn netplay_over_real_udp_sockets_stays_in_sync_at_minimum_delay() {
    use clawbound::lockstep::Input;
    use clawbound::netplay::{Packet, Session, Tuning};
    use std::net::UdpSocket;
    let socks = [UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap()];
    for s in &socks {
        s.set_nonblocking(true).unwrap();
    }
    let addr = [socks[0].local_addr().unwrap(), socks[1].local_addr().unwrap()];
    let mut games = [Game::new_teams(&[0], &[1]), Game::new_teams(&[0], &[1])];
    for g in games.iter_mut() {
        for god in g.gods.iter_mut() {
            god.ai = None;
        }
    }
    let mut sess = [Session::new(0, 2, Tuning::TUNED), Session::new(1, 2, Tuning::TUNED)];
    let t0 = std::time::Instant::now();
    let mut next = [0.0f64, 0.004];
    let mut buf = [0u8; 1500];
    while t0.elapsed().as_secs_f64() < 2.5 {
        let now_ms = (t0.elapsed().as_secs_f64() * 1000.0) as u32;
        for m in 0..2 {
            while let Ok((n, _)) = socks[m].recv_from(&mut buf) {
                if let Some(p) = Packet::decode(&buf[..n]) {
                    sess[m].receive(&p, now_ms);
                }
            }
            if t0.elapsed().as_secs_f64() < next[m] {
                continue;
            }
            next[m] += 1.0 / 60.0;
            if sess[m].ready() {
                let tick = sess[m].lockstep.tick;
                let inp = Input { mv: [(((tick * 7 + m as u32 * 3) % 200) as i32 - 100) as i8, 40], attack: tick % 5 == 0, ..Default::default() };
                sess[m].stamp(inp, tick % 30 == 0, now_ms);
                sess[m].step(&mut games[m], 1.0 / 60.0, now_ms);
            }
            for p in sess[m].packets_for(1 - m, now_ms) {
                socks[m].send_to(&p.encode(), addr[1 - m]).unwrap();
            }
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    let ticks = [sess[0].lockstep.tick, sess[1].lockstep.tick];
    assert!(ticks[0] > 100 && ticks[1] > 100, "did not keep advancing: {ticks:?}");
    assert!(sess[0].stats.desync.is_none() && sess[1].stats.desync.is_none(), "desync");
    assert_eq!([sess[0].delay(), sess[1].delay()], [1, 1], "loopback should settle on 1 tick; rtt {:?}", sess[0].rtt);
    assert!(sess[0].rtt.samples > 50, "no round trips measured");
}

mod brawls {
    use clawbound::gods::{Archetype, Role, ROSTER};
    use clawbound::sim::Game;
    use glam::Vec3;

    fn id_where(f: impl Fn(&clawbound::gods::GodDef) -> bool) -> u32 {
        ROSTER.iter().position(|d| f(d)).expect("an ancestor") as u32
    }
    fn slot_of(g: &Game, i: usize, f: impl Fn(&Archetype) -> bool) -> Option<u32> {
        let d = &ROSTER[g.gods[i].def_id as usize];
        (0..4).find(|&s| f(&d.abilities[s].arch)).map(|s| s as u32)
    }
    fn quiet(g: &mut Game) {
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.move_dir = Vec3::ZERO;
            c.attack_held = false;
        }
    }
    fn run(g: &mut Game, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
        }
    }

    /// Two enemy cats walking into each other stop body to body: they do
    /// not pass through.
    #[test]
    fn cats_collide_body_to_body() {
        let mut g = Game::new_teams(&[0], &[1]);
        quiet(&mut g);
        let c = Vec3::new(0.0, 0.0, -40.0);
        g.gods[0].pos = c - Vec3::X * 3.0;
        g.gods[1].pos = c + Vec3::X * 3.0;
        g.gods[0].move_dir = Vec3::X;
        g.gods[1].move_dir = -Vec3::X;
        let mut closest = f32::MAX;
        for _ in 0..180 {
            g.tick(1.0 / 60.0);
            g.events.clear();
            let d = Vec3::new(g.gods[1].pos.x - g.gods[0].pos.x, 0.0, g.gods[1].pos.z - g.gods[0].pos.z).length();
            closest = closest.min(d);
        }
        assert!(closest > clawbound::brawl::BODY_R * 2.0 * 0.85, "they overlapped to {closest}");
        assert!(g.gods[0].pos.x < g.gods[1].pos.x, "they passed through each other");
    }

    /// A dash into an enemy stops on the body and knocks it back.
    #[test]
    fn a_dash_into_an_enemy_is_a_tackle() {
        let dasher = id_where(|d| d.abilities.iter().any(|a| matches!(a.arch, Archetype::Dash { .. })));
        let mut g = Game::new_teams(&[dasher], &[(dasher + 1) % 13]);
        quiet(&mut g);
        let c = Vec3::new(0.0, 0.0, -40.0);
        g.gods[0].pos = c;
        g.gods[0].mana = 10_000.0;
        g.gods[1].pos = c + Vec3::Z * 2.5;
        let before = g.gods[1].pos;
        let slot = slot_of(&g, 0, |a| matches!(a, Archetype::Dash { .. })).unwrap();
        g.cast_queue.push((0, slot, c + Vec3::Z * 9.0));
        run(&mut g, 0.6);
        assert!(g.gods[0].pos.z < before.z, "the dasher stopped short of passing through: {}", g.gods[0].pos.z);
        assert!(g.gods[1].pos.z > before.z + 0.4, "the target was knocked back: {} from {}", g.gods[1].pos.z, before.z);
    }

    /// A guardian's sweep catches two enemies side by side; an assassin's
    /// bite catches one -- but bites harder from behind.
    #[test]
    fn melee_differs_by_cat() {
        let hits = |who: u32, behind: bool| -> (u32, f32) {
            let mut g = Game::new_teams(&[who], &[0, 1]);
            quiet(&mut g);
            let c = Vec3::new(0.0, 0.0, -40.0);
            g.gods[0].pos = c;
            g.gods[0].yaw = std::f32::consts::FRAC_PI_2;
            let r = ROSTER[who as usize].auto_range * 0.7;
            g.gods[1].pos = c + Vec3::new(-0.35, 0.0, r);
            g.gods[2].pos = c + Vec3::new(0.35, 0.0, r);
            // facing away from the attacker (behind) or toward it
            for k in [1, 2] {
                g.gods[k].yaw = if behind { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 };
                g.gods[k].max_hp = 1.0e6;
                g.gods[k].hp = 1.0e6;
            }
            g.gods[0].attack_held = true;
            run(&mut g, 0.05);
            let hit = [1usize, 2].iter().filter(|&&k| g.gods[k].hp < 1.0e6).count() as u32;
            let most = [1usize, 2].iter().map(|&k| 1.0e6 - g.gods[k].hp).fold(0.0f32, f32::max);
            (hit, most)
        };
        let guardian = id_where(|d| d.role == Role::Guardian && d.auto_range < 4.5);
        let assassin = id_where(|d| d.role == Role::Assassin && d.auto_range < 4.5);
        assert_eq!(hits(guardian, false).0, 2, "the sweep catches both");
        assert_eq!(hits(assassin, false).0, 1, "the bite catches one");
        let (_, front) = hits(assassin, false);
        let (_, back) = hits(assassin, true);
        assert!(back > front * 1.25, "from behind {back} vs front {front}");
    }

    /// A nova into a crowd of cats deals each less than into one cat.
    #[test]
    fn area_damage_thins_in_a_crowd() {
        let nova = id_where(|d| d.abilities.iter().any(|a| matches!(a.arch, Archetype::Nova { .. }) && !a.ult));
        let per_cat = |crowd: usize| -> f32 {
            let reds: Vec<u32> = (0..crowd as u32).map(|k| (nova + 1 + k) % 13).collect();
            let mut g = Game::new_teams(&[nova], &reds);
            quiet(&mut g);
            let c = Vec3::new(0.0, 0.0, -40.0);
            g.gods[0].pos = c;
            g.gods[0].mana = 10_000.0;
            for k in 1..=crowd {
                let a = k as f32 / crowd as f32 * std::f32::consts::TAU;
                g.gods[k].pos = c + Vec3::new(a.cos(), 0.0, a.sin()) * 1.6;
                g.gods[k].max_hp = 1.0e6;
                g.gods[k].hp = 1.0e6;
            }
            let slot = slot_of(&g, 0, |a| matches!(a, Archetype::Nova { .. })).unwrap();
            g.cast_queue.push((0, slot, c));
            run(&mut g, 1.0);
            (1..=crowd).map(|k| 1.0e6 - g.gods[k].hp).sum::<f32>() / crowd as f32
        };
        let one = per_cat(1);
        let six = per_cat(6);
        assert!(one > 0.0);
        assert!(six < one * 0.75, "six caught: {six} each vs one alone: {one}");
    }

    /// Taking ability damage from an enemy cat returns mana (the siphon).
    #[test]
    fn the_siphon_pays_back_ability_damage_as_mana() {
        let bolt = id_where(|d| d.abilities.iter().any(|a| matches!(a.arch, Archetype::Bolt { .. })));
        let mut g = Game::new_teams(&[bolt], &[(bolt + 1) % 13]);
        quiet(&mut g);
        let c = Vec3::new(0.0, 0.0, -40.0);
        g.gods[0].pos = c;
        g.gods[0].mana = 10_000.0;
        g.gods[1].pos = c + Vec3::Z * 6.0;
        g.gods[1].mana = 0.0;
        g.gods[1].max_hp = 1.0e6;
        g.gods[1].hp = 1.0e6;
        let slot = slot_of(&g, 0, |a| matches!(a, Archetype::Bolt { .. })).unwrap();
        g.cast_queue.push((0, slot, c + Vec3::Z * 6.0));
        run(&mut g, 1.2);
        let dealt = 1.0e6 - g.gods[1].hp;
        assert!(dealt > 0.0, "the bolt hit");
        assert!(g.gods[1].mana > dealt * 0.05, "mana {} from {dealt} damage", g.gods[1].mana);
    }

    /// Winded, for real: a cat hit just before it lands from a leap cools
    /// twice as long as one that was not.
    #[test]
    fn a_winded_landing_cools_twice_as_long() {
        let landing_cd = |hurt: bool| -> f32 {
            let mut g = Game::new_teams(&[0], &[1]);
            quiet(&mut g);
            g.gods[1].respawn_t = 999.0;
            g.gods[0].pos = Vec3::new(0.0, 0.0, -40.0);
            g.leap_queue.push((0, Vec3::Z));
            for _ in 0..240 {
                if hurt {
                    g.gods[0].last_dmg_t = g.t; // under fire the whole way
                }
                let was = g.gods[0].leap.is_some();
                g.tick(1.0 / 60.0);
                g.events.clear();
                if was && g.gods[0].leap.is_none() {
                    return g.gods[0].leap_cd;
                }
            }
            panic!("never landed");
        };
        let (calm, hurt) = (landing_cd(false), landing_cd(true));
        assert!(hurt > calm * 1.8, "winded cooldown {hurt} vs {calm}");
    }

    /// A dash up the pass's rising lane stays on or above the ground the
    /// whole way (it went into the slope, then snapped up).
    #[test]
    fn a_dash_uphill_stays_above_the_ground() {
        clawbound::map::with(clawbound::map::MapId::Pass, || {
            let dasher = id_where(|d| d.abilities.iter().any(|a| matches!(a.arch, Archetype::Dash { .. })));
            let mut g = Game::new_teams(&[dasher], &[(dasher + 1) % 13]);
            quiet(&mut g);
            g.gods[1].respawn_t = 999.0;
            // find the ramp: along the Top lane where the ground climbs
            let pts = clawbound::pass::lane_polyline(clawbound::consts::Lane::Top);
            let mut foot = None;
            'find: for w in pts.windows(2) {
                for k in 0..40 {
                    let p = w[0].lerp(w[1], k as f32 / 40.0);
                    let q = w[0].lerp(w[1], (k + 4) as f32 / 40.0);
                    let (a, b) = (clawbound::map::ground_y(p.x, p.z), clawbound::map::ground_y(q.x, q.z));
                    if a < 1.0 && b > a + 1.0 {
                        foot = Some((p, (q - p).normalize()));
                        break 'find;
                    }
                }
            }
            let (p, up) = foot.expect("the ramp");
            g.gods[0].pos = Vec3::new(p.x, clawbound::map::ground_y(p.x, p.z), p.z);
            g.gods[0].mana = 10_000.0;
            let slot = slot_of(&g, 0, |a| matches!(a, Archetype::Dash { .. })).unwrap();
            g.cast_queue.push((0, slot, p + up * 10.0));
            let mut deepest = 0.0f32;
            let mut biggest_jump = 0.0f32;
            let mut last_y = g.gods[0].pos.y;
            for _ in 0..40 {
                g.tick(1.0 / 60.0);
                g.events.clear();
                let c = &g.gods[0];
                deepest = deepest.min(c.pos.y - clawbound::map::ground_y(c.pos.x, c.pos.z));
                biggest_jump = biggest_jump.max((c.pos.y - last_y).abs());
                last_y = c.pos.y;
            }
            assert!(deepest > -0.05, "the cat went {deepest} into the ground");
            assert!(biggest_jump < 1.0, "a teleport of {biggest_jump} u in one frame");
        });
    }

    /// A bolt fired from the top of the pass's ramp at a cat at its foot
    /// hits: it pitches toward the aimed height (it flew level, over the
    /// target's head, and the aim itself was on the plane y = 0).
    #[test]
    fn a_bolt_downhill_hits() {
        clawbound::map::with(clawbound::map::MapId::Pass, || {
            let shooter = id_where(|d| d.abilities.iter().any(|a| matches!(a.arch, Archetype::Bolt { .. })));
            let pts = clawbound::pass::lane_polyline(clawbound::consts::Lane::Top);
            // walk the lane: the last low point before the shelf, and the first high one
            let mut samples = Vec::new();
            for w in pts.windows(2) {
                for k in 0..60 {
                    let p = w[0].lerp(w[1], k as f32 / 60.0);
                    samples.push(Vec3::new(p.x, clawbound::map::ground_y(p.x, p.z), p.z));
                }
            }
            let top = samples.iter().position(|p| p.y > 8.5).expect("the shelf");
            let foot = samples[..top].iter().rposition(|p| p.y < 0.3).expect("the ramp's foot");
            let (hi, lo) = (samples[top], samples[foot]);
            let mut g = Game::new_teams(&[shooter], &[(shooter + 1) % 13]);
            quiet(&mut g);
            g.gods[0].pos = hi;
            g.gods[0].mana = 10_000.0;
            g.gods[1].pos = lo;
            g.gods[1].max_hp = 1.0e6;
            g.gods[1].hp = 1.0e6;
            let slot = slot_of(&g, 0, |a| matches!(a, Archetype::Bolt { .. })).unwrap();
            // aim as the camera now does: at the target's point on the ground
            g.cast_queue.push((0, slot, lo));
            run(&mut g, 2.0);
            assert!(g.gods[1].hp < 1.0e6, "the bolt missed the cat below ({} u down, {:.0} u away)", hi.y - lo.y, (hi - lo).length());
        });
    }
}

/// The fight brain in a real game (`tactics` decides, `ai` carries it out).
mod fights {
    use clawbound::consts::{tower_pos, Team, TOWER_RANGE};
    use clawbound::sim::{AiState, Game};
    use clawbound::tactics::Stance;
    use glam::Vec3;

    fn run(g: &mut Game, secs: f32, mut each: impl FnMut(&Game)) {
        for _ in 0..(secs * 60.0) as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
            each(g);
        }
    }

    /// Park a god at its fountain, out of the way and brainless.
    fn park(g: &mut Game, i: usize) {
        let f = g.gods[i].team.fountain();
        g.gods[i].ai = None;
        g.gods[i].pos = Vec3::new(f.x, 0.0, f.z);
        g.gods[i].move_dir = Vec3::ZERO;
        g.gods[i].attack_held = false;
    }

    /// Three on one: the three dive, the one gives ground.
    #[test]
    fn outnumbering_dives_and_outnumbered_regroups() {
        let mut g = Game::new_teams(&[0, 1, 2, 3], &[4]);
        park(&mut g, 0);
        for (i, p) in [(1, Vec3::new(-3.0, 0.0, 0.0)), (2, Vec3::new(-3.0, 0.0, 2.0)), (3, Vec3::new(-3.0, 0.0, -2.0)), (4, Vec3::new(4.0, 0.0, 0.0))] {
            g.gods[i].pos = p;
        }
        let mut dove = [false; 5];
        let mut gave = false;
        run(&mut g, 0.6, |g| {
            // a dive on the numbers, not the kill clause: the red cat is
            // still above half health
            let fresh = g.gods[4].hp / g.gods[4].max_hp > 0.5;
            for i in 1..4 {
                dove[i] |= fresh && g.gods[i].ai.as_ref().unwrap().stance == Stance::Dive;
            }
            let red = g.gods[4].ai.as_ref().unwrap();
            gave |= matches!(red.stance, Stance::Regroup | Stance::Retreat) || red.state == AiState::Retreat;
        });
        assert!(dove[1..4].iter().filter(|&&d| d).count() >= 2, "the three commit: {dove:?}");
        assert!(gave, "the one does not stand and trade");
    }

    /// Tower discipline: alone, a bot does not walk into an enemy tower's
    /// range to siege it; with its minions under the tower, it does.
    #[test]
    fn a_bot_sieges_only_behind_its_minions() {
        let setup = || {
            let mut g = Game::new_teams(&[0, 1], &[2]);
            park(&mut g, 0);
            park(&mut g, 2);
            let lane = g.gods[1].ai.as_ref().unwrap().lane;
            let tw = tower_pos(lane, Team::Red, false);
            let home = Team::Blue.fountain();
            let back = Vec3::new(home.x - tw.x, 0.0, home.z - tw.z).normalize();
            g.gods[1].pos = Vec3::new(tw.x, 0.0, tw.z) + back * (TOWER_RANGE + 6.0);
            let path = clawbound::consts::lane_path(lane, Team::Blue);
            let wp = (0..path.len()).min_by(|&a, &b| path[a].distance(tw).total_cmp(&path[b].distance(tw))).unwrap();
            g.gods[1].ai.as_mut().unwrap().wp = wp;
            (g, tw, lane)
        };
        let flat = |a: Vec3, b: Vec3| Vec3::new(a.x - b.x, 0.0, a.z - b.z).length();

        let (mut g, tw, _) = setup();
        let mut closest = f32::MAX;
        run(&mut g, 8.0, |g| closest = closest.min(flat(g.gods[1].pos, tw)));
        assert!(closest > TOWER_RANGE - 0.75, "alone, it waits at the edge: came within {closest:.1} of {TOWER_RANGE}");

        // now a wave of its own minions under the tower
        let (mut g, tw, lane) = setup();
        g.next_wave_t = g.t;
        run(&mut g, 0.05, |_| {});
        let back = (g.gods[1].pos - tw).normalize();
        let mut k = 0;
        for m in g.minions.iter_mut().filter(|m| m.team == Team::Blue && m.lane == lane) {
            m.pos = Vec3::new(tw.x, 0.0, tw.z) + back * (8.0 + k as f32 * 0.8);
            k += 1;
        }
        assert!(k >= 2, "a wave spawned: {k}");
        let mut closest = f32::MAX;
        run(&mut g, 8.0, |g| closest = closest.min(flat(g.gods[1].pos, tw)));
        assert!(closest < TOWER_RANGE - 4.0, "behind its minions it sieges: closest {closest:.1}");
    }
}

/// Lane minions (`minionai`): waves of four or five that hold a target,
/// walk to it, and turn on a cat that hurts one of theirs.
mod minions {
    use clawbound::consts::{Lane, Team};
    use clawbound::minionai::{Foe, LEASH};
    use clawbound::sim::Game;
    use glam::Vec3;

    fn quiet(g: &mut Game) {
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.move_dir = Vec3::ZERO;
            c.attack_held = false;
        }
    }
    fn run(g: &mut Game, secs: f32, mut each: impl FnMut(&Game)) {
        for _ in 0..(secs * 60.0) as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
            each(g);
        }
    }
    /// One wave spawned, then only the minions of `keep` survive, moved
    /// round `at` in a line along x.
    fn wave(g: &mut Game, keep: &[(Team, Lane)], at: Vec3) {
        g.next_wave_t = g.t;
        run(g, 1.0 / 60.0, |_| {});
        g.minions.retain(|m| keep.contains(&(m.team, m.lane)));
        for (k, m) in g.minions.iter_mut().enumerate() {
            m.pos = at + Vec3::new(k as f32 * 0.9, 0.0, 0.0);
            m.left_at = None;
        }
    }

    #[test]
    fn a_wave_is_four_every_third_five() {
        let mut g = Game::new_teams(&[0], &[1]);
        quiet(&mut g);
        let mut sizes = Vec::new();
        for _ in 0..3 {
            let before: Vec<u32> = g.minions.iter().map(|m| m.id).collect();
            g.next_wave_t = g.t;
            run(&mut g, 1.0 / 60.0, |_| {});
            let fresh = g.minions.iter().filter(|m| !before.contains(&m.id) && m.team == Team::Blue && m.lane == Lane::Mid).count();
            sizes.push(fresh);
        }
        assert_eq!(sizes, vec![4, 4, 5]);
        let ids: Vec<u32> = g.minions.iter().map(|m| m.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "the Vec stays sorted by id");
    }

    /// The old minion stood still once a god was within 5.5 u and could
    /// only hit what was within 2.6: a cat poking from 4 u was never hit.
    #[test]
    fn a_melee_minion_walks_to_a_cat_poking_it() {
        let mut g = Game::new_teams(&[0], &[1]);
        quiet(&mut g);
        wave(&mut g, &[(Team::Red, Lane::Mid)], Vec3::new(0.0, 0.0, 0.0));
        g.minions.retain(|m| !m.caster);
        g.minions.truncate(1);
        g.gods[0].pos = Vec3::new(4.0, 0.0, 0.0);
        let hp0 = g.gods[0].hp;
        run(&mut g, 3.0, |_| {});
        assert!(g.gods[0].hp < hp0, "the minion reached the cat and bit: {} -> {}", hp0, g.gods[0].hp);
    }

    /// Minions fight the wave, but hurt a cat of theirs and they turn on you.
    #[test]
    fn a_call_for_help_retargets_the_wave() {
        let mut g = Game::new_teams(&[0], &[1]);
        quiet(&mut g);
        wave(&mut g, &[(Team::Blue, Lane::Mid), (Team::Red, Lane::Mid)], Vec3::new(0.0, 0.0, 0.0));
        // blue and red minions side by side: they pick each other
        run(&mut g, 0.5, |_| {});
        let blue: Vec<usize> = (0..g.minions.len()).filter(|&i| g.minions[i].team == Team::Blue).collect();
        assert!(blue.iter().all(|&i| matches!(g.minions[i].foe, Some(Foe::Minion(_)))), "the wave fights the wave");
        // the red cat (god 1) hits the blue cat (god 0) beside the minions
        g.gods[0].pos = Vec3::new(2.0, 0.0, 3.0);
        g.gods[1].pos = Vec3::new(3.0, 0.0, 4.5);
        g.gods[0].recent_damagers.push((1, g.t));
        let mut turned = 0;
        run(&mut g, 0.5, |g| {
            turned = turned.max(g.minions.iter().filter(|m| m.team == Team::Blue && m.foe == Some(Foe::God(1))).count());
        });
        assert!(turned >= 3, "most of the blue wave turned on the red cat: {turned}");
    }

    /// A minion chases a cat only so far from its lane, then walks back.
    #[test]
    fn a_chase_ends_at_the_leash() {
        let mut g = Game::new_teams(&[0], &[1]);
        quiet(&mut g);
        wave(&mut g, &[(Team::Red, Lane::Mid)], Vec3::new(0.0, 0.0, 0.0));
        g.minions.retain(|m| !m.caster);
        g.minions.truncate(1);
        let start = g.minions[0].pos;
        g.gods[0].pos = Vec3::new(3.0, 0.0, 0.0);
        // the cat walks steadily away along +x, a little slower than a
        // minion walks (so only the leash ends the chase)
        let mut far = 0.0f32;
        for _ in 0..(8.0 * 60.0) as usize {
            g.gods[0].pos.x += 5.0 / 60.0;
            g.tick(1.0 / 60.0);
            g.events.clear();
            // measured while it still holds the cat (afterwards it walks on
            // down its lane, which may well lead the same way)
            if let Some(m) = g.minions.first().filter(|m| m.foe == Some(Foe::God(0))) {
                far = far.max(Vec3::new(m.pos.x - start.x, 0.0, m.pos.z - start.z).length());
            }
        }
        assert!(far > 4.0, "it did chase: {far}");
        assert!(far < LEASH + 1.0, "but not past the leash: {far}");
        let m = &g.minions[0];
        assert!(m.foe != Some(Foe::God(0)), "and gave the cat up");
    }
}

/// Cloud ferries on Starwell Glacier (`ferry`): leap on, ride, walk off at
/// the far pier; a fall into the cloud sea is a trip home, not a death.
mod ferries {
    use clawbound::ferry::{CLOUD_R, SKYFALL_COST, SKYFALL_SECS};
    use clawbound::map::{self, MapId};
    use clawbound::sim::Game;
    use glam::Vec3;

    fn sky_game() -> Game {
        let mut g = Game::new_teams(&[0], &[1]);
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.move_dir = Vec3::ZERO;
            c.attack_held = false;
        }
        // the real floor as collision: the sky map's edges are what stops a
        // cat walking into the sky
        let mut b = clawbound::blockers::Blockers::new(400.0, 6.0);
        b.set_floor(clawbound::sky::floor());
        g.blockers = std::sync::Arc::new(b);
        // the red cat far away at home
        let f = g.gods[1].team.fountain();
        g.gods[1].pos = f;
        g
    }

    fn tick(g: &mut Game, secs: f32) {
        for _ in 0..(secs * 60.0).round() as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
        }
    }

    #[test]
    fn leap_on_ride_across_walk_off() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let f = g.ferries[0];
            let (a, b) = (f.a, f.b);
            let dir = (b - a).normalize();
            // wait on the pier, a little back from the edge, until the cloud
            // has just docked here
            g.gods[0].pos = a - dir * 2.5;
            while !(f.at(g.t).docked == Some(0) && f.at(g.t - 0.1).docked.is_none()) {
                tick(&mut g, 1.0 / 60.0);
                g.gods[0].pos = a - dir * 2.5;
            }
            // walking at it does not board: the floor ends at the pier
            g.gods[0].move_dir = dir;
            tick(&mut g, 0.6);
            assert!(g.gods[0].riding.is_none(), "walked onto a cloud");
            g.gods[0].move_dir = Vec3::ZERO;
            // leap
            g.leap_queue.push((0, dir));
            tick(&mut g, 1.0);
            assert_eq!(g.gods[0].riding, Some(0), "the leap landed on the cloud");
            // ride it: standing still, carried with it -- where it stands on
            // the deck does not change as the cloud drifts
            while f.at(g.t).docked.is_some() {
                tick(&mut g, 1.0 / 60.0);
            }
            let spot = g.gods[0].pos - f.at(g.t).pos;
            tick(&mut g, 1.5);
            let drift = (g.gods[0].pos - f.at(g.t).pos - spot).length();
            assert!(drift < 0.2, "carried, not dragged: moved {drift} on the deck");
            // then stays on the deck while trying to walk off the side into
            // the sky
            let side = Vec3::new(-dir.z, 0.0, dir.x);
            g.gods[0].move_dir = side;
            let mut far = 0.0f32;
            while f.at(g.t).docked != Some(1) {
                tick(&mut g, 1.0 / 60.0);
                let deck = f.at(g.t).pos;
                far = far.max(Vec3::new(g.gods[0].pos.x - deck.x, 0.0, g.gods[0].pos.z - deck.z).length());
                assert_eq!(g.gods[0].riding, Some(0), "thrown off mid-crossing at t {}", g.t);
                assert!(g.t < 200.0);
            }
            assert!(far <= CLOUD_R + 1e-3, "kept on the deck: {far}");
            assert!((g.gods[0].pos - b).length() < CLOUD_R * 2.0, "carried to the far pier: {}", g.gods[0].pos);
            // walk off onto the far island
            g.gods[0].move_dir = dir;
            tick(&mut g, 1.5);
            assert!(g.gods[0].riding.is_none(), "stepped off at the dock");
            assert!(clawbound::sky::floor().fits(g.gods[0].pos.x, g.gods[0].pos.z, 0.3), "on the island: {}", g.gods[0].pos);
        });
    }

    #[test]
    fn no_leap_into_empty_sky() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let f = g.ferries[0];
            let dir = (f.b - f.a).normalize();
            // wait until the cloud is away at the far pier
            while f.at(g.t).docked != Some(1) {
                tick(&mut g, 0.1);
            }
            g.gods[0].pos = f.a - dir * 1.5;
            g.leap_queue.push((0, dir));
            tick(&mut g, 1.2);
            assert!(g.gods[0].skyfall.is_none() && g.gods[0].riding.is_none());
            assert!(clawbound::sky::floor().fits(g.gods[0].pos.x, g.gods[0].pos.z, 0.3), "still on the island");
        });
    }

    #[test]
    fn a_fall_into_the_clouds_is_a_trip_home_not_a_death() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            g.gods[0].pos = Vec3::new(0.0, 0.0, -95.0); // over the chasm
            g.gods[0].skyfall = Some(SKYFALL_SECS);
            let hp0 = g.gods[0].hp;
            tick(&mut g, SKYFALL_SECS * 0.5);
            assert!(g.gods[0].pos.y < -5.0, "falling: {}", g.gods[0].pos.y);
            tick(&mut g, SKYFALL_SECS * 0.5 + 0.1);
            let home = g.gods[0].team.fountain();
            assert!(g.gods[0].skyfall.is_none());
            assert!(Vec3::new(g.gods[0].pos.x - home.x, 0.0, g.gods[0].pos.z - home.z).length() < 3.0, "home: {}", g.gods[0].pos);
            let lost = (hp0 - g.gods[0].hp) / g.gods[0].max_hp;
            assert!(lost > SKYFALL_COST - 0.05 && lost < SKYFALL_COST + 0.05, "lost {lost}");
            assert_eq!(g.gods[0].deaths, 0);
            assert_eq!(g.gods[1].kills, 0);
        });
    }
}

/// The Moonwell (`moonwell`): kill the Moonwyrm, claim the well, wade in
/// for a boon; everyone who wades mends.
mod moonwell {
    use clawbound::bosses::BossKind;
    use clawbound::map::{self, MapId};
    use clawbound::moonwell::{Boon, BOON_SECS, CLAIM_SECS};
    use clawbound::sim::{Game, TargetRef, Team};
    use glam::Vec3;

    fn sky_game() -> Game {
        let mut g = Game::new_teams(&[0, 2], &[1]);
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.move_dir = Vec3::ZERO;
            c.attack_held = false;
        }
        for i in 0..g.gods.len() {
            let f = g.gods[i].team.fountain();
            g.gods[i].pos = f;
        }
        g
    }
    fn tick(g: &mut Game, secs: f32) {
        for _ in 0..(secs * 60.0).round() as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
        }
    }
    fn wyrm(g: &Game) -> usize {
        g.bosses.list.iter().position(|b| b.kind == BossKind::Moonwyrm).expect("the sky map has a Moonwyrm")
    }

    #[test]
    fn only_the_sky_has_a_moonwyrm() {
        let g = Game::new_teams(&[0], &[1]);
        assert!(g.bosses.list.iter().all(|b| b.kind != BossKind::Moonwyrm));
        map::with(MapId::Sky, || {
            let g = sky_game();
            assert!(g.bosses.list[wyrm(&g)].home.length() < 1e-3, "in the middle of the well");
        });
    }

    #[test]
    fn kill_the_wyrm_claim_the_well_wade_for_a_boon() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let w = wyrm(&g);
            g.bosses.list[w].spawn_t = g.t;
            tick(&mut g, 0.1);
            assert!(g.bosses.list[w].alive);
            // the blue cat lands the killing blow
            g.bosses.list[w].hp = 10.0;
            g.deal_damage(TargetRef::Boss(w), 50.0, false, Some(0), 0.0, 0.0);
            assert!(!g.bosses.list[w].alive);
            assert!((g.bosses.claim_t[Team::Blue as usize] - CLAIM_SECS).abs() < 1e-3);
            assert_eq!(g.bosses.claim_t[Team::Red as usize], 0.0);
            // wade in, hurt: mend, and a boon
            g.gods[0].pos = Vec3::new(3.0, 0.0, 2.0);
            g.gods[0].hp = g.gods[0].max_hp * 0.5;
            // the red cat wades too: mends, no boon
            g.gods[2].pos = Vec3::new(-3.0, 0.0, 2.0);
            g.gods[2].hp = g.gods[2].max_hp * 0.5;
            // (the wyrm is gone; nothing else in the water hurts)
            tick(&mut g, 2.0);
            let (b, left) = g.gods[0].boon.expect("a boon for the claimer");
            assert!(left > BOON_SECS - 2.5 && left <= BOON_SECS);
            assert!(g.gods[0].hp > g.gods[0].max_hp * 0.55, "the claimer mends fast");
            assert!(g.gods[2].boon.is_none(), "no boon for the other side");
            assert!(g.gods[2].hp > g.gods[2].max_hp * 0.52, "but the water mends anyone");
            // once per claim: out, boon runs out, back in -- nothing new
            g.gods[0].boon = None;
            tick(&mut g, 1.0);
            assert!(g.gods[0].boon.is_none(), "one boon per cat per claim (had {b:?})");
            // the second blue cat still gets its own
            g.gods[1].pos = Vec3::new(0.0, 0.0, -3.0);
            tick(&mut g, 0.5);
            assert!(g.gods[1].boon.is_some());
        });
    }

    #[test]
    fn moonshell_turns_nine_tenths_and_quickening_halves_cooldowns() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let hp = g.gods[0].hp;
            g.deal_damage(TargetRef::God(0), 100.0, false, None, 0.0, 0.0);
            let plain = hp - g.gods[0].hp;
            g.gods[0].hp = hp;
            g.gods[0].boon = Some((Boon::Moonshell, 60.0));
            g.deal_damage(TargetRef::God(0), 100.0, false, None, 0.0, 0.0);
            let shelled = hp - g.gods[0].hp;
            assert!((shelled / plain - 0.1).abs() < 0.01, "{shelled} of {plain}");
            // cooldowns
            g.gods[0].boon = Some((Boon::Quickening, 60.0));
            g.gods[1].boon = None;
            g.gods[0].cds = [10.0; 4];
            g.gods[1].cds = [10.0; 4];
            tick(&mut g, 2.0);
            let (q, p) = (10.0 - g.gods[0].cds[0], 10.0 - g.gods[1].cds[0]);
            assert!((q / p - 2.0).abs() < 0.05, "twice as fast: {q} vs {p}");
        });
    }

    /// The Moonwyrm keeps a ring of stars and throws them at cats near the
    /// well -- either side -- and they home in; the ring gathers back.
    #[test]
    fn the_wyrm_throws_its_stars() {
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let w = wyrm(&g);
            g.bosses.list[w].spawn_t = g.t;
            tick(&mut g, 0.1);
            assert_eq!(g.bosses.list[w].stars, clawbound::moonwell::STARS);
            // a blue cat on the rim, a red one across the well, both out of
            // its bite; both keep circling so it is homing that hits
            g.gods[0].pos = Vec3::new(26.0, 0.0, 0.0);
            g.gods[2].pos = Vec3::new(-26.0, 0.0, 6.0);
            let (h0, h2) = (g.gods[0].hp, g.gods[2].hp);
            let mut thrown = 0;
            let mut least = u8::MAX;
            for k in 0..(8 * 60) {
                let a = k as f32 / 60.0 * 0.35; // ~9 u/s, a running cat
                g.gods[0].pos = Vec3::new(26.0 * a.cos(), 0.0, 26.0 * a.sin());
                g.tick(1.0 / 60.0);
                g.events.clear();
                thrown = thrown.max(g.star_bolts.len());
                least = least.min(g.bosses.list[w].stars);
            }
            assert!(thrown >= 1 && least < clawbound::moonwell::STARS, "stars thrown (in flight at once: {thrown}, ring down to {least})");
            assert!(g.gods[0].hp < h0 || g.gods[2].hp < h2, "and they struck: {h0} -> {}, {h2} -> {}", g.gods[0].hp, g.gods[2].hp);
            // everyone leaves: nothing more thrown, the ring gathers back
            for c in g.gods.iter_mut() {
                c.pos = c.team.fountain();
            }
            tick(&mut g, clawbound::moonwell::STAR_REGEN * clawbound::moonwell::STARS as f32 + 1.0);
            assert_eq!(g.bosses.list[w].stars, clawbound::moonwell::STARS);
            assert!(g.star_bolts.is_empty());
        });
    }

    /// Every few minutes the Moonwyrm flies a loop over the map (not while
    /// it is being fought), untouchable, letting stars fall on marked spots;
    /// a cat standing on a mark is burned; then it settles back in the well.
    #[test]
    fn the_wyrm_takes_wing_and_lets_its_stars_fall() {
        use clawbound::wyrmflight::{FLIGHT_SECS, TELEGRAPH};
        map::with(MapId::Sky, || {
            let mut g = sky_game();
            let w = wyrm(&g);
            g.bosses.list[w].spawn_t = g.t;
            tick(&mut g, 0.1);
            assert!(g.bosses.list[w].alive && g.bosses.list[w].flight.is_none());
            // due, but struck a moment ago: it stays to fight
            g.bosses.list[w].next_flight_t = g.t;
            g.bosses.list[w].last_dmg_t = g.t;
            tick(&mut g, 2.0);
            assert!(g.bosses.list[w].flight.is_none(), "no flight mid-fight");
            // left alone, it takes wing
            tick(&mut g, clawbound::wyrmflight::CALM_SECS);
            assert!(g.bosses.list[w].flight.is_some(), "aloft once calm");
            let hp = g.bosses.list[w].hp;
            g.deal_damage(TargetRef::Boss(w), 500.0, false, Some(0), 0.0, 0.0);
            assert_eq!(g.bosses.list[w].hp, hp, "untouchable aloft");
            // fly it out: the blue cat steps onto the first mark it sees
            let (mut marks, mut bursts, mut far) = (0usize, 0usize, 0.0f32);
            let (mut mark, mut burn) = (None, None);
            while g.bosses.list[w].flight.is_some() {
                let before = g.gods[0].hp;
                g.tick(1.0 / 60.0);
                use clawbound::sim::Event;
                if let Some(m) = mark {
                    if g.events.iter().any(|e| matches!(e, Event::StarfallFx { at, .. } if *at == m)) {
                        burn.get_or_insert(before - g.gods[0].hp);
                    }
                }
                marks += g.events.iter().filter(|e| matches!(e, Event::RingTelegraph { .. })).count();
                bursts += g.events.iter().filter(|e| matches!(e, Event::StarfallFx { .. })).count();
                g.events.clear();
                if mark.is_none() {
                    if let Some(s) = g.starfalls.last() {
                        assert!(s.left > TELEGRAPH - 0.1, "marked well before it lands");
                        g.gods[0].pos = s.at;
                        mark = Some(s.at);
                    }
                }
                far = far.max(Vec3::new(g.bosses.list[w].pos.x, 0.0, g.bosses.list[w].pos.z).length());
                assert!(g.t < 200.0, "it came back");
            }
            assert!(far > 150.0, "it flew out over the map: {far}");
            assert!((30..=120).contains(&marks), "stars fell over a {FLIGHT_SECS} s flight: {marks}");
            assert!(bursts + g.starfalls.len() == marks, "each landed: {bursts} (+{} falling) of {marks}", g.starfalls.len());
            // the frame its star landed, the cat standing on the mark lost at
            // least a star's worth (from anything else too, but mostly that)
            let burn = burn.expect("the marked star landed");
            assert!(burn > 100.0, "the cat on the mark was burned: {burn}");
            // home again, and the next flight is some way off
            let b = &g.bosses.list[w];
            assert!(b.pos.distance(b.home) < 1e-3);
            assert!(b.next_flight_t > g.t + 60.0);
        });
    }

    /// A bot whose team holds the claim walks to the well for its boon.
    #[test]
    fn a_bot_goes_to_the_well_for_its_boon() {
        map::with(MapId::Sky, || {
            let mut b = clawbound::blockers::Blockers::new(400.0, 6.0);
            b.set_floor(clawbound::sky::floor());
            let mut g = sky_game();
            g.blockers = std::sync::Arc::new(b);
            let lane = clawbound::consts::Lane::Top;
            g.gods[1].ai = Some(clawbound::sim::AiBrain::new(lane, 0.7));
            // on the west lane, level with the well, beside its star-bridge
            g.gods[1].pos = Vec3::new(-205.0, 0.0, 8.0);
            g.bosses.claims = 1;
            g.bosses.claim_id[0] = 1;
            g.bosses.claim_t[0] = CLAIM_SECS;
            let d0 = g.gods[1].pos.length();
            tick(&mut g, 20.0);
            let d1 = g.gods[1].pos.length();
            assert!(d1 < d0 - 60.0, "headed for the well: {d0} -> {d1}");
        });
    }
}

/// Cometail's Falling Star (`Archetype::Meteor`) and the dash damage that
/// every Dash in the roster described and none dealt.
mod cometail {
    use clawbound::gods::{Archetype, ROSTER};
    use clawbound::sim::Game;
    use glam::Vec3;

    fn quiet(g: &mut Game) {
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.move_dir = Vec3::ZERO;
            c.attack_held = false;
        }
    }
    fn run(g: &mut Game, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
        }
    }
    fn cometail() -> u32 {
        ROSTER.iter().position(|d| d.name == "Cometail").expect("Cometail is on the roster") as u32
    }
    fn slot(id: u32, f: impl Fn(&Archetype) -> bool) -> u32 {
        (0..4).find(|&s| f(&ROSTER[id as usize].abilities[s].arch)).unwrap() as u32
    }

    #[test]
    fn the_falling_star_vaults_high_and_lands_where_aimed_in_a_blast() {
        let c = cometail();
        let mut g = Game::new_teams(&[c], &[0, 1]);
        quiet(&mut g);
        g.gods[0].pos = Vec3::ZERO;
        g.gods[0].mana = 9999.0;
        // two enemies at the mark, one well outside it
        let mark = Vec3::new(0.0, 0.0, 16.0);
        g.gods[1].pos = mark + Vec3::new(1.5, 0.0, 0.0);
        g.gods[2].pos = mark + Vec3::new(14.0, 0.0, 0.0);
        let (h1, h2) = (g.gods[1].hp, g.gods[2].hp);
        let s = slot(c, |a| matches!(a, Archetype::Meteor { .. }));
        g.cast_queue.push((0, s, mark));
        let mut top = 0.0f32;
        for _ in 0..(2.5 * 60.0) as usize {
            g.tick(1.0 / 60.0);
            g.events.clear();
            top = top.max(g.gods[0].pos.y);
        }
        assert!(top > 10.0, "up into the sky: {top}");
        assert!((g.gods[0].pos - mark).length() < 1.5, "down on the mark: {}", g.gods[0].pos);
        assert!(g.gods[1].hp < h1 - 150.0, "the blast: {} -> {}", h1, g.gods[1].hp);
        assert_eq!(g.gods[2].hp, h2, "outside the blast, untouched");
    }

    #[test]
    fn a_dash_hurts_what_it_passes_through() {
        let c = cometail();
        let mut g = Game::new_teams(&[c], &[0]);
        quiet(&mut g);
        g.gods[0].pos = Vec3::ZERO;
        g.gods[0].mana = 9999.0;
        g.gods[1].pos = Vec3::new(0.0, 0.0, 6.0);
        let h = g.gods[1].hp;
        let s = slot(c, |a| matches!(a, Archetype::Dash { .. }));
        g.cast_queue.push((0, s, Vec3::new(0.0, 0.0, 20.0)));
        run(&mut g, 1.0);
        assert!(g.gods[1].hp < h - 50.0, "the pounce scorched what it passed: {} -> {}", h, g.gods[1].hp);
    }
}

/// The sky map's siege surge: a lane whose enemy towers have fallen sends
/// harder waves down it.
mod surge {
    use clawbound::consts::{Lane, Team};
    use clawbound::map::{self, MapId};
    use clawbound::sim::Game;

    #[test]
    fn fallen_towers_empower_the_lane() {
        map::with(MapId::Sky, || {
            let mut g = Game::new_teams(&[0], &[1]);
            assert_eq!(g.lane_surge(Team::Blue, Lane::Top), 1.0);
            // red loses its west outer tower
            let i = g.towers.iter().position(|t| t.team == Team::Red && t.lane == Lane::Top && !t.inner).unwrap();
            g.towers.remove(i);
            let per = MapId::Sky.siege_surge();
            assert!((g.lane_surge(Team::Blue, Lane::Top) - (1.0 + per)).abs() < 1e-5, "blue's west waves surge");
            assert_eq!(g.lane_surge(Team::Blue, Lane::Bot), 1.0, "not the east");
            assert_eq!(g.lane_surge(Team::Red, Lane::Top), 1.0, "not red's");
            // and the next wave down that lane is tougher
            g.next_wave_t = g.t;
            g.tick(1.0 / 60.0);
            let west = g.minions.iter().find(|m| m.team == Team::Blue && m.lane == Lane::Top).unwrap().max_hp;
            let east = g.minions.iter().find(|m| m.team == Team::Blue && m.lane == Lane::Bot).unwrap().max_hp;
            assert!((west / east - (1.0 + per)).abs() < 1e-3, "{west} vs {east}");
        });
        let g = Game::new_teams(&[0], &[1]);
        assert_eq!(clawbound::map::MapId::Jungle.siege_surge(), 0.0);
        assert_eq!(g.lane_surge(Team::Blue, Lane::Mid), 1.0);
    }
}

/// A wide wave's outer ranks: a minion marching eight u to one side of a
/// tower on the centre-line still turns on it.
mod wide_waves {
    use clawbound::consts::{tower_pos, Lane, Team};
    use clawbound::map::{self, MapId};
    use clawbound::minionai::Foe;
    use clawbound::sim::Game;
    use glam::Vec3;

    #[test]
    fn an_outer_rank_sees_the_tower_it_passes() {
        map::with(MapId::Sky, || {
            let mut g = Game::new_teams(&[0], &[1]);
            for c in g.gods.iter_mut() {
                c.ai = None;
                c.pos = c.team.fountain();
            }
            g.next_wave_t = g.t;
            g.tick(1.0 / 60.0);
            g.minions.retain(|m| m.team == Team::Blue && m.lane == Lane::Top && !m.caster);
            g.minions.truncate(1);
            let tw = tower_pos(Lane::Top, Team::Red, false);
            // twelve u off, level with it across the lane
            g.minions[0].pos = Vec3::new(tw.x - 12.0, 0.0, tw.z);
            g.minions[0].think_t = 0.0;
            g.tick(1.0 / 60.0);
            assert!(matches!(g.minions[0].foe, Some(Foe::Tower(_))), "{:?}", g.minions[0].foe);
        });
    }
}

mod clouds {
    use clawbound::gods::ROSTER;
    use clawbound::map::{self, MapId};
    use clawbound::sim::{Game, Team};
    use clawbound::skycloud::{self, Kind};
    use glam::Vec3;

    /// A bank well away from both bases and every tower, thick, and when.
    fn a_bank(g: &Game) -> (f32, skycloud::Cloud) {
        let mut t = skycloud::FIRST_CLOUD;
        loop {
            t += 5.0;
            assert!(t < 3600.0, "no bank in an hour");
            let found = skycloud::clouds(g.cloud_seed, t).into_iter().find(|c| {
                c.kind == Kind::Bank
                    && c.thick > 0.95
                    && c.pos.length() < 250.0
                    && [Team::Blue, Team::Red].iter().all(|tm| tm.base_pos().distance(c.pos) > c.r + 60.0)
                    && g.towers.iter().all(|tw| tw.pos.distance(c.pos) > 40.0)
            });
            if let Some(c) = found {
                return (t, c);
            }
        }
    }

    /// A cat in a bank cannot be seen or auto-attacked from outside it, nor
    /// from across it; at arm's length it can; stepping out into clear air
    /// it is seen again. Only on the sky map.
    #[test]
    fn a_cloud_bank_hides_a_cat_from_the_other_team() {
        // a blue cat that attacks from range
        let ranged = ROSTER.iter().position(|d| d.auto_range > 8.0).expect("a ranged cat") as u32;
        map::with(MapId::Sky, || {
            let mut g = Game::new_teams(&[ranged], &[1]);
            for c in g.gods.iter_mut() {
                c.ai = None;
                c.is_player = false;
            }
            let (t, bank) = a_bank(&g);
            g.t = t;
            let (blue, red) = (0usize, 1usize);
            let range = g.def(blue).auto_range;
            // red in the heart of the bank, blue just inside its reach but
            // well past arm's length
            g.gods[red].pos = bank.pos;
            g.gods[blue].pos = bank.pos + Vec3::X * (range - 1.0).max(skycloud::CLOSE + 2.0);
            g.vision_tick();
            assert!(!g.sees(Team::Blue, red), "hidden in the bank");
            assert!(g.sees(Team::Blue, blue) && g.sees(Team::Red, red), "a team always sees its own");
            // blue swings at the cloud: nothing to lock onto
            let hp = g.gods[red].hp;
            g.gods[blue].attack_held = true;
            for _ in 0..90 {
                g.gods[red].pos = bank.pos;
                g.gods[blue].pos = bank.pos + Vec3::X * (range - 1.0).max(skycloud::CLOSE + 2.0);
                g.tick(1.0 / 60.0);
                g.events.clear();
            }
            assert_eq!(g.gods[red].hp, hp, "no auto-attack on what cannot be seen");
            // arm's length: seen
            g.gods[blue].pos = bank.pos + Vec3::X * 2.0;
            g.vision.next_t = 0.0;
            g.vision_tick();
            assert!(g.sees(Team::Blue, red), "at arm's length, seen");
            // out in clear air, beside blue: seen
            let out = bank.pos + Vec3::Z * (bank.r * 1.5 + 30.0);
            assert!(skycloud::density(&skycloud::clouds(g.cloud_seed, g.t), out.x, out.z) < 0.1, "the test's clear air is clear");
            g.gods[red].pos = out;
            g.gods[blue].pos = out + Vec3::X * 20.0;
            g.vision.next_t = 0.0;
            g.vision_tick();
            assert!(g.sees(Team::Blue, red), "in clear air, seen");
        });
        // the jungle has no such cloud
        let mut g = Game::new_teams(&[0], &[1]);
        g.t = 1000.0;
        g.vision_tick();
        assert!(g.sees(Team::Blue, 1) && g.sees(Team::Red, 0));
    }

    /// Bots do not go for a cat the cloud hides.
    #[test]
    fn a_bot_does_not_chase_what_it_cannot_see() {
        map::with(MapId::Sky, || {
            let mut g = Game::new_teams(&[0], &[1]);
            let (t, bank) = a_bank(&g);
            g.t = t;
            g.gods[0].is_player = false;
            g.gods[0].ai = Some(clawbound::sim::AiBrain::new(clawbound::consts::Lane::Top, 1.0));
            g.gods[1].ai = None;
            g.gods[1].pos = bank.pos;
            g.gods[0].pos = bank.pos + Vec3::X * 12.0;
            g.vision_tick();
            assert!(!g.sees(Team::Blue, 1));
            clawbound::ai::think(&mut g, 0);
            let tgt = g.gods[0].ai.as_ref().unwrap().target;
            assert_ne!(tgt, Some(clawbound::sim::TargetRef::God(1)), "the bot did not pick the hidden cat");
            // the same, in clear air: it does
            g.gods[1].pos = g.gods[0].pos + Vec3::X * 6.0 + Vec3::Z * 0.0;
            // beside one of blue's own towers, in clear air
            let cs = skycloud::clouds(g.cloud_seed, g.t);
            let clear = g.towers.iter().filter(|tw| tw.team == Team::Blue).flat_map(|tw| [tw.pos + Vec3::X * 8.0, tw.pos - Vec3::X * 8.0, tw.pos + Vec3::Z * 8.0, tw.pos - Vec3::Z * 8.0]).find(|p| skycloud::density(&cs, p.x, p.z) < 0.01 && clawbound::sky::floor().sdf(p.x, p.z) < -2.0 && clawbound::sky::floor().sdf(p.x + 6.0, p.z) < -2.0);
            {
                let p = clear.expect("clear air somewhere");
                g.gods[0].pos = p;
                g.gods[1].pos = p + Vec3::X * 6.0;
                g.vision.next_t = 0.0;
                g.vision_tick();
                assert!(g.sees(Team::Blue, 1));
                clawbound::ai::think(&mut g, 0);
                assert_eq!(g.gods[0].ai.as_ref().unwrap().target, Some(clawbound::sim::TargetRef::God(1)), "in clear air it goes for it");
            }
        });
    }
}
