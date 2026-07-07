//! enemy.rs — the things that shoot back. Each enemy runs a tiny state machine (idle → chase →
//! attack → dead). It chases by walking toward the player on the XZ plane, sliding around walls with
//! the same axis-separated test the player uses, and it only opens fire when it has line-of-sight
//! (a wall raycast that reaches the player). Bodies are built from a handful of boxes and a sphere —
//! deliberately low-poly, in the PlayStation-1/Quake register — with a glowing visor so you can read
//! their facing. Combat treats each one as a [`Target`] sphere.

use crate::combat::{raycast, HitKind, Target};
use crate::genome::Genome;
use crate::level::{Level, Rng};
use crate::math::{clampf, Vec2, Vec3};
use crate::mesh::{boxx, dither, lerp_c, panel, rgb, tube, uv_sphere, Mesh3, FACE_ALL};
use crate::player::{body_blocked, footprint_blocked};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EnemyState {
    Idle,
    Chase,
    Search,
    Dead,
}

/// Bot skill tier for custom matches — scales aim, fire rate, health and detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Recruit,
    Regular,
    Veteran,
    Elite,
}
impl Difficulty {
    pub const ALL: [Difficulty; 4] = [Difficulty::Recruit, Difficulty::Regular, Difficulty::Veteran, Difficulty::Elite];
    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Recruit => "Recruit",
            Difficulty::Regular => "Regular",
            Difficulty::Veteran => "Veteran",
            Difficulty::Elite => "Elite",
        }
    }
    /// Multiplier on shot spread — lower is more accurate.
    pub fn aim_mul(self) -> f32 {
        match self {
            Difficulty::Recruit => 2.2,
            Difficulty::Regular => 1.4,
            Difficulty::Veteran => 1.0,
            Difficulty::Elite => 0.7,
        }
    }
    /// Multiplier on fire cooldown — lower fires faster.
    pub fn fire_mul(self) -> f32 {
        match self {
            Difficulty::Recruit => 1.5,
            Difficulty::Regular => 1.15,
            Difficulty::Veteran => 1.0,
            Difficulty::Elite => 0.8,
        }
    }
    pub fn health_mul(self) -> f32 {
        match self {
            Difficulty::Recruit => 0.8,
            Difficulty::Regular => 1.0,
            Difficulty::Veteran => 1.25,
            Difficulty::Elite => 1.55,
        }
    }
    pub fn detect_mul(self) -> f32 {
        match self {
            Difficulty::Recruit => 0.8,
            Difficulty::Regular => 1.0,
            Difficulty::Veteran => 1.15,
            Difficulty::Elite => 1.3,
        }
    }
    pub fn id(self) -> u8 {
        Difficulty::ALL.iter().position(|&d| d == self).unwrap_or(1) as u8
    }
    pub fn from_id(id: u8) -> Difficulty {
        Difficulty::ALL.get(id as usize).copied().unwrap_or(Difficulty::Regular)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EnemyKind {
    /// Fast, fragile, light hitscan.
    Grunt,
    /// Slow, tanky, hits harder.
    Brute,
    /// Very fast, fragile, charges into your face with rapid weak shots.
    Rusher,
    /// Slow, accurate, high-damage from long range — and backs away to keep its distance.
    Marksman,
    /// Alien Screecher: very fast, armored, aggressive harasser that picks off stragglers.
    Screecher,
    /// Alien Death-Mech: armored, cloaks in and out of sight and teleports to cut off your retreat.
    DeathMech,
}

#[derive(Clone, Copy)]
pub struct KindDef {
    pub max_health: f32,
    pub speed: f32,
    pub detect: f32,
    pub attack_range: f32,
    /// The distance the enemy tries to close to (it keeps advancing until this near).
    pub pursue: f32,
    pub attack_cooldown: f32,
    pub damage: f32,
    pub spread: f32,
    pub radius: f32,
    pub height: f32,
    /// Backs away to hold range when the player gets closer than `pursue` (kiting marksman).
    pub kite: bool,
    /// Heavily armored: takes reduced bullet damage and counts as a "tank" for acid (2× damage).
    pub armored: bool,
    pub body: egui::Color32,
    pub visor: egui::Color32,
}

impl EnemyKind {
    pub fn def(self) -> KindDef {
        match self {
            EnemyKind::Grunt => KindDef {
                max_health: 42.0,
                speed: 4.6,
                detect: 30.0,
                attack_range: 22.0,
                pursue: 6.0,
                attack_cooldown: 1.05,
                damage: 9.0,
                spread: 0.045,
                radius: 0.45,
                height: 1.7,
                kite: false,
                armored: false,
                body: rgb(96, 132, 88),
                visor: rgb(150, 255, 140),
            },
            EnemyKind::Brute => KindDef {
                max_health: 95.0,
                speed: 3.0,
                detect: 26.0,
                attack_range: 17.0,
                pursue: 5.0,
                attack_cooldown: 1.5,
                damage: 17.0,
                spread: 0.06,
                radius: 0.62,
                height: 2.15,
                kite: false,
                armored: true,
                body: rgb(150, 80, 64),
                visor: rgb(255, 150, 90),
            },
            EnemyKind::Rusher => KindDef {
                max_health: 28.0,
                speed: 7.2,
                detect: 34.0,
                attack_range: 9.0,
                pursue: 2.5,
                attack_cooldown: 0.5,
                damage: 7.0,
                spread: 0.085,
                radius: 0.4,
                height: 1.55,
                kite: false,
                armored: false,
                body: rgb(178, 58, 52),
                visor: rgb(255, 120, 90),
            },
            EnemyKind::Marksman => KindDef {
                max_health: 50.0,
                speed: 2.7,
                detect: 42.0,
                attack_range: 40.0,
                pursue: 22.0,
                attack_cooldown: 2.2,
                damage: 22.0,
                spread: 0.012,
                radius: 0.46,
                height: 1.95,
                kite: true,
                armored: false,
                body: rgb(64, 104, 120),
                visor: rgb(120, 230, 255),
            },
            EnemyKind::Screecher => KindDef {
                max_health: 64.0,
                speed: 8.4,
                detect: 38.0,
                attack_range: 13.0,
                pursue: 4.0,
                attack_cooldown: 0.42,
                damage: 8.0,
                spread: 0.07,
                radius: 0.44,
                height: 1.7,
                kite: false,
                armored: true,
                body: rgb(120, 70, 150),
                visor: rgb(220, 130, 255),
            },
            EnemyKind::DeathMech => KindDef {
                max_health: 120.0,
                speed: 4.2,
                detect: 40.0,
                attack_range: 24.0,
                pursue: 7.0,
                attack_cooldown: 1.25,
                damage: 20.0,
                spread: 0.03,
                radius: 0.6,
                height: 2.25,
                kite: false,
                armored: true,
                body: rgb(38, 44, 58),
                visor: rgb(120, 255, 220),
            },
        }
    }
}

/// A resolved enemy shot the game applies to the player (and turns into a tracer/flash).
#[derive(Clone, Copy)]
pub struct EnemyAttack {
    pub from: Vec3,
    pub to: Vec3,
    pub damage: f32,
    pub hit_player: bool,
}

pub struct Enemy {
    pub id: usize,
    pub kind: EnemyKind,
    pub pos: Vec3, // feet
    pub yaw: f32,
    pub health: f32,
    pub state: EnemyState,
    pub cooldown: f32,
    pub hurt: f32,
    pub walk_t: f32,
    pub death_t: f32,
    /// Last place the player was seen (drives search when line-of-sight breaks).
    last_seen: Option<Vec2>,
    /// How long to keep hunting toward `last_seen` before giving up.
    search_t: f32,
    /// Current sidestep direction (+1/-1) and a timer until it flips — makes engagements juke.
    strafe_dir: f32,
    strafe_t: f32,
    /// A small per-enemy approach angle so a pack fans out and flanks instead of conga-lining.
    flank: f32,
    /// 0→1 spawn-in progress: a fresh enemy rises from the ground with a brief glow.
    spawn_t: f32,
    /// In The Hunt survivor mode, the single AI creature is flagged here: drawn larger and darker, and
    /// driven by the game's predator AI rather than the per-enemy logic.
    pub hunter: bool,
    /// Which exotic creature this hunter is (only meaningful when `hunter`), so its body can take a distinct
    /// silhouette — a flying tentacled Kraken, a many-legged Widow spider, etc. `None` = the default brute.
    pub hunter_creature: Option<crate::creatures::Creature>,
    /// Death-Mech cloak phase (cycles in/out of visibility) and a teleport cooldown.
    cloak_t: f32,
    tp_cd: f32,
    /// Heritable personality — bends the fixed state-machine's thresholds (engagement range, retreat
    /// point, flank width, juke cadence, aim, persistence) so bots read as distinct fighters rather than
    /// interchangeable copies of the same script. Defaults to the neutral midpoint (original behaviour).
    pub genome: Genome,
}

impl Enemy {
    pub fn new(id: usize, kind: EnemyKind, pos: Vec3) -> Self {
        // deterministic per-enemy variation from id
        let h = (id as u32).wrapping_mul(0x9E37_79B9);
        Self {
            id,
            kind,
            pos,
            yaw: 0.0,
            health: kind.def().max_health,
            state: EnemyState::Idle,
            cooldown: 0.0,
            hurt: 0.0,
            walk_t: 0.0,
            death_t: 0.0,
            last_seen: None,
            search_t: 0.0,
            strafe_dir: if h & 1 == 0 { 1.0 } else { -1.0 },
            strafe_t: 0.6 + (h >> 8 & 0xff) as f32 / 255.0 * 0.8,
            flank: ((h >> 16 & 0xff) as f32 / 255.0 - 0.5) * 0.9, // ±0.45 rad
            spawn_t: 0.0,
            hunter: false,
            hunter_creature: None,
            cloak_t: ((h >> 24 & 0xff) as f32 / 255.0) * 5.0, // staggered cloak phase
            tp_cd: 2.0,
            genome: Genome::default(),
        }
    }

    /// Builder: give this enemy a heritable personality (evolved or a hand-picked archetype).
    pub fn with_genome(mut self, g: Genome) -> Self {
        self.genome = g;
        self
    }

    #[inline]
    pub fn alive(&self) -> bool {
        self.state != EnemyState::Dead
    }

    /// A Death-Mech is cloaked for part of its cycle: nearly invisible and holding fire while it closes in.
    pub fn cloaked(&self) -> bool {
        self.kind == EnemyKind::DeathMech && self.alive() && (self.cloak_t % 5.0) > 3.1
    }

    /// If this enemy is actively engaged, the player position it would broadcast to teammates.
    pub fn alert_source(&self) -> Option<Vec2> {
        if matches!(self.state, EnemyState::Chase | EnemyState::Search) {
            self.last_seen
        } else {
            None
        }
    }

    /// True if idle and unaware (a candidate to be alerted by a teammate).
    pub fn is_idle(&self) -> bool {
        self.state == EnemyState::Idle
    }

    /// True while actively pursuing the player (a candidate to throw a grenade).
    pub fn is_chasing(&self) -> bool {
        self.state == EnemyState::Chase
    }

    /// Be alerted to a position by a teammate: start searching toward it.
    pub fn alert(&mut self, pos: Vec2) {
        if self.state == EnemyState::Idle {
            self.state = EnemyState::Search;
            self.last_seen = Some(pos);
            self.search_t = 4.5 * (0.5 + self.genome.persistence);
        }
    }

    /// The multi-part hitbox the player's bullets test against: a body capsule from the hips to the shoulders
    /// plus a head sphere. Matches the rendered figure (torso sphere at ~0.55h, head at ~0.85h) far better than
    /// the old single torso sphere, so hits register on the head, torso and legs where the body actually is.
    pub fn target(&self) -> Target {
        let d = self.kind.def();
        let r = d.radius;
        // capsule spans roughly knees→shoulders; head sits just above the top.
        let base = self.pos + Vec3::new(0.0, d.height * 0.28, 0.0);
        let top = self.pos + Vec3::new(0.0, d.height * 0.72, 0.0);
        let head = self.pos + Vec3::new(0.0, d.height * 0.88, 0.0);
        Target {
            base,
            top,
            radius: r * 0.95,
            head,
            head_r: r * 0.62,
            id: self.id,
        }
    }

    #[inline]
    pub fn center(&self) -> Vec3 {
        let d = self.kind.def();
        self.pos + Vec3::new(0.0, d.height * 0.55, 0.0)
    }

    #[inline]
    fn eye(&self) -> Vec3 {
        let d = self.kind.def();
        self.pos + Vec3::new(0.0, d.height * 0.82, 0.0)
    }

    /// Public eye position (used by the game to feed prey their threat origin in Hunt survivor mode).
    #[inline]
    pub fn eye_pos(&self) -> Vec3 {
        self.eye()
    }

    /// Drive this enemy (the AI creature) straight at a target, climbing stairs via the support query.
    /// Returns true when within `reach` on the ground plane and roughly level — i.e. a melee strike connects.
    pub fn hunt_toward(&mut self, level: &Level, target: Vec3, speed_mul: f32, reach: f32, dt: f32) -> bool {
        self.spawn_t = (self.spawn_t + dt * 1.8).min(1.0);
        let to = Vec2::new(target.x - self.pos.x, target.z - self.pos.z);
        let dist = to.len();
        if dist > 1e-3 {
            self.yaw = to.x.atan2(-to.y);
            let dir = Vec2::new(to.x / dist, to.y / dist);
            let d = self.kind.def();
            self.step(level, dir, d.speed * speed_mul * dt);
            self.walk_t += dt * d.speed * speed_mul;
        }
        dist <= reach && (target.y - self.pos.y).abs() < 2.2
    }

    /// True if `done` and the game should drop this corpse.
    pub fn should_remove(&self) -> bool {
        // Long enough for the topple (~0.6s), a brief corpse rest, and the sink (starts 0.9s) to finish.
        self.state == EnemyState::Dead && self.death_t > 2.3
    }

    pub fn damage(&mut self, dmg: f32) {
        if self.state == EnemyState::Dead {
            return;
        }
        // Armored aliens shrug off part of incoming damage (acid bypasses this with its own 2× in game.rs).
        let dmg = if self.kind.def().armored { dmg * 0.72 } else { dmg };
        self.health -= dmg;
        self.hurt = 1.0;
        if self.health <= 0.0 {
            self.health = 0.0;
            self.state = EnemyState::Dead;
            self.death_t = 0.0;
        } else if self.state == EnemyState::Idle {
            // Getting shot wakes it up even with no line of sight.
            self.state = EnemyState::Chase;
        }
    }

    /// Advance AI one step. Returns an attack if it fired this frame.
    pub fn update(
        &mut self,
        level: &Level,
        player_eye: Vec3,
        player_feet: Vec3,
        detect_mul: f32,
        aim_mul: f32,
        fire_mul: f32,
        prey: bool,
        rng: &mut Rng,
        dt: f32,
    ) -> Option<EnemyAttack> {
        self.hurt = (self.hurt - dt * 2.5).max(0.0);
        self.spawn_t = (self.spawn_t + dt * 1.8).min(1.0);
        if self.cooldown > 0.0 {
            self.cooldown -= dt;
        }

        if self.state == EnemyState::Dead {
            self.death_t += dt;
            // Death sequence: the body topples over (handled in the mesh build via death_t), rests as a corpse,
            // then only after it's fallen does it slowly sink and fade out. Holding it at ground height through
            // the topple lets the fall actually read before the corpse is cleared.
            let sink_start = 0.9;
            let sink = ((self.death_t - sink_start).max(0.0) * 1.2).min(1.6);
            self.pos.y = level.floor_h(self.pos.x, self.pos.z) - sink;
            return None;
        }

        let to = Vec2::new(player_feet.x - self.pos.x, player_feet.z - self.pos.z);
        let dist = to.len();
        let d = self.kind.def();
        let los = self.has_los(level, player_eye);

        // Death-Mech: cycle the cloak and, when it's been pushed too far back, teleport in to cut off retreat.
        if self.kind == EnemyKind::DeathMech {
            self.cloak_t += dt;
            if self.tp_cd > 0.0 {
                self.tp_cd -= dt;
            }
            if matches!(self.state, EnemyState::Chase | EnemyState::Search) && dist > 16.0 && self.tp_cd <= 0.0 {
                let bear = Vec2::new(self.pos.x - player_feet.x, self.pos.z - player_feet.z);
                let bl = bear.len().max(1e-3);
                let tgt = Vec2::new(player_feet.x + bear.x / bl * 9.0, player_feet.z + bear.y / bl * 9.0);
                if level.has_floor_at(tgt) && !footprint_blocked(level, tgt, d.radius) {
                    self.pos.x = tgt.x;
                    self.pos.z = tgt.y;
                    self.pos.y = level.floor_h(tgt.x, tgt.y);
                    self.tp_cd = 6.0;
                    self.cloak_t = 3.2; // re-materialise briefly cloaked, then flicker back in
                    self.state = EnemyState::Chase;
                }
            }
        }

        // Face the player whenever we can see them.
        if los && dist > 1e-3 {
            self.yaw = to.x.atan2(-to.y); // matches camera yaw convention (yaw=0 → -Z)
        }

        // tick the sidestep flip timer
        self.strafe_t -= dt;
        if self.strafe_t <= 0.0 {
            self.strafe_dir = -self.strafe_dir;
            // Reaction shortens the cadence: a twitchy bot juke-flips faster and is a harder target.
            let cadence_mul = 1.4 - self.genome.reaction * 0.8;
            self.strafe_t = (0.7 + rng.f32() * 1.0) * cadence_mul;
        }

        match self.state {
            EnemyState::Idle => {
                // Prey also startle from a nearby creature they can't quite see (footsteps/scent).
                // Wary bots (high `awareness`) notice sooner; distracted ones let you get closer.
                let aware_mul = detect_mul * (0.7 + self.genome.awareness * 0.6);
                let aware = (los && dist < d.detect * aware_mul) || (prey && dist < 8.0);
                if aware {
                    self.state = EnemyState::Chase;
                } else {
                    // Idle amble: drift slowly along a gently turning heading so sentries feel like patrols,
                    // not statues. The heading rotates from a per-enemy phase plus accumulated walk time.
                    let heading = self.flank * 6.0 + self.id as f32 * 1.7 + self.walk_t * 0.15;
                    let dir = Vec2::new(heading.sin(), -heading.cos());
                    self.yaw = heading;
                    if rng.f32() < 0.7 {
                        self.step(level, dir, d.speed * 0.22 * dt);
                        self.walk_t += dt * d.speed * 0.22;
                    }
                }
                None
            }
            EnemyState::Chase if prey => {
                // The Hunt: unarmed prey. Sprint directly away from the creature, weaving to put cover
                // between you and it; never attack. Face the flee direction so the survivor looks panicked.
                self.last_seen = Some(Vec2::new(player_feet.x, player_feet.z));
                self.search_t = 5.0 * (0.5 + self.genome.persistence);
                let dirn = Vec2::new(to.x / dist.max(1e-4), to.y / dist.max(1e-4));
                let perp = Vec2::new(-dirn.y, dirn.x);
                let away = Vec2::new(-dirn.x + perp.x * self.strafe_dir * 0.6, -dirn.y + perp.y * self.strafe_dir * 0.6);
                let alen = away.len().max(1e-4);
                let fleed = Vec2::new(away.x / alen, away.y / alen);
                self.yaw = fleed.x.atan2(-fleed.y);
                self.step(level, fleed, d.speed * 1.12 * dt);
                self.walk_t += dt * d.speed * 1.12;
                // Got distance and broke sight? Slip into hiding.
                if !los && dist > 14.0 {
                    self.state = EnemyState::Search;
                }
                None
            }
            EnemyState::Chase => {
                self.last_seen = Some(Vec2::new(player_feet.x, player_feet.z));
                self.search_t = 4.5 * (0.5 + self.genome.persistence);
                let mut attack = None;
                let dirn = Vec2::new(to.x / dist.max(1e-4), to.y / dist.max(1e-4));
                let perp = Vec2::new(-dirn.y, dirn.x); // sidestep axis
                // Approach with a slight flank bias so a pack fans out instead of single-filing. Genome
                // stretches or dampens each bot's own flank angle — a Flanker swings wide, a Berserker
                // walks nearly straight in.
                let flank = self.flank * (0.3 + self.genome.flank_bias * 1.4);
                let (fc, fs) = (flank.cos(), flank.sin());
                let approach = Vec2::new(dirn.x * fc - dirn.y * fs, dirn.x * fs + dirn.y * fc);
                // Wounded bots break contact: fall back while still firing, looking for distance to recover.
                // Aggression sets both how low it lets its health run before that instinct kicks in, and how
                // close it's willing to press a kiting fight in the first place — a Berserker fights to the
                // floor; a Turtle peels off early.
                let retreat_frac = 0.12 + (1.0 - self.genome.aggression) * 0.35;
                let kite_factor = 0.5 + (1.0 - self.genome.aggression) * 0.4;
                let wounded_factor = 1.0 + (1.0 - self.genome.aggression) * 0.6;
                let wounded = self.health < retreat_frac * d.max_health;
                if (d.kite && dist < d.pursue * kite_factor) || (wounded && dist < d.pursue * wounded_factor) {
                    // back away, still juking
                    let away = Vec2::new(-approach.x + perp.x * self.strafe_dir * 0.5, -approach.y + perp.y * self.strafe_dir * 0.5);
                    self.step(level, away, d.speed * dt);
                    self.walk_t += dt * d.speed;
                } else if dist > d.pursue {
                    // close in, weaving
                    let weave = Vec2::new(approach.x + perp.x * self.strafe_dir * 0.45, approach.y + perp.y * self.strafe_dir * 0.45);
                    self.step(level, weave, d.speed * dt);
                    self.walk_t += dt * d.speed;
                } else if los {
                    // in the pocket: strafe sideways to be a hard target while shooting
                    self.step(level, Vec2::new(perp.x * self.strafe_dir, perp.y * self.strafe_dir), d.speed * 0.7 * dt);
                    self.walk_t += dt * d.speed * 0.7;
                }
                if los && dist < d.attack_range && self.cooldown <= 0.0 && !self.cloaked() {
                    self.cooldown = d.attack_cooldown * fire_mul;
                    // Accuracy tightens or widens the effective spread multiplier around the neutral point.
                    let aim_genome = aim_mul * (1.4 - self.genome.accuracy * 0.8);
                    attack = Some(self.shoot(level, player_eye, rng, d, aim_genome));
                }
                // Lost sight? Don't give up — go hunt the last known position.
                if !los {
                    self.state = EnemyState::Search;
                }
                attack
            }
            EnemyState::Search => {
                // Regained sight → re-engage immediately.
                if los && dist < d.detect * detect_mul {
                    self.state = EnemyState::Chase;
                    return None;
                }
                // Prey re-bolt if the creature closes in while they're hiding.
                if prey && dist < 7.0 {
                    self.state = EnemyState::Chase;
                    return None;
                }
                self.search_t -= dt;
                if prey {
                    // Hiding: creep slowly away from where the creature last was, staying low and quiet, then
                    // settle (freeze) once enough time passes.
                    if let Some(ls) = self.last_seen {
                        let away = Vec2::new(self.pos.x - ls.x, self.pos.z - ls.y);
                        let al = away.len();
                        if al > 1e-3 && self.search_t > 1.5 {
                            let dir = Vec2::new(away.x / al, away.y / al);
                            self.yaw = dir.x.atan2(-dir.y);
                            self.step(level, dir, d.speed * 0.45 * dt);
                            self.walk_t += dt * d.speed * 0.45;
                        }
                    }
                    if self.search_t <= 0.0 {
                        self.state = EnemyState::Idle;
                        self.last_seen = None;
                    }
                    return None;
                }
                if let Some(ls) = self.last_seen {
                    let to_l = Vec2::new(ls.x - self.pos.x, ls.y - self.pos.z);
                    let dl = to_l.len();
                    if dl > 1.2 {
                        let dir = Vec2::new(to_l.x / dl.max(1e-4), to_l.y / dl.max(1e-4));
                        self.yaw = dir.x.atan2(-dir.y);
                        self.step(level, dir, d.speed * 0.85 * dt);
                        self.walk_t += dt * d.speed * 0.85;
                    } else {
                        self.last_seen = None; // reached it, nothing here
                    }
                }
                if self.search_t <= 0.0 || self.last_seen.is_none() {
                    self.state = EnemyState::Idle;
                }
                None
            }
            EnemyState::Dead => None,
        }
    }

    fn shoot(&self, level: &Level, player_eye: Vec3, rng: &mut Rng, d: KindDef, aim_mul: f32) -> EnemyAttack {
        let from = self.eye();
        let mut dir = (player_eye - from).normalized();
        // Scatter the shot a little so enemies aren't laser-accurate.
        dir = jitter(dir, d.spread * aim_mul, rng);
        let to_player = player_eye.distance(from);
        // Did a wall get in the way of the (jittered) shot before reaching the player range?
        let h = raycast(level, &[], from, dir, to_player + 2.0);
        let blocked = matches!(h.kind, HitKind::Wall | HitKind::Obstacle | HitKind::Destructible(_)) && h.dist < to_player - 0.4;
        let endpoint = if blocked { h.point } else { from + dir * to_player };
        EnemyAttack {
            from,
            to: endpoint,
            damage: d.damage,
            hit_player: !blocked,
        }
    }

    /// Move on the XZ plane, sliding along walls (axis-separated like the player).
    fn step(&mut self, level: &Level, dir: Vec2, amount: f32) {
        let r = self.kind.def().radius;
        let bh = 1.8; // enemy standing collision height
        let nx = self.pos.x + dir.x * amount;
        if !body_blocked(level, Vec2::new(nx, self.pos.z), r, self.pos.y, bh) {
            self.pos.x = nx;
        }
        let nz = self.pos.z + dir.y * amount;
        if !body_blocked(level, Vec2::new(self.pos.x, nz), r, self.pos.y, bh) {
            self.pos.z = nz;
        }
        // Use the platform-aware support height so pursuing bots climb stairs onto mezzanines and stand on
        // raised decks, instead of being glued to the ground floor.
        self.pos.y = level.support_y(self.pos.x, self.pos.z, self.pos.y);
    }

    fn has_los(&self, level: &Level, player_eye: Vec3) -> bool {
        let from = self.eye();
        let to = player_eye - from;
        let dist = to.len();
        if dist < 1e-3 {
            return true;
        }
        let dir = to.normalized();
        let h = raycast(level, &[], from, dir, dist);
        // If the first wall/obstacle is farther than the player, nothing's blocking.
        !(matches!(h.kind, HitKind::Wall | HitKind::Obstacle | HitKind::Destructible(_)) && h.dist < dist - 0.3)
    }

    /// Build the low-poly body. Returns one merged mesh (legs/torso/arms/head + glowing visor).
    pub fn build_mesh(&self) -> Mesh3 {
        self.build_mesh_tinted(None)
    }

    /// Build the enemy body, optionally re-tinted toward a **team hue** (hostile colour) so factions read at
    /// a glance. `team` = None keeps the per-kind palette; `Some(col)` shifts the body toward that hue while
    /// preserving shading. The visor/glow and death desaturation are left alone so threat cues stay readable.
    pub fn build_mesh_tinted(&self, team: Option<egui::Color32>) -> Mesh3 {
        let d = self.kind.def();
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let fwd = Vec3::new(s, 0.0, -c); // facing
        let right = Vec3::new(c, 0.0, s);

        // Hurt makes the body flash pale; death desaturates and the visor dims.
        let dying = self.state == EnemyState::Dead;
        let mut base = if dying {
            lerp_c(d.body, rgb(40, 40, 44), (self.death_t * 0.8).min(0.7))
        } else {
            lerp_c(d.body, rgb(255, 255, 255), self.hurt * 0.6)
        };
        // The AI creature reads as a hulking dark-crimson shape, distinct from the survivors.
        if self.hunter && !dying {
            base = lerp_c(rgb(46, 18, 22), rgb(255, 255, 255), self.hurt * 0.6);
        }
        // Team re-tint: shift the (living) body toward the hostile team hue. Skipped while flashing pale from
        // a hit or desaturating in death, so those feedback cues aren't muddied.
        if let Some(tc) = team {
            if !dying && self.hurt < 0.3 && !self.hunter {
                base = crate::mesh::team_tint(base, tc, 0.55);
            }
        }
        let limb = lerp_c(base, rgb(20, 20, 24), 0.35);

        // The *exotic* hunt creatures are NOT humanoids in costume — build them from scratch as their own
        // anatomy (a floating tentacled kraken, a low scuttling spider) rather than bolting appendages onto
        // the soldier rig. The humanoid horrors (Revenant/Stalker/Brute/Wraith) deliberately keep the human
        // rig (scaled up below), so only Kraken/Widow divert here — matching every creature would leave the
        // humanoid ones with an empty body.
        if let Some(cr @ (crate::creatures::Creature::Kraken | crate::creatures::Creature::Widow)) = self.hunter_creature {
            return self.build_creature_body(cr, base, fwd, right);
        }

        let h = if self.hunter { d.height * 1.22 } else { d.height };
        // ── Walk cycle ────────────────────────────────────────────────────────────────────────────────────
        // A single phase drives an alternating gait: each leg swings opposite the other, the arms counter-
        // swing against the legs, and the torso bobs and lists slightly. `gait` is how "moving" the body is
        // (chasing/searching = full stride, idle = a subtle breathing sway) so a standing enemy isn't frozen.
        let phase = self.walk_t * 2.2;
        let moving = matches!(self.state, EnemyState::Chase | EnemyState::Search);
        let gait = if moving { 1.0 } else { 0.15 };
        let swing = phase.sin() * 0.30 * gait; // fore/aft leg swing
        let bob = (phase * 2.0).cos() * 0.05 * gait; // vertical bob at 2× (both feet hit per stride)
        let list = phase.sin() * 0.04 * gait; // slight side-to-side weight shift
        let mut m = Mesh3::new(base);

        // Legs: two boxes that alternate fore/aft, with a knee-bend (the swinging leg lifts and bends), plus
        // segmented armor — a thigh plate, a knee guard, and a shin greave — so the leg reads as a rigged,
        // armored limb rather than a smooth peg.
        let leg_h = h * 0.42;
        let leg_hw = d.radius * 0.34;
        // armor tones (defined here so the leg plating can use them; body armor block below reuses them)
        let leg_armor = lerp_c(base, rgb(48, 50, 56), 0.5);
        for side in [-1.0f32, 1.0] {
            let ph = swing * side; // opposite legs swing opposite directions
            let lift = (ph.abs() * 0.5).min(0.12) * gait; // the forward-swinging foot lifts off the ground
            let knee = (phase * side).sin().max(0.0) * 0.10 * gait; // slight knee bend on the forward swing
            let off = right * (d.radius * 0.4 * side) + fwd * ph;
            let c0 = self.pos + off + Vec3::new(list, leg_h * 0.5 + lift + bob, 0.0);
            // upper leg
            let legm = boxx(c0, Vec3::new(leg_hw, leg_h * 0.5 - knee, leg_hw), dither(limb, side as i32 + 7, 0.08), FACE_ALL);
            m.merge(&legm);
            // thigh plate on the front of the upper leg
            let thigh_c = c0 + fwd * (leg_hw * 0.7) + Vec3::new(0.0, leg_h * 0.18, 0.0);
            m.merge(&boxx(thigh_c, Vec3::new(leg_hw * 0.9, leg_h * 0.24, leg_hw * 0.45), leg_armor, FACE_ALL));
            // knee guard (a rounded cap at the joint)
            let knee_c = self.pos + off + Vec3::new(list, leg_h * 0.52 + lift + bob, 0.0) + fwd * (leg_hw * 0.6);
            m.merge(&boxx(knee_c, Vec3::new(leg_hw * 0.7, leg_hw * 0.7, leg_hw * 0.55), lerp_c(leg_armor, rgb(200, 200, 210), 0.2), FACE_ALL));
            // shin greave on the front of the lower leg
            let shin_c = self.pos + off + Vec3::new(list, leg_h * 0.22 + lift + bob, 0.0) + fwd * (leg_hw * 0.6);
            m.merge(&boxx(shin_c, Vec3::new(leg_hw * 0.8, leg_h * 0.22, leg_hw * 0.4), leg_armor, FACE_ALL));
            // boot (a slightly wider, darker block at the foot — reads as footwear and grounds the stride)
            let boot = boxx(
                self.pos + off + Vec3::new(list, 0.09 + lift + bob, leg_hw * 0.3),
                Vec3::new(leg_hw * 1.15, 0.09, leg_hw * 1.5),
                dither(lerp_c(limb, rgb(16, 16, 18), 0.4), side as i32 + 13, 0.06),
                FACE_ALL,
            );
            m.merge(&boot);
        }

        // Torso (a slightly squashed sphere reads as a low-poly chest) — rides the walk bob and lists.
        let torso_c = self.pos + Vec3::new(list, leg_h + h * 0.2 + bob, 0.0);
        let mut torso = uv_sphere(torso_c, d.radius * 1.0, 8, 6, base);
        // squash vertically
        for v in torso.verts.iter_mut() {
            v.y = torso_c.y + (v.y - torso_c.y) * 0.85;
        }
        m.merge(&torso);

        // Arms: counter-swing against the legs (left arm forward as right leg forward), with a hinged
        // forearm so the swing reads as a real limb, not a rigid stick. The forward arm bends slightly.
        let arm_h = h * 0.34;
        for side in [-1.0f32, 1.0] {
            let ph = -swing * side; // counter-swing: opposite the leg on the same side
            let off = right * (d.radius * 1.05 * side);
            let shoulder = torso_c + off + Vec3::new(0.0, arm_h * 0.35, 0.0);
            // upper arm — pivots fore/aft from the shoulder
            let upper_c = shoulder + fwd * (ph * 0.6) + Vec3::new(0.0, -arm_h * 0.28, 0.0);
            let armm = boxx(
                upper_c,
                Vec3::new(d.radius * 0.22, arm_h * 0.3, d.radius * 0.22),
                dither(limb, side as i32 + 31, 0.08),
                FACE_ALL,
            );
            m.merge(&armm);
            // forearm — hangs from the elbow, swings a touch more (leads the motion)
            let elbow = upper_c + fwd * (ph * 0.5) + Vec3::new(0.0, -arm_h * 0.3, 0.0);
            let fore_c = elbow + fwd * (ph * 0.4) + Vec3::new(0.0, -arm_h * 0.22, 0.0);
            let forem = boxx(
                fore_c,
                Vec3::new(d.radius * 0.19, arm_h * 0.26, d.radius * 0.19),
                dither(lerp_c(limb, rgb(20, 20, 24), 0.15), side as i32 + 41, 0.08),
                FACE_ALL,
            );
            m.merge(&forem);
            // gauntlet: a chunky armored cuff over the forearm + a blocky fist at the end
            let gaunt = lerp_c(base, rgb(48, 50, 56), 0.5);
            m.merge(&boxx(fore_c, Vec3::new(d.radius * 0.24, arm_h * 0.18, d.radius * 0.24), gaunt, FACE_ALL));
            let fist_c = fore_c + Vec3::new(0.0, -arm_h * 0.3, 0.0);
            m.merge(&boxx(fist_c, Vec3::new(d.radius * 0.22, d.radius * 0.24, d.radius * 0.22), dither(lerp_c(gaunt, rgb(16, 16, 18), 0.3), side as i32 + 51, 0.06), FACE_ALL));
            // elbow guard cap at the joint
            m.merge(&boxx(elbow + fwd * (d.radius * 0.1), Vec3::new(d.radius * 0.2, d.radius * 0.2, d.radius * 0.2), lerp_c(gaunt, rgb(200, 200, 210), 0.2), FACE_ALL));
        }

        // ── Armor plating ─────────────────────────────────────────────────────────────────────────────────
        // Layered over the torso so the troop reads as an armored soldier, not a smooth blob: a chest plate
        // with a raised collar, angled shoulder pauldrons, and a belt. Plated in a darker, harder metal tone
        // than the body so the team tint still shows on the underlying body but the armor reads as equipment.
        let armor = lerp_c(base, rgb(48, 50, 56), 0.55);
        // a whisper of the team hue on the armor too, so faction reads at a glance even though the plates are
        // mostly neutral metal (the body underneath carries the stronger tint)
        let armor = if let Some(tc) = team {
            if !dying && !self.hunter {
                crate::mesh::team_tint(armor, tc, 0.22)
            } else {
                armor
            }
        } else {
            armor
        };
        let armor_edge = lerp_c(armor, rgb(200, 200, 210), 0.25);
        {
            // chest plate: a beveled box on the front of the torso
            let chest_c = torso_c + fwd * (d.radius * 0.5) + Vec3::new(0.0, d.radius * 0.05, 0.0);
            let mut chest = boxx(chest_c, Vec3::new(d.radius * 0.7, d.radius * 0.62, d.radius * 0.28), armor, FACE_ALL);
            chest.material = Some(crate::mesh::Material::HexPlate);
            m.merge(&chest);
            // raised collar/gorget
            let collar_c = torso_c + fwd * (d.radius * 0.35) + Vec3::new(0.0, d.radius * 0.55, 0.0);
            m.merge(&boxx(collar_c, Vec3::new(d.radius * 0.5, d.radius * 0.16, d.radius * 0.3), armor_edge, FACE_ALL));
            // shoulder pauldrons: angled caps over each shoulder
            for side in [-1.0f32, 1.0] {
                let pc = torso_c + right * (d.radius * 0.95 * side) + Vec3::new(0.0, d.radius * 0.5, 0.0);
                let mut pauld = boxx(pc, Vec3::new(d.radius * 0.42, d.radius * 0.26, d.radius * 0.5), armor, FACE_ALL);
                crate::mesh::rotate_z_about(&mut pauld, pc, side * 0.35);
                m.merge(&pauld);
            }
            // belt / waist plate
            let belt_c = self.pos + Vec3::new(list, leg_h + bob, 0.0);
            m.merge(&boxx(belt_c, Vec3::new(d.radius * 0.85, d.radius * 0.14, d.radius * 0.72), lerp_c(armor, rgb(20, 20, 24), 0.3), FACE_ALL));
        }

        // ── Head, face & combat helmet ──────────────────────────────────────────────────────────────────
        // A rigged head: a skin-toned face block with a jaw and cheekbones, wrapped in a full combat helmet
        // (dome + brow + side ear-plates + chin guard) leaving a visor slit where the glowing optic sits.
        let head_c = self.pos + Vec3::new(list, h * 0.9 + bob, 0.0);
        let hr = d.radius * 0.55;
        // face: a skin/flesh tone (kept neutral for non-humanoid kinds via the limb tone) so a real face
        // reads under the visor rather than a blank block
        let is_humanoid = matches!(self.kind, EnemyKind::Grunt | EnemyKind::Rusher | EnemyKind::Marksman | EnemyKind::Screecher);
        let skin = if is_humanoid { lerp_c(rgb(180, 140, 116), base, 0.25) } else { limb };
        // core head/skull
        let head = boxx(head_c, Vec3::new(hr, hr, hr * 0.92), skin, FACE_ALL);
        m.merge(&head);
        // jaw/chin (a smaller block below, set forward) + cheekbones for facial structure
        m.merge(&boxx(head_c + fwd * (hr * 0.2) + Vec3::new(0.0, -hr * 0.72, 0.0), Vec3::new(hr * 0.7, hr * 0.3, hr * 0.72), lerp_c(skin, rgb(20, 20, 24), 0.1), FACE_ALL));
        for side in [-1.0f32, 1.0] {
            // cheekbone ridges
            m.merge(&boxx(head_c + right * (hr * 0.6 * side) + fwd * (hr * 0.3) + Vec3::new(0.0, -hr * 0.1, 0.0), Vec3::new(hr * 0.18, hr * 0.3, hr * 0.3), lerp_c(skin, rgb(60, 40, 34), 0.2), FACE_ALL));
            // eyes: small dark sockets flanking the bridge (only for humanoids; behind the visor glow)
            if is_humanoid {
                let eye_c = head_c + right * (hr * 0.32 * side) + fwd * (hr * 0.82) + Vec3::new(0.0, hr * 0.12, 0.0);
                m.merge(&boxx(eye_c, Vec3::new(hr * 0.16, hr * 0.1, hr * 0.06), rgb(24, 22, 26), FACE_ALL));
            }
        }
        // helmet dome (flattened sphere over the crown)
        let helm_c = head_c + Vec3::new(0.0, hr * 0.28, 0.0);
        let mut helm = uv_sphere(helm_c, hr * 1.12, 9, 6, armor);
        for v in helm.verts.iter_mut() {
            if v.y < helm_c.y {
                v.y = helm_c.y + (v.y - helm_c.y) * 0.42; // flatten the bottom so it sits like a helmet
            }
        }
        helm.material = Some(crate::mesh::Material::HexPlate);
        m.merge(&helm);
        // brow ridge over the visor slit
        let brow_c = head_c + fwd * (hr * 0.86) + Vec3::new(0.0, hr * 0.38, 0.0);
        m.merge(&boxx(brow_c, Vec3::new(hr * 1.02, hr * 0.2, hr * 0.28), armor_edge, FACE_ALL));
        // side ear-plates and a rear neck guard
        for side in [-1.0f32, 1.0] {
            m.merge(&boxx(head_c + right * (hr * 0.92 * side) + Vec3::new(0.0, hr * 0.1, 0.0), Vec3::new(hr * 0.2, hr * 0.7, hr * 0.7), armor, FACE_ALL));
        }
        m.merge(&boxx(head_c - fwd * (hr * 0.9) + Vec3::new(0.0, -hr * 0.3, 0.0), Vec3::new(hr * 0.9, hr * 0.5, hr * 0.2), lerp_c(armor, rgb(20, 20, 24), 0.2), FACE_ALL));
        // chin guard / mandible plate across the lower face
        m.merge(&boxx(head_c + fwd * (hr * 0.72) + Vec3::new(0.0, -hr * 0.5, 0.0), Vec3::new(hr * 0.66, hr * 0.24, hr * 0.28), lerp_c(armor, rgb(30, 32, 38), 0.2), FACE_ALL));

        // Glowing visor slit across the eyes, oriented by facing.
        let visor_c = head_c + fwd * (hr * 0.94) + Vec3::new(0.0, hr * 0.08, 0.0);
        let up = Vec3::UP;
        let visor_col = if dying {
            lerp_c(d.visor, rgb(30, 30, 30), (self.death_t).min(1.0))
        } else {
            d.visor
        };
        let mut visor = panel(visor_c, right, up, hr * 0.82, hr * 0.22, visor_col);
        visor.emissive = if dying { 0.1 } else { 0.9 };
        m.merge(&visor);

        // Per-kind silhouette flair so you can read the threat at a glance.
        match self.kind {
            EnemyKind::Marksman => {
                // a tall antenna/scope mast with a glowing tip
                let mast_b = head_c + Vec3::new(0.0, d.radius * 0.6, 0.0);
                let mast_t = head_c + Vec3::new(0.0, d.radius * 1.7, 0.0);
                m.merge(&boxx((mast_b + mast_t) * 0.5, Vec3::new(0.03, (mast_t.y - mast_b.y) * 0.5, 0.03), limb, FACE_ALL));
                let mut tip = boxx(mast_t, Vec3::splat(0.07), d.visor, FACE_ALL);
                tip.emissive = if dying { 0.1 } else { 0.9 };
                m.merge(&tip);
            }
            EnemyKind::Rusher => {
                // aggressive shoulder spikes
                for side in [-1.0f32, 1.0] {
                    let sc = torso_c + right * (d.radius * 1.0 * side) + Vec3::new(0.0, d.radius * 0.5, 0.0);
                    m.merge(&boxx(sc, Vec3::new(0.07, 0.16, 0.07), dither(limb, side as i32 + 3, 0.1), FACE_ALL));
                }
            }
            EnemyKind::Screecher => {
                // swept-back blade crests reading as a fast, predatory silhouette
                for side in [-1.0f32, 1.0] {
                    let sc = torso_c + right * (d.radius * 0.9 * side) + Vec3::new(0.0, d.radius * 0.7, 0.0);
                    let mut blade = boxx(sc - fwd * 0.25, Vec3::new(0.05, 0.28, 0.16), dither(limb, side as i32 + 5, 0.1), FACE_ALL);
                    crate::mesh::rotate_y_about(&mut blade, sc, side * 0.5);
                    m.merge(&blade);
                }
            }
            EnemyKind::DeathMech => {
                // tall horns + a glowing chest reactor core
                for side in [-1.0f32, 1.0] {
                    let hb = head_c + right * (d.radius * 0.4 * side) + Vec3::new(0.0, d.radius * 0.6, 0.0);
                    m.merge(&boxx(hb, Vec3::new(0.05, 0.22, 0.05), limb, FACE_ALL));
                }
                let mut core = uv_sphere(torso_c + fwd * (d.radius * 0.7), 0.13, 6, 4, d.visor);
                core.emissive = if dying { 0.1 } else { 0.9 };
                m.merge(&core);
            }
            _ => {}
        }

        // Death topple: for the first ~0.6s after dying the whole body rotates forward and falls to the
        // ground about the feet, so the kill reads as a body dropping rather than sinking straight down. After
        // it's toppled it lies flat while the corpse sinks/fades (driven by `pos.y` in `update`).
        if dying {
            let fall = (self.death_t / 0.6).clamp(0.0, 1.0);
            // ease-in-out so it starts slow (knees buckle), accelerates, then settles flat
            let ease = fall * fall * (3.0 - 2.0 * fall);
            let ang = ease * (std::f32::consts::FRAC_PI_2 * 0.98); // ~90° onto its front
            // pivot about the feet, tipping forward (about the body's right axis, i.e. rotate in the fwd/up plane)
            let pivot = Vec3::new(self.pos.x, self.pos.y + 0.05, self.pos.z);
            // slight sideways keel too, so it doesn't fall like a rigid plank
            let keel = ease * 0.18 * if (self.id & 1) == 0 { 1.0 } else { -1.0 };
            for v in m.verts.iter_mut() {
                // translate to pivot
                let mut p = *v - pivot;
                // forward tip: rotate p around the `right` axis by `ang` (tips fwd toward the ground)
                // right axis = (c,0,s); build rotation of p about that axis
                let axis = right;
                let (sa, ca) = ang.sin_cos();
                // Rodrigues' rotation
                let dot = p.x * axis.x + p.y * axis.y + p.z * axis.z;
                let cross = Vec3::new(
                    axis.y * p.z - axis.z * p.y,
                    axis.z * p.x - axis.x * p.z,
                    axis.x * p.y - axis.y * p.x,
                );
                p = p * ca + cross * sa + axis * (dot * (1.0 - ca));
                // keel roll about the forward axis
                if keel != 0.0 {
                    let (sk, ck) = keel.sin_cos();
                    let ax2 = fwd;
                    let dot2 = p.x * ax2.x + p.y * ax2.y + p.z * ax2.z;
                    let cross2 = Vec3::new(
                        ax2.y * p.z - ax2.z * p.y,
                        ax2.z * p.x - ax2.x * p.z,
                        ax2.x * p.y - ax2.y * p.x,
                    );
                    p = p * ck + cross2 * sk + ax2 * (dot2 * (1.0 - ck));
                }
                *v = p + pivot;
            }
        }

        // Spawn-in: rise from below the floor with a bright glow that fades as the body settles.
        if self.spawn_t < 1.0 {
            let s = self.spawn_t;
            m.offset.y -= (1.0 - s) * 1.8;
            m.emissive = m.emissive.max((1.0 - s) * 0.9);
        }

        // Death-Mech cloak: while cloaked the body goes nearly invisible (a faint shimmer you can just track).
        if self.cloaked() {
            crate::mesh::set_alpha(&mut m, 40);
            m.emissive *= 0.25;
        }

        m
    }

    /// Build a hunt creature as its own anatomy (not a humanoid with parts bolted on). Handles the death
    /// topple, sink and hurt-flash the same way the soldier body does. `base` is the already-computed body
    /// tint (dark crimson for hunters, flashing pale on hit). Facing is `fwd`/`right`.
    fn build_creature_body(&self, cr: crate::creatures::Creature, base: egui::Color32, fwd: Vec3, right: Vec3) -> Mesh3 {
        use crate::creatures::Creature;
        let dying = self.state == EnemyState::Dead;
        let phase = self.walk_t * 2.2;
        let moving = matches!(self.state, EnemyState::Chase | EnemyState::Search);
        let gait = if moving { 1.0 } else { 0.25 };
        let mut m = Mesh3::new(base);
        // ground reference (feet/base of the creature at the enemy's position)
        let gy = self.pos.y;

        match cr {
            Creature::Kraken => {
                // ── Storm Kraken: a floating mantle (big teardrop head) trailing eight lashing tentacles, all
                // crackling with charge. No legs, no torso — it hovers. ─────────────────────────────────────
                let ink = lerp_c(base, rgb(30, 55, 110), 0.65);
                let ink2 = lerp_c(ink, rgb(10, 20, 45), 0.4);
                let glow = rgb(130, 195, 255);
                let hover = (phase * 0.8).sin() * 0.12; // bob up and down as it floats
                let body_y = gy + 2.1 + hover;
                let mantle_c = Vec3::new(self.pos.x, body_y, self.pos.z);
                let r = 0.85; // mantle radius

                // mantle: a sphere stretched into a pointed teardrop (point up)
                let mut mantle = uv_sphere(mantle_c, r, 14, 11, ink);
                for v in mantle.verts.iter_mut() {
                    let dy = v.y - mantle_c.y;
                    v.y = mantle_c.y + dy * if dy > 0.0 { 1.7 } else { 1.05 }; // taller crown, rounded base
                }
                mantle.emissive = 0.08;
                m.merge(&mantle);
                // darker mantle underside / head ring where the tentacles root
                m.merge(&uv_sphere(mantle_c - Vec3::new(0.0, r * 0.7, 0.0), r * 0.95, 12, 6, ink2));
                // glowing ridge fins running up the crown
                for k in -2..=2 {
                    let off = right * (k as f32 * r * 0.28);
                    let a = mantle_c + off + Vec3::new(0.0, r * 0.2, 0.0) - fwd * (r * 0.1);
                    let b = a + Vec3::new(0.0, r * 1.5, 0.0) - fwd * (r * 0.25);
                    let mut fin = tube(a, b, 0.05, 4, lerp_c(glow, rgb(210, 240, 255), 0.4));
                    fin.emissive = 0.5;
                    m.merge(&fin);
                }
                // two big glowing eyes on the front of the mantle
                for side in [-1.0f32, 1.0] {
                    let ec = mantle_c + fwd * (r * 0.82) + right * (r * 0.34 * side) - Vec3::new(0.0, r * 0.1, 0.0);
                    let mut eye = uv_sphere(ec, 0.14, 8, 6, glow);
                    eye.emissive = 1.0;
                    m.merge(&eye);
                    // dark socket ring
                    m.merge(&uv_sphere(ec - fwd * 0.05, 0.17, 8, 6, ink2));
                }
                // eight tentacles hanging and lashing from the mantle base
                let charge = 0.5 + 0.5 * (phase * 1.4).sin();
                for k in 0..8 {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU;
                    let dirx = a.cos();
                    let dirz = a.sin();
                    let sway = (phase + a * 1.6).sin();
                    let root = mantle_c + Vec3::new(dirx * r * 0.55, -r * 0.65, dirz * r * 0.55);
                    // three-segment tentacle curling outward then down, swaying on the phase
                    let p1 = root + Vec3::new(dirx * 0.5, -0.55, dirz * 0.5) + right * (sway * 0.18 * gait);
                    let p2 = p1 + Vec3::new(dirx * 0.35, -0.6, dirz * 0.35) + fwd * (sway * 0.25 * gait);
                    let p3 = p2 + Vec3::new(dirx * 0.15, -0.55, dirz * 0.15) + right * (sway * 0.3 * gait);
                    m.merge(&tube(root, p1, 0.13, 7, ink));
                    m.merge(&tube(p1, p2, 0.085, 6, lerp_c(ink, glow, 0.15)));
                    m.merge(&tube(p2, p3, 0.05, 6, lerp_c(ink, glow, 0.35)));
                    // a charged node glimmering at the tip
                    let mut node = uv_sphere(p3, 0.07, 6, 5, lerp_c(glow, rgb(225, 245, 255), charge));
                    node.emissive = 0.5 + charge * 0.5;
                    m.merge(&node);
                }
            }
            Creature::Widow => {
                // ── Widow: a low, wide spider — a fat abdomen + a smaller cephalothorax carried on eight long
                // jointed legs that arch up and plant wide, with a fanged, many-eyed head. ──────────────────
                let chitin = lerp_c(base, rgb(28, 14, 30), 0.6);
                let chitin2 = lerp_c(chitin, rgb(8, 6, 10), 0.4);
                let mark = rgb(170, 50, 150);
                let body_y = gy + 0.62; // the body slung low between the legs
                let cephalo = Vec3::new(self.pos.x, body_y, self.pos.z) + fwd * 0.35;
                let abdomen = Vec3::new(self.pos.x, body_y + 0.05, self.pos.z) - fwd * 0.55;

                // abdomen: a big round sphere, slightly squashed, with the hourglass mark on top-rear
                let mut abd = uv_sphere(abdomen, 0.62, 12, 9, chitin);
                for v in abd.verts.iter_mut() {
                    let dy = v.y - abdomen.y;
                    v.y = abdomen.y + dy * 0.85;
                }
                m.merge(&abd);
                let mut spot = uv_sphere(abdomen + Vec3::new(0.0, 0.4, 0.0) - fwd * 0.25, 0.16, 7, 5, mark);
                spot.emissive = 0.35;
                m.merge(&spot);
                // cephalothorax: a smaller, flatter sphere at the front
                let mut ceph = uv_sphere(cephalo, 0.4, 10, 8, chitin2);
                for v in ceph.verts.iter_mut() {
                    let dy = v.y - cephalo.y;
                    v.y = cephalo.y + dy * 0.7;
                }
                m.merge(&ceph);
                // head/fangs at the very front
                let headc = cephalo + fwd * 0.34;
                m.merge(&uv_sphere(headc, 0.2, 8, 6, chitin2));
                // a cluster of eight glinting eyes
                let eyepulse = 0.55 + 0.45 * (phase * 1.5).sin();
                for row in 0..2 {
                    for col in 0..4 {
                        let ex = (col as f32 - 1.5) * 0.09;
                        let ey = 0.05 + row as f32 * 0.08;
                        let ec = headc + fwd * 0.14 + right * ex + Vec3::new(0.0, ey, 0.0);
                        let mut e = uv_sphere(ec, 0.035, 4, 4, lerp_c(mark, rgb(235, 90, 210), eyepulse));
                        e.emissive = 0.5 + eyepulse * 0.4;
                        m.merge(&e);
                    }
                }
                // two forward fangs that flex with the gait
                let flex = (phase * 1.8).sin() * 0.04 * gait;
                for side in [-1.0f32, 1.0] {
                    let bf = headc + fwd * 0.12 + right * (0.09 * side) - Vec3::new(0.0, 0.05, 0.0);
                    let tf = bf + fwd * 0.14 - Vec3::new(0.0, 0.22 + flex, 0.0);
                    m.merge(&tube(bf, tf, 0.035, 5, rgb(225, 215, 205)));
                }
                // eight legs: four per side, each a long two-segment limb arching UP from the body then down to
                // a planted foot well out to the side — the classic tall spider stance, scuttling on the phase.
                for side in [-1.0f32, 1.0] {
                    for li in 0..4 {
                        let along = (li as f32 - 1.5) * 0.34; // spread front-to-back
                        let gaitp = phase * 1.8 + li as f32 * 1.5 + if side > 0.0 { 0.0 } else { std::f32::consts::PI };
                        let step = gaitp.sin();
                        let lift = step.max(0.0) * 0.18 * gait; // foot lifts on the forward swing
                        let hip = Vec3::new(self.pos.x, body_y + 0.1, self.pos.z) + fwd * along + right * (0.4 * side);
                        // knee arches high and out
                        let knee = hip + right * (0.55 * side) + Vec3::new(0.0, 0.5, 0.0) + fwd * (along * 0.3);
                        // foot plants wide on the ground, shifting fore/aft with the gait
                        let foot = Vec3::new(
                            hip.x + right.x * 1.05 * side + fwd.x * (along * 0.5 + step * 0.18 * gait),
                            gy + 0.02 + lift,
                            hip.z + right.z * 1.05 * side + fwd.z * (along * 0.5 + step * 0.18 * gait),
                        );
                        m.merge(&tube(hip, knee, 0.06, 5, chitin));
                        m.merge(&tube(knee, foot, 0.045, 5, chitin2));
                        // a dark claw at the foot
                        m.merge(&uv_sphere(foot, 0.05, 4, 3, chitin2));
                    }
                }
            }
            _ => {}
        }

        // Death: sink + desaturate (the topple looks wrong on a radial creature, so these just sink and fade).
        if dying {
            let sink = (self.death_t * 0.9).min(2.0);
            m.offset.y -= sink;
            m.emissive *= (1.0 - (self.death_t * 0.5)).max(0.0);
        }
        m
    }
}

#[inline]
fn jitter(dir: Vec3, spread: f32, rng: &mut Rng) -> Vec3 {
    if spread <= 0.0 {
        return dir;
    }
    // Build a basis around dir and nudge by a small random angle in each perpendicular axis.
    let up = if dir.y.abs() > 0.95 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::UP };
    let right = dir.cross(up).normalized();
    let realup = right.cross(dir).normalized();
    let a = (rng.f32() - 0.5) * 2.0 * spread;
    let b = (rng.f32() - 0.5) * 2.0 * spread;
    (dir + right * a + realup * b).normalized()
}

