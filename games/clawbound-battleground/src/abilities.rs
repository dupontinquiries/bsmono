// Ability casting — all 13 archetype mechanics + their voxel VFX events.

use crate::consts::Team;
use crate::gods::Archetype;
use crate::sim::*;
use glam::Vec3;

/// How an ability is cast: wind-up seconds before it fires, and the
/// caster's movement multiplier while it winds up (0 = rooted, a channel).
/// One table so the balance reads in one place:
/// - escapes and self-buffs are instant (Blink, Buff, Shield): they must
///   answer a threat the moment it appears;
/// - Dash nearly so;
/// - skillshots get a short tell (Bolt 0.18 s at 60% speed; Ring, Volley,
///   Lob 0.25-0.30 s);
/// - big areas and control telegraph long enough to react to (Nova, Heal
///   0.35 s, Pull 0.40 s slowed hard; Beam and Storm channel, rooted);
/// - an ultimate winds up 1.5x as long (at least 0.45 s) and roots.
pub fn cast_timing(ab: &crate::gods::AbilityDef) -> (f32, f32) {
    let (w, m): (f32, f32) = match ab.arch {
        Archetype::Blink { .. } | Archetype::Buff { .. } | Archetype::Shield { .. } => (0.0, 1.0),
        Archetype::Dash { .. } => (0.08, 1.0),
        Archetype::Bolt { .. } => (0.18, 0.6),
        Archetype::Ring { .. } => (0.25, 0.6),
        Archetype::Volley { .. } => (0.28, 0.5),
        Archetype::Lob { .. } => (0.30, 0.6),
        Archetype::Nova { .. } => (0.35, 0.4),
        Archetype::Heal { .. } => (0.35, 0.5),
        Archetype::Pull { .. } => (0.40, 0.4),
        Archetype::Beam { .. } => (0.45, 0.0),
        Archetype::Storm { .. } => (0.55, 0.0),
        // the crouch before the vault
        Archetype::Meteor { .. } => (0.3, 0.3),
        // the litter tumbles out at once; the laugh is a breath; the ball
        // takes a moment to rise
        Archetype::Litter { .. } => (0.12, 0.8),
        Archetype::Giggle { .. } => (0.2, 0.6),
        Archetype::DanceParty { .. } => (0.4, 0.0),
        // the step is a held breath; the other goes at a gesture; the echoes
        // take a moment to rise
        Archetype::Swap { .. } => (0.12, 0.0),
        Archetype::Send { .. } => (0.1, 0.8),
        Archetype::Echoes { .. } => (0.35, 0.0),
        // the lamp goes up with the breath for the bellow
        Archetype::Lantern { .. } => (0.4, 0.0),
        // a coat is not cast, it is worn: the shift is at once, and she
        // keeps walking (`sentinel`)
        Archetype::Field { .. } => (0.0, 1.0),
    };
    if ab.ult && w > 0.0 {
        ((w * 1.5).max(0.45), 0.0)
    } else {
        (w, m)
    }
}

/// The cat's yaw facing along `dir` (a cat faces (cos yaw, 0, sin yaw)).
pub fn yaw_of(dir: Vec3) -> f32 {
    dir.z.atan2(dir.x)
}

/// The cat's facing as a direction (the inverse of `yaw_of`).
pub fn facing_of(yaw: f32) -> Vec3 {
    Vec3::new(yaw.cos(), 0.0, yaw.sin())
}

impl Game {
    /// Begin a cast: checks it can be cast now, turns the cat to its aim,
    /// and either fires it (no wind-up) or starts the wind-up; `cast` fires
    /// it when the wind-up ends (`tick_casting`). Cooldown and mana are spent
    /// when it FIRES, so an interrupted cast costs nothing.
    /// Slot `slot` of cat `gi` as its level ranks it; None while locked.
    pub fn ability(&self, gi: usize, slot: u32) -> Option<crate::gods::AbilityDef> {
        let ab = self.def(gi).abilities[slot.min(3) as usize];
        crate::ranks::ranked(ab, crate::ranks::rank(slot, self.gods[gi].level))
    }

