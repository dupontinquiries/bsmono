//! ENEMY — a minimal AI agent, ported from the C engine's `exengine/
//! enemy.h`/`enemy.c`. This is the real foundation `escalation.rs`
//! named as missing: an actual thing with a position, HP, and a state
//! machine, headless-testable exactly the way the C original was
//! designed to be ("Combat treats each enemy as a Target sphere...
//! Rendering is the caller's job — this module owns position, HP, and
//! AI state so it's fully testable headless").
//!
//! Ported faithfully, not redesigned: idle -> chase -> attack -> dead,
//! four archetypes (grunt, corrupted, hijack, apex — reusing
//! `escalation::EnemyKind` rather than a second copy of the same four
//! variants), the corruption staging escalation, the apex boss's
//! phase/slam/summon timers, and squad-scatter-on-commander-death
//! reads. Numbers are the C engine's own.
//!
//! **What this does NOT do**, same honesty as `escalation.rs`: no
//! rendering, no real line-of-sight raycast against this engine's own
//! `World` (the C original needed a `chunk_world_t`; this takes a
//! plain `has_los: bool` the caller supplies, exactly how the C
//! test suite itself exercises the AI with `chunks = NULL` and no
//! blocking geometry — the raycast is a separate, later integration
//! against `World::sdf`, not invented here), and no squad-scatter
//! COUNTDOWN — the C original reads `scatter_t` in its CHASE branch
//! but never decrements it inside `ex_enemy_update` either; that
//! timer is driven from outside, by whatever kills the commander, and
//! this port keeps that same boundary rather than guessing at it.
//!
//! **Cloakers, the person's own addition**: the C reference has a
//! player-side cloak ability (`ex_enemy_set_cloak`, ported here as an
//! `update()` parameter rather than a global) that makes
//! non-spark-hunter enemies (grunt, hijack) lose track of the player.
//! An enemy that is ITSELF cloaked/stealthed does not exist in the
//! reference at all — that's genuinely new design, not a port, and
//! is not attempted here.

use crate::escalation::EnemyKind;
use crate::weapons::Element;
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Chase,
    Attack,
    Dead,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Chase => "chase",
            State::Attack => "attack",
            State::Dead => "dead",
        }
    }
}

/// A combat target sphere — what the rest of the game's combat code
/// (raycasts, splash checks) actually needs to know about an enemy,
/// same split the C engine makes between `ex_enemy_t` (AI's own
/// state) and `ex_target_t` (what combat sees).
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub center: Vec3,
    pub radius: f32,
    pub id: i32,
    pub alive: bool,
}

/// A minimal enemy agent: position, HP, and AI state, fully testable
/// without a renderer or a world to stand in.
#[derive(Clone, Debug)]
pub struct Enemy {
    pub id: i32,
    pub kind: EnemyKind,
    pub element: Element,
    pub position: Vec3,
    pub radius: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub state: State,

    pub move_speed: f32,
    /// won't notice the player beyond this
    pub sight_range: f32,
    /// switches to Attack inside this
    pub attack_range: f32,
    pub fire_cooldown: f32,
    pub fire_timer: f32,
    /// set true on the tick a shot should be spawned
    pub wants_to_fire: bool,

    /// Destiny "owned space": which line this enemy holds. 0 front
    /// (pushes close), 1 middle (holds mid), 2 back (anchors/snipers).
    pub zone: i32,
    /// preferred distance from the player to hold at; <= 0 means no hold
    pub standoff: f32,

    /// Corrupted infection stage: 0 (n/a) or 1..3, escalating over time
    pub stage: i32,
    pub stage_timer: f32,
    /// stage 3 absorbs metal and grows -- scales radius/hp/damage
    pub grow: f32,
    /// does melee contact transmit corruption to the player
    pub transmits: bool,

    /// apex boss: 1..3, gated by remaining HP
    pub phase: i32,
    pub slam_timer: f32,
    pub summon_timer: f32,
    pub wants_slam: bool,
    pub wants_summon: bool,

