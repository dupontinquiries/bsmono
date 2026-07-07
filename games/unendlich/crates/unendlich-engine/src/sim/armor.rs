//! Armor — durability that wears with hits, and a solar variant that mends
//! itself in sunlight. Leave the solar plate out under open sky and it knits
//! back together; in a cave or at night it just sits there.
//!
//! "Sunlight" here is a single `0..=255` the caller assembles from the systems
//! we already have: `season::daylight_fraction` (is the sun up?) × the wearer's
//! sky exposure (`sampling::exposure` of upward rays — open field vs forest
//! canopy vs cave) × skylight. So the repair is genuinely gated by *being in the
//! sun*, not merely by it being daytime. Pure, integer, `Copy` — rolls back.

/// What an armor piece is made to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArmorKind {
    /// Ordinary armor — wears down, never self-repairs.
    #[default]
    Plain,
    /// Sun-forged — mends in daylight.
    Solar,
}

/// A piece of armor with durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Armor {
    pub kind: ArmorKind,
    pub durability: u16,
    pub max_durability: u16,
}

impl Armor {
    pub fn new(kind: ArmorKind, max_durability: u16) -> Armor {
        Armor { kind, durability: max_durability, max_durability }
    }
    /// Solar armor at full durability.
    pub fn solar(max_durability: u16) -> Armor {
        Armor::new(ArmorKind::Solar, max_durability)
    }

    /// Take a hit; durability floors at 0.
    pub fn damage(&mut self, amount: u16) {
        self.durability = self.durability.saturating_sub(amount);
    }

    /// Fully worn through — offers no protection until repaired/reforged.
    pub fn is_broken(&self) -> bool {
        self.durability == 0
    }

    /// Fraction of protection remaining, `[0,1]` — a caller can scale damage
    /// reduction by this.
    pub fn condition(&self) -> f32 {
        if self.max_durability == 0 {
            0.0
        } else {
            self.durability as f32 / self.max_durability as f32
        }
    }

    /// Mend in the sun. `sunlight` (`0..=255`) is daylight × sky-exposure ×
    /// skylight; `rate` is the units repaired per tick under a full noon sky.
    /// Only `Solar` armor repairs; capped at `max_durability`. Returns the
    /// amount actually repaired. At night / underground (`sunlight == 0`) → 0.
    pub fn repair_in_sun(&mut self, sunlight: u8, rate: u16) -> u16 {
        if self.kind != ArmorKind::Solar || self.durability >= self.max_durability {
            return 0;
        }
        let gained = (rate as u32 * sunlight as u32 / 255) as u16;
        let before = self.durability;
        self.durability = (self.durability + gained).min(self.max_durability);
        self.durability - before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_wears_and_breaks() {
        let mut a = Armor::solar(100);
        a.damage(30);
        assert_eq!(a.durability, 70);
        assert!(!a.is_broken());
        a.damage(1000);
        assert_eq!(a.durability, 0, "floors at zero");
        assert!(a.is_broken());
        assert_eq!(a.condition(), 0.0);
    }

    #[test]
    fn solar_armor_mends_in_sun_not_at_night() {
        let mut a = Armor::solar(100);
        a.damage(50);
        // Night / underground: no sun, no repair.
        assert_eq!(a.repair_in_sun(0, 10), 0);
        assert_eq!(a.durability, 50);
        // Noon, open sky: full repair rate.
        let mended = a.repair_in_sun(255, 10);
        assert_eq!(mended, 10);
        assert_eq!(a.durability, 60);
        // Dappled light (half sun): partial repair.
        let half = a.repair_in_sun(128, 10);
        assert!(half > 0 && half < 10, "partial sun → partial mend: {half}");
    }

    #[test]
    fn plain_armor_never_self_repairs() {
        let mut a = Armor::new(ArmorKind::Plain, 100);
        a.damage(40);
        assert_eq!(a.repair_in_sun(255, 20), 0, "plain armor doesn't mend in sun");
        assert_eq!(a.durability, 60);
    }

    #[test]
    fn repair_caps_at_max() {
        let mut a = Armor::solar(100);
        a.damage(5);
        let mended = a.repair_in_sun(255, 50); // would overshoot
        assert_eq!(mended, 5, "only repairs up to full");
        assert_eq!(a.durability, 100);
        assert_eq!(a.repair_in_sun(255, 50), 0, "already full → nothing to mend");
    }
}
