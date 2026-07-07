//! player.rs — the body the camera is bolted to: WASD movement with wall-sliding collision,
//! gravity + jumping, a four-slot weapon loadout, recoil/view-punch, head-bob, and falling into a
//! pit (a `Cell::Void`) costs health and respawns you. Physics is deliberately arcade-simple in the
//! Quake/Doom tradition: instant-ish acceleration, axis-separated collision so you slide along walls
//! instead of sticking, and a circle (not a capsule) for the body footprint.

use crate::camera::FpsCam;
use crate::level::{Cell, Level};
use crate::math::{clampf, Vec2, Vec3};
use crate::weapons::{WeaponKind, WeaponState};
use crate::loadout::Loadout;

/// One frame of intent, filled by the app from the keyboard/mouse and handed to [`Player::update`].
#[derive(Clone, Copy, Default)]
pub struct Input {
    /// Forward axis, +1 = W, -1 = S.
    pub fwd: f32,
    /// Strafe axis, +1 = D, -1 = A.
    pub strafe: f32,
    /// Mouse-look deltas already scaled to radians.
    pub dyaw: f32,
    pub dpitch: f32,
    pub jump: bool,
    /// Trigger held this frame (the game decides if the weapon actually fires).
    pub fire: bool,
    /// Aim-down-sights held (right mouse).
    pub ads: bool,
    /// Sprint held (left shift).
    pub sprint: bool,
    /// Crouch held (toggles a lower, slower, steadier stance).
    pub crouch: bool,
    /// Crawl/prone held — drops fully prone: lowest profile, slowest move, steadiest aim. Bound to shift by
    /// default (sprint moves to a different binding at the app layer).
    pub crawl: bool,
    /// Dive pressed this frame (a lunge when sprinting).
    pub dive: bool,
    /// Reload pressed this frame (R).
    pub reload: bool,
    /// Deploy scorestreaks pressed this frame (Q = UAV, F = strike, C = sentry).
    pub deploy_uav: bool,
    pub deploy_strike: bool,
    pub deploy_sentry: bool,
    /// Throw a frag grenade pressed this frame.
    pub throw_frag: bool,
    /// Activate the class super ability pressed this frame.
    pub activate_super: bool,
    /// A pressed weapon slot (0..4), if any.
    pub want_slot: Option<usize>,
    /// Free-fly (noclip) movement — used by the map editor's flyaround camera.
    pub fly: bool,
}

pub struct Player {
    /// Feet position (the floor point the body stands on).
    pub pos: Vec3,
    /// Velocity; xz is walking, y is gravity/jump.
    pub vel: Vec3,
    pub cam: FpsCam,
    pub on_ground: bool,
    pub health: f32,
    pub armor: f32,
    pub weapons: [WeaponState; 4],
    pub current: usize,
    pub radius: f32,
    pub eye_height: f32,
    /// Head-bob accumulator and its current vertical offset.
    pub bob_t: f32,
    pub bob_y: f32,
    /// Recoil pitch that's been pushed onto the view and is decaying back.
    pub recoil: f32,
    /// Horizontal recoil (yaw sway) pushed onto the view, decaying back.
    pub recoil_yaw: f32,
    pub alive: bool,
    /// Counts down while dead, then we respawn.
    pub dead_timer: f32,
    pub spawn: Vec3,
    /// Set the frame we took fall damage, so the app can flash the screen.
    pub hurt_flash: f32,
    /// Seconds since we last took damage — drives Black-Ops-style health regeneration.
    pub time_since_hit: f32,
    /// Aim-down-sights amount, 0 (hip) .. 1 (fully aimed); lerps toward the held state.
    pub ads_t: f32,
    /// True while actually sprinting this frame (used to gate firing/ADS and tilt the viewmodel).
    pub sprinting: bool,
    /// Loadout-derived modifiers (perks).
    pub loadout: Loadout,
    pub move_mul: f32,
    pub recoil_mul: f32,
    /// ADS-speed multiplier (Marksman perk aims faster) and health-regen-rate multiplier (Regen perk).
    pub ads_mul: f32,
    pub regen_mul: f32,
    /// Outgoing-damage multiplier (from an equipped legendary's perks).
    pub damage_mul: f32,
    pub jetpack: bool,
    /// Jetpack charge 0..1 (a "mana"/fuel bar). Thrust drains it; it recharges on the ground. Empty = no lift
    /// until it recovers — so the jetpack is a burst-mobility tool, not free flight. Only meaningful when
    /// `jetpack` is set. `jet_active` tracks whether thrust fired this frame (for the ion-blast VFX).
    pub jet_fuel: f32,
    pub jet_active: bool,
    /// Crouch amount 0..1 (lerps), a dive lunge timer, a landing-impact dip, and a procedural step phase.
    pub crouch_t: f32,
    /// Prone/crawl amount 0..1 (lerps) — fully prone flattens the body, slows the crawl and steadies aim.
    pub prone_t: f32,
    pub dive_t: f32,
    pub dive_yaw: f32,
    /// Slide state: `slide_t` counts down while sliding (a crouch-sprint burst), `slide_yaw` locks the slide
    /// heading, and `slide_cd` is a small cooldown so you can't chain slides infinitely. A slide gives a
    /// forward speed boost that decays with friction while you stay low, then eases back to a crouch/stand.
    pub slide_t: f32,
    pub slide_yaw: f32,
    pub slide_cd: f32,
    pub land_dip: f32,
    pub step_phase: f32,
    was_air: bool,
    /// The chosen class, the health cap it sets, and a per-frame gravity multiplier (a gravity grenade or
    /// slam can drop this toward zero for floaty movement).
    pub class: crate::class::ClassKind,
    pub max_health: f32,
    pub gravity_scale: f32,
    /// Active vault/mantle: time remaining (0 = not mantling) and the interpolation endpoints. While this runs
    /// the body is pulled smoothly from `mantle_from` up onto `mantle_to`, ignoring gravity and input.
    pub mantle_t: f32,
    mantle_from: Vec3,
    mantle_to: Vec3,
    /// How deep the feet are under the nearest water surface this frame (0 = dry, ~eye_height = fully
    /// submerged). Drives buoyancy, swim drag, the swim/tread state, and the underwater screen tint.
    pub submersion: f32,
    /// True while the head (eye) is below the water surface — swaps to swim controls and muffles audio/vision.
    pub underwater: bool,
}

const GRAVITY: f32 = 24.0;
const JUMP_SPEED: f32 = 8.0;
const WALK_SPEED: f32 = 7.2;
// Sprint toned down from a 47% boost to ~22% — still a meaningful rush but far less twitchy, so fights don't
// devolve into everyone sprint-strafing at high speed. Applies in every mode.
const SPRINT_SPEED: f32 = 8.8;
const ADS_SPEED: f32 = 5.6;
// Jetpack fuel: full charge gives ~1.4s of continuous thrust, and it refills over ~2.5s on the ground. Tuned
// so the jetpack is a burst-repositioning tool (a boosted double-jump, a ledge grab, a dodge) rather than
// sustained flight — the balance the hunt-mode complaint was about.
const JET_BURN_TIME: f32 = 1.4;
const JET_RECHARGE_TIME: f32 = 2.5;
const ACCEL: f32 = 60.0;
const FRICTION: f32 = 9.0;
const KILL_PLANE_DROP: f32 = 14.0;
/// How long a vault/climb takes to play out (seconds).
const MANTLE_TIME: f32 = 0.32;
/// Health regen: after this long without taking damage, health refills at REGEN_RATE/sec.
const REGEN_DELAY: f32 = 3.2;
const REGEN_RATE: f32 = 24.0;

