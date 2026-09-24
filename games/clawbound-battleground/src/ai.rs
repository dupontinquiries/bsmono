// AI for the 9 non-player gods: lane pushing, target selection, strafing,
// ability heuristics per archetype, retreat + fountain heal, auto-buy.

use crate::consts::*;
use crate::gods::Archetype;
use crate::sim::*;
use glam::Vec3;

impl Game {
    pub fn ai_move_target(&self, gi: usize) -> Vec3 {
        let g = &self.gods[gi];
        let brain = g.ai.as_ref().unwrap();
        match brain.state {
            AiState::Retreat => {
                let f = g.team.fountain();
                Vec3::new(f.x, 0.0, f.z)
            }
            AiState::Fight => {
                // handled directly via move_dir (orbit strafe)
                g.pos
            }
            AiState::Push => {
                let path = brain.lane.path_for(g.team);
                let mut wp = brain.wp.min(path.len() - 1);
                // advance waypoint when close
                while wp + 1 < path.len() && g.pos.distance(path[wp]) < 6.0 {
                    wp += 1;
                }
                path[wp]
            }
        }
    }
}

pub fn think(game: &mut Game, gi: usize) {
    let t = game.t;
    let g = &game.gods[gi];
    let team = g.team;
    let def = game.def(gi);
    let hp_frac = g.hp / g.max_hp;
    let retreat_at = 0.30 - g.ai.as_ref().unwrap().aggression * 0.06;

    let brain_state = g.ai.as_ref().unwrap().state;
    let mut state = brain_state;

    // --- state transitions -----------------------------------------------------
    if brain_state != AiState::Retreat && hp_frac < retreat_at {
        state = AiState::Retreat;
    }
    if brain_state == AiState::Retreat {
        let f = team.fountain();
        let healed = hp_frac > 0.9 || g.pos.distance(f) < FOUNTAIN_RADIUS && hp_frac > 0.85;
        if healed {
            state = AiState::Push;
        }
    }

    // --- target acquisition ------------------------------------------------------
    let aggro = AGGRO_RANGE * (0.8 + g.ai.as_ref().unwrap().aggression * 0.4) * game.weather.vision_mul();
    let mut target: Option<TargetRef> = None;
    let mut best_score = f32::MIN;

    for j in 0..game.gods.len() {
        let e = &game.gods[j];
        if e.team != team && e.respawn_t <= 0.0 {
            let d = e.pos.distance(g.pos);
            if d < aggro {
                let score = 100.0 - d - (e.hp / e.max_hp) * 60.0 + if e.is_player { 15.0 } else { 0.0 };
                if score > best_score {
                    best_score = score;
                    target = Some(TargetRef::God(j));
                }
            }
        }
    }
    if target.is_none() {
        for mi in 0..game.minions.len() {
            let m = &game.minions[mi];
            if m.team != team {
                let d = m.pos.distance(g.pos);
                if d < aggro * 0.8 {
                    let score = 50.0 - d;
                    if score > best_score {
                        best_score = score;
                        target = Some(TargetRef::Minion(mi));
                    }
                }
            }
        }
    }
    // push objective: enemy tower / titan when no threats nearby
    if target.is_none() && brain_state == AiState::Push {
        for ti in 0..game.towers.len() {
            let tw = &game.towers[ti];
            if tw.team != team && tw.pos.distance(g.pos) < def.auto_range + 4.0 {
                target = Some(TargetRef::Tower(ti));
                break;
            }
        }
        if target.is_none() {
            let et = team.other();
            let tp = game.titans[et as usize];
            if tp.hp > 0.0 && !game.titan_shielded(et) && tp.pos.distance(g.pos) < def.auto_range + 4.0 {
                target = Some(TargetRef::Titan(et));
            }
        }
    }

    // strafe timer
    {
        let b = g.ai.as_ref().unwrap();
        if b.strafe_t <= 0.0 {
            // pick new dir later (needs mut)
        }
    }

    let mut move_dir = Vec3::ZERO;
    let mut new_state = state;
    let mut cast_req: Option<(u32, Vec3)> = None;
    let mut new_wp = game.gods[gi].ai.as_ref().unwrap().wp;
    let mut strafe_dir = game.gods[gi].ai.as_ref().unwrap().strafe_dir;
    let mut strafe_t = game.gods[gi].ai.as_ref().unwrap().strafe_t;

    match state {
        AiState::Retreat => {
            let f = team.fountain();
            move_dir = Vec3::new(f.x - g.pos.x, 0.0, f.z - g.pos.z).normalize_or_zero();
        }
        AiState::Fight => {
            if let Some(tg) = target {
                let tpos = game.target_pos(tg);
                let to = Vec3::new(tpos.x - g.pos.x, 0.0, tpos.z - g.pos.z);
                let d = to.length();
                let ideal = (def.auto_range * 0.8).max(2.4);
                let vs_god = matches!(tg, TargetRef::God(_));
                if vs_god {
                    // strafe orbit
                    strafe_t -= 1.0 / 60.0;
                    if strafe_t <= 0.0 {
                        strafe_t = 0.9 + game.rng.f32() * 1.2;
                        if game.rng.f32() < 0.5 {
                            strafe_dir = -strafe_dir;
                        }
                    }
                    let perp = Vec3::new(-to.z, 0.0, to.x).normalize_or_zero() * strafe_dir;
                    if d > ideal + 1.5 {
                        move_dir = to.normalize_or_zero() + perp * 0.35;
                    } else if d < ideal - 1.5 {
                        move_dir = -to.normalize_or_zero() + perp * 0.5;
                    } else {
                        move_dir = perp;
                    }
                } else if d > ideal + 1.0 {
                    // hold ground vs minions — just close the gap a bit
                    move_dir = to.normalize_or_zero() * 0.6;
                }
                // ability heuristics
                cast_req = choose_cast(game, gi, tg, d);
            } else {
                new_state = AiState::Push;
            }
        }
        AiState::Push => {
            // siege: walk to and attack nearby enemy structures when unopposed
            let mut siege: Option<(Vec3, f32)> = None;
            for ti in 0..game.towers.len() {
                let tw = &game.towers[ti];
                if tw.team != team && tw.pos.distance(g.pos) < 16.0 {
                    siege = Some((tw.pos, def.auto_range));
                    break;
                }
            }
            if siege.is_none() {
                let et = team.other();
                let tp = game.titans[et as usize];
                if tp.hp > 0.0 && !game.titan_shielded(et) && tp.pos.distance(g.pos) < 18.0 {
                    siege = Some((tp.pos, def.auto_range));
                }
            }
            if let Some((sp, rng_)) = siege {
                let d = Vec3::new(sp.x - g.pos.x, 0.0, sp.z - g.pos.z);
                if d.length() > rng_ * 0.7 {
                    move_dir = d.normalize_or_zero();
                }
            } else {
                let path = crate::consts::lane_path(
                    game.gods[gi].ai.as_ref().unwrap().lane,
                    team,
                );
                let mut wp = new_wp.min(path.len() - 1);
                while wp + 1 < path.len() && g.pos.distance(path[wp]) < 6.0 {
                    wp += 1;
                }
                new_wp = wp;
                let tp = path[wp];
                move_dir = Vec3::new(tp.x - g.pos.x, 0.0, tp.z - g.pos.z).normalize_or_zero();
            }
            if target.is_some() {
                new_state = AiState::Fight;
            }
        }
    }

    // SIEGE OVERRIDE: no enemy gods nearby → break structures even through minions
    if new_state != AiState::Retreat {
        let enemy_gods_close = (0..game.gods.len()).any(|j| {
            game.gods[j].team != team
                && game.gods[j].respawn_t <= 0.0
                && game.gods[j].pos.distance(g.pos) < 12.0
        });
        if !enemy_gods_close {
            let mut structure: Option<(TargetRef, Vec3)> = None;
            for ti in 0..game.towers.len() {
                let tw = &game.towers[ti];
                if tw.team != team && tw.pos.distance(g.pos) < 15.0 {
                    structure = Some((TargetRef::Tower(ti), tw.pos));
                    break;
                }
            }
            if structure.is_none() {
                let et = team.other();
                let tp = game.titans[et as usize];
                if tp.hp > 0.0 && !game.titan_shielded(et) && tp.pos.distance(g.pos) < 16.0 {
                    structure = Some((TargetRef::Titan(et), tp.pos));
                }
            }
            if let Some((st, sp)) = structure {
                let d = Vec3::new(sp.x - g.pos.x, 0.0, sp.z - g.pos.z);
                let want = def.auto_range * 0.7;
                if d.length() > want {
                    move_dir = d.normalize_or_zero();
                } else {
                    move_dir = Vec3::ZERO;
                }
                new_state = AiState::Push;
                target = Some(st);
                cast_req = choose_cast(game, gi, st, d.length());
            }
        }
    }

    // objectives: a boss is up, we're healthy and unthreatened → go fight it
    // (bolder ancestors from further away). Inside the pit it becomes the target.
    if state == AiState::Push {
        let enemy_near = (0..game.gods.len()).any(|j| game.gods[j].team != team && game.gods[j].respawn_t <= 0.0 && game.gods[j].pos.distance(g.pos) < 14.0);
        if !enemy_near {
            if let Some(bi) = game.contestable_boss(g.pos, team, hp_frac) {
                let b = &game.bosses.list[bi];
                let engaged = b.last_hit_team == Some(team) && t - b.last_dmg_t < 6.0;
                let reach_ok = engaged || b.pos.distance(g.pos) < 28.0 + g.ai.as_ref().unwrap().aggression * 20.0;
                if reach_ok {
                    let d = Vec3::new(b.pos.x - g.pos.x, 0.0, b.pos.z - g.pos.z);
                    if d.length() > def.auto_range * 0.8 {
                        move_dir = d.normalize_or_zero();
                    } else {
                        move_dir = Vec3::ZERO;
                    }
                    target = Some(TargetRef::Boss(bi));
                    if cast_req.is_none() {
                        cast_req = choose_cast(game, gi, TargetRef::Boss(bi), d.length());
                    }
                }
            }
        }
    }

    // the hunt: hurt, with no enemy ancestor in sight and prey close by,
    // an ancestor breaks off to chase it (a mouthful of health + gold). If a
    // dash is off cooldown and the animal is in range, pounce with it.
    if hp_frac < 0.6 && state != AiState::Fight {
        let enemy_near = (0..game.gods.len()).any(|j| game.gods[j].team != team && game.gods[j].respawn_t <= 0.0 && game.gods[j].pos.distance(g.pos) < 16.0);
        if !enemy_near {
            let mut best: Option<(f32, Vec3)> = None;
            for p in &game.wild.prey {
                let d = p.pos.distance(g.pos);
                let reach = if p.kind == crate::wild::PreyKind::Glowfish { 14.0 } else { 9.0 };
                if d < reach && best.map_or(true, |(bd, _)| d < bd) {
                    best = Some((d, p.pos));
                }
            }
            if let Some((d, pp)) = best {
                let lead = pp + Vec3::new(pp.x - g.pos.x, 0.0, pp.z - g.pos.z).normalize_or_zero() * 1.5;
                move_dir = Vec3::new(lead.x - g.pos.x, 0.0, lead.z - g.pos.z).normalize_or_zero();
                if cast_req.is_none() {
                    for slot in 0..4u32 {
                        if let crate::gods::Archetype::Dash { dist, .. } = def.abilities[slot as usize].arch {
                            if game.gods[gi].cds[slot as usize] <= 0.0 && d < dist * 0.9 && d > 2.5 && !def.abilities[slot as usize].ult {
                                cast_req = Some((slot, lead));
                            }
                        }
                    }
                }
            }
        }
    }

    // avoid enemy fountain when retreating through
    let enemy_f = team.other().fountain();
    if g.pos.distance(enemy_f) < FOUNTAIN_RADIUS + 6.0 {
        move_dir = Vec3::new(g.pos.x - enemy_f.x, 0.0, g.pos.z - enemy_f.z).normalize_or_zero();
    }

    // Route: on a map carved out of rock, the straight line to a far target
    // runs through a cliff and the way there may start by walking AWAY from
    // it (out of a clearing, back along a ledge). The floor plan is a graph
    // of its shapes; head for the first portal on the route. Not in a
    // fight -- strafing is meant to be local.
    let move_dir = match crate::map::floor() {
        Some(f) if move_dir.length_squared() > 1e-6 && !matches!(game.gods[gi].ai.as_ref().map(|b| b.state), Some(AiState::Fight)) => {
            let pos = game.gods[gi].pos;
            if game.steer_mem.len() <= gi {
                game.steer_mem.resize(gi + 1, Default::default());
            }
            let now = game.t;
            let m = &game.steer_mem[gi];
            let reached = m.route_wp.is_some_and(|w| Vec3::new(w.x - pos.x, 0.0, w.z - pos.z).length() < 1.5);
            if !(now - m.route_t < crate::steer::ROUTE_EVERY) || reached {
                let tgt = game.ai_move_target(gi);
                let wp = f.next_waypoint(pos, tgt, 0.5);
                let m = &mut game.steer_mem[gi];
                m.route_wp = wp;
                m.route_t = now;
            }
            match game.steer_mem[gi].route_wp {
                Some(wp) => Vec3::new(wp.x - pos.x, 0.0, wp.z - pos.z).normalize_or_zero() * move_dir.length(),
                None => move_dir,
            }
        }
        _ => move_dir,
    };

    // Steer: skirt rough ground a sidestep avoids, turn from walls rather
    // than sliding into them. The brain above says where; this says which
    // way round.
    let move_dir = {
        let g = &game.gods[gi];
        let who = crate::going::Carriage { stalking: game.stalking(gi) };
        if game.steer_mem.len() <= gi {
            game.steer_mem.resize(gi + 1, Default::default());
        }
        let blockers = game.blockers.clone();
        let (pos, prev, now) = (g.pos, g.move_dir, game.t);
        let may_plan = game.plans_this_tick < crate::steer::PLANS_PER_TICK;
        let before = game.steer_mem[gi].plans;
        let d = crate::steer::steer_with(&blockers, pos, move_dir, prev, crate::consts::GOD_RADIUS, who, &mut game.steer_mem[gi], now, may_plan);
        game.plans_this_tick += game.steer_mem[gi].plans - before;
        d
    };

    // write back
    {
        let g = &mut game.gods[gi];
        g.move_dir = move_dir;
        g.attack_held = target.is_some() || state == AiState::Push;
        if let Some(b) = &mut g.ai {
            b.state = new_state;
            b.target = target;
            b.wp = new_wp;
            b.strafe_dir = strafe_dir;
            b.strafe_t = strafe_t;
        }
    }
    if let Some((slot, aim)) = cast_req {
        game.cast_queue.push((gi, slot, aim));
    }
    let _ = t;
}

