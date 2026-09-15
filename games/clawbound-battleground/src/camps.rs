//! camps — jungle camps, as a herd rather than a health bar.
//!
//! The map had three prey (`wild::PreyKind`: hare, stag, glowfish) and two
//! bosses (`bosses::BossKind`: boar, drake). Nothing in between, and nothing
//! with any internal structure: a camp was one creature you hit until it
//! died.
//!
//! A camp here is a **leader flanked by smaller creatures**. That shape is
//! the whole design: the flankers are what make clearing one a decision
//! rather than a dps check. They are individually weak, they aggro together,
//! and on most camps they are what actually kills you if you open on the
//! leader without accounting for them.
//!
//! ## The roster spans eras deliberately
//!
//! Clawbound's ancestors are already myth rather than history, so the
//! bestiary is drawn from across the Cenozoic and the Holocene rather than
//! from one period. An aurochs, a glyptodon and a musk ox can share a jungle
//! because the premise is a hunting-ground out of time, not a biome.
//!
//! Two constraints on what got in:
//!
//! * **No dinosaurs.** The silhouette language of this game is mammalian
//!   quadrupeds; a theropod reads as a different game. The one bird on the
//!   roster is a terror bird, which is a bird rather than a dinosaur in the
//!   way a player means it, and it is the roster's most borderline entry.
//! * **Nothing tiny.** The smallest flanker is capybara-sized. Below that a
//!   creature is scenery at gameplay camera distance — the game already has
//!   `wild::PreyKind` for things that small, and they read as ambience.
//!
//! ## Layer
//!
//! Pure data and validation: no renderer, no `Game`, no meshes. The point is
//! that [`CampSpec`] is also the shape the AI map-generation backend emits —
//! a generated camp is the same struct as a hand-written one and goes through
//! the same [`CampSpec::validate`]. That is what keeps a generated map from
//! being a second, weaker code path.

use crate::anim::Build;

/// Roughly when a beast is from. Flavour and naming, not simulation — but it
/// keeps the roster honest about spanning eras rather than drifting back to
/// "generic fantasy animal".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// ~34–23 Mya. The big hornless rhinos.
    Oligocene,
    /// ~23–5 Mya.
    Miocene,
    /// ~2.6 Mya–11.7 kya. The classic ice-age megafauna.
    Pleistocene,
    /// 11.7 kya–present, including things lost in recorded history.
    Holocene,
}

impl Era {
    pub fn label(self) -> &'static str {
        match self {
            Era::Oligocene => "Oligocene",
            Era::Miocene => "Miocene",
            Era::Pleistocene => "Pleistocene",
            Era::Holocene => "Holocene",
        }
    }
}

/// How big a thing is, in gameplay terms.
///
/// This drives the [`Build`] used for animation and the camp tier it may
/// appear in, so it is not decoration: a `Colossal` creature cannot be a
/// flanker, and `Small` cannot lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Size {
    /// Capybara to wolf. Flanker only.
    Small,
    /// Boar to elk.
    Medium,
    /// Bison, aurochs, woolly rhino. The standard camp leader.
    Large,
    /// Ground sloth, paraceratherium. Objective-tier.
    Huge,
    /// Mammoth. One per map, if that.
    Colossal,
}

impl Size {
    pub fn build(self) -> Build {
        match self {
            Size::Small => Build::hare(),
            Size::Medium => Build::stag(),
            Size::Large | Size::Huge => Build::megafauna(),
            Size::Colossal => Build::colossus(),
        }
    }

    /// Body radius in world units — also the blocker footprint for the
    /// living creature, so a bison is something you path around.
    pub fn radius(self) -> f32 {
        match self {
            Size::Small => 0.5,
            Size::Medium => 0.9,
            Size::Large => 1.6,
            Size::Huge => 2.6,
            Size::Colossal => 4.0,
        }
    }

    pub fn can_lead(self) -> bool {
        self >= Size::Medium
    }
    pub fn can_flank(self) -> bool {
        self <= Size::Medium
    }
}