impl Player {
    pub fn new(level: &Level) -> Self {
        let spawn = level.spawn(0);
        let mut cam = FpsCam::default();
        cam.pos = spawn + Vec3::new(0.0, 1.6, 0.0);
        // Face roughly toward the centre of the map so you're not staring at a wall on spawn.
        cam.yaw = 0.7;
        let mut s = Self {
            pos: spawn,
            vel: Vec3::ZERO,
            cam,
            on_ground: true,
            health: 100.0,
            armor: 0.0,
            weapons: [
                WeaponState::new(WeaponKind::Ar),
                WeaponState::new(WeaponKind::Pistol),
                WeaponState::new(WeaponKind::Ar),
                WeaponState::new(WeaponKind::Pistol),
            ],
            current: 0,
            radius: 0.34,
            eye_height: 1.6,
            bob_t: 0.0,
            bob_y: 0.0,
            recoil: 0.0,
            recoil_yaw: 0.0,
            alive: true,
            dead_timer: 0.0,
            spawn,
            hurt_flash: 0.0,
            time_since_hit: 999.0,
            ads_t: 0.0,
            sprinting: false,
            loadout: Loadout::default(),
            move_mul: 1.0,
            ads_mul: 1.0,
            regen_mul: 1.0,
            recoil_mul: 1.0,
            damage_mul: 1.0,
            jetpack: false,
            jet_fuel: 1.0,
            jet_active: false,
            crouch_t: 0.0,
            prone_t: 0.0,
            dive_t: 0.0,
            dive_yaw: 0.0,
            slide_t: 0.0,
            slide_yaw: 0.0,
            slide_cd: 0.0,
            land_dip: 0.0,
            step_phase: 0.0,
            was_air: false,
            class: crate::class::ClassKind::Trooper,
            max_health: 100.0,
            gravity_scale: 1.0,
            mantle_t: 0.0,
            mantle_from: Vec3::ZERO,
            mantle_to: Vec3::ZERO,
            submersion: 0.0,
            underwater: false,
        };
        s.apply_loadout(Loadout::default());
        s
    }

    /// Equip a loadout: set the primary/secondary weapons (slots 0/1, duplicated into 2/3 so the existing
    /// four-slot cycle stays valid) and fold the perks into reload/move/recoil/jetpack and magazine size.
    pub fn apply_loadout(&mut self, l: Loadout) {
        self.loadout = l;
        // A legendary primary swaps in its base archetype for slots 0/2.
        let prim = l.primary_kind();
        let kinds = [prim, l.secondary, prim, l.secondary];
        let reload_mul = l.reload_mul();
        let mag_mul = l.mag_mul();
        for (i, w) in self.weapons.iter_mut().enumerate() {
            *w = WeaponState::new(kinds[i]);
            w.reload_mul = reload_mul;
            if mag_mul > 1.0 {
                w.mag = (w.mag as f32 * mag_mul) as u32;
                w.reserve = (w.reserve as f32 * mag_mul) as u32;
            }
        }
        self.current = 0;
        self.move_mul = l.move_mul();
        self.recoil_mul = l.recoil_mul();
        self.ads_mul = l.ads_mul();
        self.regen_mul = l.regen_mul();
        self.damage_mul = l.damage_mul();
        self.jetpack = l.jetpack();
    }

    #[inline]
    pub fn weapon(&self) -> &WeaponState {
        &self.weapons[self.current]
    }
    #[inline]
    pub fn weapon_mut(&mut self) -> &mut WeaponState {
        &mut self.weapons[self.current]
    }

    /// World position of the eye (camera) this frame, bob and all.
    #[inline]
    pub fn eye(&self) -> Vec3 {
        // Crouch lowers the head; a dive drops it lower still; a landing dips briefly; prone drops it near the
        // ground for the lowest possible profile.
        let dive_drop = (self.dive_t * 2.0).min(1.0) * 0.5;
        let lower = self.crouch_t * 0.55 + self.prone_t * 0.85 + dive_drop + self.land_dip;
        let mut e = self.pos + Vec3::new(0.0, self.eye_height + self.bob_y - lower, 0.0);
        // Vault feel: while mantling, the head doesn't rise linearly with the body. It lags low as you haul up
        // (a heave), then surges over the lip. A little lateral bob sells the reach of the leading hand.
        if self.mantle_t > 0.0 {
            let k = (1.0 - (self.mantle_t / MANTLE_TIME)).clamp(0.0, 1.0);
            // dip low through the first half of the climb, then overshoot slightly as you clear the top
            let heave = if k < 0.5 {
                -0.14 * (k / 0.5) // sink relative to the body while pulling up
            } else {
                let u = (k - 0.5) / 0.5;
                -0.14 + 0.20 * u * (2.0 - u) // rise and overshoot a touch over the lip
            };
            e.y += heave;
            // lateral heave toward the leading hand (the mantle's forward-right reach), easing out at the top
            let side = (k * std::f32::consts::PI).sin() * 0.05;
            let right = self.cam.right();
            e.x += right.x * side;
            e.z += right.z * side;
        }
        e
    }

    /// Extra view rotation (pitch, roll) in radians to layer on top of the look angles for movement feel. Right
    /// now this is the vault: a forward pitch-down as you heave up and a slight roll toward the reaching hand,
    /// both easing out as you clear the ledge. The app adds these to the render camera so the climb reads as a
    /// physical effort rather than a rigid slide upward.
    pub fn view_lean(&self) -> (f32, f32) {
        if self.mantle_t > 0.0 {
            let k = (1.0 - (self.mantle_t / MANTLE_TIME)).clamp(0.0, 1.0);
            let env = (k * std::f32::consts::PI).sin(); // 0 → 1 → 0 across the climb
            let pitch = -env * 0.13; // look down toward the ledge you're pulling onto
            let roll = env * 0.06; // tilt toward the leading hand
            (pitch, roll)
        } else {
            (0.0, 0.0)
        }
    }

    /// Where a muzzle flash / bullet originates: a bit in front of the eye so it clears the gun.
    #[inline]
    pub fn muzzle(&self) -> Vec3 {
        self.eye() + self.cam.forward() * 0.6 + self.cam.right() * 0.18 - Vec3::new(0.0, 0.12, 0.0)
    }

    /// Add a recoil kick (radians): `v` pitches the view up, `h` sways it horizontally. Both decay.
    pub fn add_recoil(&mut self, v: f32, h: f32) {
        self.recoil += v;
        self.recoil_yaw += h;
    }

    pub fn take_damage(&mut self, dmg: f32) {
        if !self.alive {
            return;
        }
        // Armor soaks a third of incoming damage until it runs out.
        let soak = self.armor.min(dmg * 0.5);
        self.armor -= soak;
        self.health -= dmg - soak;
        self.hurt_flash = 1.0;
        self.time_since_hit = 0.0;
        if self.health <= 0.0 {
            self.health = 0.0;
            self.alive = false;
            self.dead_timer = 2.2;
        }
    }

    /// Restore health up to 100. Returns true if any was actually given (so pickups can be greedy).
    pub fn heal(&mut self, amt: f32) -> bool {
        if self.health >= self.max_health {
            return false;
        }
        self.health = (self.health + amt).min(self.max_health);
        true
    }

    /// Apply a class pick: set the health cap and refill to it, and remember the class for grenade/super.
    pub fn set_class(&mut self, class: crate::class::ClassKind) {
        self.class = class;
        self.max_health = class.def().max_health;
        self.health = self.max_health;
    }

    /// Add armor up to 100.
    pub fn add_armor(&mut self, amt: f32) -> bool {
        if self.armor >= 100.0 {
            return false;
        }
        self.armor = (self.armor + amt).min(100.0);
        true
    }

    /// Resupply: add a magazine's worth of rounds to every weapon's reserve. Returns true if anything fit.
    pub fn add_ammo(&mut self) -> bool {
        let mut gained = false;
        for w in &mut self.weapons {
            if w.add_reserve(w.kind.def().mag_size) {
                gained = true;
            }
        }
        gained
    }

    fn respawn(&mut self) {
        self.pos = self.spawn;
        self.vel = Vec3::ZERO;
        self.health = 100.0;
        self.armor = 0.0;
        self.alive = true;
        self.on_ground = true;
        self.apply_loadout(self.loadout);
        self.time_since_hit = 999.0;
        self.ads_t = 0.0;
        self.sprinting = false;
    }

    pub fn switch(&mut self, slot: usize) {
        if slot < self.weapons.len() {
            self.current = slot;
        }
    }