    /// leads a squad
    pub is_commander: bool,
    /// leader id this troop answers to, -1 = independent
    pub squad: i32,
    /// >0: leaderless panic countdown, breaks zone discipline. Driven
    /// from OUTSIDE this module (see the module doc) -- update() only
    /// reads it.
    pub scatter_t: f32,
    /// true after scattering with no commander left: wider, shyer
    pub demoralized: bool,
}

impl Enemy {
    /// Spawn a ranged Grunt at a position — the default kind, same as
    /// the C engine's `ex_enemy_new`.
    pub fn new(id: i32, pos: Vec3, hp: f32) -> Enemy {
        Enemy::new_kind(id, pos, hp, EnemyKind::Grunt, Element::None)
    }

    pub fn new_kind(id: i32, pos: Vec3, hp: f32, kind: EnemyKind, element: Element) -> Enemy {
        let mut e = Enemy {
            id,
            kind,
            element,
            position: pos,
            radius: 0.9,
            hp,
            max_hp: hp,
            state: State::Idle,
            move_speed: 0.0,
            sight_range: 0.0,
            attack_range: 0.0,
            fire_cooldown: 0.0,
            fire_timer: 0.0,
            wants_to_fire: false,
            zone: 0,
            standoff: 0.0,
            stage: 0,
            stage_timer: 0.0,
            grow: 1.0,
            transmits: false,
            phase: 0,
            slam_timer: 0.0,
            summon_timer: 0.0,
            wants_slam: false,
            wants_summon: false,
            is_commander: false,
            squad: -1,
            scatter_t: 0.0,
            demoralized: false,
        };
        match kind {
            EnemyKind::Corrupted => {
                // horror: fast, relentless, melee-only, notices you from
                // anywhere (spark resonance -- can't be hidden from).
                // Begins at Stage 1 wrong-geometry and escalates through
                // Stage 2/3 over time, growing as it absorbs metal.
                e.radius = 1.1;
                e.move_speed = 6.5;
                e.sight_range = 1e9;
                e.attack_range = 2.2; // melee lunge
                e.fire_cooldown = 0.8;
                e.stage = 1;
                e.grow = 1.0;
                e.transmits = true;
            }
            EnemyKind::Hijack => {
                // swarm unit: fast, fragile, erratic, never flees, closes
                // with short weak shots. Deadly only in numbers.
                e.radius = 0.8;
                e.hp = 45.0;
                e.max_hp = 45.0;
                e.move_speed = 8.5;
                e.sight_range = 130.0;
                e.attack_range = 70.0; // erratic ranged pot-shots while closing
                e.fire_cooldown = 0.5;
            }
            EnemyKind::Apex => {
                // the apex Corrupted: largest, weeks of absorbed
                // infrastructure, phase-gated -- reconfigures as it takes
                // damage, adding a ground-slam wave then summoning
                // Hijack scouts.
                e.radius = 3.0;
                e.hp = if hp > 0.0 { hp } else { 1600.0 };
                e.max_hp = e.hp;
                e.move_speed = 3.6;
                e.sight_range = 1e9;
                e.attack_range = 4.5;
                e.fire_cooldown = 1.2;
                e.stage = 3; // fully corrupted from the start
                e.grow = 1.0;
                e.transmits = true;
                e.phase = 1;
                e.slam_timer = 6.0;
                e.summon_timer = 12.0;
            }
            EnemyKind::Grunt => {
                // standard ranged trooper: the default enemy. Long sight
                // + range so it engages from real distance instead of
                // being a free target up close.
                e.move_speed = 4.0;
                e.sight_range = 140.0;
                e.attack_range = 90.0;
                e.fire_cooldown = 1.1;
            }
        }
        e
    }

    /// A combat Target sphere for this enemy.
    pub fn as_target(&self) -> Target {
        Target { center: self.position, radius: self.radius, id: self.id, alive: self.state != State::Dead }
    }