/// One species.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beast {
    // ── Large / leaders ────────────────────────────────────────────────
    /// Steppe bison. The archetype the whole camp shape was designed around.
    SteppeBison,
    /// Aurochs — the wild cattle Europe hunted out by 1627.
    Aurochs,
    WoollyRhino,
    /// Megaloceros: the Irish elk, 3.6m antler span.
    IrishElk,
    /// Glyptodon — an armadillo the size of a car. Tanky, slow, low damage.
    Glyptodon,
    MuskOx,
    /// Gaur, the living wild ox — the modern entry on a mostly-extinct list.
    Gaur,

    // ── Huge / Colossal — objectives ──────────────────────────────────
    /// Megatherium, the giant ground sloth. Rears to fight.
    GroundSloth,
    /// Paraceratherium: the largest land mammal that ever lived.
    Paraceratherium,
    WoollyMammoth,
    /// Steller's sea cow — a river objective, extinct within 27 years of
    /// being described.
    SeaCow,

    // ── Medium ─────────────────────────────────────────────────────────
    /// Phorusrhacos. The roster's most borderline entry — see the module
    /// header. A bird, but a 2.5m flightless predatory one.
    TerrorBird,
    Tapir,
    DireWolf,
    /// Smilodon. A cat, so it fights like the gods do.
    Smilodon,

    // ── Small / flankers ───────────────────────────────────────────────
    Capybara,
    /// Thylacine — gone in 1936.
    Thylacine,
    /// Giant beaver, Castoroides. Bear-sized rodent.
    GiantBeaver,
    Peccary,
}

/// A species' fixed properties.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeastStats {
    pub name: &'static str,
    pub era: Era,
    pub size: Size,
    pub hp: f32,
    pub damage: f32,
    /// World units/sec.
    pub speed: f32,
    /// Gold to the killer.
    pub gold: f32,
    /// Attack reach beyond the body radius.
    pub reach: f32,
}

impl Beast {
    pub fn stats(self) -> BeastStats {
        use Beast::*;
        use Era::*;
        let s = |name, era, size, hp, damage, speed, gold, reach| BeastStats {
            name,
            era,
            size,
            hp,
            damage,
            speed,
            gold,
            reach,
        };
        match self {
            // Leaders. Speed is the main lever between them: the bison is a
            // fair fight, the rhino punishes standing still, the glyptodon
            // cannot catch you at all and is priced accordingly.
            SteppeBison => s("Steppe Bison", Pleistocene, Size::Large, 1400.0, 62.0, 5.5, 160.0, 1.4),
            Aurochs => s("Aurochs", Holocene, Size::Large, 1250.0, 58.0, 6.0, 150.0, 1.4),
            WoollyRhino => s("Woolly Rhino", Pleistocene, Size::Large, 1700.0, 78.0, 6.5, 195.0, 1.6),
            IrishElk => s("Irish Elk", Pleistocene, Size::Large, 1150.0, 54.0, 7.5, 155.0, 2.2),
            Glyptodon => s("Glyptodon", Pleistocene, Size::Large, 2300.0, 40.0, 3.0, 175.0, 1.2),
            MuskOx => s("Musk Ox", Holocene, Size::Large, 1300.0, 55.0, 5.0, 145.0, 1.3),
            Gaur => s("Gaur", Holocene, Size::Large, 1500.0, 66.0, 6.2, 170.0, 1.5),

            // Objectives.
            GroundSloth => s("Megatherium", Pleistocene, Size::Huge, 4200.0, 120.0, 3.2, 420.0, 2.8),
            Paraceratherium => s("Paraceratherium", Oligocene, Size::Huge, 5000.0, 135.0, 4.0, 480.0, 3.2),
            WoollyMammoth => s("Woolly Mammoth", Pleistocene, Size::Colossal, 7500.0, 180.0, 3.6, 700.0, 4.0),
            SeaCow => s("Steller's Sea Cow", Holocene, Size::Huge, 3800.0, 70.0, 2.2, 380.0, 2.0),

            // Medium.
            TerrorBird => s("Terror Bird", Miocene, Size::Medium, 620.0, 70.0, 9.5, 90.0, 1.1),
            Tapir => s("Tapir", Holocene, Size::Medium, 520.0, 34.0, 6.5, 60.0, 0.9),
            DireWolf => s("Dire Wolf", Pleistocene, Size::Medium, 480.0, 52.0, 9.0, 72.0, 1.0),
            Smilodon => s("Smilodon", Pleistocene, Size::Medium, 700.0, 84.0, 8.5, 110.0, 1.2),

            // Flankers.
            Capybara => s("Capybara", Holocene, Size::Small, 180.0, 14.0, 6.0, 26.0, 0.6),
            Thylacine => s("Thylacine", Holocene, Size::Small, 220.0, 30.0, 8.5, 38.0, 0.7),
            GiantBeaver => s("Castoroides", Pleistocene, Size::Small, 340.0, 26.0, 5.5, 44.0, 0.7),
            Peccary => s("Peccary", Holocene, Size::Small, 200.0, 24.0, 7.0, 30.0, 0.6),
        }
    }