    /// Advance the body one fixed step.
    pub fn update(&mut self, level: &Level, input: &Input, dt: f32) {
        // Decay timers/feedback regardless of alive state.
        self.recoil = approach(self.recoil, 0.0, 6.0 * dt);
        self.recoil_yaw = approach(self.recoil_yaw, 0.0, 7.0 * dt);
        self.hurt_flash = approach(self.hurt_flash, 0.0, 2.5 * dt);
        for w in &mut self.weapons {
            w.tick(dt);
        }

        if !self.alive {
            self.dead_timer -= dt;
            // Let the body keep falling/settling while dead for a beat, then respawn.
            self.vel.y -= GRAVITY * self.gravity_scale * dt;
            self.pos.y += self.vel.y * dt;
            if self.dead_timer <= 0.0 {
                self.respawn();
            }
            self.sync_camera();
            return;
        }

        // ── Look (still works during a vault) ──────────────────────────────────
        self.cam.add_look(input.dyaw, input.dpitch);

        // ── Vault / climb in progress: pull the body onto the ledge, ignore gravity & input this frame ──
        if self.update_mantle(dt) {
            self.crouch_t = approach(self.crouch_t, 0.0, 7.0 * dt);
            self.sync_camera();
            return;
        }

        // ── Free-fly (map editor): noclip movement where you look; no gravity or collision. ──────────────
        if input.fly {
            let (right, _u, fwd) = self.cam.basis();
            let speed = if input.sprint { 34.0 } else { 13.0 };
            let mut v = fwd * input.fwd + right * input.strafe;
            if input.jump {
                v = v + Vec3::new(0.0, 1.0, 0.0);
            }
            if input.crouch {
                v = v + Vec3::new(0.0, -1.0, 0.0);
            }
            if v.len_sq() > 0.0001 {
                v = v.normalized() * speed;
            }
            self.pos = self.pos + v * dt;
            self.vel = Vec3::ZERO;
            self.on_ground = false;
            self.sync_camera();
            return;
        }

        if let Some(slot) = input.want_slot {
            self.switch(slot);
        }
        if input.reload {
            self.weapon_mut().start_reload();
        }

        // ── Sprint / ADS state ──────────────────────────────────────────────────
        let moving_fwd = input.fwd > 0.1;
        // Sprinting and aiming are mutually exclusive; sprint wins while you hold it and push forward.
        self.sprinting = input.sprint && moving_fwd && !input.ads && self.on_ground && self.crouch_t < 0.5 && self.prone_t < 0.3;
        let ads_target = if input.ads && !self.sprinting { 1.0 } else { 0.0 };
        // Marksman perk raises the ADS rate (ads_mul<1 → faster aim). Divide so a smaller mul = quicker.
        self.ads_t = approach(self.ads_t, ads_target, (ADS_SPEED / self.ads_mul) * dt);

        // ── Crouch / dive stance ────────────────────────────────────────────────
        self.dive_t = (self.dive_t - dt).max(0.0);
        self.slide_t = (self.slide_t - dt).max(0.0);
        self.slide_cd = (self.slide_cd - dt).max(0.0);
        self.land_dip = approach(self.land_dip, 0.0, 4.0 * dt);
        // Jetpack charge: recharge whenever grounded (and always bleed the per-frame active flag; the thrust
        // code re-sets it when it actually fires). On the ground fuel refills over JET_RECHARGE_TIME.
        self.jet_active = false;
        if self.on_ground && self.jet_fuel < 1.0 {
            self.jet_fuel = (self.jet_fuel + dt / JET_RECHARGE_TIME).min(1.0);
        }
        // Start a dive: a forward lunge out of a sprint (or any grounded run), once per landing. Toned down —
        // a shorter, lower hop so a dive repositions you rather than launching you across a room.
        if input.dive && self.dive_t <= 0.0 && self.on_ground && (self.sprinting || moving_fwd) {
            self.dive_t = 0.5;
            self.dive_yaw = self.cam.yaw;
            self.vel.y = 3.4; // gentler hop into the dive
            self.on_ground = false;
        }
        // ── Slide: crouch while sprinting to drop low and surge forward ─────────────────────────────────────
        // Pressing crouch during a grounded sprint kicks off a slide: the heading locks, you get a forward
        // speed burst, and you stay low (small hitbox) while it decays with friction. Great for sliding into
        // cover or under fire. A short cooldown stops infinite slide-chaining. Speed carries the boost.
        let cur_speed = (self.vel.x * self.vel.x + self.vel.z * self.vel.z).sqrt();
        if input.crouch && self.slide_t <= 0.0 && self.slide_cd <= 0.0 && self.on_ground && self.sprinting && cur_speed > WALK_SPEED * 0.8 {
            self.slide_t = 0.7;
            self.slide_cd = 1.1; // can't immediately re-slide
            self.slide_yaw = self.cam.yaw;
            // inject a forward burst along the current heading
            let sfwd = Vec3::new(self.slide_yaw.sin(), 0.0, -self.slide_yaw.cos());
            let boost = SPRINT_SPEED * 1.5;
            self.vel.x = sfwd.x * boost;
            self.vel.z = sfwd.z * boost;
        }
        // Crouch target: held crouch, forced low during the back half of a dive, or fully low during a slide.
        let crouch_target = if input.crouch || (self.dive_t > 0.0 && self.dive_t < 0.4) || self.slide_t > 0.0 { 1.0 } else { 0.0 };
        // A slide crouches faster than a normal duck — you snap low as you go down.
        let crouch_rate = if self.slide_t > 0.0 { 14.0 } else { 7.0 };
        self.crouch_t = approach(self.crouch_t, crouch_target, crouch_rate * dt);
        // Prone/crawl: hold crawl on the ground to go fully prone (not while sliding/diving). Rises/falls a bit
        // slower than crouch — going prone and getting back up is a deliberate commitment.
        let prone_target = if input.crawl && self.on_ground && self.slide_t <= 0.0 && self.dive_t <= 0.0 { 1.0 } else { 0.0 };
        self.prone_t = approach(self.prone_t, prone_target, 5.0 * dt);

        // ── Health regeneration (Black Ops style) ───────────────────────────────
        // Regen perk both shortens the delay before regen starts and speeds the refill.
        self.time_since_hit += dt;
        let delay = REGEN_DELAY / self.regen_mul;
        if self.time_since_hit >= delay && self.health < self.max_health {
            self.health = (self.health + REGEN_RATE * self.regen_mul * dt).min(self.max_health);
        }

        // ── Horizontal acceleration ────────────────────────────────────────────
        let fwd = self.cam.forward_flat();
        let right = Vec3::new(-fwd.z, 0.0, fwd.x); // right = forward × up (points to the player's right)
        let mut wish = fwd * input.fwd + right * input.strafe;
        if wish.len_sq() > 1.0 {
            wish = wish.normalized();
        }
        // Sprint is faster; ADS is slower and steadier. Lightweight perk scales all of it. Crouch slows you.
        // Prone slows you to a crawl (on top of crouch), so going flat is a trade of mobility for a low profile.
        let speed = if self.sprinting {
            SPRINT_SPEED
        } else {
            WALK_SPEED * (1.0 - 0.32 * self.ads_t)
        } * self.move_mul
            * (1.0 - 0.42 * self.crouch_t)
            * (1.0 - 0.62 * self.prone_t);
        let mut wish_vel = wish * speed;

        // A dive overrides control with a forward lunge along the dive heading — toned down from 1.7× to 1.35×
        // sprint so it repositions rather than rockets you across the map.
        if self.dive_t > 0.3 {
            let dfwd = Vec3::new(self.dive_yaw.sin(), 0.0, -self.dive_yaw.cos());
            wish_vel = dfwd * (SPRINT_SPEED * 1.35 * self.move_mul);
        }
        // A slide keeps the locked heading and lets the boosted speed bleed off with friction: you carry
        // momentum forward and low, with only slight steering, until the slide ends (or you release crouch).
        if self.slide_t > 0.0 {
            if !input.crouch && self.slide_t < 0.45 {
                // releasing crouch ends the slide early (pop back up)
                self.slide_t = 0.0;
            } else {
                let sfwd = Vec3::new(self.slide_yaw.sin(), 0.0, -self.slide_yaw.cos());
                // steer a little toward the aim, but mostly hold the slide line
                let steer = right * (input.strafe * 0.25);
                let dir = (sfwd + steer).normalized();
                // target speed decays across the slide (fast → crouch speed) so it eases out naturally
                let t = (self.slide_t / 0.7).clamp(0.0, 1.0);
                let sp = WALK_SPEED * 0.5 + (SPRINT_SPEED * 1.5 - WALK_SPEED * 0.5) * t;
                wish_vel = dir * (sp * self.move_mul);
            }
        }

        // Accelerate toward the wish velocity (snappy), apply friction when idle.
        let mut hvel = Vec2::new(self.vel.x, self.vel.z);
        let target = Vec2::new(wish_vel.x, wish_vel.z);
        if self.slide_t > 0.0 || self.dive_t > 0.3 {
            // during a slide/dive the momentum is authored directly — track the target hard so the boost and
            // its decay curve aren't smeared out by normal ground accel
            hvel = v2_approach(hvel, target, ACCEL * 2.5 * dt);
        } else if wish.len_sq() > 1e-4 {
            hvel = v2_approach(hvel, target, ACCEL * dt);
        } else {
            hvel = v2_approach(hvel, Vec2::new(0.0, 0.0), FRICTION * dt * WALK_SPEED);
        }
        self.vel.x = hvel.x;
        self.vel.z = hvel.y;

        // ── Water: buoyancy + swim ───────────────────────────────────────────────
        // Read the water surface under the feet; how far the feet sit below it is `submersion`. Deep enough
        // that the head is under → swim controls (buoyant float, jump becomes "swim up", strong drag). This
        // runs before gravity so it can override the fall.
        let surf = level.water_surface_at(self.pos.x, self.pos.z);
        let feet = self.pos.y;
        let head = self.pos.y + self.body_height();
        self.submersion = surf.map(|s| (s - feet).max(0.0)).unwrap_or(0.0);
        self.underwater = surf.map(|s| s >= head - 0.15).unwrap_or(false);
        let wading = self.submersion > 0.15;

        // ── Vertical: gravity + jump (+ jetpack thrust) ──────────────────────────
        // Jump doubles as the climb input: if you're pressed up against a ledge you can reach, vault it;
        // otherwise it's an ordinary jump (or a jetpack boost in the air).
        if input.jump && self.try_mantle(level) {
            self.sync_camera();
            return;
        }
        if self.submersion > 0.55 {
            // Genuinely in the water. Two regimes:
            //  • Shallow/surface water (a pool, a flooded street): buoyancy floats you up to the surface, so
            //    you bob and tread with your chest at the waterline.
            //  • DEEP water (an underwater map — the surface is many metres overhead): free 3D swimming. There's
            //    no surface to float to, so buoyancy goes near-neutral and you hold depth, moving up/down at
            //    will (jump = ascend, crouch = dive). This is what makes a fully-submerged map play in the
            //    Z-axis as its primary lane instead of yo-yoing you to a distant surface.
            let deep = self.submersion > 3.0;
            if deep {
                // near-neutral, faintly *positive* buoyancy: a body less dense than water drifts slowly up
                // toward the surface when idle (so a dropped/limp body floats rather than pinning to the bed),
                // while the swim inputs (±20) completely override this gentle ±1.2 term — hold dive to sink or
                // hold depth, tap up to ascend. This also removes the old dead-zone where an idle body between
                // the deep threshold and the shallow float regime would sink away from the surface forever.
                self.vel.y += 1.2 * dt; // faint positive buoyancy (drift up toward the surface when idle)
                if input.jump {
                    self.vel.y += 20.0 * dt; // swim up
                }
                if input.crouch {
                    self.vel.y -= 20.0 * dt; // dive down
                }
                // strong, isotropic drag — heavy, floaty movement in every direction
                let drag = (1.0 - 3.0 * dt).max(0.0);
                self.vel.x *= drag;
                self.vel.z *= drag;
                self.vel.y *= (1.0 - 3.0 * dt).max(0.0);
            } else {
                let target_depth = 0.55; // the body floats with feet ~this far under, chest at the surface
                let buoy = ((self.submersion - target_depth) * 9.0).clamp(-6.0, 9.0);
                self.vel.y += buoy * dt;
                if input.jump {
                    self.vel.y += 22.0 * dt; // kick to the surface / climb out
                }
                if input.crouch {
                    self.vel.y -= 18.0 * dt; // dive
                }
                let drag = (1.0 - 3.2 * dt).max(0.0);
                self.vel.x *= drag;
                self.vel.z *= drag;
                self.vel.y *= (1.0 - 2.4 * dt).max(0.0);
            }
            self.on_ground = false;
        } else {
            if self.on_ground && input.jump {
                self.vel.y = JUMP_SPEED;
                self.on_ground = false;
            } else if self.jetpack && input.jump && !self.on_ground && self.vel.y < 6.0 && self.jet_fuel > 0.0 {
                // Hold jump in the air for a thruster boost, capped so it's lift not flight — and now gated by
                // the charge bar: thrust drains fuel, and when it's empty you fall until it recharges. This is
                // the balance fix: burst mobility, not indefinite hover.
                self.vel.y += 34.0 * dt;
                self.jet_fuel = (self.jet_fuel - dt / JET_BURN_TIME).max(0.0);
                self.jet_active = true;
            }
            self.vel.y -= GRAVITY * self.gravity_scale * dt;
            // Shallow wading (feet-to-knee): a horizontal drag that scales with depth — the surge in Flood
            // Zone genuinely impedes you, and any shallow pool slows a crossing without stopping it.
            if wading {
                let d = (self.submersion / 1.2).clamp(0.0, 1.0);
                let drag = (1.0 - (1.5 + 3.0 * d) * dt).max(0.0);
                self.vel.x *= drag;
                self.vel.z *= drag;
            }
        }
        let _ = head;

        // ── Move with collision (slide along walls, axis-separated) ──────────────
        let delta = self.vel * dt;
        self.move_xz(level, delta.x, delta.z);
        self.pos.y += delta.y;

        // ── Ceiling: a rising body bonks the underside of an overhead slab (can't pass up through a floor) ──
        if self.vel.y > 0.0 {
            let head = self.pos.y + self.body_height();
            let ceil = level.ceiling_at(self.pos.x, self.pos.z, head - delta.y.max(0.0));
            if head > ceil {
                self.pos.y = ceil - self.body_height();
                self.vel.y = 0.0;
            }
        }

        // ── Floor / pit handling (follows the elevated terrain surface) ───────────
        let over_floor = self.floor_solid_under(level);
        if over_floor {
            let surface = level.support_y(self.pos.x, self.pos.z, self.pos.y);
            if self.pos.y <= surface {
                self.pos.y = surface;
                // Landing impact: dip the camera/viewmodel proportional to the fall speed.
                if self.was_air {
                    self.land_dip = (-self.vel.y * 0.045).clamp(0.0, 0.7);
                }
                self.vel.y = 0.0;
                self.on_ground = true;
            } else {
                self.on_ground = false;
            }
        } else {
            // Over a pit (Void) or the void outside the map: keep falling.
            self.on_ground = false;
            if self.pos.y < level.floor_y + level.min_h() - KILL_PLANE_DROP {
                // Fell out of the world: hurt and respawn.
                self.take_damage(34.0);
                if self.alive {
                    self.pos = self.spawn;
                    self.vel = Vec3::ZERO;
                    self.on_ground = true;
                }
            }
        }

        // ── Head-bob: drive a small vertical sine from horizontal speed ──────────
        let speed = (self.vel.x * self.vel.x + self.vel.z * self.vel.z).sqrt();
        if self.on_ground && speed > 0.5 {
            self.bob_t += dt * speed * 1.1;
            self.bob_y = (self.bob_t * 2.0).sin() * 0.055 * (1.0 - 0.5 * self.crouch_t);
            self.step_phase += dt * speed * 1.1;
        } else {
            self.bob_y = approach(self.bob_y, 0.0, 4.0 * dt);
        }
        self.was_air = !self.on_ground;

        self.sync_camera();
    }