    pub fn begin_cast(&mut self, gi: usize, slot: u32, aim: Vec3) {
        if self.over.is_some() || self.gods[gi].respawn_t > 0.0 || self.gods[gi].casting.is_some() || self.gods[gi].skyfall.is_some() {
            return;
        }
        // giggling or dancing (`kit`): no casting
        if !self.kit_free(gi) {
            return;
        }
        // locked until its level (`ranks`)
        let Some(ab) = self.ability(gi, slot) else { return };
        let g = &self.gods[gi];
        if g.cds[slot as usize] > 0.0 || g.mana < ab.cost {
            return;
        }
        let (wind, move_mul) = cast_timing(&ab);
        // turn to the aim now: a cast always faces where it goes
        let d = Vec3::new(aim.x - g.pos.x, 0.0, aim.z - g.pos.z);
        let g = &mut self.gods[gi];
        if d.length() > 0.3 {
            g.yaw = yaw_of(d);
        }
        g.face_hold = wind + 0.3;
        // gathering to cast is already acting: it breaks a stalk from the
        // first frame of the wind-up, not when the ability lands
        g.last_act_t = self.t;
        if wind <= 0.0 {
            self.cast(gi, slot, aim);
        } else {
            g.casting = Some(Casting { slot, aim, t: 0.0, dur: wind, move_mul });
            self.events.push(Event::CastStart { god: gi, slot, aim, dur: wind });
        }
    }

    /// Advance wind-ups; fire those that finish; drop those interrupted (the
    /// cat died, leapt or is dashing).
    pub fn tick_casting(&mut self, dt: f32) {
        for gi in 0..self.gods.len() {
            let Some(mut c) = self.gods[gi].casting else { continue };
            let g = &self.gods[gi];
            if g.respawn_t > 0.0 || g.leap.is_some() || g.dash.is_some() {
                self.gods[gi].casting = None;
                continue;
            }
            c.t += dt;
            if c.t >= c.dur {
                self.gods[gi].casting = None;
                self.cast(gi, c.slot, c.aim);
            } else {
                self.gods[gi].casting = Some(c);
            }
        }
    }