    pub fn name(self) -> &'static str {
        self.stats().name
    }
    pub fn size(self) -> Size {
        self.stats().size
    }
    pub fn build(self) -> Build {
        // Smilodon is a cat and must move like one, whatever its size class
        // says — this is the one place the size->build mapping is overridden,
        // because the gods are felines and a sabre cat sharing their gait is
        // the point of putting it on the map.
        match self {
            Beast::Smilodon => Build::feline(),
            Beast::TerrorBird => Build::hare(), // bipedal, light, springy
            _ => self.size().build(),
        }
    }
}

/// What killing a camp grants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Boon {
    /// Flat move speed, world units/sec.
    Swiftness(f32),
    /// Multiplier on outgoing damage.
    Ferocity(f32),
    /// Health per second.
    Mending(f32),
    /// Multiplier on gold earned.
    Fortune(f32),
    /// Flat damage reduction.
    Hide(f32),
    /// No buff — the camp is worth gold only.
    None,
}

impl Boon {
    pub fn label(self) -> &'static str {
        match self {
            Boon::Swiftness(_) => "SWIFTNESS",
            Boon::Ferocity(_) => "FEROCITY",
            Boon::Mending(_) => "MENDING",
            Boon::Fortune(_) => "FORTUNE",
            Boon::Hide(_) => "THICK HIDE",
            Boon::None => "",
        }
    }
}

/// What a camp is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// An ordinary jungle camp. One player can clear it.
    Camp,
    /// Contested, team-scale. `Huge`/`Colossal` leaders only.
    Objective,
}

/// A camp: one leader, some flankers, a boon.
///
/// This is the unit the AI backend generates. Every field it can set is here,
/// and [`Self::validate`] is the contract — a generated camp that fails it is
/// rejected rather than loaded, so a bad generation is a visible error rather
/// than an unplayable map.
#[derive(Debug, Clone, PartialEq)]
pub struct CampSpec {
    pub name: &'static str,
    pub leader: Beast,
    /// `(species, count)`. Flankers aggro with the leader.
    pub flankers: Vec<(Beast, u32)>,
    pub boon: Boon,
    pub boon_secs: f32,
    pub tier: Tier,
    /// Seconds from match start to first spawn.
    pub first_spawn: f32,
    pub respawn: f32,
    /// Position on the map, world units. `None` = the generator places it.
    pub at: Option<(f32, f32)>,
}

/// Why a camp spec was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CampError {
    /// The leader is too small to lead.
    LeaderTooSmall(Beast),
    /// A flanker is too big to flank — it would upstage the leader.
    FlankerTooLarge(Beast),
    /// An objective needs a Huge or Colossal leader.
    ObjectiveLeaderTooSmall(Beast),
    /// A Huge/Colossal creature in an ordinary camp.
    CampLeaderTooLarge(Beast),
    /// No flankers. The camp shape IS leader-plus-flankers.
    NoFlankers,
    /// More flankers than a player can reasonably track.
    TooManyFlankers(u32),
    /// A respawn of zero would re-spawn the camp every tick.
    BadRespawn(f32),
    EmptyName,
}

/// The most flankers a camp may field.
///
/// Eight is where a camp stops being a fight with a shape and becomes a
/// swarm: past that the player cannot tell which creature is hitting them,
/// and the leader — the thing the camp is supposed to be *about* — is hidden
/// behind bodies.
pub const MAX_FLANKERS: u32 = 8;

impl CampSpec {
    pub fn flanker_count(&self) -> u32 {
        self.flankers.iter().map(|(_, n)| *n).sum()
    }

    /// Total gold for a full clear.
    pub fn total_gold(&self) -> f32 {
        self.leader.stats().gold
            + self.flankers.iter().map(|(b, n)| b.stats().gold * *n as f32).sum::<f32>()
    }

    /// Total hp for a full clear — the honest measure of how long a camp
    /// takes, as opposed to the leader's bar.
    pub fn total_hp(&self) -> f32 {
        self.leader.stats().hp
            + self.flankers.iter().map(|(b, n)| b.stats().hp * *n as f32).sum::<f32>()
    }