    /// Apply damage. Returns whether this killed it.
    pub fn damage(&mut self, amount: f32) -> bool {
        if self.state == State::Dead {
            return false;
        }
        self.hp -= amount;
        if self.hp <= 0.0 {
            self.hp = 0.0;
            self.state = State::Dead;
            return true;
        }
        // taking fire from an unseen source still pulls it into the fight
        if self.state == State::Idle {
            self.state = State::Chase;
        }
        false
    }

    /// Advance the AI one tick. `has_los` is whatever the caller's own
    /// line-of-sight check against the real world decided (ignored for
    /// spark-hunters, which see through everything); `player_cloaked`
    /// is the C engine's player-side cloak ability, which blinds
    /// non-spark-hunters entirely.
    pub fn update(&mut self, has_los: bool, player_cloaked: bool, player_pos: Vec3, dt: f32) {
        self.wants_to_fire = false;
        if self.state == State::Dead {
            return;
        }

        if self.fire_timer > 0.0 {
            self.fire_timer -= dt;
        }

        // Corrupted escalate: Stage 1 -> 2 -> 3 over time. Each stage is
        // faster and hits harder; Stage 3 keeps growing.
        if self.kind == EnemyKind::Corrupted && self.stage < 3 {
            self.stage_timer += dt;
            let threshold = if self.stage == 1 { 10.0 } else { 16.0 };
            if self.stage_timer >= threshold {
                self.stage += 1;
                self.stage_timer = 0.0;
                self.move_speed *= 1.15;
                self.radius *= 1.12;
            }
        }
        if self.kind == EnemyKind::Corrupted && self.stage >= 3 {
            self.grow = (self.grow + dt * 0.03).min(2.2);
        }

        // apex boss: phase by remaining HP, time slam/summon signals
        self.wants_slam = false;
        self.wants_summon = false;
        if self.kind == EnemyKind::Apex {
            self.grow = (self.grow + dt * 0.02).min(1.8);
            let frac = self.hp / self.max_hp;
            let new_phase = if frac > 0.66 {
                1
            } else if frac > 0.33 {
                2
            } else {
                3
            };
            if new_phase != self.phase {
                self.phase = new_phase;
                self.move_speed += 0.8;
                self.slam_timer = 3.0;
            }
            if self.phase >= 2 {
                self.slam_timer -= dt;
                if self.slam_timer <= 0.0 {
                    self.wants_slam = true;
                    self.slam_timer = 5.0;
                }
            }
            if self.phase >= 3 {
                self.summon_timer -= dt;
                if self.summon_timer <= 0.0 {
                    self.wants_summon = true;
                    self.summon_timer = 9.0;
                }
            }
        }

        let to_player = player_pos - self.position;
        let dist = (to_player.x * to_player.x + to_player.z * to_player.z + to_player.y * to_player.y).sqrt();
        // Corrupted & the Apex hunt by spark resonance -- always know
        // where you are, ignore line of sight, not fooled by cloak.
        // Grunts and Hijacks rely on sight, so an active cloak makes
        // them lose track of you entirely.
        let spark_hunter = matches!(self.kind, EnemyKind::Corrupted | EnemyKind::Apex);
        let mut has_los = if spark_hunter { true } else { has_los };
        if player_cloaked && !spark_hunter {
            has_los = false;
            if self.state == State::Chase || self.state == State::Attack {
                self.state = State::Idle;
            }
        }

        match self.state {
            State::Idle => {
                if dist <= self.sight_range && has_los {
                    self.state = State::Chase;
                }
            }
            State::Chase => {
                // COMMANDER DOWN -> SCATTER: panics and flees erratically,
                // abandoning its zone. No firing while panicking.
                if self.scatter_t > 0.0 {
                    if dist > 1e-3 {
                        let inv = 1.0 / dist;
                        let (mut ax, mut az) = (-to_player.x * inv, -to_player.z * inv); // away from player
                        self.stage_timer += dt;
                        let w = (self.stage_timer * 5.0 + self.id as f32).sin() * 0.9; // erratic veer
                        let (px, pz) = (-az, ax);
                        ax += px * w;
                        az += pz * w;
                        let l = (ax * ax + az * az).sqrt();
                        if l > 1e-3 {
                            ax /= l;
                            az /= l;
                        }
                        self.position.x += ax * self.move_speed * 1.2 * dt; // flee a touch faster
                        self.position.z += az * self.move_speed * 1.2 * dt;
                    }
                    return;
                }
                // Destiny owned-space: hold at the zone's standoff
                // distance rather than crowding the player. DEMORALIZED
                // squads hold much wider + shyer.
                let mut hold = self.standoff;
                if self.demoralized {
                    hold = (if hold > 0.0 { hold } else { 8.0 }) + 18.0;
                }
                let at_hold = hold <= 0.0 || dist <= hold + 1.0;
                if dist <= self.attack_range && has_los && at_hold {
                    self.state = State::Attack;
                    return;
                }
                // walk toward the player on XZ, stop advancing at standoff
                if dist > 1e-3 && (hold <= 0.0 || dist > hold) {
                    let inv = 1.0 / dist;
                    let (mut mx, mut mz) = (to_player.x * inv, to_player.z * inv);
                    // Hijacks move erratically -- a wobbling strafe layered
                    // on the approach.
                    if self.kind == EnemyKind::Hijack {
                        self.stage_timer += dt; // reuse timer as a wobble phase
                        let w = (self.stage_timer * 6.0 + self.id as f32).sin() * 0.7;
                        let (px, pz) = (-mz, mx); // perpendicular
                        mx += px * w;
                        mz += pz * w;
                        let l = (mx * mx + mz * mz).sqrt();
                        if l > 1e-3 {
                            mx /= l;
                            mz /= l;
                        }
                    }
                    self.position.x += mx * self.move_speed * dt;
                    self.position.z += mz * self.move_speed * dt;
                }
            }
            State::Attack => {
                if dist > self.attack_range || !has_los {
                    self.state = State::Chase;
                    return;
                }
                if self.fire_timer <= 0.0 {
                    self.wants_to_fire = true;
                    self.fire_timer = self.fire_cooldown;
                }
            }
            State::Dead => {}
        }
    }

