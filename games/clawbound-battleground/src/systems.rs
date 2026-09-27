// Per-tick systems: waves, movement, auto-attacks, projectiles, zones, regen.

use crate::consts::*;
use crate::gods::{Archetype, AutoKind};
use crate::sim::*;
use glam::Vec3;

impl Game {
    pub fn tick(&mut self, dt: f32) {
        if self.over.is_some() {
            return;
        }
        self.t += dt;

        // 1. minion waves
        if self.t >= self.next_wave_t {
            self.next_wave_t = self.t + WAVE_INTERVAL;
            for &lane in crate::map::lanes() {
                for team in [Team::Blue, Team::Red] {
                    self.spawn_wave(lane, team);
                }
            }
            self.waves += 1;
        }

        // 2. AI decisions
        self.plans_this_tick = 0;
        let n = self.gods.len();
        for i in 0..n {
            if self.gods[i].respawn_t > 0.0 {
                continue;
            }
            if self.gods[i].ai.is_some() && self.gods[i].skyfall.is_none() {
                crate::ai::think(self, i);
            }
        }

        // 3. cast queue (player requests + AI requests)
        let queue = std::mem::take(&mut self.cast_queue);
        for (gi, slot, aim) in queue {
            self.begin_cast(gi, slot, aim);
        }
        self.tick_casting(dt);

        // 4. movement
        self.move_gods(dt);
        self.resolve_bodies();
        self.move_minions(dt);
        self.logs_tick(dt);
        // who can see whom through the sky map's cloud (skycloud.rs)
        self.vision_tick();

        // 5. auto-attacks
        self.god_autoattacks(dt);
        self.minion_attacks(dt);
        self.tower_attacks(dt);
        self.titan_attacks(dt);

        // 6. projectiles + zones
        self.projectiles_tick(dt);
        self.zones_tick(dt);
        self.kit_tick(dt);

        // 6b. sky + map events (weather.rs / map_events.rs)
        self.atmosphere_tick(dt);
        // 6c. the living jungle: heal trees + prey (wild.rs)
        self.wild_tick(dt);
        // 6d. objectives (bosses.rs)
        self.bosses_tick(dt);
        self.moonwell_tick(dt);
        // overtime (the sky map): said once, as it begins
        let ot = crate::map::OVERTIME_AT;
        if crate::map::active().overtime(ot + 60.0) > 1.0 && self.t >= ot && self.t - dt < ot {
            self.events.push(Event::Announce { text: "OVERTIME: THE BASTIONS WEAKEN".into() });
        }
        // 6e. catastrophes (disasters.rs)
        self.disasters_tick(dt);

        // 7. regen / timers
        self.timers_tick(dt);
    }

    fn spawn_wave(&mut self, lane: Lane, team: Team) {
        let path = lane.path_for(team);
        let scale = 1.0 + (self.t / 60.0).min(15.0) * 0.07;
        let hp = MINION_HP * scale * self.lane_surge(team, lane);
        // melee in ranks (four abreast, eight for a big wave), casters behind
        let (melee, casters) = crate::map::active().wave(self.waves);
        let rank = |k: usize, n: usize, back: f32| -> [f32; 2] {
            // eight abreast for a big wave (the sky's 32 u lanes), else four
            let per = if n > 8 { 8 } else { n.min(4).max(1) };
            let (row, col) = (k / per, k % per);
            let across = (col as f32 - (per as f32 - 1.0) * 0.5) * 1.3 + if row % 2 == 1 { 0.4 } else { 0.0 };
            [back - row as f32 * 1.4, across]
        };
        let ahead = {
            let d = path[1.min(path.len() - 1)] - path[0];
            Vec3::new(d.x, 0.0, d.z).normalize_or(Vec3::Z)
        };
        let side = Vec3::new(-ahead.z, 0.0, ahead.x);
        for k in 0..melee + casters {
            let caster = k >= melee;
            // a small wave keeps the wedge it had (a front of three, the
            // casters behind); a big one marches in ranks
            let o = if melee + casters <= 5 {
                const FRONT: [[f32; 2]; 3] = [[0.0, 0.0], [1.3, 0.9], [1.3, -0.9]];
                const BACK: [[f32; 2]; 2] = [[-1.6, 0.5], [-1.6, -0.5]];
                if caster { BACK[(k - melee) % 2] } else { FRONT[k % 3] }
            } else {
                let melee_rows = ((melee + 3) / 4) as f32;
                if caster { rank(k - melee, casters, -melee_rows * 1.4 - 0.4) } else { rank(k, melee, 0.0) }
            };
            let id = self.next_minion_id;
            self.next_minion_id += 1;
            let think_t = self.rng.f32() * crate::minionai::THINK_EVERY;
            self.minions.push(Minion {
                team,
                lane,
                caster,
                pos: path[0] + ahead * o[0] + side * o[1],
                hp,
                max_hp: hp,
                atk_timer: 0.0,
                atk_anim: 0.0,
                walk_anim: self.rng.f32() * 6.0,
                wp: 1.min(path.len() - 1),
                last_hit: None,
                yaw: 0.0,
                id,
                foe: None,
                think_t,
                left_at: None,
                // ranks spread wider on a wide lane
                lane_off: o[1] * if melee + casters > 5 { 1.6 } else { 1.0 },
                giggle_t: 0.0,
                dance_t: 0.0,
            });
        }
    }

    /// How much harder `team`'s minions in `lane` hit and last: 1 plus the
    /// map's siege surge for each enemy tower fallen in that lane.
    pub fn lane_surge(&self, team: Team, lane: Lane) -> f32 {
        let per = crate::map::active().siege_surge();
        if per <= 0.0 {
            return 1.0;
        }
        let standing = self.towers.iter().filter(|t| t.team != team && t.lane == lane && t.hp > 0.0).count();
        1.0 + per * 2usize.saturating_sub(standing) as f32
    }

    /// The index of the minion with this id (the Vec is sorted by id).
    pub fn minion_index(&self, id: u32) -> Option<usize> {
        self.minions.binary_search_by_key(&id, |m| m.id).ok()
    }

    /// A minion's foe as a `TargetRef` and a position, if it is still there
    /// and still fightable.
    pub fn resolve_foe(&self, foe: crate::minionai::Foe) -> Option<(TargetRef, Vec3)> {
        use crate::minionai::Foe;
        match foe {
            Foe::God(i) => {
                let g = self.gods.get(i)?;
                (g.respawn_t <= 0.0).then_some((TargetRef::God(i), g.pos))
            }
            Foe::Minion(id) => {
                let i = self.minion_index(id)?;
                Some((TargetRef::Minion(i), self.minions[i].pos))
            }
            Foe::Tower(i) => {
                let t = self.towers.get(i)?;
                (t.hp > 0.0).then_some((TargetRef::Tower(i), t.pos))
            }
            Foe::Titan(team) => {
                let t = &self.titans[team as usize];
                (t.hp > 0.0 && !self.titan_shielded(team)).then_some((TargetRef::Titan(team), t.pos))
            }
        }
    }

    /// Attack reach of minion `m` against a foe: structures are big, so a
    /// minion hits them from a little further off.
    fn minion_reach(m: &Minion, foe: crate::minionai::Foe) -> f32 {
        use crate::minionai::Foe;
        let base = if m.caster { MINION_RANGE_CASTER } else { MINION_RANGE_MELEE + 0.4 };
        match foe {
            Foe::Tower(_) => base + if m.caster { 0.0 } else { 1.0 },
            Foe::Titan(_) => base + 2.5,
            _ => base,
        }
    }