    /// Check the camp is playable. Run on hand-written and generated camps
    /// alike.
    pub fn validate(&self) -> Result<(), CampError> {
        if self.name.trim().is_empty() {
            return Err(CampError::EmptyName);
        }
        let lsize = self.leader.size();
        if !lsize.can_lead() {
            return Err(CampError::LeaderTooSmall(self.leader));
        }
        match self.tier {
            Tier::Objective if lsize < Size::Huge => {
                return Err(CampError::ObjectiveLeaderTooSmall(self.leader))
            }
            Tier::Camp if lsize >= Size::Huge => {
                return Err(CampError::CampLeaderTooLarge(self.leader))
            }
            _ => {}
        }
        for (b, _) in &self.flankers {
            if !b.size().can_flank() {
                return Err(CampError::FlankerTooLarge(*b));
            }
        }
        let n = self.flanker_count();
        if n == 0 {
            return Err(CampError::NoFlankers);
        }
        if n > MAX_FLANKERS {
            return Err(CampError::TooManyFlankers(n));
        }
        if self.respawn <= 1.0 {
            return Err(CampError::BadRespawn(self.respawn));
        }
        Ok(())
    }
}

/// The stock roster.
///
/// Positions are left `None` here; the map decides where camps sit, which is
/// what lets a generated map reuse these definitions with its own layout.
pub fn stock_camps() -> Vec<CampSpec> {
    use Beast::*;
    vec![
        CampSpec {
            name: "THE WALLOW",
            leader: SteppeBison,
            // Peccaries under a bison: the flankers are chip damage you
            // cannot ignore while the leader is the one that actually hurts.
            flankers: vec![(Peccary, 3)],
            boon: Boon::Ferocity(1.12),
            boon_secs: 90.0,
            tier: Tier::Camp,
            first_spawn: 75.0,
            respawn: 100.0,
            at: None,
        },
        CampSpec {
            name: "THE THORN HERD",
            leader: Aurochs,
            flankers: vec![(Capybara, 2), (Peccary, 2)],
            boon: Boon::Swiftness(0.8),
            boon_secs: 75.0,
            tier: Tier::Camp,
            first_spawn: 75.0,
            respawn: 100.0,
            at: None,
        },
        CampSpec {
            name: "THE FROST SCAR",
            leader: WoollyRhino,
            flankers: vec![(Thylacine, 2)],
            boon: Boon::Ferocity(1.20),
            boon_secs: 90.0,
            tier: Tier::Camp,
            first_spawn: 105.0,
            respawn: 130.0,
            at: None,
        },
        CampSpec {
            name: "THE ANTLER COURT",
            leader: IrishElk,
            flankers: vec![(Capybara, 3)],
            boon: Boon::Swiftness(1.2),
            boon_secs: 75.0,
            tier: Tier::Camp,
            first_spawn: 75.0,
            respawn: 100.0,
            at: None,
        },
        CampSpec {
            name: "THE SHELLED KEEP",
            leader: Glyptodon,
            // A slow tank guarded by fast wolves: you cannot kite the escort
            // and burn the leader at the same time.
            flankers: vec![(DireWolf, 2)],
            boon: Boon::Hide(9.0),
            boon_secs: 110.0,
            tier: Tier::Camp,
            first_spawn: 110.0,
            respawn: 140.0,
            at: None,
        },
        CampSpec {
            name: "THE RIVER FOLD",
            leader: Tapir,
            flankers: vec![(GiantBeaver, 2), (Capybara, 1)],
            boon: Boon::Mending(11.0),
            boon_secs: 60.0,
            tier: Tier::Camp,
            first_spawn: 60.0,
            respawn: 90.0,
            at: None,
        },
        CampSpec {
            name: "THE SHRIEKING GROVE",
            leader: TerrorBird,
            flankers: vec![(Thylacine, 3)],
            boon: Boon::Fortune(1.15),
            boon_secs: 90.0,
            tier: Tier::Camp,
            first_spawn: 90.0,
            respawn: 115.0,
            at: None,
        },
        CampSpec {
            name: "THE SABRE DEN",
            leader: Smilodon,
            flankers: vec![(Thylacine, 2), (Peccary, 1)],
            boon: Boon::Ferocity(1.18),
            boon_secs: 85.0,
            tier: Tier::Camp,
            first_spawn: 120.0,
            respawn: 140.0,
            at: None,
        },
        CampSpec {
            name: "THE TUNDRA WALL",
            leader: MuskOx,
            flankers: vec![(Peccary, 2), (GiantBeaver, 1)],
            boon: Boon::Hide(7.0),
            boon_secs: 90.0,
            tier: Tier::Camp,
            first_spawn: 75.0,
            respawn: 100.0,
            at: None,
        },
        // ── objectives ───────────────────────────────────────────────────
        CampSpec {
            name: "THE SLOW GIANT",
            leader: GroundSloth,
            flankers: vec![(GiantBeaver, 3)],
            boon: Boon::Ferocity(1.35),
            boon_secs: 150.0,
            tier: Tier::Objective,
            first_spawn: 300.0,
            respawn: 300.0,
            at: None,
        },
        CampSpec {
            name: "THE SKYSHOULDER",
            leader: Paraceratherium,
            flankers: vec![(Capybara, 4)],
            boon: Boon::Fortune(1.40),
            boon_secs: 150.0,
            tier: Tier::Objective,
            first_spawn: 360.0,
            respawn: 330.0,
            at: None,
        },
        CampSpec {
            name: "THE LAST TUSK",
            leader: WoollyMammoth,
            flankers: vec![(DireWolf, 2), (Thylacine, 2)],
            boon: Boon::Ferocity(1.50),
            boon_secs: 180.0,
            tier: Tier::Objective,
            first_spawn: 480.0,
            respawn: 420.0,
            at: None,
        },
        CampSpec {
            name: "THE DROWNED PASTURE",
            leader: SeaCow,
            flankers: vec![(GiantBeaver, 2), (Capybara, 2)],
            boon: Boon::Mending(26.0),
            boon_secs: 140.0,
            tier: Tier::Objective,
            first_spawn: 270.0,
            respawn: 280.0,
            at: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roster ships playable. If this fails, the game has a camp nobody
    /// can clear.
    #[test]
    fn every_stock_camp_validates() {
        for c in stock_camps() {
            assert_eq!(c.validate(), Ok(()), "{} is not playable", c.name);
        }
    }

    /// The design constraint the module exists for: a camp is a leader
    /// FLANKED by smaller creatures.
    #[test]
    fn every_camp_has_a_leader_strictly_larger_than_its_flankers() {
        for c in stock_camps() {
            let ls = c.leader.size();
            for (f, n) in &c.flankers {
                assert!(*n > 0, "{}: zero count for {:?}", c.name, f);
                assert!(
                    f.size() < ls,
                    "{}: flanker {} ({:?}) is not smaller than leader {} ({:?})",
                    c.name,
                    f.name(),
                    f.size(),
                    c.leader.name(),
                    ls
                );
            }
        }
    }

    /// Eras must actually span, or the roster has quietly collapsed back to
    /// one period.
    #[test]
    fn the_roster_spans_more_than_one_era() {
        let mut eras: Vec<Era> = Vec::new();
        for c in stock_camps() {
            let e = c.leader.stats().era;
            if !eras.contains(&e) {
                eras.push(e);
            }
        }
        assert!(eras.len() >= 3, "only {} eras represented: {eras:?}", eras.len());
    }

    /// Nothing tiny, per the module header.
    #[test]
    fn no_creature_is_below_the_size_floor() {
        for c in stock_camps() {
            for (f, _) in &c.flankers {
                assert!(f.size() >= Size::Small);
                assert!(f.stats().hp >= 150.0, "{} is too flimsy to be a flanker", f.name());
            }
        }
    }

    /// Objectives are the only place Huge/Colossal appears, both ways round.
    #[test]
    fn objective_scale_is_reserved_for_objectives() {
        for c in stock_camps() {
            match c.tier {
                Tier::Objective => assert!(c.leader.size() >= Size::Huge, "{}", c.name),
                Tier::Camp => assert!(c.leader.size() < Size::Huge, "{}", c.name),
            }
        }
        // And the validator enforces it rather than relying on the roster.
        let bad = CampSpec {
            name: "X",
            leader: Beast::WoollyMammoth,
            flankers: vec![(Beast::Capybara, 1)],
            boon: Boon::None,
            boon_secs: 1.0,
            tier: Tier::Camp,
            first_spawn: 10.0,
            respawn: 10.0,
            at: None,
        };
        assert_eq!(bad.validate(), Err(CampError::CampLeaderTooLarge(Beast::WoollyMammoth)));
    }

    /// Objectives must be worth more than camps, or nobody contests them.
    #[test]
    fn objectives_pay_more_than_camps() {
        let camps = stock_camps();
        let worst_obj = camps
            .iter()
            .filter(|c| c.tier == Tier::Objective)
            .map(|c| c.total_gold())
            .fold(f32::MAX, f32::min);
        let best_camp = camps
            .iter()
            .filter(|c| c.tier == Tier::Camp)
            .map(|c| c.total_gold())
            .fold(0.0f32, f32::max);
        assert!(worst_obj > best_camp, "objective {worst_obj} vs camp {best_camp}");
    }

    /// Objectives must also not be available at the opening, or the early
    /// game is decided by who walked to one first.
    #[test]
    fn objectives_spawn_later_than_camps() {
        for c in stock_camps() {
            if c.tier == Tier::Objective {
                assert!(c.first_spawn >= 240.0, "{} opens too early", c.name);
            }
        }
    }

    // ── the validator is the AI backend's contract ────────────────────────

    fn base() -> CampSpec {
        CampSpec {
            name: "T",
            leader: Beast::SteppeBison,
            flankers: vec![(Beast::Peccary, 2)],
            boon: Boon::None,
            boon_secs: 10.0,
            tier: Tier::Camp,
            first_spawn: 60.0,
            respawn: 90.0,
            at: None,
        }
    }

    #[test]
    fn a_camp_with_no_flankers_is_rejected() {
        let mut c = base();
        c.flankers.clear();
        assert_eq!(c.validate(), Err(CampError::NoFlankers));
    }

    #[test]
    fn a_swarm_is_rejected() {
        let mut c = base();
        c.flankers = vec![(Beast::Peccary, MAX_FLANKERS + 1)];
        assert_eq!(c.validate(), Err(CampError::TooManyFlankers(MAX_FLANKERS + 1)));
        // Exactly at the cap is fine.
        c.flankers = vec![(Beast::Peccary, MAX_FLANKERS)];
        assert_eq!(c.validate(), Ok(()));
    }

    #[test]
    fn a_small_leader_is_rejected() {
        let mut c = base();
        c.leader = Beast::Capybara;
        assert_eq!(c.validate(), Err(CampError::LeaderTooSmall(Beast::Capybara)));
    }

    #[test]
    fn an_oversized_flanker_is_rejected() {
        let mut c = base();
        c.flankers = vec![(Beast::WoollyRhino, 1)];
        assert_eq!(c.validate(), Err(CampError::FlankerTooLarge(Beast::WoollyRhino)));
    }

    /// A zero respawn would re-spawn the camp every tick.
    #[test]
    fn a_zero_respawn_is_rejected() {
        let mut c = base();
        c.respawn = 0.0;
        assert_eq!(c.validate(), Err(CampError::BadRespawn(0.0)));
    }

    #[test]
    fn an_unnamed_camp_is_rejected() {
        let mut c = base();
        c.name = "   ";
        assert_eq!(c.validate(), Err(CampError::EmptyName));
    }

    /// Totals count the flankers, not just the leader's bar.
    #[test]
    fn totals_include_the_escort() {
        let c = base();
        let expect_gold =
            Beast::SteppeBison.stats().gold + Beast::Peccary.stats().gold * 2.0;
        assert!((c.total_gold() - expect_gold).abs() < 1e-3);
        assert!(c.total_hp() > Beast::SteppeBison.stats().hp);
    }

    /// A sabre cat must move like the gods, not like a bison.
    #[test]
    fn smilodon_uses_the_feline_gait() {
        assert_eq!(Beast::Smilodon.build(), Build::feline());
        assert_eq!(Beast::SteppeBison.build(), Build::megafauna());
        assert_eq!(Beast::WoollyMammoth.build(), Build::colossus());
    }

    /// Body radius must rise with size, since it doubles as the blocker
    /// footprint a living camp presents.
    #[test]
    fn radius_rises_with_size() {
        let sizes = [Size::Small, Size::Medium, Size::Large, Size::Huge, Size::Colossal];
        for w in sizes.windows(2) {
            assert!(w[0].radius() < w[1].radius(), "{:?} vs {:?}", w[0], w[1]);
        }
    }
}