    pub fn stage_name(&self) -> &'static str {
        match self.stage {
            1 => "wrong-geometry",
            2 => "mid-corruption",
            3 => "full-corruption",
            _ => "",
        }
    }

    pub fn melee_damage(&self) -> f32 {
        let base = 15.0;
        if self.kind == EnemyKind::Apex {
            return 34.0 * self.grow; // heavy boss blows
        }
        if self.kind != EnemyKind::Corrupted {
            return base;
        }
        base * (0.7 + 0.5 * self.stage as f32) * self.grow // stage + growth scaling
    }

    pub fn visual_radius(&self) -> f32 {
        if self.kind == EnemyKind::Apex {
            return self.radius * self.grow;
        }
        self.radius * (if self.kind == EnemyKind::Corrupted && self.stage >= 3 { self.grow } else { 1.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported directly from games/blackstar/tests/test_enemy.c, case for
    // case. `tick` mirrors the reference's own helper: run the AI a few
    // ticks with no LOS-blocking geometry (has_los = true, no cloak).
    fn tick(e: &mut Enemy, player: Vec3, n: i32) {
        for _ in 0..n {
            e.update(true, false, player, 1.0 / 60.0);
        }
    }

    #[test]
    fn engagement_ranges_reach_real_distance() {
        // a grunt must notice and be able to attack from well beyond a
        // tiny point-blank bubble -- the C engine's own regression guard
        // for a real fixed bug (was 22u, poke-from-outside range)
        let mut g = Enemy::new_kind(1, Vec3::ZERO, 100.0, EnemyKind::Grunt, Element::Fire);
        assert!(g.sight_range >= 100.0);
        assert!(g.attack_range >= 60.0);

        let player = Vec3::new(60.0, 0.0, 0.0);
        g.state = State::Idle;
        tick(&mut g, player, 10);
        assert!(g.state == State::Chase || g.state == State::Attack);
        tick(&mut g, player, 120);
        assert!(g.state == State::Attack || g.wants_to_fire);

        let h = Enemy::new_kind(2, Vec3::ZERO, 45.0, EnemyKind::Hijack, Element::Fire);
        assert!(h.attack_range >= 50.0, "hijack attack range regressed: {}", h.attack_range);
    }

    #[test]
    fn stays_idle_beyond_sight_range() {
        let mut g = Enemy::new_kind(3, Vec3::ZERO, 100.0, EnemyKind::Grunt, Element::Fire);
        let far = Vec3::new(5000.0, 0.0, 0.0);
        tick(&mut g, far, 30);
        assert_eq!(g.state, State::Idle);
    }

    #[test]
    fn idle_chase_attack_dead_lifecycle() {
        let mut g = Enemy::new_kind(4, Vec3::new(50.0, 0.0, 0.0), 100.0, EnemyKind::Grunt, Element::Fire);
        assert_eq!(g.state, State::Idle);
        let player = Vec3::ZERO;
        tick(&mut g, player, 5);
        assert_ne!(g.state, State::Idle, "did not notice the player");

        let died = g.damage(9999.0);
        assert!(died && g.state == State::Dead);

        // a dead enemy ignores updates: stays dead, doesn't move
        let px = g.position.x;
        tick(&mut g, player, 10);
        assert_eq!(g.state, State::Dead);
        assert!((g.position.x - px).abs() < 1e-4);
    }

    #[test]
    fn corrupted_and_apex_hunt_from_anywhere() {
        let mut c = Enemy::new_kind(5, Vec3::ZERO, 90.0, EnemyKind::Corrupted, Element::Vine);
        assert!(c.sight_range > 1e6);
        assert!(c.attack_range < 10.0, "corrupted should be melee-only: {}", c.attack_range);

        let player = Vec3::new(400.0, 0.0, 0.0);
        tick(&mut c, player, 5);
        assert_ne!(c.state, State::Idle);
        let d0 = c.position.x;
        tick(&mut c, player, 60);
        assert!(c.position.x > d0, "did not advance toward the player");
    }

    #[test]
    fn player_cloak_blinds_sight_hunters_not_spark_hunters() {
        // grunt: sight-based, should lose the player under cloak
        let mut g = Enemy::new_kind(6, Vec3::ZERO, 100.0, EnemyKind::Grunt, Element::None);
        g.state = State::Chase;
        g.update(true, true, Vec3::new(10.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(g.state, State::Idle, "cloak should blind a sight-based hunter");

        // apex: spark resonance, cloak should not matter at all
        let mut a = Enemy::new_kind(7, Vec3::ZERO, 1600.0, EnemyKind::Apex, Element::None);
        a.state = State::Chase;
        let before = a.state;
        a.update(false, true, Vec3::new(400.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(a.state, before, "cloak should not affect a spark hunter's tracking");
    }

    #[test]
    fn apex_phases_and_signals_gate_on_remaining_hp() {
        let mut a = Enemy::new_kind(8, Vec3::ZERO, 1000.0, EnemyKind::Apex, Element::None);
        assert_eq!(a.phase, 1);
        a.hp = 500.0; // 50%, should trip phase 2
        a.update(true, false, Vec3::new(3.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(a.phase, 2);
        a.hp = 200.0; // 20%, should trip phase 3
        a.update(true, false, Vec3::new(3.0, 0.0, 0.0), 1.0 / 60.0);
        assert_eq!(a.phase, 3);
    }
}
