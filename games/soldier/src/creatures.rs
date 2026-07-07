//! creatures.rs — "The Hunt" mode. An asymmetric mode where the player is a horror creature stalking a map
//! of unarmed prey. The prey carry no guns: they flee, break line of sight and hide. The player has no
//! firearm either — just a lethal melee strike and a lunge — and picks from a small bestiary, each creature
//! trading speed against reach.

use egui::Color32;

#[inline]
fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Which ruleset the match runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMode {
    /// The normal wave shooter.
    Standard,
    /// The Hunt: player is a creature, enemies are unarmed prey.
    Hunt,
}

/// In The Hunt, which side the player takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HuntRole {
    /// Play the creature, hunt the prey.
    Predator,
    /// Play one of the hunted: survive an AI creature alongside three AI survivors.
    Survivor,
}

/// A playable horror creature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Creature {
    Revenant,
    Stalker,
    Brute,
    Wraith,
    /// A giant flying lightning octopus-kraken — hovers and drifts, immense reach from lashing tentacles, and
    /// arcs lightning. The apex exotic predator.
    Kraken,
    /// A creepy giant spider — skitters fast and low, medium fanged reach, unsettling.
    Widow,
}

impl Creature {
    pub const ALL: [Creature; 6] = [Creature::Revenant, Creature::Stalker, Creature::Brute, Creature::Wraith, Creature::Kraken, Creature::Widow];

    pub fn name(self) -> &'static str {
        match self {
            Creature::Revenant => "Revenant",
            Creature::Stalker => "Stalker",
            Creature::Brute => "Brute",
            Creature::Wraith => "Wraith",
            Creature::Kraken => "Storm Kraken",
            Creature::Widow => "Widow",
        }
    }

    pub fn desc(self) -> &'static str {
        match self {
            Creature::Revenant => "Balanced hunter. Quick, lethal reach.",
            Creature::Stalker => "Fastest. Short reach, relentless.",
            Creature::Brute => "Slow, huge reach, one-hit slam.",
            Creature::Wraith => "Fast lunge, medium reach.",
            Creature::Kraken => "Flying lightning kraken. Immense reach, arcs storm.",
            Creature::Widow => "Skittering spider. Fast, fanged, creeping.",
        }
    }

    /// Base move-speed multiplier applied on top of the player's walk speed.
    pub fn speed_mul(self) -> f32 {
        match self {
            Creature::Revenant => 1.18,
            Creature::Stalker => 1.38,
            Creature::Brute => 0.98,
            Creature::Wraith => 1.22,
            Creature::Kraken => 1.30, // drifts fast, hovering
            Creature::Widow => 1.44,  // fastest — a scuttling rush
        }
    }

    /// Reach of the melee strike, in world units.
    pub fn reach(self) -> f32 {
        match self {
            Creature::Revenant => 2.8,
            Creature::Stalker => 2.3,
            Creature::Brute => 3.6,
            Creature::Wraith => 2.7,
            Creature::Kraken => 4.4, // lashing tentacles — the longest reach
            Creature::Widow => 2.9,  // fanged lunge
        }
    }

    /// Strike damage. (Prey have modest health, so most of these one-shot.)
    pub fn damage(self) -> f32 {
        match self {
            Creature::Revenant => 120.0,
            Creature::Stalker => 80.0,
            Creature::Brute => 200.0,
            Creature::Wraith => 110.0,
            Creature::Kraken => 140.0,
            Creature::Widow => 95.0,
        }
    }

    /// Cooldown between strikes, seconds.
    pub fn strike_cd(self) -> f32 {
        match self {
            Creature::Revenant => 0.6,
            Creature::Stalker => 0.45,
            Creature::Brute => 0.95,
            Creature::Wraith => 0.6,
            Creature::Kraken => 0.7,
            Creature::Widow => 0.5,
        }
    }

    /// Forward lunge speed used by the dive/lunge ability.
    pub fn lunge(self) -> f32 {
        match self {
            Creature::Revenant => 17.0,
            Creature::Stalker => 21.0,
            Creature::Brute => 14.0,
            Creature::Wraith => 23.0,
            Creature::Kraken => 20.0,
            Creature::Widow => 24.0, // a vicious pounce
        }
    }

    /// Signature colour — tints the claws on the viewmodel and the predator-vision vignette.
    pub fn color(self) -> Color32 {
        match self {
            Creature::Revenant => rgb(150, 60, 70),
            Creature::Stalker => rgb(90, 130, 80),
            Creature::Brute => rgb(120, 90, 60),
            Creature::Wraith => rgb(90, 100, 150),
            Creature::Kraken => rgb(90, 170, 255), // lightning blue
            Creature::Widow => rgb(140, 60, 150),  // venomous violet
        }
    }

    /// Whether this creature hovers/flies (affects movement + silhouette). The Kraken is airborne.
    pub fn flying(self) -> bool {
        matches!(self, Creature::Kraken)
    }

    /// Whether this creature arcs lightning on its strike (Kraken) — the game can spawn an electric VFX.
    pub fn electric(self) -> bool {
        matches!(self, Creature::Kraken)
    }

    pub fn id(self) -> u8 {
        Creature::ALL.iter().position(|&c| c == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> Creature {
        Creature::ALL.get(id as usize).copied().unwrap_or(Creature::Revenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creature_ids_round_trip() {
        for c in Creature::ALL {
            assert_eq!(Creature::from_id(c.id()), c);
        }
    }

    #[test]
    fn stalker_outpaces_brute_and_brute_outreaches_stalker() {
        assert!(Creature::Stalker.speed_mul() > Creature::Brute.speed_mul());
        assert!(Creature::Brute.reach() > Creature::Stalker.reach());
    }

    #[test]
    fn exotic_creatures_have_distinct_signatures() {
        // Kraken: flying + electric, the longest reach of all.
        assert!(Creature::Kraken.flying(), "kraken should fly");
        assert!(Creature::Kraken.electric(), "kraken arcs lightning");
        let longest = Creature::ALL.iter().map(|c| c.reach()).fold(0.0f32, f32::max);
        assert_eq!(Creature::Kraken.reach(), longest, "kraken should have the longest reach");
        // Widow: fastest of all, not flying.
        assert!(!Creature::Widow.flying());
        let fastest = Creature::ALL.iter().map(|c| c.speed_mul()).fold(0.0f32, f32::max);
        assert_eq!(Creature::Widow.speed_mul(), fastest, "widow should be the fastest");
        // only the kraken flies/arcs
        assert_eq!(Creature::ALL.iter().filter(|c| c.flying()).count(), 1);
        assert_eq!(Creature::ALL.iter().filter(|c| c.electric()).count(), 1);
    }
}