/// Distribute `n` enemies across the level's rooms (skipping the spawn room) with a kind mix.
pub fn populate(level: &Level, n: usize, seed: u64) -> Vec<Enemy> {
    // Best-effort, exactly once per process: if a previous `bot_arena` run left a bred population on disk,
    // pick it up automatically so real matches use it without any extra setup. A no-op (and free after the
    // first call) if the file doesn't exist or this build has no filesystem access.
    #[cfg(not(target_arch = "wasm32"))]
    {
        static LOAD_ONCE: std::sync::Once = std::sync::Once::new();
        LOAD_ONCE.call_once(|| {
            load_default_pool();
        });
    }
    let mut rng = Rng::new(seed ^ 0x51DE_5EED);
    let mut out = Vec::new();
    if level.rooms.len() <= 1 {
        return out;
    }
    let mut id = 0usize;
    let mut attempts = 0;
    // Which rooms may enemies spawn in? Normally any room except the player's spawn (room 0). But some
    // authored maps (Concrete Slab, Redwood Canyon) define ONLY spawn rooms — their walkable space is
    // freestanding platforms, not grid rooms. If there's no non-spawn room to use, fall back to spawning
    // around the *enemy* spawn (the last room), scattered widely, so waves actually populate instead of
    // coming up empty (which instantly "clears" the wave → infinite wave-spam).
    let spawn_only = level.rooms.len() <= 2;
    while out.len() < n && attempts < n * 20 {
        attempts += 1;
        let (base_room, scatter) = if spawn_only {
            (&level.rooms[level.rooms.len() - 1], 18)
        } else {
            let ri = 1 + (rng.next_u64() as usize % (level.rooms.len() - 1));
            (&level.rooms[ri], 3)
        };
        let cx = (base_room.rect.x0 + base_room.rect.x1) / 2;
        let cz = (base_room.rect.z0 + base_room.rect.z1) / 2;
        let rx = (cx + rng.i32_range(-scatter, scatter)).clamp(1, level.w - 2);
        let rz = (cz + rng.i32_range(-scatter, scatter)).clamp(1, level.h - 2);
        let ts = level.ts();
        let wp = Vec2::new((rx as f32 + 0.5) * ts, (rz as f32 + 0.5) * ts);
        // Validate against the true walkable surface (grid floor OR platform top) and keep enemies off cover.
        let Some(surf_y) = level.surface_at(wp) else { continue };
        if footprint_blocked(level, wp, 0.5) {
            continue;
        }
        // don't spawn on top of the player's own spawn
        let pspawn = &level.rooms[0];
        let pdx = rx - (pspawn.rect.x0 + pspawn.rect.x1) / 2;
        let pdz = rz - (pspawn.rect.z0 + pspawn.rect.z1) / 2;
        if pdx * pdx + pdz * pdz < 36 {
            continue;
        }
        let p = Vec3::new(wp.x, surf_y, wp.y);
        // Kind mix scales with the wave (larger n = later wave): grunts and a brute anchor early packs;
        // deeper waves bring rushers, marksmen, fast Screechers, and Death-Mechs (which arrive in trios).
        let roll = rng.f32();
        let advanced = ((n as f32 - 6.0) / 16.0).clamp(0.0, 0.5); // 0 early → 0.5 in deep waves
        let b1 = 0.14;
        let b2 = b1 + advanced * 0.5; // rusher
        let b3 = b2 + advanced * 0.35; // marksman
        let b4 = b3 + advanced * 0.5; // screecher
        let b5 = b4 + advanced * 0.4; // death-mech
        let kind = if roll < b1 {
            EnemyKind::Brute
        } else if roll < b2 {
            EnemyKind::Rusher
        } else if roll < b3 {
            EnemyKind::Marksman
        } else if roll < b4 {
            EnemyKind::Screecher
        } else if roll < b5 {
            EnemyKind::DeathMech
        } else {
            EnemyKind::Grunt
        };
        let base = p; // the validated surface position (grid floor or platform top)
        let mut e = Enemy::new(id, kind, base);
        e.yaw = rng.f32() * std::f32::consts::TAU;
        e.genome = roster_genome(id, seed);
        out.push(e);
        id += 1;
        // Death-Mechs move in trios: spawn up to two more around the first, budget and floor permitting.
        if kind == EnemyKind::DeathMech {
            for k in 0..2 {
                if out.len() >= n {
                    break;
                }
                let mp = Vec2::new(base.x + ((k as f32) - 0.5) * 2.4, base.z + 1.6 + k as f32 * 0.8);
                if let Some(sy) = level.surface_at(mp) {
                    if !footprint_blocked(level, mp, 0.6) {
                        let mut m = Enemy::new(id, EnemyKind::DeathMech, Vec3::new(mp.x, sy, mp.y));
                        m.yaw = rng.f32() * std::f32::consts::TAU;
                        m.tp_cd = 2.0 + k as f32; // stagger their first blinks
                        m.genome = roster_genome(id, seed);
                        out.push(m);
                        id += 1;
                    }
                }
            }
        }
    }
    out
}

