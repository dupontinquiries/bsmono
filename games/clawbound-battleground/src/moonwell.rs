//! moonwell — the healing water at the heart of Starwell Glacier, and what
//! it grants to the team that claims it.
//!
//! The Moonwell is shallow: a paw deep at the rim, a leg deep in the
//! middle. Anyone who wades in it mends. Coiled in it sleeps the Moonwyrm
//! (`bosses::BossKind::Moonwyrm`), a cat-dragon of starlight; the team that
//! brings it down CLAIMS the well for [`CLAIM_SECS`]. While the claim
//! holds, each of that team's cats that wades in is granted one [`Boon`],
//! drawn at random, for [`BOON_SECS`] -- once per cat per claim:
//!
//! * **Moonshell** -- damage taken x0.1 (the asked-for 90% reduction).
//! * **Quickening** -- cooldowns run at double speed (halved).
//! * **Starstride** -- +40% move speed.
//! * **Wyrmfang** -- +50% damage dealt.
//!
//! Strong on purpose: the well is meant to swing a fight, and the team
//! that took it has to walk to the middle of the map to cash it in, so the
//! other side knows where they will be.
//!
//! The rules are pure functions here; `Game::moonwell_tick` applies them.

use crate::sim::Rng;

/// A claim lasts this long after the Moonwyrm falls.
pub const CLAIM_SECS: f32 = 120.0;
/// A boon lasts this long ("2 minutes").
pub const BOON_SECS: f32 = 120.0;
/// Healing while wading, fraction of max health per second: everyone, and
/// the claiming team's cats while the claim holds.
pub const HEAL_PER_S: f32 = 0.03;
pub const CLAIMED_HEAL_PER_S: f32 = 0.06;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boon {
    Moonshell,
    Quickening,
    Starstride,
    Wyrmfang,
}

pub const BOONS: [Boon; 4] = [Boon::Moonshell, Boon::Quickening, Boon::Starstride, Boon::Wyrmfang];

impl Boon {
    pub fn name(self) -> &'static str {
        match self {
            Boon::Moonshell => "MOONSHELL",
            Boon::Quickening => "QUICKENING",
            Boon::Starstride => "STARSTRIDE",
            Boon::Wyrmfang => "WYRMFANG",
        }
    }
    pub fn what(self) -> &'static str {
        match self {
            Boon::Moonshell => "90% less damage taken",
            Boon::Quickening => "cooldowns halved",
            Boon::Starstride => "+40% move speed",
            Boon::Wyrmfang => "+50% damage dealt",
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Boon::Moonshell => [0.80, 0.88, 1.00],
            Boon::Quickening => [0.55, 1.00, 0.85],
            Boon::Starstride => [1.00, 0.92, 0.55],
            Boon::Wyrmfang => [1.00, 0.55, 0.75],
        }
    }
}

/// A random boon.
pub fn draw(rng: &mut Rng) -> Boon {
    BOONS[(rng.next_u() % BOONS.len() as u64) as usize]
}

/// Multiplier on damage a cat TAKES under `boon`.
pub fn taken_mul(boon: Option<Boon>) -> f32 {
    if boon == Some(Boon::Moonshell) { 0.1 } else { 1.0 }
}
/// Multiplier on damage a cat DEALS.
pub fn dealt_mul(boon: Option<Boon>) -> f32 {
    if boon == Some(Boon::Wyrmfang) { 1.5 } else { 1.0 }
}
/// How fast cooldowns run.
pub fn cooldown_rate(boon: Option<Boon>) -> f32 {
    if boon == Some(Boon::Quickening) { 2.0 } else { 1.0 }
}
/// Multiplier on move speed.
pub fn move_mul(boon: Option<Boon>) -> f32 {
    if boon == Some(Boon::Starstride) { 1.4 } else { 1.0 }
}

/// Does a cat wading in the well get a boon now? Its team holds a claim
/// (`claim_id` is that claim's number, 0 = none ever), and it has not had
/// one from THIS claim (`last_claim`), and it has none running.
pub fn grants(claim_left: f32, claim_id: u32, last_claim: u32, has_boon: bool) -> bool {
    claim_left > 0.0 && claim_id != 0 && last_claim != claim_id && !has_boon
}

impl crate::sim::Game {
    /// The well, each tick: wading cats mend; a claiming team's cats that
    /// wade in are granted a boon; boons run down. Nothing on other maps.
    pub fn moonwell_tick(&mut self, dt: f32) {
        for g in self.gods.iter_mut() {
            if let Some((b, left)) = g.boon {
                g.boon = if left - dt > 0.0 { Some((b, left - dt)) } else { None };
            }
        }
        if crate::map::active() != crate::map::MapId::Sky {
            return;
        }
        for i in 0..self.gods.len() {
            let g = &self.gods[i];
            if g.respawn_t > 0.0 || g.skyfall.is_some() || !crate::sky::in_pool(g.pos.x, g.pos.z) || g.leap.is_some() {
                continue;
            }
            let ti = g.team as usize;
            let claimed = self.bosses.claim_t[ti] > 0.0;
            let heal = if claimed { CLAIMED_HEAL_PER_S } else { HEAL_PER_S };
            let grant = grants(self.bosses.claim_t[ti], self.bosses.claim_id[ti], g.boon_claim, g.boon.is_some());
            let boon = if grant { Some(draw(&mut self.rng)) } else { None };
            let g = &mut self.gods[i];
            g.hp = (g.hp + g.max_hp * heal * dt).min(g.max_hp);
            if let Some(b) = boon {
                g.boon = Some((b, BOON_SECS));
                g.boon_claim = self.bosses.claim_id[ti];
                let pos = g.pos;
                let name = crate::gods::god(g.def_id).name;
                self.events.push(crate::sim::Event::NovaFx { pos, radius: 3.5, color: b.color() });
                self.events.push(crate::sim::Event::Announce { text: format!("{name}: {} -- {}", b.name(), b.what()) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boons_do_what_they_say() {
        assert!((taken_mul(Some(Boon::Moonshell)) - 0.1).abs() < 1e-6, "90% reduced damage");
        assert!((cooldown_rate(Some(Boon::Quickening)) - 2.0).abs() < 1e-6, "halved cooldowns");
        assert_eq!(taken_mul(Some(Boon::Wyrmfang)), 1.0);
        assert_eq!(taken_mul(None), 1.0);
        assert_eq!(dealt_mul(Some(Boon::Wyrmfang)), 1.5);
        assert_eq!(move_mul(Some(Boon::Starstride)), 1.4);
        assert_eq!(cooldown_rate(None), 1.0);
        assert_eq!(BOON_SECS, 120.0, "two minutes");
    }

    #[test]
    fn every_boon_comes_up() {
        let mut rng = Rng::new(3);
        let mut seen = [0u32; 4];
        for _ in 0..400 {
            let b = draw(&mut rng);
            seen[BOONS.iter().position(|&x| x == b).unwrap()] += 1;
        }
        assert!(seen.iter().all(|&n| (60..140).contains(&n)), "roughly even: {seen:?}");
    }

    #[test]
    fn once_per_cat_per_claim() {
        assert!(grants(100.0, 1, 0, false), "first wade in a claim");
        assert!(!grants(100.0, 1, 1, false), "already had this claim's");
        assert!(grants(100.0, 2, 1, false), "a new claim, a new boon");
        assert!(!grants(0.0, 1, 0, false), "the claim ran out");
        assert!(!grants(100.0, 1, 0, true), "one boon at a time");
        assert!(!grants(100.0, 0, 0, false), "no claim ever");
    }
}
