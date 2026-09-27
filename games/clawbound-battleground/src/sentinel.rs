//! sentinel -- the Sentinel role: a cat bigger and slower than the rest,
//! who casts nothing and wears fields instead.
//!
//! A Sentinel's four keys are `Archetype::Field`s. Pressing one does not
//! fire anything: it SHIFTS what her coat is made of ([`Game::shift_coat`]),
//! and whatever it is made of is a field that is on around her every tick
//! for as long as she wears it ([`Game::sentinel_tick`]). She always wears
//! one: the first coat she has, from the moment she first stands up.
//!
//! The Sentinels are the Geartooth engineers' work (the clan that built the
//! Thornmaze's machine, which knows them: `machine`). Oremantle, the first
//! of them, was built in the likeness of Stonewhisper's grandmother:
//!
//! * **Granite** -- every friend in the field, and she, takes a share less
//!   from every blow (`GodUnit::ward`, read by `Game::deal_damage`).
//! * **Rime** -- every enemy in it is slowed.
//! * **Lodestone** -- every enemy in it is dragged toward her, to arm's
//!   length and no nearer.
//! * **The Mountain's Heart** (the ultimate) -- magma: everything hostile in
//!   it burns (cats, minions, towers -- `aoe_targets`), for `secs`; then she
//!   cools back into the coat she wore before.
//!
//! What varies between Sentinels is only the data (materials, radii,
//! amounts); what a material DOES is here, once. Size -- drawn and struck at
//! `Role::size` -- is the role's, not the material's: `hitbox::cat_sized`
//! and `models` read it.
//!
//! Headless; the scene tints her coat by material (`Material::tint`) and
//! draws the field's edge, the cues play her tread and her shifts.

use glam::Vec3;

use crate::gods::{Archetype, GodDef, Material};
use crate::sim::{Event, Game};

/// Granite spares at most this share of a blow, however ranked.
pub const GRANITE_CAP: f32 = 0.35;
/// Rime slows at most this much.
pub const RIME_CAP: f32 = 0.5;
/// Lodestone draws an enemy no nearer than this (flat distance).
pub const LODE_HOLD: f32 = 2.6;
/// Magma burns in pulses this far apart (so the numbers read).
pub const BURN_EVERY: f32 = 0.5;
/// Units walked between two of her footfalls (each one heard).
pub const TREAD: f32 = 3.4;

/// Is this ancestor a Sentinel by kit (her keys are fields)?
pub fn is_sentinel(def: &GodDef) -> bool {
    def.abilities.iter().any(|a| matches!(a.arch, Archetype::Field { .. }))
}

/// The field she is wearing: what it is, how far it reaches, how strong it
/// is (ranked), and its power scaling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Worn {
    pub slot: u32,
    pub material: Material,
    pub radius: f32,
    pub amount: f32,
    pub power_scale: f32,
}

impl Game {
    /// What Sentinel `gi` is wearing now (None for anyone else, or before
    /// her first coat).
    pub fn worn(&self, gi: usize) -> Option<Worn> {
        let slot = self.gods[gi].field_slot?;
        let ab = self.ability(gi, slot)?;
        match ab.arch {
            Archetype::Field { material, radius, amount, .. } => Some(Worn { slot, material, radius, amount, power_scale: ab.power_scale }),
            _ => None,
        }
    }

    /// Shift Sentinel `gi`'s coat to the one on key `slot`. A coat that
    /// lasts (`secs` > 0, the ultimate) remembers the one it replaced and
    /// goes back to it when it runs out; an ordinary coat ends one early.
    pub fn shift_coat(&mut self, gi: usize, slot: u32) {
        let Some(ab) = self.ability(gi, slot) else { return };
        let Archetype::Field { material, radius, secs, .. } = ab.arch else { return };
        let pos = self.gods[gi].pos;
        let g = &mut self.gods[gi];
        if secs > 0.0 {
            if g.field_t <= 0.0 {
                g.field_back = g.field_slot;
            }
            g.field_t = secs;
        } else {
            g.field_t = 0.0;
            g.field_back = None;
        }
        g.field_slot = Some(slot);
        g.burn_t = 0.0;
        self.events.push(Event::CoatShift { god: gi, pos, material, radius });
    }

    /// The first coat `gi` has (lowest unlocked key that is not the
    /// ultimate), if any.
    fn first_coat(&self, gi: usize) -> Option<u32> {
        (0..4u32).find(|&s| self.ability(gi, s).is_some_and(|a| !a.ult && matches!(a.arch, Archetype::Field { .. })))
    }