/// Personality for roster slot `id`: mostly the hand-tuned archetypes (cycled, so a wave reads as a mix of
/// distinct fighters rather than clones), salted with the odd fully-random genome so no two matches ever
/// play out quite the same way. `EVOLVED_POOL` (see below) takes over automatically once a run has produced
/// one — that's the hook that lets bred champions actually show up in real matches, not just the arena tool.
fn roster_genome(id: usize, seed: u64) -> Genome {
    let pool = evolved_pool();
    if !pool.is_empty() {
        return pool[id % pool.len()];
    }
    let mut rng = Rng::new(seed ^ (id as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ 0xB0757);
    if rng.f32() < 0.8 {
        Genome::archetype(id + (rng.next_u64() as usize))
    } else {
        Genome::random(&mut rng)
    }
}

/// An evolved genome pool, set once by `set_evolved_pool` after a `bot_arena` run. Empty by default, in
/// which case `roster_genome` falls back to the hand-authored archetypes above. A `Mutex` keeps this sound
/// under egui's single-threaded update loop without plumbing a pool handle through every spawn call site.
static EVOLVED_POOL: std::sync::Mutex<Vec<Genome>> = std::sync::Mutex::new(Vec::new());

/// Install a bred population (best-first, as returned by `evolve::evolve`) so future roster spawns draw
/// their personalities from it instead of the static archetypes — the bridge from the offline simulator to
/// actual gameplay.
pub fn set_evolved_pool(pool: Vec<Genome>) {
    if let Ok(mut p) = EVOLVED_POOL.lock() {
        *p = pool;
    }
}

/// Best-effort: if a previously-bred population was saved to `evolve::DEFAULT_POOL_PATH` (by running the
/// `bot_arena` example), load and install it. Silently does nothing if the file is missing or unreadable —
/// this is meant to be called unconditionally at game startup, not gated behind a check first. Returns
/// how many genomes were loaded, so a caller can log it if it wants to.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_default_pool() -> usize {
    match crate::evolve::load_pool(crate::evolve::DEFAULT_POOL_PATH) {
        Some(pool) => {
            let n = pool.len();
            set_evolved_pool(pool);
            n
        }
        None => 0,
    }
}

