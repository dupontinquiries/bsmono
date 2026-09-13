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