    /// Push the camera (eye) to follow the body, applying bob and recoil pitch.
    fn sync_camera(&mut self) {
        self.cam.pos = self.eye();
        // Recoil is a transient pitch-up that doesn't permanently change the stored pitch.
        // We bake it in here and the stored pitch keeps its true value.
        // (add_look already clamped pitch; recoil sits on top for rendering.)
    }

    /// The camera the renderer should use this frame: body camera plus recoil punch and procedural motion
    /// (footstep sway + a forward pitch during a dive).
    pub fn view_cam(&self) -> FpsCam {
        let mut c = self.cam;
        // Footstep sway: a small figure-eight on the look angles, scaled down while aiming.
        let sway = 1.0 - 0.8 * self.ads_t;
        let yaw_sway = (self.step_phase).sin() * 0.006 * sway;
        let pitch_sway = (self.step_phase * 2.0).sin() * 0.004 * sway;
        let dive_pitch = (self.dive_t * 2.0).min(1.0) * 0.22; // glance downward through the dive
        let (vault_pitch, _vault_roll) = self.view_lean(); // look down toward the ledge while mantling
        c.pitch = clampf(c.pitch + self.recoil + pitch_sway - dive_pitch + vault_pitch, -FpsCam::PITCH_LIMIT, FpsCam::PITCH_LIMIT);
        c.yaw += self.recoil_yaw + yaw_sway;
        c
    }

    fn floor_solid_under(&self, level: &Level) -> bool {
        level.supported_at(self.pos.xz(), self.pos.y)
    }

    /// Axis-separated horizontal move: try X, keep it only if the body stays clear; then try Z.
    /// This is what produces wall-sliding (blocked on one axis, free on the other).
    /// Carry the player along an external flow (an air current / conveyor), sliding along walls. Positional,
    /// so it doesn't fight the velocity friction — step into a jetstream and it sweeps you along.
    pub fn ride(&mut self, level: &Level, dx: f32, dz: f32) {
        if self.alive {
            self.move_xz(level, dx, dz);
        }
    }