fn evolved_pool() -> Vec<Genome> {
    EVOLVED_POOL.lock().map(|p| p.clone()).unwrap_or_default()
}

// Keep clampf referenced even if a future refactor drops its only use.
#[allow(dead_code)]
fn _touch(x: f32) -> f32 {
    clampf(x, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Level, MapConfig};

    fn lvl() -> Level {
        Level::generate(MapConfig::default())
    }

    #[test]
    fn every_authored_map_populates_a_full_wave() {
        // Regression for the wave-spam bug: authored maps whose walkable surface is freestanding platforms
        // (Concrete Slab, Redwood Canyon) used to spawn ZERO enemies — populate only tried grid rooms and
        // grid-floor validation, which those maps don't have. An empty wave "clears" instantly → infinite
        // wave spam. populate must now place a full wave on ANY authored map, validated on the real surface.
        use crate::level::PRESETS;
        for name in ["Concrete Slab", "Redwood Canyon", "Desert Town", "Flood Zone", "Pyre Garage"] {
            let Some(p) = PRESETS.iter().find(|p| p.name == name) else { continue };
            let l = Level::generate(p.config(7));
            let e = populate(&l, 13, 42);
            assert!(e.len() >= 10, "{name} spawned only {} enemies (wave-spam risk)", e.len());
            // every enemy must stand on a real walkable surface, not fall through the world
            for en in &e {
                assert!(
                    l.surface_at(crate::math::Vec2::new(en.pos.x, en.pos.z)).is_some(),
                    "{name}: an enemy spawned off any walkable surface at {:?}",
                    en.pos
                );
            }
        }
    }

    /// `EVOLVED_POOL` is a process-global static, and Rust runs tests in parallel by default — without this,
    /// the three tests below would race each other's `set_evolved_pool` calls and flake. It guards nothing
    /// but test ordering; unrelated tests never touch it and run fully in parallel as normal.
    static POOL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn evolved_pool_feeds_roster_spawns() {
        let _guard = POOL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The bridge from the offline GA to real matches: once a pool is installed, populate() should draw
        // personalities from it (in order, cycling) instead of the hand-authored archetype fallback.
        let l = lvl();
        let pool = vec![
            Genome { aggression: 0.11, awareness: 0.22, flank_bias: 0.33, reaction: 0.44, accuracy: 0.55, persistence: 0.66 },
            Genome { aggression: 0.91, awareness: 0.82, flank_bias: 0.73, reaction: 0.64, accuracy: 0.55, persistence: 0.46 },
        ];
        set_evolved_pool(pool.clone());
        let roster = populate(&l, 6, 123);
        assert!(!roster.is_empty(), "the test map should have room to place a roster");
        for (i, e) in roster.iter().enumerate() {
            assert_eq!(e.genome, pool[i % pool.len()], "roster slot {i} should draw from the installed pool");
        }
        // Clear it so this test doesn't leak state into others that assume the archetype fallback.
        set_evolved_pool(Vec::new());
    }

    #[test]
    fn empty_pool_falls_back_to_varied_archetypes() {
        let _guard = POOL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let l = lvl();
        set_evolved_pool(Vec::new());
        let roster = populate(&l, 8, 777);
        assert!(!roster.is_empty());
        // Not every bot should be the neutral default genome — the fallback should actually assign
        // personality, not silently no-op.
        let distinct = roster.iter().filter(|e| e.genome != Genome::default()).count();
        assert!(distinct > 0, "fallback roster spawns should carry non-default personalities");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn load_default_pool_installs_a_pool_saved_by_evolve() {
        let _guard = POOL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Exercises the real path a live game takes: evolve::save_pool writes a file, enemy::load_default_pool
        // reads *that exact path* and installs it, and the very next roster spawn draws from it. This is the
        // bridge between the offline bot_arena tool and an actual playthrough, proven end to end.
        let saved = vec![
            Genome { aggression: 0.81, awareness: 0.11, flank_bias: 0.42, reaction: 0.63, accuracy: 0.29, persistence: 0.55 },
            Genome::archetype(2),
        ];
        crate::evolve::save_pool(crate::evolve::DEFAULT_POOL_PATH, &saved).expect("save should succeed in a writable test cwd");
        set_evolved_pool(Vec::new()); // clear any state a previous test left behind
        let n = load_default_pool();
        assert_eq!(n, saved.len(), "should report loading exactly what was saved");
        let l = lvl();
        let roster = populate(&l, 4, 55);
        assert!(!roster.is_empty());
        for (i, e) in roster.iter().enumerate() {
            assert_eq!(e.genome, saved[i % saved.len()], "roster should draw from the just-loaded pool");
        }
        set_evolved_pool(Vec::new());
        let _ = std::fs::remove_file(crate::evolve::DEFAULT_POOL_PATH);
    }

    #[test]
    fn a_marksman_holds_range_while_a_rusher_charges() {
        let l = lvl();
        let mut rng = Rng::new(3);
        let (a, b) = clear_pair(&l); // two nearby floor points with clear space between
        let eye = a + Vec3::new(0.0, 1.6, 0.0);
        let dist = |e: &Enemy| ((e.pos.x - a.x).powi(2) + (e.pos.z - a.z).powi(2)).sqrt();

        let mut mk = Enemy::new(0, EnemyKind::Marksman, b);
        mk.state = EnemyState::Chase;
        let dm0 = dist(&mk);
        for _ in 0..30 {
            mk.update(&l, eye, a, 1.0, 1.0, 1.0, false, &mut rng, 1.0 / 30.0);
        }
        // A marksman never charges the player — it holds or extends range.
        assert!(dist(&mk) >= dm0 - 0.05, "marksman should not close in");

        let mut ru = Enemy::new(1, EnemyKind::Rusher, b);
        ru.state = EnemyState::Chase;
        let dr0 = dist(&ru);
        for _ in 0..30 {
            ru.update(&l, eye, a, 1.0, 1.0, 1.0, false, &mut rng, 1.0 / 30.0);
        }
        // A rusher closes the gap aggressively.
        assert!(dist(&ru) < dr0 - 0.3, "rusher should charge in (d0={dr0}, d1={})", dist(&ru));
    }

    /// Find two floor points a few units apart, in a non-spawn room, with no crate between them.
    fn clear_pair(l: &Level) -> (Vec3, Vec3) {
        for room in l.rooms.iter().skip(1) {
            for z in (room.rect.z0 + 1)..(room.rect.z1 - 1) {
                for x in (room.rect.x0 + 1)..(room.rect.x1 - 2) {
                    let a = l.tile_center(x, z).with_y(l.floor_y);
                    let b = l.tile_center(x + 2, z).with_y(l.floor_y);
                    if !l.has_floor_at(a.xz()) || !l.has_floor_at(b.xz()) {
                        continue;
                    }
                    let from = a + Vec3::new(0.0, 1.4, 0.0);
                    let to = (b + Vec3::new(0.0, 1.6, 0.0)) - from;
                    let dist = to.len();
                    let h = raycast(l, &[], from, to.normalized(), dist);
                    let clear = !(matches!(h.kind, HitKind::Wall | HitKind::Obstacle | HitKind::Destructible(_)) && h.dist < dist - 0.2);
                    if clear && !footprint_blocked(l, a.xz(), 0.4) && !footprint_blocked(l, b.xz(), 0.4) {
                        return (a, b);
                    }
                }
            }
        }
        // Fallback: spawn room centre.
        let s = l.spawn(1);
        (s, s + Vec3::new(4.0, 0.0, 0.0))
    }

    #[test]
    fn enemy_takes_damage_and_dies() {
        let l = lvl();
        let mut e = Enemy::new(0, EnemyKind::Grunt, l.spawn(1));
        assert!(e.alive());
        e.damage(1000.0);
        assert!(!e.alive());
        assert_eq!(e.state, EnemyState::Dead);
    }

    #[test]
    fn idle_enemy_wakes_and_chases_a_visible_player() {
        let l = lvl();
        let (epos, ppos) = clear_pair(&l);
        let mut e = Enemy::new(0, EnemyKind::Grunt, epos);
        let player_feet = ppos;
        let player_eye = player_feet + Vec3::new(0.0, 1.6, 0.0);
        let mut rng = Rng::new(1);
        e.update(&l, player_eye, player_feet, 1.0, 1.0, 1.0, false, &mut rng, 0.016);
        assert_eq!(e.state, EnemyState::Chase);
    }

    #[test]
    fn chasing_enemy_moves_toward_the_player() {
        let l = lvl();
        let (epos, near) = clear_pair(&l);
        // Aim the player along the same clear axis but farther, so the enemy wants to close in.
        let dir = (near - epos).normalized();
        let target = epos + dir * 9.0;
        let mut e = Enemy::new(0, EnemyKind::Grunt, epos);
        e.state = EnemyState::Chase;
        let player_eye = target + Vec3::new(0.0, 1.6, 0.0);
        let mut rng = Rng::new(2);
        let before = e.pos.distance(target);
        for _ in 0..40 {
            e.update(&l, player_eye, target, 1.0, 1.0, 1.0, false, &mut rng, 0.016);
        }
        let after = e.pos.distance(target);
        assert!(after < before, "enemy should have closed distance: {before} -> {after}");
    }

    #[test]
    fn populate_places_enemies_off_the_spawn_room() {
        let l = lvl();
        let es = populate(&l, 8, 7);
        assert!(!es.is_empty());
        for e in &es {
            // none should be standing inside geometry
            assert!(!footprint_blocked(&l, e.pos.xz(), 0.4));
        }
    }

    #[test]
    fn builds_a_mesh_with_triangles() {
        let l = lvl();
        let e = Enemy::new(0, EnemyKind::Brute, l.spawn(1));
        let m = e.build_mesh();
        assert!(m.tri_count() > 8);
    }
    #[test]
    fn screecher_is_fast_and_armored() {
        let s = EnemyKind::Screecher.def();
        assert!(s.speed > EnemyKind::Grunt.def().speed, "screecher should be fast");
        assert!(s.armored, "screecher should be armored");
    }

    #[test]
    fn armored_aliens_shrug_off_part_of_the_hit() {
        // Same raw damage to a Grunt vs a Screecher: the armored one loses less health.
        let mut grunt = Enemy::new(0, EnemyKind::Grunt, Vec3::ZERO);
        let mut screech = Enemy::new(1, EnemyKind::Screecher, Vec3::ZERO);
        let g0 = grunt.health;
        let s0 = screech.health;
        grunt.damage(30.0);
        screech.damage(30.0);
        let g_lost = g0 - grunt.health;
        let s_lost = s0 - screech.health;
        assert!(s_lost < g_lost, "armor should reduce damage taken ({} vs {})", s_lost, g_lost);
    }

    #[test]
    fn death_mech_cloaks_in_and_out() {
        let mut m = Enemy::new(0, EnemyKind::DeathMech, Vec3::ZERO);
        m.state = EnemyState::Chase;
        // sweep the cloak cycle and confirm it both cloaks and decloaks across the period
        let mut saw_cloaked = false;
        let mut saw_visible = false;
        let l = lvl();
        let mut rng = Rng::new(5);
        let eye = Vec3::new(0.0, 1.6, 40.0); // far away so it never fires, just cycles
        for _ in 0..400 {
            m.update(&l, eye, Vec3::new(0.0, 0.0, 40.0), 1.0, 1.0, 1.0, false, &mut rng, 1.0 / 60.0);
            if m.cloaked() { saw_cloaked = true; } else { saw_visible = true; }
        }
        assert!(saw_cloaked && saw_visible, "a death-mech should phase in and out of cloak");
    }

    #[test]
    fn death_mech_teleports_in_when_pushed_too_far() {
        let l = lvl();
        let mut rng = Rng::new(9);
        let (a, b) = clear_pair(&l);
        let eye = a + Vec3::new(0.0, 1.6, 0.0);
        // place the mech far from the player along the open line
        let far = Vec3::new(a.x + (b.x - a.x) * 4.0, b.y, a.z + (b.z - a.z) * 4.0);
        let mut m = Enemy::new(0, EnemyKind::DeathMech, far);
        m.state = EnemyState::Chase;
        m.tp_cd = 0.0;
        let d0 = ((m.pos.x - a.x).powi(2) + (m.pos.z - a.z).powi(2)).sqrt();
        if d0 <= 16.0 {
            return; // map too small to test the blink; skip
        }
        for _ in 0..20 {
            m.update(&l, eye, a, 1.0, 1.0, 1.0, false, &mut rng, 1.0 / 60.0);
        }
        let d1 = ((m.pos.x - a.x).powi(2) + (m.pos.z - a.z).powi(2)).sqrt();
        assert!(d1 < d0 - 3.0, "death-mech should teleport closer ({} -> {})", d0, d1);
    }

    #[test]
    fn dying_body_topples_forward_before_sinking() {
        // The death animation should rotate the body over (topple) during the first ~0.6s: the highest point
        // of the mesh must drop as it falls, and it must not immediately sink through the floor.
        let mut e = Enemy::new(0, EnemyKind::Grunt, Vec3::new(0.0, 0.0, 0.0));
        let standing_top = e.build_mesh_tinted(None).verts.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        e.state = EnemyState::Dead;
        e.death_t = 0.0;
        let d0_top = e.build_mesh_tinted(None).verts.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        e.death_t = 0.6;
        let d6 = e.build_mesh_tinted(None);
        let d6_top = d6.verts.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        // just-dead should look about like standing (topple hasn't progressed)
        assert!((d0_top - standing_top).abs() < 0.15, "body shouldn't jump at the instant of death");
        // fully toppled should be markedly lower than standing (lying down)
        assert!(d6_top < standing_top - 0.3, "toppled body should be much lower ({d6_top:.2} vs {standing_top:.2})");
    }

    #[test]
    fn every_hunt_creature_builds_visible_geometry() {
        // Guards the creature-body gate: whether a creature keeps the humanoid rig (Revenant/Stalker/Brute/
        // Wraith) or gets a dedicated exotic body (Kraken/Widow), it must never render as an empty mesh — an
        // invisible predator is a broken Hunt. Regression: the gate once diverted *all* creatures to the
        // dedicated builder, which only knew Kraken/Widow, leaving the four humanoid horrors with zero tris.
        use crate::creatures::Creature;
        for cr in Creature::ALL {
            let mut e = Enemy::new(1, EnemyKind::Grunt, Vec3::new(0.0, 0.0, 0.0));
            e.hunter = true;
            e.hunter_creature = Some(cr);
            let m = e.build_mesh();
            assert!(
                m.verts.len() >= 12 && !m.tris.is_empty(),
                "Hunt creature {:?} built a near-empty body ({} verts, {} tris) — it would be invisible",
                cr,
                m.verts.len(),
                m.tris.len()
            );
        }
    }

    #[test]
    fn corpse_persists_through_the_topple_then_is_removed() {
        // The corpse must not be removed until after it has toppled and sunk.
        let mut e = Enemy::new(0, EnemyKind::Grunt, Vec3::new(0.0, 0.0, 0.0));
        e.state = EnemyState::Dead;
        e.death_t = 0.5; // mid-topple
        assert!(!e.should_remove(), "corpse should linger through the topple");
        e.death_t = 2.4; // after sink
        assert!(e.should_remove(), "corpse should eventually be cleared");
    }

}