    /// Re-think minion targets (`minionai`): drop the ones that died or got
    /// away, and every `THINK_EVERY` seconds pick again from what is near.
    fn minion_think(&mut self, dt: f32) {
        use crate::minionai::{pick, Class, Foe, Seen, AGGRO_CASTER, AGGRO_MELEE, CALL_FOR, LEASH, THINK_EVERY};
        let now = self.t;
        let mut out: Vec<(Option<Foe>, f32, Option<Vec3>)> = Vec::with_capacity(self.minions.len());
        for m in &self.minions {
            let aggro = if m.caster { AGGRO_CASTER } else { AGGRO_MELEE };
            let mut think_t = m.think_t - dt;
            let mut left_at = m.left_at;
            // the held target, if it is still there, near, and inside the leash
            let current = m.foe.and_then(|f| {
                let (_, p) = self.resolve_foe(f)?;
                let d = flat(p - m.pos);
                let leashed = left_at.is_some_and(|l| flat(m.pos - l) > LEASH) && !matches!(f, Foe::Tower(_) | Foe::Titan(_));
                (d < aggro + 2.0 && !leashed).then_some((f, d))
            });
            if m.foe.is_some() && current.is_none() {
                think_t = 0.0;
            }
            let mut foe = current.map(|c| c.0);
            if think_t <= 0.0 {
                think_t = THINK_EVERY;
                let mut seen: Vec<Seen> = Vec::new();
                for (gi, g) in self.gods.iter().enumerate() {
                    if g.team == m.team || g.respawn_t > 0.0 {
                        continue;
                    }
                    let d = flat(g.pos - m.pos);
                    if d < aggro {
                        seen.push(Seen { foe: Foe::God(gi), class: Class::God, dist: d });
                    }
                }
                // calls for help: an enemy god that hit an allied god near us
                for a in &self.gods {
                    if a.team != m.team || a.respawn_t > 0.0 || flat(a.pos - m.pos) > aggro + 4.0 {
                        continue;
                    }
                    for &(s, t) in &a.recent_damagers {
                        if now - t < CALL_FOR {
                            if let Some(e) = self.gods.get(s) {
                                let d = flat(e.pos - m.pos);
                                if e.team != m.team && e.respawn_t <= 0.0 && d < aggro + 2.0 {
                                    seen.push(Seen { foe: Foe::God(s), class: Class::Call, dist: d });
                                }
                            }
                        }
                    }
                }
                for e in &self.minions {
                    if e.team != m.team {
                        let d = flat(e.pos - m.pos);
                        if d < aggro {
                            seen.push(Seen { foe: Foe::Minion(e.id), class: Class::Minion, dist: d });
                        }
                    }
                }
                // A tower is seen from further off: a wave marching its
                // ranks down a wide lane passes a tower on the centre-line
                // up to eight u to one side, and at the plain aggro range
                // the outer ranks walked straight past it (the sky map's
                // lanes stalled with every tower at full health).
                for (ti, tw) in self.towers.iter().enumerate() {
                    if tw.team != m.team && tw.hp > 0.0 {
                        let d = flat(tw.pos - m.pos);
                        if d < aggro + 10.0 {
                            seen.push(Seen { foe: Foe::Tower(ti), class: Class::Structure, dist: d });
                        }
                    }
                }
                let et = m.team.other();
                let tp = &self.titans[et as usize];
                if tp.hp > 0.0 && !self.titan_shielded(et) {
                    let d = flat(tp.pos - m.pos);
                    if d < aggro + 2.5 {
                        seen.push(Seen { foe: Foe::Titan(et), class: Class::Structure, dist: d });
                    }
                }
                // a minion walking back from a chase does not re-engage a god
                // it was leashed off, only what is in its lane
                let returning = m.foe.is_some() && current.is_none() && left_at.is_some_and(|l| flat(m.pos - l) > LEASH);
                if returning {
                    seen.retain(|s| s.class != Class::God && s.class != Class::Call);
                }
                let cur = current.map(|(f, d)| {
                    let class = seen.iter().find(|s| s.foe == f).map_or(Class::God, |s| s.class);
                    Seen { foe: f, class, dist: d }
                });
                foe = pick(cur, &seen);
            }
            if foe.is_some() && left_at.is_none() {
                left_at = Some(m.pos);
            }
            out.push((foe, think_t, left_at));
        }
        for (m, (foe, think_t, left_at)) in self.minions.iter_mut().zip(out) {
            m.foe = foe;
            m.think_t = think_t;
            m.left_at = left_at;
        }
    }

    // --- movement ---------------------------------------------------------------

