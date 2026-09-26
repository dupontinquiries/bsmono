// AI for the non-player gods: lane pushing, target selection, strafing,
// ability heuristics per archetype, retreat + fountain heal, auto-buy.
//
// A fight against an enemy god is fought in a stance (`tactics`): the bot
// reads the local fight (`read_fight`), `tactics::decide` picks dive / hold
// / regroup / retreat under the team's page of the playbook, and the
// movement below carries it out -- with the sidestep kept in every stance
// but the run home.

use crate::consts::*;
use crate::gods::Archetype;
use crate::sim::*;
use crate::tactics::{Genes, Read, Stance};
use glam::Vec3;

/// How long a teammate's dive call stands.
pub const CALL_FOR: f32 = 4.0;
/// Radius of "the fight" around a bot and its target.
pub const FIGHT_R: f32 = 14.0;

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
    let genes = bot_genes(game, gi);
    let retreat_at = genes.retreat;

    let brain_state = g.ai.as_ref().unwrap().state;
    let mut state = brain_state;

    // --- state transitions -----------------------------------------------------
    // (in a fight with a god the stance decides when to go home, so a kill
    // in reach can keep a hurt cat in it -- `tactics::decide`)
    let dueling = brain_state == AiState::Fight && matches!(g.ai.as_ref().unwrap().target, Some(TargetRef::God(_)));
    if brain_state != AiState::Retreat && hp_frac < retreat_at && !dueling {
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

    // Score: near and weak, leaning by the page's `focus`; the current
    // target is kept unless another is clearly better (a bot flipping
    // between two targets is half of what looked like circling); a
    // teammate's dive pulls the team onto the same cat; a cat sheltering
    // under its tower is worth less to a style that respects towers.
    let prev_target = g.ai.as_ref().unwrap().target;
    let call = game.calls[team as usize].filter(|&(_, at)| t - at < CALL_FOR);
    for j in 0..game.gods.len() {
        let e = &game.gods[j];
        if e.team != team && e.respawn_t <= 0.0 {
            let d = e.pos.distance(g.pos);
            if d < aggro {
                let mut score = 100.0 - d * (1.0 - genes.focus * 0.5) - (e.hp / e.max_hp) * 60.0 * (0.5 + genes.focus) + if e.is_player { 15.0 } else { 0.0 };
                if prev_target == Some(TargetRef::God(j)) {
                    score += 12.0;
                }
                if call.is_some_and(|(c, _)| c == j) {
                    score += 30.0 * genes.rally;
                }
                if enemy_tower_over(game, team, e.pos).is_some() {
                    score -= 12.0 * genes.tower;
                }
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
    let mut stance = Stance::Hold;
    let mut new_anchor: Option<Vec3> = None;
    let mut new_call: Option<(usize, f32)> = None;

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
                // the sidestep: a new direction every second or so (the dodging)
                strafe_t -= 1.0 / 60.0;
                if strafe_t <= 0.0 {
                    strafe_t = 0.9 + game.rng.f32() * 1.2;
                    if game.rng.f32() < 0.5 {
                        strafe_dir = -strafe_dir;
                    }
                }
                let perp = Vec3::new(-to.z, 0.0, to.x).normalize_or_zero() * strafe_dir;
                if let TargetRef::God(j) = tg {
                    let b = g.ai.as_ref().unwrap();
                    let anchor = b.anchor.unwrap_or(g.pos);
                    new_anchor = Some(anchor);
                    let mut r = read_fight(game, gi, j);
                    r.target_dist = d;
                    r.ideal = ideal;
                    r.from_anchor = Vec3::new(g.pos.x - anchor.x, 0.0, g.pos.z - anchor.z).length();
                    r.called = call.is_some_and(|(c, _)| c == j);
                    r.stance = b.stance;
                    r.stance_t = b.stance_t;
                    stance = crate::tactics::decide(&r, &genes);
                    let want = ideal * crate::tactics::distance_for(stance, &genes);
                    let side = perp * crate::tactics::strafe_for(stance, &genes);
                    move_dir = match stance {
                        Stance::Dive | Stance::Hold => {
                            if d > want + 1.5 {
                                to.normalize_or_zero() + side * 0.4
                            } else if d < want - 1.5 {
                                -to.normalize_or_zero() + side * 0.6
                            } else {
                                side.normalize_or_zero() * side.length().min(1.0)
                            }
                        }
                        Stance::Regroup => {
                            let safe = regroup_point(game, gi, anchor);
                            let back = Vec3::new(safe.x - g.pos.x, 0.0, safe.z - g.pos.z);
                            if back.length() > 2.0 {
                                back.normalize_or_zero() + side * 0.5
                            } else if d < want {
                                -to.normalize_or_zero() * 0.6 + side
                            } else {
                                side
                            }
                        }
                        Stance::Retreat => {
                            new_state = AiState::Retreat;
                            let f = team.fountain();
                            Vec3::new(f.x - g.pos.x, 0.0, f.z - g.pos.z).normalize_or_zero()
                        }
                    };
                    if stance == Stance::Dive && b.stance != Stance::Dive {
                        new_call = Some((j, t));
                    }
                    cast_req = choose_cast(game, gi, tg, d, stance);
                } else {
                    if d > ideal + 1.0 {
                        // hold ground vs minions and structures -- just close the gap a bit
                        move_dir = to.normalize_or_zero() * 0.6;
                    }
                    cast_req = choose_cast(game, gi, tg, d, Stance::Hold);
                }
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
                cast_req = choose_cast(game, gi, st, d.length(), Stance::Hold);
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
                let reach_ok = engaged || b.pos.distance(g.pos) < 14.0 + genes.objective * 28.0 + g.ai.as_ref().unwrap().aggression * 12.0;
                if reach_ok {
                    let d = Vec3::new(b.pos.x - g.pos.x, 0.0, b.pos.z - g.pos.z);
                    if d.length() > def.auto_range * 0.8 {
                        move_dir = d.normalize_or_zero();
                    } else {
                        move_dir = Vec3::ZERO;
                    }
                    target = Some(TargetRef::Boss(bi));
                    if cast_req.is_none() {
                        cast_req = choose_cast(game, gi, TargetRef::Boss(bi), d.length(), Stance::Dive);
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

    // Tower discipline. An enemy tower with none of our minions under it
    // shoots the nearest cat, ramping. Outside a committed dive, a bot does
    // not stand in that range: it backs out, and it does not walk in to
    // siege until minions arrive to take the shots -- unless the tower is
    // nearly down (sooner for a page that respects towers less). This is
    // where most of the "went too deep" deaths were: bots sieging alone
    // (Push/tower) and fights that drifted under a tower before the run home.
    let mut backing_off = false;
    if new_state != AiState::Retreat {
        let pos = g.pos;
        let near = (0..game.towers.len())
            .filter(|&ti| {
                let tw = &game.towers[ti];
                tw.team != team && tw.hp > 0.0 && tw.pos.distance(pos) < TOWER_RANGE + 2.5
            })
            .min_by(|&a, &b| game.towers[a].pos.distance(pos).total_cmp(&game.towers[b].pos.distance(pos)));
        if let Some(ti) = near {
            let tw = &game.towers[ti];
            let uncovered = !tower_covered(game, team, tw.pos);
            let finishing = tw.hp / tw.max_hp < 0.1 + (1.5 - genes.tower) * 0.15 && hp_frac > 0.5;
            let kill_dive = stance == Stance::Dive && matches!(target, Some(TargetRef::God(j)) if game.gods[j].hp / game.gods[j].max_hp < 0.22);
            let committed = stance == Stance::Dive && matches!(target, Some(TargetRef::God(_))) && hp_frac > 0.4;
            let shot_at = tw.aim == Some(TargetRef::God(gi));
            if uncovered && !finishing && !kill_dive && !(committed && !shot_at) {
                let away = Vec3::new(pos.x - tw.pos.x, 0.0, pos.z - tw.pos.z).normalize_or_zero();
                let d = Vec3::new(pos.x - tw.pos.x, 0.0, pos.z - tw.pos.z).length();
                if d < TOWER_RANGE + 1.0 {
                    let side = Vec3::new(-away.z, 0.0, away.x) * strafe_dir * 0.3;
                    move_dir = away + side;
                    backing_off = true;
                    if matches!(target, Some(TargetRef::Tower(_)) | Some(TargetRef::Titan(_))) {
                        target = None;
                        cast_req = None;
                    }
                } else {
                    // at the edge: no step further in
                    let inward = -move_dir.dot(away);
                    if inward > 0.0 {
                        move_dir += away * inward;
                    }
                    if matches!(target, Some(TargetRef::Tower(_))) {
                        target = None;
                        cast_req = None;
                    }
                }
            }
        }
    }

    // the escape: running home with an enemy on its heels, a dash or blink
    // goes toward home instead of into the fight
    if new_state == AiState::Retreat && cast_req.is_none() {
        let close = (0..game.gods.len()).any(|j| game.gods[j].team != team && game.gods[j].respawn_t <= 0.0 && game.gods[j].pos.distance(g.pos) < 7.0);
        if close {
            for slot in 0..4usize {
                let ab = &def.abilities[slot];
                if let Archetype::Dash { dist, .. } | Archetype::Blink { dist, .. } = ab.arch {
                    if game.gods[gi].cds[slot] <= 0.0 && game.gods[gi].mana >= ab.cost && !ab.ult {
                        let f = team.fountain();
                        let home = Vec3::new(f.x - g.pos.x, 0.0, f.z - g.pos.z).normalize_or_zero();
                        cast_req = Some((slot as u32, g.pos + home * dist));
                        break;
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
        Some(f) if move_dir.length_squared() > 1e-6 && !backing_off && !matches!(game.gods[gi].ai.as_ref().map(|b| b.state), Some(AiState::Fight)) => {
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
            let fighting_god = new_state == AiState::Fight && matches!(target, Some(TargetRef::God(_)));
            let st = if fighting_god { stance } else if new_state == AiState::Retreat { Stance::Retreat } else { Stance::Hold };
            if st == b.stance {
                b.stance_t += 1.0 / 60.0;
            } else {
                b.stance = st;
                b.stance_t = 0.0;
            }
            b.anchor = if fighting_god { new_anchor } else { None };
            b.state = new_state;
            b.target = target;
            b.wp = new_wp;
            b.strafe_dir = strafe_dir;
            b.strafe_t = strafe_t;
        }
    }
    if let Some(c) = new_call {
        game.calls[team as usize] = Some(c);
    }
    if let Some((slot, aim)) = cast_req {
        game.cast_queue.push((gi, slot, aim));
    }
    let _ = t;
}

/// Choose an ability cast for the current target based on archetype heuristics.
/// Only a Dive spends the engage abilities (dash, blink, pull) on closing
/// in; the other stances keep them.
fn choose_cast(game: &Game, gi: usize, target: TargetRef, dist: f32, stance: Stance) -> Option<(u32, Vec3)> {
    let engage = stance == Stance::Dive;
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
                if engage && dist > 8.0 && dist < range && is_god_target { 0.5 } else { 0.0 }
            }
            Archetype::Blink { .. } => {
                if engage && dist > 9.0 && dist < range && is_god_target { 0.55 } else { 0.0 }
            }
            Archetype::Pull { .. } => {
                if engage && is_god_target && dist < range { 0.85 } else { 0.0 }
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

/// The team's page, bent a little by this bot's own temper (the old
/// per-bot `aggression`, 0.45..0.95): bolder cats commit earlier and go
/// home later.
pub fn bot_genes(game: &Game, gi: usize) -> Genes {
    let g = &game.gods[gi];
    let mut genes = game.styles[g.team as usize].genes;
    let a = g.ai.as_ref().map_or(0.7, |b| b.aggression) - 0.7;
    genes.commit = (genes.commit - a * 0.3).max(0.75);
    genes.retreat = (genes.retreat - a * 0.08).max(0.08);
    genes
}

/// A god's weight in a fight: (health, damage). Damage is a unitless
/// proxy -- level and items -- because only ratios between sides matter.
fn god_weight(g: &GodUnit) -> (f32, f32) {
    (g.hp + g.shield, 1.0 + 0.15 * (g.level as f32 - 1.0) + 0.12 * g.items.len() as f32)
}

/// Minions of `team` under an enemy tower at `tower`, enough of them to
/// hold its fire for a while (a tower shoots minions before gods --
/// `systems::tower_attacks` -- and one minion does not last).
pub fn tower_covered(game: &Game, team: Team, tower: Vec3) -> bool {
    game.minions.iter().filter(|m| m.team == team && m.hp > 0.0 && m.pos.distance(tower) < TOWER_RANGE - 1.0).count() >= 2
}

/// The enemy tower whose range covers `pos`, if one does and it is not
/// covered by `team`'s minions.
pub fn enemy_tower_over(game: &Game, team: Team, pos: Vec3) -> Option<usize> {
    let ti = (0..game.towers.len()).find(|&ti| {
        let tw = &game.towers[ti];
        tw.team != team && tw.hp > 0.0 && tw.pos.distance(pos) < TOWER_RANGE
    })?;
    (!tower_covered(game, team, game.towers[ti].pos)).then_some(ti)
}

/// Read the fight between bot `gi` and enemy god `j`: both sides' weight
/// near it, an enemy tower over it, health and readiness. The distance,
/// anchor, call and stance fields are the caller's.
pub fn read_fight(game: &Game, gi: usize, j: usize) -> Read {
    let me = &game.gods[gi];
    let foe = &game.gods[j];
    let team = me.team;
    let mid = (me.pos + foe.pos) * 0.5;
    let (mut ours, mut theirs) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    for g in &game.gods {
        if g.respawn_t > 0.0 {
            continue;
        }
        let near = if g.team == team { g.pos.distance(me.pos) < FIGHT_R } else { g.pos.distance(foe.pos) < FIGHT_R || g.pos.distance(mid) < FIGHT_R };
        if near {
            let (h, d) = god_weight(g);
            let side = if g.team == team { &mut ours } else { &mut theirs };
            side.0 += h;
            side.1 += d;
        }
    }
    for m in &game.minions {
        if m.hp > 0.0 && m.pos.distance(mid) < FIGHT_R * 0.7 {
            let side = if m.team == team { &mut ours } else { &mut theirs };
            side.0 += m.hp * 0.3;
            side.1 += 0.15;
        }
    }
    // a tower over the target or over me: worth about a cat and a half
    let tower = if enemy_tower_over(game, team, foe.pos).or_else(|| enemy_tower_over(game, team, me.pos)).is_some() {
        (me.max_hp * 1.5, 1.5)
    } else {
        (0.0, 0.0)
    };
    // our own tower over us: on our side of the scale
    let own = game.towers.iter().any(|tw| tw.team == team && tw.hp > 0.0 && tw.pos.distance(me.pos) < TOWER_RANGE * 0.9);
    if own {
        ours.0 += me.max_hp;
        ours.1 += 1.2;
    }
    let def = game.def(gi);
    let (mut ready, mut total) = (0.0f32, 0.0f32);
    for (k, ab) in def.abilities.iter().enumerate() {
        if matches!(ab.arch, Archetype::Shield { .. } | Archetype::Buff { .. } | Archetype::Heal { .. }) {
            continue;
        }
        total += 1.0;
        if me.cds[k] <= 0.0 && me.mana >= ab.cost {
            ready += 1.0;
        }
    }
    Read {
        my_hp: me.hp / me.max_hp,
        ours,
        theirs,
        tower,
        target_hp: foe.hp / foe.max_hp,
        target_dist: me.pos.distance(foe.pos),
        ideal: (def.auto_range * 0.8).max(2.4),
        from_anchor: 0.0,
        ready: if total > 0.0 { ready / total } else { 1.0 },
        called: false,
        stance: Stance::Hold,
        stance_t: 0.0,
    }
}

/// Where a regrouping bot heads: back to where the fight began if it has
/// chased away from there, else to the allies near it (pulled harder by a
/// page that groups), else toward home.
fn regroup_point(game: &Game, gi: usize, anchor: Vec3) -> Vec3 {
    let me = &game.gods[gi];
    if Vec3::new(me.pos.x - anchor.x, 0.0, me.pos.z - anchor.z).length() > 3.0 {
        return anchor;
    }
    let (mut sum, mut n) = (Vec3::ZERO, 0.0);
    for (k, g) in game.gods.iter().enumerate() {
        if k != gi && g.team == me.team && g.respawn_t <= 0.0 && g.pos.distance(me.pos) < 22.0 {
            sum += g.pos;
            n += 1.0;
        }
    }
    if n > 0.0 {
        return sum / n;
    }
    let f = me.team.fountain();
    me.pos + Vec3::new(f.x - me.pos.x, 0.0, f.z - me.pos.z).normalize_or_zero() * 8.0
}