    fn move_xz(&mut self, level: &Level, dx: f32, dz: f32) {
        let r = self.radius;
        let (feet, bh) = (self.pos.y, self.body_height());

        // Step-up assist: the max height of a ledge lip the body will automatically step over instead of
        // catching on. This is the standard FPS "step offset" — without it, the seam where an obstacle's top
        // face meets its side face grabs the body (float error puts your feet a hair below the top, so the
        // side re-blocks you), producing the classic stuck-on-the-edge stutter. With it, a low lip is just
        // walked over. Kept below MIN_RISE so it never overlaps the vault path (real climbs still vault).
        const STEP: f32 = 0.42;

        // Try the full XZ move; if an axis is blocked at the current foot height but *clear* a step higher
        // (i.e. the thing blocking us is a low lip we can rise onto), step the feet up and take the move.
        let nx = self.pos.x + dx;
        if !body_blocked(level, Vec2::new(nx, self.pos.z), r, feet, bh) {
            self.pos.x = nx;
        } else if self.on_ground && !body_blocked(level, Vec2::new(nx, self.pos.z), r, feet + STEP, bh - STEP) {
            // clear one step up → snap the feet up onto the lip and keep moving
            let lip = self.lip_top(level, Vec2::new(nx, self.pos.z), r, feet, STEP);
            self.pos.y = lip;
            self.pos.x = nx;
        } else {
            self.vel.x = 0.0;
        }

        let nz = self.pos.z + dz;
        if !body_blocked(level, Vec2::new(self.pos.x, nz), r, self.pos.y, bh) {
            self.pos.z = nz;
        } else if self.on_ground && !body_blocked(level, Vec2::new(self.pos.x, nz), r, self.pos.y + STEP, bh - STEP) {
            let lip = self.lip_top(level, Vec2::new(self.pos.x, nz), r, self.pos.y, STEP);
            self.pos.y = lip;
            self.pos.z = nz;
        } else {
            self.vel.z = 0.0;
        }
    }

    /// The height of the low ledge lip just ahead at `p` — the top of the tallest obstacle/step within `step`
    /// of the feet that the body is walking onto. Used by the step-up assist to snap the feet onto the lip.
    fn lip_top(&self, level: &Level, p: Vec2, r: f32, feet: f32, step: f32) -> f32 {
        let mut top = feet;
        // terrain / platform support just ahead
        let t = level.support_y(p.x, p.y, feet + step + 0.1);
        if t > top && t <= feet + step + 1e-3 {
            top = t;
        }
        // obstacle tops whose footprint the body overlaps and whose top is within a step
        for o in &level.obstacles {
            let cx = clampf(p.x, o.center.x - o.half.x, o.center.x + o.half.x);
            let cz = clampf(p.y, o.center.z - o.half.z, o.center.z + o.half.z);
            let ddx = p.x - cx;
            let ddz = p.y - cz;
            if ddx * ddx + ddz * ddz < r * r {
                let otop = o.center.y + o.half.y;
                if otop > top && otop <= feet + step + 1e-3 {
                    top = otop;
                }
            }
        }
        top
    }

    /// Standing collision height of the body, shrinking as the player crouches — so ducking lets you slip
    /// under an overhead beam or a low railing that would stop you upright. Going prone shrinks it further.
    pub fn body_height(&self) -> f32 {
        1.8 - self.crouch_t * 0.6 - self.prone_t * 0.7
    }

    pub fn is_mantling(&self) -> bool {
        self.mantle_t > 0.0
    }

    /// True while a slide is active — used by the viewmodel/HUD to tilt the camera and by the app for FOV kick.
    pub fn is_sliding(&self) -> bool {
        self.slide_t > 0.0
    }

    /// **Vault / climb detection** — the prerequisite query. Look just ahead at foot level; if something
    /// blocks the body there, find the top of that ledge. If the top is within climbing reach (a real step up,
    /// not a kerb, and not an unscalable wall) and there's clear standing room with headroom on top, return the
    /// landing point on top of the ledge. Walls read as full-height so their "top" is the ceiling — out of
    /// reach — which is exactly why you can mantle a crate or a deck edge but not climb a sheer wall.
    fn mantle_target(&self, level: &Level) -> Option<Vec3> {
        const MIN_RISE: f32 = 0.45; // below this it's just a step the floor-follow handles
        const MAX_RISE: f32 = 1.7; // above this it's out of arm's reach
        let fwd = self.cam.forward_flat();
        let dir = Vec2::new(fwd.x, fwd.z);
        let dl = dir.len();
        if dl < 1e-2 {
            return None;
        }
        let dir = Vec2::new(dir.x / dl, dir.y / dl);
        let reach = self.radius + 0.45;
        let p0 = self.pos.xz();
        let ahead = Vec2::new(p0.x + dir.x * reach, p0.y + dir.y * reach);
        if !body_blocked(level, ahead, self.radius, self.pos.y + 0.1, 0.5) {
            return None;
        }
        let mut top = level.support_y(ahead.x, ahead.y, self.pos.y + MAX_RISE + 0.6);
        for o in &level.obstacles {
            if ahead.x >= o.center.x - o.half.x
                && ahead.x <= o.center.x + o.half.x
                && ahead.y >= o.center.z - o.half.z
                && ahead.y <= o.center.z + o.half.z
            {
                top = top.max(o.center.y + o.half.y);
            }
        }
        let rise = top - self.pos.y;
        if rise < MIN_RISE || rise > MAX_RISE {
            return None;
        }
        let land = Vec3::new(ahead.x + dir.x * 0.35, top, ahead.y + dir.y * 0.35);
        if body_blocked(level, land.xz(), self.radius, top + 0.05, self.body_height()) {
            return None;
        }
        if level.ceiling_at(land.x, land.z, top + 0.1) < top + self.body_height() {
            return None;
        }
        Some(land)
    }

    /// Begin a vault/climb up onto a ledge ahead if the geometry allows it. Returns true if a mantle started.
    fn try_mantle(&mut self, level: &Level) -> bool {
        if self.mantle_t > 0.0 {
            return false;
        }
        if let Some(land) = self.mantle_target(level) {
            self.mantle_from = self.pos;
            self.mantle_to = land;
            self.mantle_t = MANTLE_TIME;
            self.vel = Vec3::ZERO;
            self.on_ground = false;
            true
        } else {
            false
        }
    }

    /// Advance an in-progress vault/climb: ease the body up and forward onto the ledge. Returns true while the
    /// mantle is still running (the caller should skip normal movement/gravity this frame).
    fn update_mantle(&mut self, dt: f32) -> bool {
        if self.mantle_t <= 0.0 {
            return false;
        }
        self.mantle_t -= dt;
        let k = (1.0 - (self.mantle_t / MANTLE_TIME)).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - k) * (1.0 - k); // ease-out
        let vlead = ease.powf(0.6); // rise over the lip slightly ahead of sliding forward
        self.pos.x = self.mantle_from.x + (self.mantle_to.x - self.mantle_from.x) * ease;
        self.pos.z = self.mantle_from.z + (self.mantle_to.z - self.mantle_from.z) * ease;
        self.pos.y = self.mantle_from.y + (self.mantle_to.y - self.mantle_from.y) * vlead;
        if self.mantle_t <= 0.0 {
            self.pos = self.mantle_to;
            self.vel = Vec3::ZERO;
            self.on_ground = true;
            self.mantle_t = 0.0;
        }
        true
    }
}