    fn move_gods(&mut self, dt: f32) {
        // Cloned once: the loop takes `&mut self.gods[i]`, so the borrow
        // checker will not allow reading `self.blockers` inside it. An Arc
        // clone is a refcount bump, not a copy of the grid.
        let blockers = self.blockers.clone();
        // Leap requests. Refused silently when the cat cannot leap right
        // now; the binding is a press, not a promise.
        let now = self.t;
        // The sky map's clouds this tick: where each deck is, how far it
        // moved since last tick (riders are carried by that), and which pier
        // it is docked at.
        let ferries = self.ferries.clone();
        let clouds: Vec<(Vec3, Vec3, Option<u8>)> = ferries
            .iter()
            .map(|f| {
                let c = f.at(now);
                (c.pos, c.pos - f.at(now - dt).pos, c.docked)
            })
            .collect();
        let sky = crate::map::active() == crate::map::MapId::Sky;
        // dashes that finish this tick: their damage is dealt after the
        // movement loop (it needs the whole game, the loop holds one cat)
        let mut dash_ends: Vec<(usize, Team, Vec3, Vec3, f32, Option<(f32, f32)>)> = Vec::new();
        for (i, aim) in std::mem::take(&mut self.leap_queue) {
            let Some(g) = self.gods.get_mut(i) else { continue };
            if g.respawn_t > 0.0 || g.dash.is_some() || g.fall.is_some() || g.skyfall.is_some() {
                continue;
            }
            // Bounding: from a perch, or buffered during a landing, the next
            // leap needs no cooldown and barely a gather.
            let landing = matches!(g.leap, Some(l) if l.phase == crate::leap::Phase::Land);
            let Some(chained) = crate::brawl::may_leap(g.leap.is_some(), landing, g.perch.is_some(), g.leap_cd, now - g.last_dmg_t) else {
                continue;
            };
            let aim = if aim.length_squared() > 1e-4 { aim } else { Vec3::new(g.yaw.cos(), 0.0, g.yaw.sin()) };
            // each cloud where it will be when a leap from here to it lands
            let platforms: Vec<crate::ferry::Platform> = ferries
                .iter()
                .map(|f| {
                    let run = (f.at(now).pos - g.pos).length().min(crate::leap::LEAP_RANGE);
                    crate::ferry::Platform { at: f.at(now + crate::leap::time_to_land(run, chained)).pos, r: crate::ferry::CLOUD_R }
                })
                .collect();
            if let Some((plan, ride)) = crate::leap::plan_onto(g.pos, aim, g.perch, &blockers, &platforms) {
                g.yaw = (plan.to.z - g.pos.z).atan2(plan.to.x - g.pos.x);
                g.leap = Some(crate::leap::LeapState::begin(g.pos, plan, chained));
                g.perch = None;
                g.riding = None;
                g.ride_plan = ride.map(|k| k as u8);
            }
        }
        // the starlight lifts (lift.rs): riders are carried, not walked
        self.lifts_tick(dt);
        for i in 0..self.gods.len() {
            if self.gods[i].lift.is_some() {
                continue;
            }
            if self.gods[i].respawn_t > 0.0 {
                // A cat that dies mid-leap respawns on the ground.
                self.gods[i].leap = None;
                self.gods[i].perch = None;
                self.gods[i].fall = None;
                self.gods[i].riding = None;
                self.gods[i].ride_plan = None;
                self.gods[i].skyfall = None;
                continue;
            }
            // Falling into the cloud sea: down through the clouds, then the
            // Starwell sets the cat down at its fountain (not a death).
            if let Some(left) = self.gods[i].skyfall {
                let g = &mut self.gods[i];
                let fallen = crate::ferry::SKYFALL_SECS - left;
                g.pos.y = -0.5 * crate::leap::GRAVITY * 0.4 * fallen * fallen;
                g.move_dir = Vec3::ZERO;
                if left - dt <= 0.0 {
                    let f = g.team.fountain();
                    g.pos = Vec3::new(f.x, crate::map::ground_y(f.x, f.z), f.z);
                    g.hp = (g.hp - g.max_hp * crate::ferry::SKYFALL_COST).max(1.0);
                    g.skyfall = None;
                    self.events.push(Event::Announce { text: format!("{} is carried home by the Starwell", self.def(i).name) });
                } else {
                    g.skyfall = Some(left - dt);
                }
                continue;
            }
            // a rider is carried with its cloud (not while in the air)
            if let Some(k) = self.gods[i].riding {
                if self.gods[i].leap.is_none() {
                    if let Some(&(_, delta, _)) = clouds.get(k as usize) {
                        self.gods[i].pos += Vec3::new(delta.x, 0.0, delta.z);
                    }
                }
            }
            // a wind-up slows the caster; a channel roots it
            let speed = self.move_speed(i) * self.gods[i].casting.map_or(1.0, |c| c.move_mul);
            self.gods[i].face_hold = (self.gods[i].face_hold - dt).max(0.0);
            let dir = crate::kit::confused(self.gods[i].move_dir, self.gods[i].giggle_t, self.t, i);
            let yaw_cur = self.gods[i].yaw;
            let g = &mut self.gods[i];
            g.leap_cd = (g.leap_cd - dt).max(0.0);
            g.hush_t = (g.hush_t - dt).max(0.0);
            // A dash (an ability) outranks a leap (a move).
            if g.dash.is_some() {
                g.leap = None;
                g.perch = None;
                g.fall = None;
            }
            if let Some(l) = &mut g.leap {
                let (p, done) = l.tick(dt);
                g.pos = p;
                if done {
                    g.perch = l.perch;
                    g.leap = None;
                    // aimed at a cloud: on it, or missed it
                    if let Some(k) = g.ride_plan.take() {
                        let on = ferries.get(k as usize).is_some_and(|f| f.on_deck(now, g.pos.x, g.pos.z, -0.6));
                        if on {
                            g.riding = Some(k);
                        } else if blockers.floor().is_some_and(|f| !f.fits(g.pos.x, g.pos.z, GOD_RADIUS * 0.5)) {
                            g.skyfall = Some(crate::ferry::SKYFALL_SECS);
                        }
                    }
                    // winded: the next leap cools twice as long
                    g.leap_cd = crate::leap::LEAP_COOLDOWN * if now - g.last_dmg_t < crate::brawl::WINDED_SECS { crate::brawl::WINDED_COOLDOWN } else { 1.0 };
                }
            } else if let Some(d) = &mut g.dash {
                d.t += dt;
                let a = (d.t / d.dur).min(1.0);
                // over the ground the whole way: a straight line up a ramp
                // went INTO the slope, then snapped up at the end
                let p = d.from.lerp(d.to, a);
                let gy = crate::map::ground_y(p.x, p.z);
                g.pos = Vec3::new(p.x, p.y.max(gy), p.z) + Vec3::Y * (a * (1.0 - a) * 4.0 * d.arc);
                if a >= 1.0 {
                    let gy = crate::map::ground_y(d.to.x, d.to.z);
                    g.pos = Vec3::new(d.to.x, d.to.y.max(gy), d.to.z);
                    dash_ends.push((i, g.team, d.from, d.to, d.dmg, d.blast));
                    g.dash = None;
                }
                g.walk_anim += dt * 12.0;
            } else if dir.length_squared() > 0.0001 {
                let on_ridge = g.perch.is_some() && g.perch_narrow;
                g.pos += dir.normalize() * speed * if on_ridge { crate::leap::NARROW_SPEED } else { 1.0 } * dt;
                // Facing: toward the move, within a cone round where the
                // player looks (`face_ref`, the camera's forward): strafing
                // turns the cat only part way, and moving back toward the
                // camera it keeps facing away from it and backs up. Not at
                // all while a cast holds it on its aim.
                let want = facing_yaw(dir, g.face_ref);
                let mut d = want - yaw_cur;
                while d > std::f32::consts::PI {
                    d -= std::f32::consts::TAU;
                }
                while d < -std::f32::consts::PI {
                    d += std::f32::consts::TAU;
                }
                if g.face_hold <= 0.0 {
                    g.yaw = yaw_cur + d * (dt * 14.0).min(1.0);
                }
                g.walk_anim += dt * 9.0;
            } else {
                g.walk_anim += dt * 1.5;
            }
            // Prop collision. Runs AFTER the move and BEFORE the arena clamp:
            // a push-out can shove a unit past the rim, and the clamp is the
            // thing that must have the last word on staying inside the map.
            //
            // A dashing god is resolved at its feet height so it clears low
            // obstacles -- `resolve_at` skips blockers topping out below the
            // given height, which is what stops a leap over a boulder being
            // yanked sideways mid-flight.
            let feet = match (&g.leap, g.perch) {
                (Some(l), _) => l.feet_y(g.pos.y),
                // Standing on a prop: that prop is below the feet, so it
                // does not push; its taller neighbours still do.
                (None, Some(top)) => top,
                // Falling: what is below the paws does not push, so a cat
                // drops past a wall's top and is only then shouldered clear.
                _ if g.dash.is_some() || g.fall.is_some() => g.pos.y,
                _ => f32::NEG_INFINITY,
            };
            // In the air over the sky map's chasms, or riding a cloud across
            // one, the floor's edge does not apply (it would drag the cat
            // back to the nearest island); props still do.
            g.pos = if sky && (g.riding.is_some() || g.leap.is_some() || g.skyfall.is_some()) {
                blockers.resolve_free(g.pos, GOD_RADIUS, feet)
            } else {
                blockers.resolve_at(g.pos, GOD_RADIUS, feet)
            };
            // On a cloud: stay on the deck, or step off onto a pier it is
            // docked at.
            if let Some(k) = g.riding {
                if g.leap.is_none() {
                    if let Some(&(deck, _, docked)) = clouds.get(k as usize) {
                        let off = Vec3::new(g.pos.x - deck.x, 0.0, g.pos.z - deck.z).length();
                        let onto_pier = docked.is_some() && blockers.floor().is_some_and(|f| f.fits(g.pos.x, g.pos.z, GOD_RADIUS));
                        if off > crate::ferry::CLOUD_R - GOD_RADIUS {
                            if onto_pier {
                                g.riding = None;
                            } else {
                                g.pos = crate::ferry::clamp_to_deck(g.pos, deck, GOD_RADIUS);
                            }
                        }
                        if g.riding.is_some() {
                            g.pos.y = deck.y;
                        }
                    } else {
                        g.riding = None;
                    }
                }
            }
            // Perch upkeep: still over the thing we are standing on?
            if let Some(top) = g.perch {
                use crate::leap::{Fall, KNOCK_OFF_FRAC};
                // Which way a cat goes over is not a choice it gets; derive
                // it from state so replays agree.
                let sign = if (g.hp as i32 + i as i32) % 2 == 0 { 1.0 } else { -1.0 };
                match blockers.standing_on(g.pos.x, g.pos.z, top) {
                    Some(kind) => {
                        g.pos.y = top;
                        g.perch_narrow = kind == crate::blockers::Perch::Narrow;
                        // A ridge is a strong place to stand and a poor one
                        // to be hit: a solid blow takes the footing. The cat
                        // goes over on its back -- and lands on its feet.
                        if g.perch_narrow && g.perch_hp - g.hp > KNOCK_OFF_FRAC * g.max_hp {
                            let side = Vec3::new(-g.yaw.sin(), 0.0, g.yaw.cos()) * sign;
                            g.pos += side * 0.6;
                            g.perch = None;
                            g.fall = Some(Fall::new(top, sign * 2.4));
                        }
                    }
                    None => {
                        // Off the edge. Walked: a hop down, upright all the
                        // way. Dragged or shoved with no input: a tumble.
                        let meant_it = g.move_dir.length_squared() > 0.01;
                        g.perch = None;
                        g.fall = Some(Fall::new(top, if meant_it { 0.0 } else { sign * 1.5 }));
                    }
                }
            }
            g.perch_hp = g.hp;
            if g.perch.is_none() && g.leap.is_none() && g.dash.is_none() {
                // The ground here: 0 on the jungle, the shelf's height on the
                // pass's climbing lane (`map::ground_y`). Above it by more
                // than a step: a fall. Otherwise stand on it -- walking down
                // a ramp is not falling.
                let ground = crate::map::ground_y(g.pos.x, g.pos.z);
                if g.fall.is_none() {
                    if g.pos.y > ground + crate::map::step_down() {
                        g.fall = Some(crate::leap::Fall::new(g.pos.y, 0.0)); // e.g. a dash that ended in the air
                    } else if g.pos.y != ground && (g.pos.y > ground || crate::map::step_down() > 0.0) {
                        g.pos.y = ground;
                    }
                }
                if let Some(f) = &mut g.fall {
                    let y0 = g.pos.y;
                    let y1 = f.tick(y0, dt);
                    // Whatever standable thing is under the paws catches
                    // the fall: the rock below the wall, not the floor
                    // below the rock.
                    // ...and strictly BELOW where the fall began. Without
                    // that, a cat knocked off a ridge "lands" back on it in
                    // the same tick: a shove of 0.6 u does not clear a wall
                    // 0.9 u in radius, and the integration test said so.
                    let from_y = f.from_y;
                    let under = blockers.landing_under(g.pos.x, g.pos.z, y0).filter(|u| u.0 < from_y - 0.05);
                    let floor = under.map_or(ground, |u| u.0.max(ground));
                    if y1 <= floor {
                        g.pos.y = floor;
                        g.leap = Some(crate::leap::LeapState::landing(g.pos, under, (f.from_y - floor).max(0.0)));
                        g.fall = None;
                    } else {
                        g.pos.y = y1;
                    }
                }
            }

            // arena bounds
            let r = Vec3::new(g.pos.x, 0.0, g.pos.z).length();
            if r > crate::consts::arena_radius() - 2.0 {
                let s = (crate::consts::arena_radius() - 2.0) / r;
                g.pos.x *= s;
                g.pos.z *= s;
            }
        }
        for (gi, team, from, to, dmg, blast) in dash_ends {
            self.dash_strikes(gi, team, from, to, dmg, blast);
        }
        // separation
        for a in 0..self.gods.len() {
            for b in (a + 1)..self.gods.len() {
                let (pa, pb) = (self.gods[a].pos, self.gods[b].pos);
                let d = Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z);
                let dist = d.length();
                if dist < 1.1 && dist > 0.0001 {
                    let push = d.normalize() * (1.1 - dist) * 0.5;
                    self.gods[a].pos -= push;
                    self.gods[b].pos += push;
                }
            }
        }
    }

    fn move_minions(&mut self, dt: f32) {
        self.minion_think(dt);
        let blockers = self.blockers.clone();
        for mi in 0..self.minions.len() {
            let (mteam, mlane, foe) = (self.minions[mi].team, self.minions[mi].lane, self.minions[mi].foe);
            // where to walk: to the foe until in reach; back to where it left
            // the lane; else on down the lane
            let goal: Option<Vec3> = match foe.and_then(|f| self.resolve_foe(f).map(|(_, p)| (f, p))) {
                Some((f, p)) => {
                    let m = &self.minions[mi];
                    let reach = Self::minion_reach(m, f);
                    // casters stand a little inside their reach
                    let want = if m.caster { reach * 0.85 } else { reach - 0.3 };
                    (flat(p - m.pos) > want).then_some(p)
                }
                None => {
                    let m = &mut self.minions[mi];
                    match m.left_at {
                        Some(l) if flat(l - m.pos) > 1.5 => Some(l),
                        _ => {
                            m.left_at = None;
                            let path = crate::consts::lane_path(mlane, mteam);
                            // its own line down the lane, `lane_off` across
                            let slot = |wp: usize| {
                                let w = wp.min(path.len() - 1);
                                let a = path[w.saturating_sub(1)];
                                let b = path[(w + 1).min(path.len() - 1)];
                                let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z).normalize_or_zero();
                                path[w] + Vec3::new(-d.z, 0.0, d.x) * m.lane_off
                            };
                            if flat(slot(m.wp) - m.pos) < 1.5 && m.wp + 1 < path.len() {
                                m.wp += 1;
                            }
                            Some(slot(m.wp))
                        }
                    }
                }
            };
            let m = &mut self.minions[mi];
            match goal {
                Some(target) => {
                    let d = Vec3::new(target.x - m.pos.x, 0.0, target.z - m.pos.z);
                    if d.length_squared() > 0.001 {
                        let shuffle = if m.dance_t > 0.0 { crate::kit::DANCE_MOVE } else { 1.0 };
                        let step = (7.4 * shuffle * dt).min(d.length());
                        m.pos += d.normalize() * step;
                        m.yaw = d.z.atan2(d.x);
                        m.walk_anim += dt * 10.0;
                        // Minions path down a lane and will walk into anything the
                        // lane clips. Resolving here rather than only for gods
                        // matters because a wave piling into a trunk is what makes
                        // props read as solid from across the map.
                        m.pos = blockers.resolve(m.pos, MINION_RADIUS);
                        m.pos.y = crate::map::ground_y(m.pos.x, m.pos.z);
                    }
                }
                None => {
                    // in reach: stand and face the foe
                    if let Some(p) = foe.and_then(|f| self.resolve_foe(f)).map(|(_, p)| p) {
                        let m = &mut self.minions[mi];
                        m.yaw = (p.z - m.pos.z).atan2(p.x - m.pos.x);
                        m.walk_anim += dt * 2.0;
                    }
                }
            }
        }
        // minion separation (cheap: sample pairs)
        for a in 0..self.minions.len() {
            for b in (a + 1)..self.minions.len().min(a + 14) {
                let (pa, pb) = (self.minions[a].pos, self.minions[b].pos);
                let d = Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z);
                let dist = d.length();
                if dist < 0.85 && dist > 0.0001 {
                    let push = d.normalize() * (0.85 - dist) * 0.4;
                    self.minions[a].pos -= push;
                    self.minions[b].pos += push;
                }
            }
        }
    }

    // --- auto attacks -------------------------------------------------------------

    fn god_autoattacks(&mut self, dt: f32) {
        for i in 0..self.gods.len() {
            if self.gods[i].respawn_t > 0.0 {
                continue;
            }
            // a giggling or dancing cat does not strike (`kit`)
            let wants = (self.gods[i].attack_held || self.gods[i].ai.is_some()) && self.kit_free(i);
            if !wants {
                self.gods[i].atk_timer = (self.gods[i].atk_timer - dt).max(0.0);
                self.gods[i].atk_anim = (self.gods[i].atk_anim - dt * 4.0).max(0.0);
                continue;
            }
            // acquire target: nearest enemy god > minion > tower in range
            let g = &self.gods[i];
            let def = self.def(i);
            let range = def.auto_range;
            let mut target: Option<TargetRef> = None;
            let mut bd = range;
            for j in 0..self.gods.len() {
                let e = &self.gods[j];
                // a cat hidden in cloud cannot be locked onto
                if e.team != g.team && e.respawn_t <= 0.0 && self.sees(g.team, j) {
                    let d = e.pos.distance(g.pos);
                    if d < bd {
                        bd = d;
                        target = Some(TargetRef::God(j));
                    }
                }
            }
            if target.is_none() {
                let mut bd2 = range + 0.5;
                for (mi, m) in self.minions.iter().enumerate() {
                    if m.team != g.team {
                        let d = m.pos.distance(g.pos);
                        if d < bd2 {
                            bd2 = d;
                            target = Some(TargetRef::Minion(mi));
                        }
                    }
                }
            }
            if target.is_none() {
                for (ti, tw) in self.towers.iter().enumerate() {
                    if tw.team != g.team {
                        let d = tw.pos.distance(g.pos);
                        if d < range + 2.0 {
                            target = Some(TargetRef::Tower(ti));
                            break;
                        }
                    }
                }
            }
            // timers
            let atk_timer_next = self.gods[i].atk_timer - dt;
            // A target that no longer EXISTS counts as no target. `target` is
            // chosen before this tick's deaths are applied, so `is_none()`
            // alone lets a stale index through to `target_pos` and, before it
            // returned Option, to a bounds panic.
            let target_gone = target.is_some_and(|t| self.target_pos_opt(t).is_none());
            // The attack button with nothing in range: acknowledge it (the
            // shoulder shuffle), at most every 0.8 s -- pressing a button
            // that visibly does nothing reads as a broken button.
            if (target.is_none() || target_gone) && self.gods[i].attack_held && self.gods[i].ai.is_none() && self.t - self.gods[i].whiff_t > 0.8 {
                self.gods[i].whiff_t = self.t;
                self.events.push(Event::Whiff { god: i });
            }
            if atk_timer_next > 0.0 || target.is_none() || target_gone {
                let g = &mut self.gods[i];
                g.atk_timer = atk_timer_next.max(0.0);
                g.atk_anim = (g.atk_anim - dt * 4.0).max(0.0);
                continue;
            }
            // fire!
            let interval = self.auto_interval(i);
            // the Thrice-Pounce Claws count this blow (curios.rs)
            let dmg = self.auto_damage(i) + self.pounce_bonus(i);
            let (pen_pct, pen_flat) = (self.gods[i].pen_pct, self.gods[i].pen_flat);
            let team = self.gods[i].team;
            let from = self.gods[i].pos + Vec3::Y * 1.4;
            let now_t = self.t;
            let g = &mut self.gods[i];
            g.atk_timer = interval;
            g.atk_anim = 1.0;
            g.last_act_t = now_t;
            match def.auto {
                AutoKind::Melee => {
                    let to = self.target_pos(target.unwrap());
                    let dir = (to - from).normalize_or_zero();
                    self.events.push(Event::DashFx { pos: from + dir * 1.2, dir, color: team.rgb() });
                    let magic = def.dmg_type == crate::gods::DmgType::Magical;
                    let main = target.unwrap();
                    let profile = crate::brawl::melee_for(def);
                    // from behind (assassins most): the dive's reward
                    let mut dmg = dmg;
                    if let (Some(m), TargetRef::God(j)) = (profile, main) {
                        if self.from_behind(i, j) {
                            dmg *= m.backstab;
                        }
                    }
                    let extra = profile.map(|m| self.cleave_targets(i, main, def.auto_range * 1.15, m)).unwrap_or_default();
                    self.basic_attack = true;
                    self.deal_damage(main, dmg, magic, Some(i), pen_pct, pen_flat);
                    // the sweep or cleave catches those beside it
                    let splash = profile.map_or(0.0, |m| m.splash);
                    for t in extra {
                        self.deal_damage(t, dmg * splash, magic, Some(i), pen_pct, pen_flat);
                    }
                    self.basic_attack = false;
                }
                AutoKind::Ranged => {
                    let to = self.target_pos(target.unwrap());
                    let dir = (to - from).normalize_or_zero();
                    let speed = 42.0;
                    self.projectiles.push(Projectile {
                        team,
                        pos: from,
                        vel: dir * speed,
                        dmg,
                        radius: 0.55,
                        life: 2.2,
                        kind: ProjKind::Arrow,
                        owner: i,
                        pen_pct,
                        pen_flat,
                        color: team.rgb(),
                        auto: true,
                    });
                }
            }
        }
    }

    /// Where a target is, or `None` if it is no longer there.
    ///
    /// A `TargetRef` is an INDEX, and `on_minion_death` does
    /// `self.minions.remove(mi)`, which shifts every later index down by one.
    /// Any ref taken before that removal — an AI's chosen target held across
    /// frames, a projectile in flight, a beam being drawn — now names a
    /// different minion, and a ref to the last one names nothing at all.
    ///
    /// That is the reported crash, on an M2 Max: "index out of bounds: the len
    /// is 10 but the index is 10", panicking inside `Scene::draw`. The
    /// renderer is the victim rather than the culprit; it drew a beam at a
    /// minion the sim had already removed.
    ///
    /// Returning `Option` rather than clamping or holding the last position:
    /// a clamped index silently retargets onto whichever minion shifted into
    /// the slot, which is a wrong hit that looks like a right one, and a
    /// remembered position draws a beam at empty ground. Absent is what the
    /// target actually is.
    pub fn target_pos_opt(&self, t: TargetRef) -> Option<Vec3> {
        Some(match t {
            TargetRef::God(i) => self.gods.get(i)?.pos,
            TargetRef::Minion(i) => self.minions.get(i)?.pos,
            TargetRef::Tower(i) => self.towers.get(i)?.pos,
            TargetRef::Titan(team) => self.titans.get(team as usize)?.pos,
            TargetRef::Boss(bi) => self.bosses.list.get(bi)?.pos,
        })
    }

    /// Where a target is, falling back to the origin when it has gone.
    ///
    /// Kept for the call sites that cannot meaningfully skip — see
    /// [`Self::target_pos_opt`] for why a missing target is a real state and
    /// not an error. Prefer the `_opt` form anywhere the work can simply not
    /// happen.
    pub fn target_pos(&self, t: TargetRef) -> Vec3 {
        self.target_pos_opt(t).unwrap_or(Vec3::ZERO)
    }

    fn minion_attacks(&mut self, dt: f32) {
        let n = self.minions.len();
        for mi in 0..n {
            if mi >= self.minions.len() {
                break; // earlier kill shifted indices
            }
            // the held foe (`minion_think`), if it is in reach
            let target: Option<TargetRef> = {
                let m = &self.minions[mi];
                m.foe.and_then(|f| {
                    let (t, p) = self.resolve_foe(f)?;
                    (flat(p - m.pos) < Self::minion_reach(m, f)).then_some(t)
                })
            };
            let surge = self.lane_surge(self.minions[mi].team, self.minions[mi].lane);
            let m = &mut self.minions[mi];
            m.atk_timer -= dt;
            m.atk_anim = (m.atk_anim - dt * 4.0).max(0.0);
            if m.atk_timer > 0.0 || target.is_none() || m.giggle_t > 0.0 || m.dance_t > 0.0 {
                continue;
            }
            m.atk_timer = MINION_ATK_CD;
            m.atk_anim = 1.0;
            let mut dmg = if m.caster { MINION_CASTER_DMG } else { MINION_DMG };
            dmg *= 1.0 + (self.t / 60.0).min(15.0) * 0.08;
            dmg *= surge;
            // structures take bonus minion damage so lanes actually fall
            if matches!(target.unwrap(), TargetRef::Tower(_) | TargetRef::Titan(_)) {
                dmg *= 2.4 * crate::map::active().minion_structure_mul();
            }
            match target.unwrap() {
                TargetRef::God(gi) => {
                    if m.caster {
                        self.spawn_minion_bolt(mi, TargetRef::God(gi));
                    } else {
                        self.deal_damage(TargetRef::God(gi), dmg, false, None, 0.0, 0.0);
                    }
                }
                TargetRef::Minion(oi) => {
                    if m.caster {
                        self.spawn_minion_bolt(mi, TargetRef::Minion(oi));
                    } else {
                        self.deal_damage(TargetRef::Minion(oi), dmg, false, None, 0.0, 0.0);
                    }
                }
                TargetRef::Tower(ti) => self.deal_damage(TargetRef::Tower(ti), dmg, false, None, 0.0, 0.0),
                TargetRef::Titan(team) => self.deal_damage(TargetRef::Titan(team), dmg, false, None, 0.0, 0.0),
                TargetRef::Boss(_) => {} // minions never fight the beasts
            }
        }
    }

    fn spawn_minion_bolt(&mut self, mi: usize, target: TargetRef) {
        let (pos, team) = (self.minions[mi].pos + Vec3::Y * 1.0, self.minions[mi].team);
        // Skip rather than fire at the origin: the target died between being
        // chosen and being shot at, which is ordinary in a lane fight.
        let Some(tp) = self.target_pos_opt(target) else { return };
        let to = tp + Vec3::Y * 1.0;
        let dir = (to - pos).normalize_or_zero();
        let dmg = MINION_CASTER_DMG * (1.0 + (self.t / 60.0).min(15.0) * 0.08);
        self.projectiles.push(Projectile {
            team,
            pos,
            vel: dir * 26.0,
            dmg,
            radius: 0.5,
            life: 1.6,
            kind: ProjKind::Bolt,
            owner: usize::MAX,
            pen_pct: 0.0,
            pen_flat: 0.0,
            color: team.rgb(),
            auto: false,
        });
    }

    fn tower_attacks(&mut self, dt: f32) {
        let n = self.towers.len();
        for ti in 0..n {
            let tw = &self.towers[ti];
            if tw.hp <= 0.0 {
                continue;
            }
            // target: minions first, then gods
            let mut target: Option<TargetRef> = None;
            let mut bd = TOWER_RANGE;
            for (mi, m) in self.minions.iter().enumerate() {
                if m.team != tw.team {
                    let d = m.pos.distance(tw.pos);
                    if d < bd {
                        bd = d;
                        target = Some(TargetRef::Minion(mi));
                    }
                }
            }
            if target.is_none() {
                let mut bd2 = TOWER_RANGE;
                for gi in 0..self.gods.len() {
                    let g = &self.gods[gi];
                    if g.team != tw.team && g.respawn_t <= 0.0 {
                        let d = g.pos.distance(tw.pos);
                        if d < bd2 {
                            bd2 = d;
                            target = Some(TargetRef::God(gi));
                        }
                    }
                }
                // ramp reset when switching to minions
                let tw = &mut self.towers[ti];
                if tw.last_target.map_or(false, |t| matches!(t, TargetRef::Minion(_))) {
                    tw.ramp = 0;
                }
            }
            // timers
            let atk_timer_next = self.towers[ti].atk_timer - dt;
            self.towers[ti].aim = target;
            let fire_anim_next = (self.towers[ti].fire_anim - dt * 3.0).max(0.0);
            // A target that no longer EXISTS counts as no target. `target` is
            // chosen before this tick's deaths are applied, so `is_none()`
            // alone lets a stale index through to `target_pos` and, before it
            // returned Option, to a bounds panic.
            let target_gone = target.is_some_and(|t| self.target_pos_opt(t).is_none());
            if atk_timer_next > 0.0 || target.is_none() || target_gone {
                let tw = &mut self.towers[ti];
                tw.atk_timer = atk_timer_next.max(0.0);
                tw.fire_anim = fire_anim_next;
                continue;
            }
            // fire!
            let last_target = self.towers[ti].last_target;
            let cur_ramp = self.towers[ti].ramp;
            let is_god = matches!(target.unwrap(), TargetRef::God(_));
            let new_ramp = if is_god && last_target == target {
                (cur_ramp + 1).min(3)
            } else if is_god {
                cur_ramp
            } else {
                0
            };
            let dmg = TOWER_DMG * TOWER_RAMP.powi(new_ramp as i32);
            let from = self.towers[ti].pos + Vec3::Y * 5.2;
            let Some(tp) = self.target_pos_opt(target.unwrap()) else { continue };
            let to = tp + Vec3::Y * 1.0;
            let dir = (to - from).normalize_or_zero();
            let tteam = self.towers[ti].team;
            let tw = &mut self.towers[ti];
            tw.atk_timer = TOWER_ATK_CD;
            tw.fire_anim = 1.0;
            tw.ramp = new_ramp;
            tw.last_target = target;
            // Applied as it fires rather than on impact: the bolt has no flag to
            // carry it, and at 34 u/s over <= 24 u it lands a moment later.
            if let Some(TargetRef::God(gi)) = target {
                let g = &mut self.gods[gi];
                g.slow = g.slow.max(TOWER_SLOW);
                g.slow_t = g.slow_t.max(TOWER_SLOW_SECS);
            }
            self.projectiles.push(Projectile {
                team: tteam,
                pos: from,
                vel: dir * 34.0,
                dmg,
                radius: 0.7,
                life: 2.0,
                kind: ProjKind::Bolt,
                owner: usize::MAX,
                pen_pct: 0.0,
                pen_flat: 0.0,
                color: tw.team.rgb(),
                auto: false,
            });
        }
    }

    /// Is `p` on the Elder's ground: within `TITAN_MOVE_R` of its pedestal,
    /// or in the push lane running `TITAN_PUSH` out from it toward the enemy.
    pub fn titan_zone(&self, t: usize, p: Vec3) -> bool {
        titan_zone(self.titans[t].home, self.titans[t].team, p)
    }

    /// The Elders: each guards its ground. It notices an enemy cat (else a
    /// minion) within `TITAN_SIGHT` that stands on its ground, turns to it,
    /// strides after it -- never off its ground -- and strikes when it is in
    /// reach and faced. With nothing to fight it walks home to its pedestal
    /// and turns to face the enemy again.
    fn titan_attacks(&mut self, dt: f32) {
        for t in 0..2 {
            let titan = self.titans[t];
            if titan.hp <= 0.0 {
                continue;
            }
            let team = titan.team;
            let mut target: Option<(TargetRef, Vec3)> = None;
            let mut bd = TITAN_SIGHT;
            for gi in 0..self.gods.len() {
                let g = &self.gods[gi];
                if g.team != team && g.respawn_t <= 0.0 && titan_zone(titan.home, team, g.pos) {
                    let d = flat(g.pos - titan.pos);
                    if d < bd {
                        bd = d;
                        target = Some((TargetRef::God(gi), g.pos));
                    }
                }
            }
            if target.is_none() {
                for mi in 0..self.minions.len() {
                    let m = &self.minions[mi];
                    if m.team != team && titan_zone(titan.home, team, m.pos) {
                        let d = flat(m.pos - titan.pos);
                        if d < bd {
                            bd = d;
                            target = Some((TargetRef::Minion(mi), m.pos));
                        }
                    }
                }
            }
            // where to stand and which way to face
            let (goal, face) = match target {
                Some((_, p)) => {
                    let to = Vec3::new(p.x - titan.pos.x, 0.0, p.z - titan.pos.z);
                    let stop = if to.length() > TITAN_RANGE * 0.8 { p - to.normalize_or_zero() * (TITAN_RANGE * 0.7) } else { titan.pos };
                    (stop, to.z.atan2(to.x))
                }
                None => {
                    let home = titan.home;
                    let at_home = flat(home - titan.pos) < 0.6;
                    let to = home - titan.pos;
                    let rest = if team.push_z() > 0.0 { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 };
                    (home, if at_home { rest } else { to.z.atan2(to.x) })
                }
            };
            let mut ti = self.titans[t];
            // turn, at a heavy pace
            let dyaw = (face - ti.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            let turn = dyaw.clamp(-TITAN_TURN * dt, TITAN_TURN * dt);
            ti.yaw = (ti.yaw + turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            // stride: slower while it is still turning to face its way
            let to = Vec3::new(goal.x - ti.pos.x, 0.0, goal.z - ti.pos.z);
            let want = if to.length() > 0.3 { TITAN_SPEED * (1.0 - (dyaw.abs() / 1.2).min(0.8)) } else { 0.0 };
            ti.stride += (want - ti.stride) * (dt * 3.0).min(1.0);
            if to.length() > 0.05 && ti.stride > 0.01 {
                let step = to.normalize_or_zero() * (ti.stride * dt).min(to.length());
                let next = ti.pos + step;
                // never off its ground, never into rock
                if titan_zone(ti.home, team, next) && !self.blockers.blocked(next.x, next.z, 1.5) {
                    ti.pos = next;
                } else {
                    ti.stride = 0.0;
                }
            }
            ti.walk = (ti.walk + ti.stride * dt * 1.3).rem_euclid(std::f32::consts::TAU);
            ti.atk_anim = (ti.atk_anim - dt * 3.0).max(0.0);
            ti.atk_timer = (ti.atk_timer - dt).max(0.0);
            self.titans[t] = ti;
            // strike what is in reach and faced
            let Some((tr, p)) = target else { continue };
            let in_reach = flat(p - ti.pos) < TITAN_RANGE;
            if ti.atk_timer > 0.0 || !in_reach || dyaw.abs() > TITAN_FACE {
                continue;
            }
            {
                let titan = &mut self.titans[t];
                titan.atk_timer = TITAN_ATK_CD;
                titan.atk_anim = 1.0;
            }
            self.deal_damage(tr, TITAN_DMG, false, None, 0.0, 0.0);
            let slam = ti.pos + ti.fwd() * 3.0;
            self.events.push(Event::NovaFx { pos: slam + Vec3::Y * 0.2, radius: 3.0, color: team.rgb() });
        }
    }

    // --- projectiles ----------------------------------------------------------------

    fn projectiles_tick(&mut self, dt: f32) {
        let mut dead = vec![false; self.projectiles.len()];
        for pi in 0..self.projectiles.len() {
            let p = &mut self.projectiles[pi];
            match p.kind {
                ProjKind::Lob { start, target, t, dur, radius } => {
                    p.life -= dt;
                    let nt = (t + dt / dur).min(1.0);
                    let height = nt * (1.0 - nt) * 4.5;
                    p.pos = start.lerp(target, nt) + Vec3::Y * height;
                    if nt >= 1.0 || p.life <= 0.0 {
                        // detonate
                        dead[pi] = true;
                        let team = p.team;
                        let (dmg, pen_pct, pen_flat, owner) = (p.dmg, p.pen_pct, p.pen_flat, p.owner);
                        self.events.push(Event::Explosion { pos: Vec3::new(target.x, 0.3, target.z), radius, color: p.color });
                        self.aoe_damage(Vec3::new(target.x, 0.0, target.z), radius, team, dmg, owner, pen_pct, pen_flat);
                    }
                    continue;
                }
                _ => {
                    p.pos += p.vel * dt;
                    p.life -= dt;
                    if p.life <= 0.0 || p.pos.y < -0.5 || Vec3::new(p.pos.x, 0.0, p.pos.z).length() > crate::consts::arena_radius() + 4.0 {
                        dead[pi] = true;
                        continue;
                    }
                }
            }
            // collision: enemy units within radius+0.6
            let p = &self.projectiles[pi];
            let mut hit: Option<TargetRef> = None;
            for gi in 0..self.gods.len() {
                let g = &self.gods[gi];
                if g.team != p.team && g.respawn_t <= 0.0 {
                    // The body, not a ball over the feet: see `hitbox`.
                    if crate::hitbox::cat(g.pos, g.yaw, g.sneak && g.hush_t <= 0.0).hit_by_moving_sphere(p.pos - p.vel * dt, p.pos, p.radius + 0.2) {
                        hit = Some(TargetRef::God(gi));
                        break;
                    }
                }
            }
            if hit.is_none() {
                for mi in 0..self.minions.len() {
                    let m = &self.minions[mi];
                    if m.team != p.team {
                        if crate::hitbox::minion(m.pos, m.yaw).hit_by_moving_sphere(p.pos - p.vel * dt, p.pos, p.radius + 0.2) {
                            hit = Some(TargetRef::Minion(mi));
                            break;
                        }
                    }
                }
            }
            if hit.is_none() {
                for ti in 0..self.towers.len() {
                    let tw = &self.towers[ti];
                    if tw.team != p.team {
                        let c = tw.pos + Vec3::Y * 2.5;
                        if (c - p.pos).length() < p.radius + 1.6 {
                            hit = Some(TargetRef::Tower(ti));
                            break;
                        }
                    }
                }
            }
            if hit.is_none() {
                for t in 0..2 {
                    let tt = &self.titans[t];
                    if tt.team != p.team {
                        let c = tt.pos + Vec3::Y * 2.5;
                        if (c - p.pos).length() < p.radius + 2.2 {
                            hit = Some(TargetRef::Titan(tt.team));
                            break;
                        }
                    }
                }
            }
            if hit.is_none() {
                for (bi, b) in self.bosses.list.iter().enumerate() {
                    if b.grounded() && (b.pos + Vec3::Y * 2.0 - p.pos).length() < p.radius + 2.2 {
                        hit = Some(TargetRef::Boss(bi));
                        break;
                    }
                }
            }
            if let Some(h) = hit {
                let (pdmg, ppen, ppf, powner, ppos, pcolor, pauto) = {
                    let p = &self.projectiles[pi];
                    (p.dmg, p.pen_pct, p.pen_flat, p.owner, p.pos, p.color, p.auto)
                };
                let owner = if powner == usize::MAX { None } else { Some(powner) };
                self.basic_attack = pauto;
                self.deal_damage(h, pdmg, false, owner, ppen, ppf);
                self.basic_attack = false;
                self.events.push(Event::Explosion { pos: ppos, radius: 0.9, color: pcolor });
                dead[pi] = true;
            }
        }
        let mut i = 0;
        self.projectiles.retain(|_| {
            let keep = !dead[i];
            i += 1;
            keep
        });
    }

    pub fn aoe_damage(&mut self, center: Vec3, radius: f32, team: Team, dmg: f32, owner: usize, pen_pct: f32, pen_flat: f32) {
        let mut hits: Vec<TargetRef> = Vec::new();
        for gi in 0..self.gods.len() {
            let g = &self.gods[gi];
            if g.team != team && g.respawn_t <= 0.0 && (g.pos.y - center.y).abs() < crate::brawl::AOE_REACH_Y && crate::hitbox::cat(g.pos, g.yaw, false).in_disc(center, radius + 0.2) {
                hits.push(TargetRef::God(gi));
            }
        }
        for mi in 0..self.minions.len() {
            let m = &self.minions[mi];
            if m.team != team && m.pos.distance(center) <= radius + 0.4 {
                hits.push(TargetRef::Minion(mi));
            }
        }
        for ti in 0..self.towers.len() {
            let tw = &self.towers[ti];
            if tw.team != team && tw.pos.distance(center) <= radius + 1.5 {
                hits.push(TargetRef::Tower(ti));
            }
        }
        for t in 0..2 {
            let tt = &self.titans[t];
            if tt.team != team && tt.pos.distance(center) <= radius + 2.5 {
                hits.push(TargetRef::Titan(tt.team));
            }
        }
        for (bi, b) in self.bosses.list.iter().enumerate() {
            if b.grounded() && b.pos.distance(center) <= radius + 2.0 {
                hits.push(TargetRef::Boss(bi));
            }
        }
        // a crowd thins it: each extra cat caught lowers every cat's share
        // (brawl::aoe_falloff); minions and structures take it whole
        let cats = hits.iter().filter(|h| matches!(h, TargetRef::God(_))).count();
        let thin = crate::brawl::aoe_falloff(cats);
        for h in hits {
            let d = if matches!(h, TargetRef::God(_)) { dmg * thin } else { dmg };
            self.deal_damage(h, d, true, if owner == usize::MAX { None } else { Some(owner) }, pen_pct, pen_flat);
        }
    }

    // --- zones ------------------------------------------------------------------

    fn zones_tick(&mut self, dt: f32) {
        let mut i = 0;
        while i < self.zones.len() {
            // snapshot (no outstanding borrows)
            let (zteam, zpos, zradius, zcolor, kind_id, timer, val, strike, owner) = {
                let z = &self.zones[i];
                let (kid, t, v, st) = match &z.kind {
                    ZoneKind::Storm { until, dps, strike_t, owner: _ } => (0u8, *until, *dps, *strike_t),
                    ZoneKind::Ring { trigger_t, dmg, .. } => (1u8, *trigger_t, *dmg, 0.0),
                    ZoneKind::Dance { until, .. } => (2u8, *until, 0.0, 0.0),
                };
                let own = match &z.kind {
                    ZoneKind::Storm { owner, .. } | ZoneKind::Ring { owner, .. } | ZoneKind::Dance { owner, .. } => *owner,
                };
                (z.team, z.pos, z.radius, z.color, kid, t, v, st, own)
            };

            let mut remove = false;
            let new_timer = timer - dt;
            if kind_id == 0 {
                // storm: tick damage + strikes (snapshot targets first —
                // deal_damage may remove minions and shift indices)
                let mut targets = Vec::new();
                for gi in 0..self.gods.len() {
                    let g = &self.gods[gi];
                    if g.team != zteam && g.respawn_t <= 0.0 && g.pos.distance(zpos) <= zradius {
                        targets.push(TargetRef::God(gi));
                    }
                }
                for mi in 0..self.minions.len() {
                    let m = &self.minions[mi];
                    if m.team != zteam && m.pos.distance(zpos) <= zradius {
                        targets.push(TargetRef::Minion(mi));
                    }
                }
                for t in targets {
                    self.deal_damage(t, val * dt, true, Some(owner), 0.0, 0.0);
                }
                let mut strike2 = strike - dt;
                if strike2 <= 0.0 {
                    strike2 = 0.45;
                    let ang = self.rng.f32() * std::f32::consts::TAU;
                    let r = self.rng.f32() * zradius * 0.8;
                    let at = zpos + Vec3::new(ang.cos() * r, 0.0, ang.sin() * r);
                    self.events.push(Event::StrikeFx { at, color: zcolor });
                    self.aoe_damage(at, 1.8, zteam, val * 0.5, owner, 0.0, 0.0);
                }
                if new_timer <= 0.0 {
                    remove = true;
                }
                if let ZoneKind::Storm { until, strike_t, .. } = &mut self.zones[i].kind {
                    *until = new_timer;
                    *strike_t = strike2;
                }
            } else if kind_id == 2 {
                // the glitter-ball: its dancers are `kit_tick`'s; here it
                // only counts down
                if new_timer <= 0.0 {
                    remove = true;
                } else if let ZoneKind::Dance { until, .. } = &mut self.zones[i].kind {
                    *until = new_timer;
                }
            } else {
                // ring: detonate at trigger
                if new_timer <= 0.0 {
                    self.events.push(Event::Explosion { pos: zpos, radius: zradius, color: zcolor });
                    self.aoe_damage(zpos, zradius, zteam, val, owner, 0.0, 0.0);
                    remove = true;
                } else if let ZoneKind::Ring { trigger_t, .. } = &mut self.zones[i].kind {
                    *trigger_t = new_timer;
                }
            }

            if remove {
                self.zones.remove(i);
            } else {
                i += 1;
            }
        }
        // apply storm slows
        let storm_zones: Vec<(Team, Vec3, f32)> = self
            .zones
            .iter()
            .filter(|z| matches!(z.kind, ZoneKind::Storm { .. }))
            .map(|z| (z.team, z.pos, z.radius))
            .collect();
        for (zteam, zpos, zradius) in storm_zones {
            for g in self.gods.iter_mut() {
                if g.team != zteam && g.pos.distance(zpos) <= zradius {
                    g.slow = g.slow.max(0.25);
                    g.slow_t = g.slow_t.max(0.3);
                }
            }
        }
    }

    // --- timers -------------------------------------------------------------------

    fn timers_tick(&mut self, dt: f32) {
        for i in 0..self.gods.len() {
            // precompute respawn spawn (needs &mut rng before borrowing god)
            let will_respawn = self.gods[i].respawn_t > 0.0 && self.gods[i].respawn_t - dt <= 0.0;
            let spawn = if will_respawn {
                let lane = self.gods[i].ai.as_ref().map_or(Lane::Mid, |a| a.lane);
                Some(Self::spawn_pos(self.gods[i].team, lane, &mut self.rng))
            } else {
                None
            };
            let was_dead = self.gods[i].respawn_t > 0.0;
            let last_dmg = self.gods[i].last_dmg_t;
            let now = self.t;
            // out of the fight: after 6 s, 1.2% a second -- or the Moonmilk
            // Flask's quicker, deeper mending (curios.rs)
            let (mend_after, mend_pct) = self.mending(i);
            if !was_dead {
                self.curios_tick(i, dt);
            }

            {
                let g = &mut self.gods[i];
                // Quickening (the Moonwell): cooldowns at double speed
                let rate = crate::moonwell::cooldown_rate(g.boon.map(|b| b.0));
                for c in g.cds.iter_mut() {
                    *c = (*c - dt * rate).max(0.0);
                }
                if g.buff_t > 0.0 {
                    g.buff_t -= dt;
                    if g.buff_t <= 0.0 {
                        g.buff_atkspd = 0.0;
                        g.buff_ms = 0.0;
                    }
                }
                if g.blessing_t > 0.0 {
                    g.blessing_t -= dt;
                    g.mana = (g.mana + g.max_mana * BLESSING_MP_PER_S * dt).min(g.max_mana);
                }
                if g.slow_t > 0.0 {
                    g.slow_t -= dt;
                    if g.slow_t <= 0.0 {
                        g.slow = 0.0;
                    }
                }
                if g.shield_t > 0.0 {
                    g.shield_t -= dt;
                    if g.shield_t <= 0.0 {
                        g.shield = 0.0;
                    }
                }
                g.atk_anim = (g.atk_anim - dt * 4.0).max(0.0);

                if was_dead {
                    g.respawn_t -= dt;
                    if g.respawn_t <= 0.0 {
                        g.pos = spawn.unwrap();
                        g.hp = g.max_hp;
                        g.mana = g.max_mana;
                        g.recent_damagers.clear();
                    }
                } else {
                    // regen
                    g.mana = (g.mana + (2.6 + 0.5 * g.level as f32) * dt).min(g.max_mana);
                    if now - last_dmg > mend_after {
                        g.hp = (g.hp + g.max_hp * mend_pct * dt).min(g.max_hp);
                    }
                    let f = g.team.fountain();
                    if g.pos.distance(f) < FOUNTAIN_RADIUS {
                        g.hp = (g.hp + g.max_hp * FOUNTAIN_REGEN * dt * 4.0).min(g.max_hp);
                        g.mana = (g.mana + g.max_mana * FOUNTAIN_REGEN * dt * 4.0).min(g.max_mana);
                    }
                }
            }

            // fountain damages enemies (after the mutable borrow ends)
            if !was_dead {
                let gteam = self.gods[i].team;
                let f = gteam.fountain();
                if self.gods[i].pos.distance(f) < FOUNTAIN_RADIUS {
                    for e in 0..self.gods.len() {
                        let eg = &self.gods[e];
                        if eg.team != gteam && eg.respawn_t <= 0.0 && eg.pos.distance(f) < FOUNTAIN_RADIUS {
                            self.deal_damage(TargetRef::God(e), FOUNTAIN_DPS * dt, false, None, 0.0, 0.0);
                        }
                    }
                }
            }
        }
        // AI auto-buy occasionally
        for i in 0..self.gods.len() {
            if self.gods[i].ai.is_some() {
                let due = {
                    let b = self.gods[i].ai.as_mut().unwrap();
                    b.buy_t -= dt;
                    b.buy_t <= 0.0
                };
                if due {
                    self.gods[i].ai.as_mut().unwrap().buy_t = 2.0;
                    self.auto_buy(i);
                }
            }
        }
    }
}

impl GodUnit {
    pub fn yaw_toward(&mut self, dir: Vec3, dt: f32) -> f32 {
        let want = dir.z.atan2(dir.x);
        let mut d = want - self.yaw;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        self.yaw + d * (dt * 14.0).min(1.0)
    }
}

/// Convenience for renderer: archetype color of an ability.
pub fn ability_color(def: &crate::gods::GodDef, slot: u32) -> [f32; 3] {
    def.abilities[slot as usize].color
}

/// Re-export for headless harness.
pub use crate::gods::Archetype as ArchT;
pub type ArchAlias = Archetype;

/// Half-angle of the facing cone round the player's view: movement turns
/// the cat at most this far from where the camera looks.
pub const FACE_CONE: f32 = 1.2; // ~70 degrees
/// Moving further than this from the view direction (toward the camera),
/// the cat does not turn round: it faces the view and backs up.
pub const BACKPEDAL: f32 = 2.0; // ~115 degrees

/// The yaw a moving cat turns toward: its movement direction, clamped into
/// the facing cone round `view` (none if `view` is zero: bots face where
/// they go); past BACKPEDAL, the view itself.
pub fn facing_yaw(move_dir: Vec3, view: Vec3) -> f32 {
    let m = crate::abilities::yaw_of(move_dir);
    if view.length_squared() < 1e-6 {
        return m;
    }
    let v = crate::abilities::yaw_of(view);
    let mut d = m - v;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    if d.abs() > BACKPEDAL {
        v
    } else {
        v + d.clamp(-FACE_CONE, FACE_CONE)
    }
}

/// Horizontal length: minions and their targets stand at different
/// heights on sloped ground, and reach is measured across the ground.
fn flat(v: Vec3) -> f32 {
    Vec3::new(v.x, 0.0, v.z).length()
}

/// The Elder's ground (see `Game::titan_zone`), for an Elder whose pedestal
/// is `home`: a disc round it, and a lane out from it toward the enemy.
pub fn titan_zone(home: Vec3, team: Team, p: Vec3) -> bool {
    let d = Vec3::new(p.x - home.x, 0.0, p.z - home.z);
    if d.length() <= TITAN_MOVE_R {
        return true;
    }
    let ahead = d.z * team.push_z();
    ahead >= 0.0 && ahead <= TITAN_PUSH && d.x.abs() <= TITAN_PUSH_HALF
}