    /// Execute one cast. `aim` is a ground point (abilities aim through it).
    pub fn cast(&mut self, gi: usize, slot: u32, aim: Vec3) {
        if self.over.is_some() || self.gods[gi].respawn_t > 0.0 {
            return;
        }
        // the ability at the rank the cat's level gives it (`ranks`)
        let Some(ab) = self.ability(gi, slot) else { return };
        let g = &self.gods[gi];
        if g.cds[slot as usize] > 0.0 || g.mana < ab.cost {
            return;
        }
        // cooldown with CDR
        let cd = ab.cooldown * (1.0 - g.cdr);
        let (from, power, pen_pct, pen_flat) = (g.pos, g.power, g.pen_pct, g.pen_flat);
        let team = g.team;
        let pos_y = from + Vec3::Y * 1.3;
        // how steeply to fire at the aim's HEIGHT (aim is on the real
        // terrain): bolts, volleys and beams pitch toward it
        let aim_slope = |dist: f32| ((aim.y + 1.0) - (from.y + 1.3)) / dist.max(3.0);
        let aim_flat = Vec3::new(aim.x, 0.0, aim.z);
        let mut dir = aim_flat - Vec3::new(from.x, 0.0, from.z);
        let dist = dir.length();
        if dist < 0.3 {
            // aim on the cat itself: along its facing -- (cos, sin), the
            // cat's convention. This read (sin, cos), the CAMERA's
            // convention, and fired along the facing mirrored.
            dir = facing_of(g.yaw);
        } else {
            dir = dir.normalize();
        }
        let dmg_scale = |base: f32, scale: f32| base + power * scale;

        // spend + cooldown
        let g = &mut self.gods[gi];
        g.mana -= ab.cost;
        g.cds[slot as usize] = cd;
        g.atk_anim = 1.0;
        g.last_act_t = self.t;
        // The sneak boost lives in the kit, not on a key: movement and
        // self-buff abilities open a window of quiet at FULL speed.
        g.hush_t = g.hush_t.max(match ab.arch {
            Archetype::Buff { duration, .. } => duration,
            Archetype::Blink { .. } => crate::consts::HUSH_BLINK_SECS,
            Archetype::Dash { .. } | Archetype::Meteor { .. } => crate::consts::HUSH_DASH_SECS,
            _ => 0.0,
        });

        self.events.push(Event::Cast { god: gi, slot, aim });

        match ab.arch {
            Archetype::Bolt { speed, dmg, radius } => {
                let d = dmg_scale(dmg, ab.power_scale);
                // toward the aimed HEIGHT too: a bolt from the shelf at a cat
                // below flew level over its head (aim is on the terrain now)
                let v3 = (dir + Vec3::Y * aim_slope(dist)).normalize_or_zero();
                self.projectiles.push(Projectile {
                    team,
                    pos: pos_y,
                    vel: v3 * speed,
                    dmg: d,
                    radius,
                    life: 1.6,
                    kind: ProjKind::Bolt,
                    owner: gi,
                    pen_pct,
                    pen_flat,
                    color: ab.color,
                    auto: false,
                });
                self.events.push(Event::BeamFx { from: pos_y, dir, length: 1.2, width: 0.4, color: ab.color, dur: 0.12 });
            }
            Archetype::Volley { count, spread_deg, dmg, speed } => {
                let d = dmg_scale(dmg, ab.power_scale);
                let base_ang = dir.z.atan2(dir.x);
                for k in 0..count {
                    let t = k as f32 / (count.max(2) - 1) as f32 - 0.5;
                    let ang = base_ang + t * spread_deg.to_radians();
                    let d2 = Vec3::new(ang.cos(), 0.0, ang.sin());
                    self.projectiles.push(Projectile {
                        team,
                        pos: pos_y,
                        vel: (d2 + Vec3::Y * aim_slope(dist)).normalize_or_zero() * speed,
                        dmg: d,
                        radius: 0.5,
                        life: 1.4,
                        kind: ProjKind::Arrow,
                        owner: gi,
                        pen_pct,
                        pen_flat,
                        color: ab.color,
                        auto: false,
                    });
                }
            }
            Archetype::Lob { radius, dmg } => {
                let d = dmg_scale(dmg, ab.power_scale);
                let clamped = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(20.0);
                self.projectiles.push(Projectile {
                    team,
                    pos: pos_y,
                    vel: dir * 10.0,
                    dmg: d,
                    radius: 0.6,
                    life: 2.5,
                    kind: ProjKind::Lob { start: pos_y, target: clamped, t: 0.0, dur: 0.75, radius },
                    owner: gi,
                    pen_pct,
                    pen_flat,
                    color: ab.color,
                    auto: false,
                });
                self.events.push(Event::RingTelegraph { pos: clamped, radius, delay: 0.75, color: ab.color });
            }
            Archetype::Beam { length, width, dmg } => {
                let dir = (dir + Vec3::Y * aim_slope(dist)).normalize_or_zero();
                let d = dmg_scale(dmg, ab.power_scale);
                self.events.push(Event::BeamFx { from: pos_y, dir, length, width, color: ab.color, dur: 0.45 });
                // damage everything in the slab
                let end = pos_y + dir * length;
                let mut hits = Vec::new();
                for j in 0..self.gods.len() {
                    let e = &self.gods[j];
                    if e.team != team && e.respawn_t <= 0.0 {
                        if crate::hitbox::cat_sized(e.pos, e.yaw, false, self.def(j).role.size()).hit_by_beam(pos_y, end, width + 0.2) {
                            hits.push(TargetRef::God(j));
                        }
                    }
                }
                for mi in 0..self.minions.len() {
                    let m = &self.minions[mi];
                    if m.team != team {
                        let c = m.pos + Vec3::Y * 0.6;
                        if point_segment_dist(c, pos_y, end) <= width + 0.4 {
                            hits.push(TargetRef::Minion(mi));
                        }
                    }
                }
                for ti in 0..self.towers.len() {
                    let tw = &self.towers[ti];
                    if tw.team != team && point_segment_dist(tw.pos + Vec3::Y * 2.0, pos_y, end) <= width + 1.4 {
                        hits.push(TargetRef::Tower(ti));
                    }
                }
                for t in 0..2 {
                    let tt = &self.titans[t];
                    if tt.team != team && point_segment_dist(tt.pos + Vec3::Y * 2.0, pos_y, end) <= width + 2.0 {
                        hits.push(TargetRef::Titan(tt.team));
                    }
                }
                for (bi, b) in self.bosses.list.iter().enumerate() {
                    if b.grounded() && point_segment_dist(b.pos + Vec3::Y * 2.0, pos_y, end) <= width + 2.0 {
                        hits.push(TargetRef::Boss(bi));
                    }
                }
                for h in hits {
                    self.deal_damage(h, d, true, Some(gi), pen_pct, pen_flat);
                }
            }
            Archetype::Nova { radius, dmg, slow } => {
                let d = dmg_scale(dmg, ab.power_scale);
                self.events.push(Event::NovaFx { pos: from, radius, color: ab.color });
                let targets = self.aoe_targets(from, radius, team);
                for t in targets {
                    self.deal_damage(t, d, true, Some(gi), pen_pct, pen_flat);
                    if let TargetRef::God(j) = t {
                        self.gods[j].slow = self.gods[j].slow.max(slow);
                        self.gods[j].slow_t = self.gods[j].slow_t.max(2.0);
                    }
                }
            }
            Archetype::Ring { radius, delay, dmg } => {
                let d = dmg_scale(dmg, ab.power_scale);
                let at = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(14.0);
                self.events.push(Event::RingTelegraph { pos: at, radius, delay, color: ab.color });
                self.zones.push(Zone {
                    team,
                    kind: ZoneKind::Ring { trigger_t: delay, dmg: d, owner: gi, done: false },
                    pos: at,
                    radius,
                    color: ab.color,
                });
            }
            Archetype::Storm { radius, duration, dps } => {
                let d = dps + power * ab.power_scale;
                let at = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(12.0);
                self.events.push(Event::StormFx { pos: at, radius, color: ab.color, dur: duration });
                self.zones.push(Zone {
                    team,
                    kind: ZoneKind::Storm { until: duration, dps: d, strike_t: 0.2, owner: gi },
                    pos: at,
                    radius,
                    color: ab.color,
                });
            }
            Archetype::Dash { dist: dd, dmg } => {
                let d = if dmg > 0.0 { dmg_scale(dmg, ab.power_scale) } else { 0.0 };
                let dest = clamp_arena(from + dir * dd);
                self.events.push(Event::DashFx { pos: from + Vec3::Y * 1.0, dir, color: ab.color });
                self.gods[gi].dash = Some(DashState { from, to: dest, t: 0.0, dur: 0.28, dmg: d, arc: 0.35, blast: None });
            }
            Archetype::Meteor { range, radius, dmg, vault } => {
                // up into the sky, and down as a falling star where aimed
                let d = dmg_scale(dmg, ab.power_scale);
                let dest = clamp_arena(from + dir * dist.min(range));
                let dest = crate::map::onto_floor(dest, crate::consts::GOD_RADIUS);
                let dest = Vec3::new(dest.x, crate::map::ground_y(dest.x, dest.z), dest.z);
                self.events.push(Event::NovaFx { pos: from, radius: 2.5, color: ab.color });
                self.events.push(Event::RingTelegraph { pos: dest, radius, delay: vault, color: ab.color });
                self.gods[gi].dash = Some(DashState { from, to: dest, t: 0.0, dur: vault, dmg: 0.0, arc: 16.0, blast: Some((radius, d)) });
            }
            Archetype::Litter { count, dmg, speed, life } => {
                let d = dmg_scale(dmg, ab.power_scale);
                self.events.push(Event::NovaFx { pos: from, radius: 1.8, color: ab.color });
                self.litter(gi, dir, count, d, speed, life);
            }
            Archetype::Giggle { radius, dmg, secs } => {
                let d = if dmg > 0.0 { dmg_scale(dmg, ab.power_scale) } else { 0.0 };
                self.events.push(Event::GiggleFx { pos: from, radius });
                self.giggle(gi, radius, d, secs);
            }
            Archetype::DanceParty { radius, duration, slow, slow_secs } => {
                let at = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(crate::kit::PARTY_RANGE);
                let at = Vec3::new(at.x, crate::map::ground_y(at.x, at.z), at.z);
                self.events.push(Event::StormFx { pos: at, radius, color: ab.color, dur: duration });
                self.dance_party(gi, at, radius, duration, slow, slow_secs, ab.color);
            }
            Archetype::Swap { radius, dmg, slow, secs } => {
                let d = if dmg > 0.0 { dmg_scale(dmg, ab.power_scale) } else { 0.0 };
                self.step_between(gi, aim_flat, radius, d, slow, secs);
            }
            Archetype::Send { range, speed } => {
                let to = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(range);
                self.send_other(gi, to, speed);
            }
            Archetype::Echoes { each, life, dmg } => {
                let d = dmg_scale(dmg, ab.power_scale);
                self.every_shadow(gi, each, life, d);
            }
            Archetype::Lantern { radius, dmg, flee, reveal } => {
                let d = dmg_scale(dmg, ab.power_scale);
                self.raise_the_lamp(gi, radius, d, flee, reveal);
            }
            Archetype::Field { .. } => {
                self.shift_coat(gi, slot);
            }
            Archetype::Blink { dist: dd, dmg } => {
                let d = if dmg > 0.0 { dmg_scale(dmg, ab.power_scale) } else { 0.0 };
                let dest = clamp_arena(from + dir * dd);
                // land ON the ground there (the pass has heights)
                let dest = Vec3::new(dest.x, crate::map::ground_y(dest.x, dest.z), dest.z);
                self.events.push(Event::NovaFx { pos: from, radius: 2.0, color: ab.color });
                self.gods[gi].pos = dest;
                self.events.push(Event::NovaFx { pos: dest, radius: 2.0, color: ab.color });
                if d > 0.0 {
                    let targets = self.aoe_targets(dest, 3.0, team);
                    for t in targets {
                        self.deal_damage(t, d, true, Some(gi), pen_pct, pen_flat);
                    }
                }
            }
            Archetype::Shield { amount, duration } => {
                let d = amount + power * ab.power_scale;
                self.gods[gi].shield = d;
                self.gods[gi].shield_t = duration;
                self.events.push(Event::ShieldFx { pos: from + Vec3::Y * 1.2, dur: duration, color: ab.color });
                // shield lowest-hp ally nearby too
                let mut best: Option<usize> = None;
                for j in 0..self.gods.len() {
                    let e = &self.gods[j];
                    if e.team == team && j != gi && e.respawn_t <= 0.0 && e.pos.distance(from) < 10.0 {
                        best = match best {
                            None => Some(j),
                            Some(b) => {
                                if e.hp / e.max_hp < self.gods[b].hp / self.gods[b].max_hp {
                                    Some(j)
                                } else {
                                    best
                                }
                            }
                        };
                    }
                }
                if let Some(b) = best {
                    self.gods[b].shield = d;
                    self.gods[b].shield_t = duration;
                    self.events.push(Event::ShieldFx { pos: self.gods[b].pos + Vec3::Y * 1.2, dur: duration, color: ab.color });
                }
            }
            Archetype::Buff { atk_speed, move_speed, duration } => {
                self.gods[gi].buff_atkspd = atk_speed;
                self.gods[gi].buff_ms = move_speed;
                self.gods[gi].buff_t = duration;
                self.events.push(Event::StormFx { pos: from, radius: 1.6, color: ab.color, dur: 0.8 });
            }
            Archetype::Heal { amount, radius } => {
                let d = amount + power * ab.power_scale * 0.5;
                self.events.push(Event::HealFx { pos: from, radius });
                self.gods[gi].hp = (self.gods[gi].hp + d).min(self.gods[gi].max_hp);
                for j in 0..self.gods.len() {
                    let e = &self.gods[j];
                    if e.team == team && j != gi && e.pos.distance(from) < radius {
                        self.gods[j].hp = (self.gods[j].hp + d * 0.5).min(self.gods[j].max_hp);
                    }
                }
            }
            Archetype::Pull { radius, dmg } => {
                let d = dmg_scale(dmg, ab.power_scale);
                let at = Vec3::new(from.x, 0.0, from.z) + dir * dist.min(radius);
                self.events.push(Event::PullFx { pos: at, radius, color: ab.color });
                let targets = self.aoe_targets(at, radius, team);
                for t in targets {
                    self.deal_damage(t, d, true, Some(gi), pen_pct, pen_flat);
                    if let TargetRef::God(j) = t {
                        let to_center = at - Vec3::new(self.gods[j].pos.x, 0.0, self.gods[j].pos.z);
                        let pull_dist = to_center.length().min(radius);
                        if pull_dist > 1.2 {
                            self.gods[j].pos += to_center.normalize() * (pull_dist * 0.65);
                        }
                        self.gods[j].slow = self.gods[j].slow.max(0.4);
                        self.gods[j].slow_t = self.gods[j].slow_t.max(1.6);
                    }
                }
            }
        }
    }