/// Is a body of radius `r` centred at world XZ point `p` intersecting a blocking wall or a crate?
/// Void (pit) tiles do NOT block — you can walk over a pit edge and fall in.
/// Does a body of radius `r`, standing with its feet at height `y` and rising `body_h` above that, collide
/// with a wall or obstacle at world XZ point `p`?
///
/// This is the 3-D collision core. Walls and Solid cells are full-height barriers (map boundaries, building
/// shells) and block at any height. Obstacles, however, block only when the body's vertical span overlaps the
/// obstacle's own vertical extent — so a knee-high crate stops you on the ground but not when you're up on a
/// catwalk above it, an overhead beam lets you walk underneath, and a railing on an upper deck doesn't wall
/// off the floor below. That vertical-span test is what makes stacked, multi-level geometry actually playable.
pub fn body_blocked(level: &Level, p: Vec2, r: f32, y: f32, body_h: f32) -> bool {
    let ts = level.ts();
    // Walls / Solid: full-height barriers, height-independent.
    let (minx, minz) = level.world_to_tile(Vec2::new(p.x - r, p.y - r));
    let (maxx, maxz) = level.world_to_tile(Vec2::new(p.x + r, p.y + r));
    for tz in minz..=maxz {
        for tx in minx..=maxx {
            if level.cell(tx, tz) == Cell::Wall || level.cell(tx, tz) == Cell::Solid {
                let rx0 = tx as f32 * ts;
                let rz0 = tz as f32 * ts;
                let cx = clampf(p.x, rx0, rx0 + ts);
                let cz = clampf(p.y, rz0, rz0 + ts);
                let ddx = p.x - cx;
                let ddz = p.y - cz;
                if ddx * ddx + ddz * ddz < r * r {
                    return true;
                }
            }
        }
    }
    // Cover crates / structural obstacles: circle-vs-rect footprint AND vertical-span overlap.
    // A small epsilon on the feet means standing *exactly* on an obstacle's top (feet == top, within float
    // error) reads as "above it" rather than "overlapping its side" — this is what stops the body from
    // catching on the top-meets-side seam of every crate, slab and step when you walk toward its edge.
    const TOP_EPS: f32 = 0.03;
    let (feet, head) = (y + TOP_EPS, y + body_h);
    for o in &level.obstacles {
        let (olo, ohi) = (o.center.y - o.half.y, o.center.y + o.half.y);
        if head <= olo || feet >= ohi {
            continue; // body passes cleanly under or over this obstacle
        }
        let cx = clampf(p.x, o.center.x - o.half.x, o.center.x + o.half.x);
        let cz = clampf(p.y, o.center.z - o.half.z, o.center.z + o.half.z);
        let ddx = p.x - cx;
        let ddz = p.y - cz;
        if ddx * ddx + ddz * ddz < r * r {
            return true;
        }
    }
    // Destructibles block exactly like obstacles while intact — and stop blocking the instant they're rubble.
    for d in &level.destructibles {
        if !d.blocks() {
            continue;
        }
        let (olo, ohi) = (d.center.y - d.half.y, d.center.y + d.half.y);
        if head <= olo || feet >= ohi {
            continue;
        }
        let cx = clampf(p.x, d.center.x - d.half.x, d.center.x + d.half.x);
        let cz = clampf(p.y, d.center.z - d.half.z, d.center.z + d.half.z);
        let ddx = p.x - cx;
        let ddz = p.y - cz;
        if ddx * ddx + ddz * ddz < r * r {
            return true;
        }
    }
    false
}

/// Convenience 2-D footprint query for ground-level pathfinding and line-of-sight: treats every wall and
/// obstacle as blocking regardless of height (a full-height probe standing on the floor at `p`).
pub fn footprint_blocked(level: &Level, p: Vec2, r: f32) -> bool {
    body_blocked(level, p, r, level.floor_h(p.x, p.y) - 0.5, 50.0)
}

/// Result of a physics landing audit — how the simulated jetpack drops came to rest.
#[derive(Clone, Copy, Debug, Default)]
pub struct LandingAudit {
    pub trials: u32,
    /// Came to rest standing cleanly on a surface.
    pub clean: u32,
    /// Came to rest with the body embedded inside a wall or prop — the broken case.
    pub embedded: u32,
    /// Fell out of the world entirely.
    pub fell: u32,
}

impl LandingAudit {
    pub fn clean_rate(&self) -> f32 {
        if self.trials == 0 {
            1.0
        } else {
            self.clean as f32 / self.trials as f32
        }
    }
}

/// **Jetpack-landing fuzzer.** Launch a body from above a spread of points — every obstacle's top plus random
/// floor positions — let gravity drop it, snap it to the support surface each step, and check where it comes
/// to rest: cleanly on geometry, embedded inside a prop (the bug), or fallen out of the world. Running this at
/// scale across a map scores how trustworthy its collision assets are; a high embedded-rate means props you
/// sink into instead of stand on. This is the simulation-based asset check — cheap enough to run by the
/// million across the whole preset library.
pub fn audit_landings(level: &Level, random_samples: usize, seed: u64) -> LandingAudit {
    const BH: f32 = 1.8;
    const DT: f32 = 1.0 / 60.0;
    let ts = level.ts();
    let mut a = LandingAudit::default();
    let mut s = seed | 1;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 11) as f64 / (1u64 << 53) as f64) as f32
    };
    // drop points: directly over every obstacle (the stress case) + random floor positions
    let mut spots: Vec<(f32, f32)> = level.obstacles.iter().map(|o| (o.center.x, o.center.z)).collect();
    for _ in 0..random_samples {
        spots.push((rnd() * level.w as f32 * ts, rnd() * level.h as f32 * ts));
    }
    for (x, z) in spots {
        if !level.has_floor_at(Vec2::new(x, z)) {
            continue; // only audit places a player could actually be
        }
        // start at an apex above the highest local surface and fall (a jetpack coming back down)
        let mut y = level.support_y(x, z, 1.0e6) + 3.0 + rnd() * 4.0;
        let mut vy = 1.5; // a touch of upward momentum first
        for _ in 0..360 {
            vy -= GRAVITY * DT;
            y += vy * DT;
            let surf = level.support_y(x, z, y);
            if vy <= 0.0 && y <= surf {
                y = surf;
                break;
            }
        }
        a.trials += 1;
        if y < level.floor_y + level.min_h() - 60.0 {
            a.fell += 1;
        } else if prop_sunk(level, x, z, y, y + BH) {
            a.embedded += 1; // sank into a prop instead of standing on it → broken collision asset
        } else {
            a.clean += 1;
        }
    }
    a
}

/// True when a body resting at (x,z) spanning [feet, head] has sunk *into* a prop — it's horizontally well
/// inside an obstacle's footprint yet its feet are below that obstacle's top. This isolates the real
/// stand-on-cover bug from a body merely grazing a wall edge (which the mover prevents in play).
fn prop_sunk(level: &Level, x: f32, z: f32, feet: f32, head: f32) -> bool {
    for o in &level.obstacles {
        let (olo, ohi) = (o.center.y - o.half.y, o.center.y + o.half.y);
        let inside = x > o.center.x - o.half.x + 0.06
            && x < o.center.x + o.half.x - 0.06
            && z > o.center.z - o.half.z + 0.06
            && z < o.center.z + o.half.z - 0.06;
        if inside && feet < ohi - 0.06 && head > olo + 0.06 {
            return true;
        }
    }
    false
}

#[inline]
fn approach(v: f32, target: f32, max_step: f32) -> f32 {
    let d = target - v;
    if d.abs() <= max_step {
        target
    } else {
        v + d.signum() * max_step
    }
}

