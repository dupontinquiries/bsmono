//! powerup.rs — pickups and the timed **buffs** they grant, shared across modes.
//!
//! Ported from Pop Island 2: Speed Boost (a burst of pace for a few seconds),
//! Shield (soaks the next hit — a stun or a catch — then pops), and Triple Toss
//! (your next throws fire three projectiles). The `Buffs` struct is the portable
//! per-creature state; a mode ticks it each frame and queries it when resolving
//! movement, hits and throws.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power { Speed, Shield, Triple }

pub const POWERS: [Power; 3] = [Power::Speed, Power::Shield, Power::Triple];

impl Power {
    pub fn name(self) -> &'static str {
        match self { Power::Speed => "Speed Boost", Power::Shield => "Shield", Power::Triple => "Triple Toss" }
    }
    pub fn color(self) -> u32 {
        match self { Power::Speed => 0xffe040, Power::Shield => 0x5a9aff, Power::Triple => 0xff6acf }
    }
    /// Duration in seconds a timed buff lasts (Shield is charge-based, not timed).
    pub fn duration(self) -> f32 {
        match self { Power::Speed => 5.0, Power::Triple => 8.0, Power::Shield => 0.0 }
    }
}

/// The buffs currently active on one creature.
#[derive(Clone, Copy, Default)]
pub struct Buffs {
    pub speed_t: f32,   // seconds of Speed Boost remaining
    pub triple_t: f32,  // seconds of Triple Toss remaining
    pub shield: bool,   // a Shield charge waiting to soak the next hit
}

/// Speed multiplier applied while the Speed Boost is up.
pub const SPEED_MULT: f32 = 1.6;

impl Buffs {
    pub fn new() -> Self { Buffs::default() }

    /// Pick up a power-up.
    pub fn grant(&mut self, p: Power) {
        match p {
            Power::Speed => self.speed_t = self.speed_t.max(Power::Speed.duration()),
            Power::Triple => self.triple_t = self.triple_t.max(Power::Triple.duration()),
            Power::Shield => self.shield = true,
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.speed_t = (self.speed_t - dt).max(0.0);
        self.triple_t = (self.triple_t - dt).max(0.0);
    }

    pub fn speed_boosted(&self) -> bool { self.speed_t > 0.0 }
    pub fn triple_active(&self) -> bool { self.triple_t > 0.0 }

    /// Current movement multiplier from buffs.
    pub fn move_mult(&self) -> f32 { if self.speed_boosted() { SPEED_MULT } else { 1.0 } }

    /// A hit lands. If a shield is up it soaks the hit (pops) and returns `false`
    /// (the creature is spared); otherwise returns `true` (the hit connects).
    pub fn absorb_hit(&mut self) -> bool {
        if self.shield { self.shield = false; false } else { true }
    }

    /// How many projectiles a throw fires right now (3 while Triple Toss is up).
    pub fn throw_count(&self) -> u32 { if self.triple_active() { 3 } else { 1 } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_boost_multiplies_then_expires() {
        let mut b = Buffs::new();
        assert_eq!(b.move_mult(), 1.0);
        b.grant(Power::Speed);
        assert!((b.move_mult() - SPEED_MULT).abs() < 1e-6);
        for _ in 0..(6 * 30) { b.tick(1.0 / 30.0); }
        assert!(!b.speed_boosted(), "speed boost should lapse after its duration");
        assert_eq!(b.move_mult(), 1.0);
    }

    #[test]
    fn shield_soaks_one_hit_only() {
        let mut b = Buffs::new();
        b.grant(Power::Shield);
        assert!(!b.absorb_hit(), "the shield should spare the first hit");
        assert!(b.absorb_hit(), "with the shield spent, the next hit connects");
    }

    #[test]
    fn triple_toss_triples_then_expires() {
        let mut b = Buffs::new();
        assert_eq!(b.throw_count(), 1);
        b.grant(Power::Triple);
        assert_eq!(b.throw_count(), 3);
        for _ in 0..(9 * 30) { b.tick(1.0 / 30.0); }
        assert_eq!(b.throw_count(), 1, "triple toss should lapse");
    }
}