    /// The key of the coat that is `material`, if she has it unlocked.
    pub fn coat_key(&self, gi: usize, material: Material) -> Option<u32> {
        (0..4u32).find(|&s| self.ability(gi, s).is_some_and(|a| matches!(a.arch, Archetype::Field { material: m, .. } if m == material)))
    }

    /// Which coat a bot Sentinel wants now (None: keep what she wears).
    /// The heart on a fight (two enemy cats in it, or one badly hurt);
    /// lodestone on a cat near but out of her reach, to draw it in; rime on
    /// a cat already at her paws, to keep it there; granite otherwise --
    /// with friends round her, it is the one that does the most.
    pub fn coat_for(&self, gi: usize) -> Option<u32> {
        let g = &self.gods[gi];
        if g.field_t > 0.0 {
            return None; // the heart is burning: let it
        }
        let foes: Vec<(f32, f32)> = self
            .gods
            .iter()
            .filter(|e| e.team != g.team && e.respawn_t <= 0.0)
            .map(|e| (Vec3::new(e.pos.x - g.pos.x, 0.0, e.pos.z - g.pos.z).length(), e.hp / e.max_hp.max(1.0)))
            .collect();
        let reach = |m: Material| self.coat_key(gi, m).and_then(|s| self.ability(gi, s)).map(|a| match a.arch {
            Archetype::Field { radius, .. } => radius,
            _ => 0.0,
        });
        let ready = |m: Material| self.coat_key(gi, m).filter(|&s| g.cds[s as usize] <= 0.0);
        if let (Some(s), Some(r)) = (ready(Material::Magma), reach(Material::Magma)) {
            let inside: Vec<&(f32, f32)> = foes.iter().filter(|f| f.0 < r * 0.8).collect();
            if inside.len() >= 2 || inside.iter().any(|f| f.1 < 0.5) {
                return Some(s);
            }
        }
        let nearest = foes.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
        let want = match reach(Material::Lodestone) {
            Some(r) if nearest > LODE_HOLD + 1.5 && nearest < r => Material::Lodestone,
            _ if nearest <= LODE_HOLD + 1.5 && reach(Material::Rime).is_some() => Material::Rime,
            _ => Material::Granite,
        };
        let key = self.coat_key(gi, want).or_else(|| self.coat_key(gi, Material::Granite))?;
        (g.field_slot != Some(key)).then_some(key)
    }