/// Choose an ability cast for the current target based on archetype heuristics.
fn choose_cast(game: &Game, gi: usize, target: TargetRef, dist: f32) -> Option<(u32, Vec3)> {
    let g = &game.gods[gi];
    let def = game.def(gi);
    let tpos = game.target_pos(target);
    let is_god_target = matches!(target, TargetRef::God(_));
    let mut best: Option<(u32, f32)> = None; // (slot, priority)

    for (slot, ab) in def.abilities.iter().enumerate() {
        if g.cds[slot] > 0.0 || g.mana < ab.cost {
            continue;
        }
        let range: f32 = match ab.arch {
            Archetype::Bolt { .. } => 20.0,
            Archetype::Volley { .. } => 17.0,
            Archetype::Lob { .. } => 18.0,
            Archetype::Beam { length, .. } => length * 0.9,
            Archetype::Nova { radius, .. } => radius * 0.85,
            Archetype::Ring { radius: _, .. } => 12.0,
            Archetype::Storm { .. } => 12.0,
            Archetype::Dash { dist: dd, .. } => if dist > 7.0 { dd } else { 0.0 },
            Archetype::Blink { dist: dd, .. } => if dist > 9.0 { dd } else { 0.0 },
            Archetype::Shield { .. } | Archetype::Buff { .. } | Archetype::Heal { .. } => f32::INFINITY,
            Archetype::Pull { radius, .. } => radius + 4.0,
        };
        let pri = match ab.arch {
            Archetype::Nova { .. } => {
                if dist < range && is_god_target { 0.8 } else { 0.2 }
            }
            Archetype::Beam { .. } | Archetype::Bolt { .. } | Archetype::Volley { .. } => {
                if dist < range { 0.7 } else { 0.0 }
            }
            Archetype::Lob { .. } | Archetype::Ring { .. } => {
                if dist < range { 0.65 } else { 0.0 }
            }
            Archetype::Storm { .. } => {
                if is_god_target && dist < range { 0.9 } else { 0.0 }
            }
            Archetype::Dash { .. } => {
                if dist > 8.0 && dist < range && is_god_target { 0.5 } else { 0.0 }
            }
            Archetype::Blink { .. } => {
                if dist > 9.0 && dist < range && is_god_target { 0.55 } else { 0.0 }
            }
            Archetype::Pull { .. } => {
                if is_god_target && dist < range { 0.85 } else { 0.0 }
            }
            Archetype::Shield { .. } | Archetype::Buff { .. } | Archetype::Heal { .. } => {
                let hurt = g.hp / g.max_hp;
                if hurt < 0.62 { 0.95 } else if is_god_target && matches!(ab.arch, Archetype::Buff { .. }) { 0.4 } else { 0.0 }
            }
        };
        if pri > 0.0 && best.map_or(true, |(_, bp)| pri > bp) {
            best = Some((slot as u32, pri));
        }
    }

    // aim: lead the target a bit
    let aim = match target {
        TargetRef::God(j) => {
            let v = game.gods[j].move_dir * game.move_speed(j) * 0.25;
            tpos + v
        }
        _ => tpos,
    };
    best.map(|(slot, _)| (slot, aim))
}