    /// What a finished dash deals. A dash "damaging enemies passed
    /// through" (`Archetype::Dash`) carried its damage in `DashState::dmg`
    /// and nothing ever dealt it -- every dash in the roster hit for 0.
    /// Now: everything within 2 u of the path, once, as the dash ends. And
    /// a meteor's blast where it lands: damage, a slow, and the flash.
    pub fn dash_strikes(&mut self, gi: usize, team: Team, from: Vec3, to: Vec3, dmg: f32, blast: Option<(f32, f32)>) {
        let mut hits: Vec<TargetRef> = Vec::new();
        if dmg > 0.0 {
            for k in 0..=4 {
                for t in self.aoe_targets(from.lerp(to, k as f32 / 4.0), 2.0, team) {
                    if !hits.contains(&t) {
                        hits.push(t);
                    }
                }
            }
            self.deal_all(hits, dmg, false, gi);
        }
        if let Some((radius, bdmg)) = blast {
            let color = [1.0, 0.92, 0.62];
            self.events.push(Event::Explosion { pos: to, radius, color });
            self.events.push(Event::NovaFx { pos: to, radius, color });
            self.events.push(Event::StrikeFx { at: to, color });
            let targets = self.aoe_targets(to, radius, team);
            for t in &targets {
                if let TargetRef::God(j) = *t {
                    let g = &mut self.gods[j];
                    g.slow = g.slow.max(0.45);
                    g.slow_t = g.slow_t.max(1.5);
                }
            }
            self.deal_all(targets, bdmg, true, gi);
        }
    }