    /// Once a tick: every Sentinel's field does what it does.
    pub fn sentinel_tick(&mut self, dt: f32) {
        for g in self.gods.iter_mut() {
            g.ward = 1.0;
        }
        for gi in 0..self.gods.len() {
            if !is_sentinel(self.def(gi)) {
                continue;
            }
            // she always wears something: her first coat, from the start
            if self.gods[gi].field_slot.is_none() {
                if let Some(s) = self.first_coat(gi) {
                    self.shift_coat(gi, s);
                }
            }
            // a lasting coat runs out, and she cools back
            if self.gods[gi].field_t > 0.0 {
                self.gods[gi].field_t -= dt;
                if self.gods[gi].field_t <= 0.0 {
                    self.gods[gi].field_t = 0.0;
                    let back = self.gods[gi].field_back.take().or_else(|| self.first_coat(gi));
                    self.gods[gi].field_slot = None;
                    if let Some(s) = back {
                        self.shift_coat(gi, s);
                    }
                }
            }
            if self.gods[gi].respawn_t > 0.0 {
                continue;
            }
            // her tread: heavy enough to hear
            let g = &self.gods[gi];
            let moved = g.move_dir.length_squared() > 0.01 && g.dash.is_none() && g.leap.is_none();
            if moved {
                self.gods[gi].stride += self.move_speed(gi) * dt;
                if self.gods[gi].stride >= TREAD {
                    self.gods[gi].stride = 0.0;
                    self.events.push(Event::Tread { pos: self.gods[gi].pos });
                }
            }
            let Some(w) = self.worn(gi) else { continue };
            let (at, team, power) = (self.gods[gi].pos, self.gods[gi].team, self.gods[gi].power);
            let flat = |p: Vec3| Vec3::new(p.x - at.x, 0.0, p.z - at.z).length();
            match w.material {
                Material::Granite => {
                    let spared = w.amount.clamp(0.0, GRANITE_CAP);
                    for g in self.gods.iter_mut() {
                        if g.team == team && g.respawn_t <= 0.0 && flat(g.pos) <= w.radius {
                            g.ward = g.ward.min(1.0 - spared);
                        }
                    }
                }
                Material::Rime => {
                    let slow = w.amount.clamp(0.0, RIME_CAP);
                    for g in self.gods.iter_mut() {
                        if g.team != team && g.respawn_t <= 0.0 && flat(g.pos) <= w.radius {
                            g.slow = g.slow.max(slow);
                            g.slow_t = g.slow_t.max(0.25);
                        }
                    }
                }
                Material::Lodestone => {
                    let blockers = &self.blockers;
                    for g in self.gods.iter_mut() {
                        let d = flat(g.pos);
                        let loose = g.leap.is_none() && g.dash.is_none() && g.perch.is_none() && g.lift.is_none() && g.riding.is_none();
                        if g.team == team || g.respawn_t > 0.0 || d > w.radius || d <= LODE_HOLD || !loose {
                            continue;
                        }
                        let toward = Vec3::new(at.x - g.pos.x, 0.0, at.z - g.pos.z) / d;
                        let step = (w.amount * dt).min(d - LODE_HOLD);
                        // `resolve` keeps it on a floored map's floor too
                        // (the Drowned Halls' walls are the floor's edge)
                        let p = blockers.resolve(g.pos + toward * step, crate::consts::GOD_RADIUS);
                        g.pos = crate::abilities::clamp_arena(Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z));
                    }
                }
                Material::Magma => {
                    self.gods[gi].burn_t += dt;
                    while self.gods[gi].burn_t >= BURN_EVERY {
                        self.gods[gi].burn_t -= BURN_EVERY;
                        let d = (w.amount + power * w.power_scale) * BURN_EVERY;
                        let (pp, pf) = (self.gods[gi].pen_pct, self.gods[gi].pen_flat);
                        for t in self.aoe_targets(at, w.radius, team) {
                            self.deal_damage(t, d, true, Some(gi), pp, pf);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::{Role, ROSTER};

    fn oremantle() -> u32 {
        ROSTER.iter().find(|d| d.role == Role::Sentinel).unwrap().id
    }

    /// Every Sentinel wears only fields, and only a Sentinel does: the role
    /// and the kit cannot disagree.
    #[test]
    fn sentinels_and_fields_go_together() {
        let n = ROSTER.iter().filter(|d| d.role == Role::Sentinel).count();
        assert!(n >= 1, "no Sentinel in the roster");
        for d in ROSTER.iter() {
            let fields = d.abilities.iter().filter(|a| matches!(a.arch, Archetype::Field { .. })).count();
            if d.role == Role::Sentinel {
                assert_eq!(fields, 4, "{} is a Sentinel with a key that is not a field", d.name);
                assert!(d.abilities.iter().all(|a| a.cost == 0.0), "{}'s coats cost mana", d.name);
            } else {
                assert_eq!(fields, 0, "{} wears a field and is not a Sentinel", d.name);
            }
        }
    }

    /// A Sentinel is the biggest and the slowest cat there is.
    #[test]
    fn a_sentinel_is_big_and_slow() {
        let s = &ROSTER[oremantle() as usize];
        assert!(s.role.size() > 1.2 && s.role.size() < 1.7, "size {}", s.role.size());
        for d in ROSTER.iter().filter(|d| d.role != Role::Sentinel) {
            assert_eq!(d.role.size(), 1.0);
            assert!(d.move_speed > s.move_speed + 0.8, "{} ({}) is not clearly faster than {} ({})", d.name, d.move_speed, s.name, s.move_speed);
        }
    }

    const AT: Vec3 = Vec3::new(-38.0, 0.0, -8.0);
    const DT: f32 = 1.0 / 60.0;

    /// Oremantle (blue, 0) with a friend (blue, 1) and two enemies (red, 2
    /// and 3), level `lv`, everyone placed round her at `AT`.
    fn scene(lv: u32, friend: f32, near: f32, far: f32) -> Game {
        let mut g = Game::new_teams(&[oremantle(), 0], &[1, 2]);
        for (i, dx) in [(0, 0.0), (1, friend), (2, near), (3, far)] {
            let p = AT + Vec3::new(dx, 0.0, 0.0);
            g.gods[i].pos = Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z);
            g.gods[i].level = lv;
        }
        g
    }

    fn wear(g: &mut Game, m: Material) {
        let k = g.coat_key(0, m).expect("the coat is unlocked");
        g.shift_coat(0, k);
    }

    /// Granite: a friend inside the field takes a share less from the same
    /// blow than a friend outside it, and the share is the coat's amount.
    #[test]
    fn granite_spares_friends_inside_it() {
        let taken = |friend: f32| {
            let mut g = scene(1, friend, 30.0, 40.0);
            g.sentinel_tick(DT);
            let before = g.gods[1].hp;
            g.deal_damage(crate::sim::TargetRef::God(1), 100.0, false, Some(2), 0.0, 0.0);
            before - g.gods[1].hp
        };
        let (inside, outside) = (taken(3.0), taken(20.0));
        let Archetype::Field { amount, .. } = ROSTER[oremantle() as usize].abilities[0].arch else { panic!() };
        let ratio = inside / outside;
        assert!((ratio - (1.0 - amount)).abs() < 0.01, "inside {inside}, outside {outside}: spared {:.3}, meant {amount}", 1.0 - ratio);
    }

    /// Rime slows an enemy inside it, and nobody outside it or on her side.
    #[test]
    fn rime_slows_enemies_inside_it() {
        let mut g = scene(2, 3.0, 4.0, 20.0);
        g.sentinel_tick(DT);
        wear(&mut g, Material::Rime);
        g.sentinel_tick(DT);
        assert!(g.gods[2].slow >= 0.2, "enemy inside: slow {}", g.gods[2].slow);
        assert_eq!(g.gods[3].slow, 0.0, "enemy outside");
        assert_eq!(g.gods[1].slow, 0.0, "her friend");
        let fast = g.move_speed(3);
        let slow = g.move_speed(2);
        assert!(slow < fast * 0.85, "slowed {slow} vs free {fast}");
    }

    /// Lodestone draws an enemy inside it toward her over a second, stops
    /// it at arm's length, and leaves one outside where it was.
    #[test]
    fn lodestone_draws_enemies_in_to_arms_length() {
        let mut g = scene(3, 20.0, 6.0, 25.0);
        g.sentinel_tick(DT);
        wear(&mut g, Material::Lodestone);
        let d = |g: &Game, i: usize| Vec3::new(g.gods[i].pos.x - g.gods[0].pos.x, 0.0, g.gods[i].pos.z - g.gods[0].pos.z).length();
        let (d0, far0) = (d(&g, 2), d(&g, 3));
        for _ in 0..60 {
            g.sentinel_tick(DT);
        }
        let d1 = d(&g, 2);
        assert!(d0 - d1 > 1.2, "drawn only {:.2} u in a second ({d0:.2} -> {d1:.2})", d0 - d1);
        for _ in 0..600 {
            g.sentinel_tick(DT);
        }
        let d2 = d(&g, 2);
        assert!(d2 >= LODE_HOLD - 1e-3 && d2 < LODE_HOLD + 0.6, "held at {d2:.4}: never nearer than {LODE_HOLD}");
        assert!((d(&g, 3) - far0).abs() < 1e-3, "the far one moved");
    }

    /// The Mountain's Heart burns an enemy inside it (and not one outside),
    /// then runs out and she is back in the coat she wore before.
    #[test]
    fn the_heart_burns_then_cools_back() {
        let mut g = scene(5, 20.0, 4.0, 25.0);
        g.sentinel_tick(DT);
        wear(&mut g, Material::Rime);
        let heart = g.coat_key(0, Material::Magma).expect("the ultimate at level 5");
        let Archetype::Field { secs, .. } = g.ability(0, heart).unwrap().arch else { panic!() };
        g.begin_cast(0, heart, g.gods[0].pos);
        assert_eq!(g.worn(0).map(|w| w.material), Some(Material::Magma), "the shift is at once");
        let (near0, far0) = (g.gods[2].hp, g.gods[3].hp);
        for _ in 0..60 {
            g.sentinel_tick(DT);
        }
        let burned = near0 - g.gods[2].hp;
        assert!(burned > 20.0, "burned {burned} in a second");
        assert_eq!(g.gods[3].hp, far0, "the far one burned");
        for _ in 0..((secs + 0.5) / DT) as usize {
            g.sentinel_tick(DT);
        }
        assert_eq!(g.worn(0).map(|w| w.material), Some(Material::Rime), "cooled back to what she wore");
    }

    /// Shifting is a key press: at once, free, and it does not interrupt
    /// her walk (no wind-up, full speed).
    #[test]
    fn a_shift_is_instant_and_free() {
        let mut g = scene(2, 20.0, 30.0, 40.0);
        g.sentinel_tick(DT);
        let mana = g.gods[0].mana;
        let k = g.coat_key(0, Material::Rime).unwrap();
        g.begin_cast(0, k, g.gods[0].pos);
        assert!(g.gods[0].casting.is_none());
        assert_eq!(g.worn(0).map(|w| w.material), Some(Material::Rime));
        assert_eq!(g.gods[0].mana, mana);
        assert!(g.gods[0].cds[k as usize] > 0.0, "a shift has a short cooldown");
    }

    /// She wears her first coat from the first tick, with no key pressed.
    #[test]
    fn she_is_never_bare() {
        let mut g = Game::new_teams(&[oremantle()], &[0]);
        assert!(g.worn(0).is_none());
        g.sentinel_tick(1.0 / 60.0);
        let w = g.worn(0).expect("a coat on the first tick");
        assert_eq!(w.material, Material::Granite);
    }
}