#[inline]
fn v2_approach(v: Vec2, target: Vec2, max_step: f32) -> Vec2 {
    let dx = target.x - v.x;
    let dy = target.y - v.y;
    let d = (dx * dx + dy * dy).sqrt();
    if d <= max_step || d < 1e-6 {
        target
    } else {
        Vec2::new(v.x + dx / d * max_step, v.y + dy / d * max_step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Level, MapConfig};

    fn lvl() -> Level {
        Level::generate(MapConfig::default())
    }

    #[test]
    fn spawns_alive_with_full_health_and_a_pistol() {
        let l = lvl();
        let p = Player::new(&l);
        assert!(p.alive);
        assert_eq!(p.health, 100.0);
        assert_eq!(p.current, 0);
        assert!(p.weapon().mag > 0);
    }

    #[test]
    fn crawl_goes_prone_lowering_the_body_and_slowing_the_crawl() {
        let l = lvl();
        let mut p = Player::new(&l);
        let stand_h = p.body_height();
        // hold crawl for a while on the ground
        let input = Input { crawl: true, ..Default::default() };
        for _ in 0..60 {
            p.update(&l, &input, 1.0 / 60.0);
        }
        assert!(p.prone_t > 0.8, "should be prone after holding crawl, got {}", p.prone_t);
        assert!(p.body_height() < stand_h - 0.4, "prone should shrink the hitbox ({:.2} vs {:.2})", p.body_height(), stand_h);
        // can't sprint while prone
        let sprint_prone = Input { crawl: true, sprint: true, fwd: 1.0, ..Default::default() };
        p.update(&l, &sprint_prone, 1.0 / 60.0);
        assert!(!p.sprinting, "should not sprint while prone");
        // releasing crawl stands back up
        let up = Input::default();
        for _ in 0..80 {
            p.update(&l, &up, 1.0 / 60.0);
        }
        assert!(p.prone_t < 0.1, "should stand back up after releasing crawl, got {}", p.prone_t);
    }

    #[test]
    fn jetpack_fuel_drains_thrusting_and_recharges_grounded() {
        let l = lvl();
        let mut p = Player::new(&l);
        p.jetpack = true;
        p.jet_fuel = 1.0;
        // get airborne, then hold jump to thrust — fuel should drain
        p.on_ground = false;
        p.pos.y += 3.0;
        let thrust = Input { jump: true, ..Default::default() };
        for _ in 0..30 {
            p.update(&l, &thrust, 1.0 / 60.0);
        }
        assert!(p.jet_fuel < 1.0, "thrust should drain fuel, got {}", p.jet_fuel);
        // drain to empty and confirm thrust no longer fires
        p.jet_fuel = 0.0;
        p.update(&l, &thrust, 1.0 / 60.0);
        assert!(!p.jet_active, "no thrust when fuel is empty");
        // on the ground, fuel recharges
        p.on_ground = true;
        let idle = Input::default();
        for _ in 0..200 {
            p.update(&l, &idle, 1.0 / 60.0);
        }
        assert!(p.jet_fuel > 0.5, "fuel should recharge on the ground, got {}", p.jet_fuel);
    }

    #[test]
    fn vault_adds_camera_feel_that_resolves_when_not_mantling() {
        let l = lvl();
        let mut p = Player::new(&l);
        // not mantling: no vault lean, eye is the plain eye position
        assert_eq!(p.view_lean(), (0.0, 0.0));
        let plain_eye = p.eye();
        // simulate a mantle in progress at its midpoint
        p.mantle_t = MANTLE_TIME * 0.5;
        let (pitch, roll) = p.view_lean();
        assert!(pitch < 0.0, "should pitch down toward the ledge mid-vault, got {pitch}");
        assert!(roll > 0.0, "should roll toward the reaching hand mid-vault, got {roll}");
        // the eye should be displaced from the plain eye during the vault (the heave)
        let vault_eye = p.eye();
        assert!((vault_eye.y - plain_eye.y).abs() > 1e-4 || (vault_eye.x - plain_eye.x).abs() > 1e-4, "vault should shift the camera");
        // and it must fully resolve to the plain position once the mantle ends
        p.mantle_t = 0.0;
        assert_eq!(p.view_lean(), (0.0, 0.0));
        let ex = p.eye();
        assert!((ex.x - plain_eye.x).abs() < 1e-6 && (ex.y - plain_eye.y).abs() < 1e-6);
    }

    #[test]
    fn walking_into_a_wall_does_not_pass_through_it() {
        let l = lvl();
        let mut p = Player::new(&l);
        // Shove the player hard in many directions; never end up inside a wall.
        let dirs: [(f32, f32); 6] = [
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
        ];
        for (fx, fz) in dirs {
            let mut q = Player::new(&l);
            q.cam.yaw = fz.atan2(fx);
            let input = Input { fwd: 1.0, ..Default::default() };
            for _ in 0..600 {
                q.update(&l, &input, 1.0 / 60.0);
            }
            assert!(
                !body_blocked(&l, q.pos.xz(), q.radius * 0.95, q.pos.y, q.body_height()),
                "player ended inside geometry at {:?}",
                q.pos
            );
        }
        let _ = p.update(&l, &Input::default(), 0.016);
    }

    #[test]
    fn jumping_leaves_the_ground_then_lands() {
        let l = lvl();
        let mut p = Player::new(&l);
        let jump = Input { jump: true, ..Default::default() };
        p.update(&l, &jump, 1.0 / 60.0);
        assert!(!p.on_ground, "should be airborne right after a jump");
        // Let gravity bring it back.
        for _ in 0..200 {
            p.update(&l, &Input::default(), 1.0 / 60.0);
            if p.on_ground {
                break;
            }
        }
        assert!(p.on_ground, "should land again");
        assert!((p.pos.y - l.floor_y).abs() < 0.01);
    }

    #[test]
    fn taking_lethal_damage_kills_then_respawns() {
        let l = lvl();
        let mut p = Player::new(&l);
        p.take_damage(250.0);
        assert!(!p.alive);
        for _ in 0..200 {
            p.update(&l, &Input::default(), 1.0 / 60.0);
        }
        assert!(p.alive, "should have respawned");
        assert_eq!(p.health, 100.0);
    }

    #[test]
    fn weapon_switching_changes_the_active_slot() {
        let l = lvl();
        let mut p = Player::new(&l);
        let sw = Input { want_slot: Some(2), ..Default::default() };
        p.update(&l, &sw, 0.016);
        assert_eq!(p.current, 2);
    }
#[test]
fn obstacles_block_by_vertical_span_not_just_footprint() {
    use crate::level::{Level, MapConfig, Obstacle};
    use crate::math::Vec3;
    let mut l = Level::generate(MapConfig::default());
    // the spawn room centre is guaranteed open floor away from walls
    let sc = l.rooms[0].center;
    let c = Vec3::new(sc.x, 0.5, sc.z);
    l.obstacles.clear();
    l.obstacles.push(Obstacle { center: c, half: Vec3::new(0.6, 0.5, 0.6) });
    let p = crate::math::Vec2::new(c.x, c.z);
    // a body standing on the ground overlaps the crate → blocked
    assert!(body_blocked(&l, p, 0.4, 0.0, 1.8), "grounded body should hit the crate");
    // the same body up on a catwalk at y=3 is entirely above the crate → free to walk over it
    assert!(!body_blocked(&l, p, 0.4, 3.0, 1.8), "elevated body should clear the low crate");
    // and a tall body can duck (shrink) under a high beam
    l.obstacles.clear();
    l.obstacles.push(Obstacle { center: Vec3::new(c.x, 2.4, c.z), half: Vec3::new(0.6, 0.3, 0.6) }); // beam at [2.1,2.7]
    assert!(!body_blocked(&l, p, 0.4, 0.0, 1.8), "standing body passes under a high beam");
}

#[test]
fn a_rising_body_bonks_an_overhead_slab() {
    use crate::level::{Level, MapConfig, Platform};
    let mut l = Level::generate(MapConfig::default());
    // a slab whose walkable top is at 3.0 (underside ~2.82) directly overhead
    l.platforms.push(Platform { x0: 0.0, z0: 0.0, x1: 200.0, z1: 200.0, top: 3.0 });
    let head_below = 2.0;
    assert!(l.ceiling_at(20.0, 20.0, head_below).is_finite(), "slab overhead should register as a ceiling");
    assert!(l.ceiling_at(20.0, 20.0, 10.0).is_infinite(), "no ceiling above a head already past the slab");
}

#[test]
fn the_player_vaults_onto_a_reachable_ledge() {
    use crate::level::{Level, MapConfig, Platform};
    let mut l = Level::generate(MapConfig::default());
    let sc = l.rooms[0].center;
    // a chest-high deck right in front of the spawn, walkable on top at y=1.2
    l.platforms.push(Platform { x0: sc.x - 6.0, z0: sc.z + 0.7, x1: sc.x + 6.0, z1: sc.z + 6.0, top: 1.2 });
    // also a wall of Solid obstruction at the deck's near face so the body is blocked at foot level
    l.obstacles.push(crate::level::Obstacle { center: crate::math::Vec3::new(sc.x, 0.6, sc.z + 0.85), half: crate::math::Vec3::new(6.0, 0.6, 0.15) });

    let mut p = Player::new(&l);
    p.pos = crate::math::Vec3::new(sc.x, l.floor_h(sc.x, sc.z), sc.z);
    p.cam.yaw = 0.0; // forward_flat() = (sin0, 0, -cos0) = (0,0,-1): faces -Z … point the ledge that way
    // face +Z toward the deck: yaw so forward_flat ~ (0,0,+1)
    p.cam.yaw = std::f32::consts::PI;
    let before_y = p.pos.y;
    let input = Input { jump: true, ..Default::default() };
    p.update(&l, &input, 1.0 / 60.0);
    assert!(p.is_mantling(), "jump facing a reachable ledge should start a vault");
    // play it out
    for _ in 0..40 {
        p.update(&l, &Input::default(), 1.0 / 60.0);
        if !p.is_mantling() { break; }
    }
    assert!(p.pos.y > before_y + 0.8, "player should end up on top of the ledge, y {:.2} -> {:.2}", before_y, p.pos.y);
    assert!(p.on_ground, "should be standing after the climb");
}

#[test]
fn a_sheer_wall_cannot_be_climbed() {
    use crate::level::{Level, MapConfig};
    let l = Level::generate(MapConfig::default());
    let mut p = Player::new(&l);
    // walk into a wall for a while with jump held — should never start a mantle (walls are full-height)
    let mut mantled = false;
    let input = Input { fwd: 1.0, jump: true, ..Default::default() };
    for _ in 0..400 {
        p.update(&l, &input, 1.0 / 60.0);
        if p.is_mantling() { mantled = true; break; }
    }
    // it's fine to mantle a low cover crate that happens to be ahead, but the player must never be inside geometry
    assert!(!body_blocked(&l, p.pos.xz(), p.radius * 0.9, p.pos.y, p.body_height()) || mantled,
        "player must not be stuck inside a wall");
}

#[test]
fn a_body_lands_on_top_of_cover_not_inside_it() {
    use crate::level::{Level, MapConfig, Obstacle};
    use crate::math::Vec3;
    let mut l = Level::generate(MapConfig::default());
    let sc = l.rooms[0].center;
    l.obstacles.clear();
    // a chest-high crate on open floor, top at y = 1.1
    let crate_top = 1.1;
    l.obstacles.push(Obstacle { center: Vec3::new(sc.x, crate_top / 2.0, sc.z), half: Vec3::new(0.8, crate_top / 2.0, 0.8) });
    // the support query must report the crate top, not the floor, when a body is above it
    let s = l.support_y(sc.x, sc.z, crate_top + 1.0);
    assert!((s - (l.floor_y + crate_top)).abs() < 0.05, "should stand on the crate top, got {:.2}", s);

    // and a player jetpacking down lands on top, never embedded
    let mut p = Player::new(&l);
    p.pos = Vec3::new(sc.x, l.floor_h(sc.x, sc.z) + 4.0, sc.z); // up in the air over the crate
    p.vel = Vec3::new(0.0, 2.0, 0.0); // a little upward, then falls
    for _ in 0..240 {
        p.update(&l, &Input::default(), 1.0 / 60.0);
        if p.on_ground { break; }
    }
    assert!((p.pos.y - (l.floor_y + crate_top)).abs() < 0.2, "player should rest on the crate top, y={:.2}", p.pos.y);
}

#[test]
fn the_landing_fuzzer_finds_clean_collision_across_the_library() {
    use crate::level::{Level, PRESETS};
    // sample a spread of presets; every one should land bodies on geometry, not inside it
    for name in ["Crossfire Mesa", "Triad Foundry", "Berthing Docks", "Pinnacle", "Salt Mine", "Vanguard Reach"] {
        let p = PRESETS.iter().find(|p| p.name == name).unwrap();
        let lvl = Level::generate(p.config(7));
        let a = audit_landings(&lvl, 1500, 12345);
        assert!(a.trials > 50, "{} should produce drop trials", name);
        assert!(a.clean_rate() > 0.98, "{} clean landing rate {:.1}% too low (props you sink into)", name, a.clean_rate() * 100.0);
    }
}

#[test]
fn sprinting_then_crouch_triggers_a_forward_slide_boost() {
    use crate::level::{Level, MapConfig};
    let lvl = Level::generate(MapConfig::default());
    let mut pl = Player::new(&lvl);
    let (sx, sz) = (lvl.spawn(0).x, lvl.spawn(0).z);
    let base = lvl.floor_h(sx, sz);
    pl.pos = Vec3::new(sx, base, sz);
    pl.cam.yaw = std::f32::consts::FRAC_PI_2; // face +X
    // sprint forward for a bit to build up to sprint speed
    let sprint = Input { fwd: 1.0, sprint: true, ..Default::default() };
    for _ in 0..40 {
        pl.update(&lvl, &sprint, 1.0 / 60.0);
    }
    let sprint_speed = (pl.vel.x * pl.vel.x + pl.vel.z * pl.vel.z).sqrt();
    assert!(sprint_speed > 7.0, "should have built up sprint speed (got {sprint_speed:.1})");
    // now press crouch while sprinting → a slide should start and boost speed above sprint
    let slide = Input { fwd: 1.0, sprint: true, crouch: true, ..Default::default() };
    pl.update(&lvl, &slide, 1.0 / 60.0);
    assert!(pl.is_sliding(), "crouch while sprinting should start a slide");
    let slide_speed = (pl.vel.x * pl.vel.x + pl.vel.z * pl.vel.z).sqrt();
    assert!(slide_speed > sprint_speed, "slide should boost speed above sprint ({slide_speed:.1} vs {sprint_speed:.1})");
    // the slide forces the body low
    for _ in 0..6 {
        pl.update(&lvl, &slide, 1.0 / 60.0);
    }
    assert!(pl.crouch_t > 0.5, "sliding should drop the body low");
    // and it decays: after the slide window, speed eases back down
    for _ in 0..60 {
        pl.update(&lvl, &Input { fwd: 1.0, ..Default::default() }, 1.0 / 60.0);
    }
    assert!(!pl.is_sliding(), "the slide should end after its window");
}

#[test]
fn a_body_walks_up_a_low_lip_instead_of_catching_on_the_edge() {
    use crate::level::{Level, MapConfig, Obstacle};
    // Reproduces the "stuck where the top meets the side" bug: a low crate directly ahead. Walking into it
    // should step the body up onto the top (step-up assist), not stop it dead against the vertical face.
    let mut lvl = Level::generate(MapConfig::default());
    // clear obstacles and place a single knee-high crate ahead on +X
    lvl.obstacles.clear();
    lvl.destructibles.clear();
    let (sx, sz) = (lvl.spawn(0).x, lvl.spawn(0).z);
    let base = lvl.floor_h(sx, sz);
    let crate_top = base + 0.35; // a low lip, below the vault threshold
    lvl.obstacles.push(Obstacle {
        center: Vec3::new(sx + 2.0, base + 0.175, sz),
        half: Vec3::new(0.6, 0.175, 0.6),
    });
    let mut pl = Player::new(&lvl);
    pl.pos = Vec3::new(sx, base, sz);
    pl.vel = Vec3::ZERO;
    // hold "forward" toward +X. forward_flat = (sin yaw, 0, -cos yaw), so yaw = π/2 points +X.
    pl.cam.yaw = std::f32::consts::FRAC_PI_2;
    let input = Input { fwd: 1.0, ..Default::default() };
    let start_x = pl.pos.x;
    for _ in 0..180 {
        pl.update(&lvl, &input, 1.0 / 60.0);
    }
    // it should have climbed onto the crate (feet near its top) and moved past the near face, not stalled.
    assert!(
        pl.pos.x > start_x + 1.5,
        "body should have walked up and over the low crate, not caught on its edge (x {:.2} -> {:.2})",
        start_x,
        pl.pos.x
    );
    assert!(
        pl.pos.y >= crate_top - 0.1,
        "body should be standing on top of the crate after stepping up (y={:.2}, top={:.2})",
        pl.pos.y,
        crate_top
    );
}

#[test]
fn a_body_dropped_into_deep_water_floats_instead_of_sinking() {
    use crate::level::{Level, WaterBody, PRESETS};
    use egui::Color32;
    // Build a level, punch a deep water body into it, drop the player deep under the surface and confirm
    // buoyancy carries them up toward the surface rather than letting gravity pin them to the bed.
    let p = PRESETS.iter().find(|p| p.name == "Flood Zone").unwrap_or(&PRESETS[0]);
    let mut lvl = Level::generate(p.config(3));
    let (sx, sz) = (lvl.spawn(0).x, lvl.spawn(0).z);
    let surface_y = lvl.floor_h(sx, sz) + 6.0;
    lvl.water.push(WaterBody {
        cx: sx,
        cz: sz,
        radius: 40.0,
        y: surface_y,
        col: Color32::from_rgb(60, 110, 140),
        alpha: 150,
        reflect: 0.4,
        emissive: 0.0,
        lava: false,
        disc: true,
    });
    let mut pl = Player::new(&lvl);
    pl.pos = Vec3::new(sx, surface_y - 4.0, sz);
    pl.vel = Vec3::ZERO;
    pl.on_ground = false;
    let input = Input::default(); // no jump — pure buoyancy
    let start_y = pl.pos.y;
    for _ in 0..240 {
        pl.update(&lvl, &input, 1.0 / 60.0);
    }
    assert!(pl.submersion > 0.0, "player should register as submerged in the water body");
    assert!(
        pl.pos.y > start_y + 1.0,
        "buoyancy should float the body upward (start {:.2} -> {:.2})",
        start_y,
        pl.pos.y
    );
    assert!(
        pl.pos.y < surface_y + 1.5,
        "the body should settle around the surface, not launch out (y={:.2}, surface={:.2})",
        pl.pos.y,
        surface_y
    );
}

}