    /// Deal `dmg` to each target. Minions last and highest index first: a
    /// minion that dies is REMOVED, which shifts every later index, so in
    /// any other order a kill makes the next hit land on the wrong minion.
    pub fn deal_all(&mut self, mut targets: Vec<TargetRef>, dmg: f32, magic: bool, gi: usize) {
        targets.sort_by_key(|t| match *t {
            TargetRef::Minion(i) => (1, usize::MAX - i),
            _ => (0, 0),
        });
        let (pp, pf) = (self.gods[gi].pen_pct, self.gods[gi].pen_flat);
        for t in targets {
            self.deal_damage(t, dmg, magic, Some(gi), pp, pf);
        }
    }

    pub fn aoe_targets(&self, center: Vec3, radius: f32, team: Team) -> Vec<TargetRef> {
        let mut out = Vec::new();
        for gi in 0..self.gods.len() {
            let g = &self.gods[gi];
            if g.team != team && g.respawn_t <= 0.0 && crate::hitbox::cat_sized(g.pos, g.yaw, false, self.def(gi).role.size()).in_disc(center, radius + 0.2) {
                out.push(TargetRef::God(gi));
            }
        }
        for mi in 0..self.minions.len() {
            let m = &self.minions[mi];
            if m.team != team && m.pos.distance(center) <= radius + 0.4 {
                out.push(TargetRef::Minion(mi));
            }
        }
        for ti in 0..self.towers.len() {
            let tw = &self.towers[ti];
            if tw.team != team && tw.pos.distance(center) <= radius + 1.5 {
                out.push(TargetRef::Tower(ti));
            }
        }
        for t in 0..2 {
            let tt = &self.titans[t];
            if tt.team != team && tt.pos.distance(center) <= radius + 2.5 {
                out.push(TargetRef::Titan(tt.team));
            }
        }
        for (bi, b) in self.bosses.list.iter().enumerate() {
            if b.grounded() && b.pos.distance(center) <= radius + 2.0 {
                out.push(TargetRef::Boss(bi));
            }
        }
        out
    }
}

pub fn point_segment_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(0.0001)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

pub fn clamp_arena(mut p: Vec3) -> Vec3 {
    let r = Vec3::new(p.x, 0.0, p.z).length();
    if r > crate::consts::arena_radius() - 2.0 {
        let s = (crate::consts::arena_radius() - 2.0) / r;
        p.x *= s;
        p.z *= s;
    }
    p.y = 0.0;
    p
}
