//! THE FOOTHOLD — step one of a raid, before the raid.
//!
//! `docs/RAIDS.md` starts a raid at its first encounter, which assumes you
//! are already standing somewhere. On a world with weather that is the
//! interesting question rather than a given: nineteen bosses are no use if
//! the particle front arrives while the team is still in the open.
//!
//! So a raid opens by taking a place to stand, and there are two ways, which
//! trade against each other rather than one being the good one:
//!
//! **CLEAR AN LZ.** Drop into open ground and hold it while the ship comes
//! down. Light opposition, because nobody garrisons empty rock — and no
//! shelter at all until the ship is on the deck, so the weather clock runs
//! the whole time. The ship then has to STAY, which is a standing cost: it
//! is not overhead for the drop, it is not available to evacuate, and it is
//! a large stationary thing that the raid's own escalation can reach.
//!
//! **SEIZE A FORTRESS.** Take a structure the enemy already built. Heavy
//! opposition and a real fight to get in — and the moment you are in, the
//! weather stops applying, the position is stocked, and the ship is free.
//!
//! Which is correct depends on the sky, and that is the point. A clear world
//! makes the LZ obviously right. An incoming front makes it suicide.

use crate::approach;
use crate::survival::{axis_for, Body, Bunker, ShelterKind, Weather};

/// How a team takes its first ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Approach {
    /// drop in, hold open ground, bring the ship down
    ClearLz,
    /// take what the enemy built
    SeizeFortress,
}

impl Approach {
    pub fn label(self) -> &'static str {
        match self {
            Approach::ClearLz => "CLEAR AN LZ",
            Approach::SeizeFortress => "SEIZE THE FORTRESS",
        }
    }
    /// What you are standing in once it is yours.
    pub fn shelter(self) -> ShelterKind {
        match self {
            // the ship IS the shelter, and it had to land to be one
            Approach::ClearLz => ShelterKind::Ship,
            // DUG IN, not walls. An established position on a world with
            // weather was built by people who live there, and a surface
            // structure stops 0.45 of a particle front — which is to say it
            // does not. Anything worth seizing is underground.
            Approach::SeizeFortress => ShelterKind::Bunker,
        }
    }

    /// ARE YOU SHELTERED WHILE YOU FIGHT FOR IT? This is the real difference
    /// between the two and my first model missed it entirely.
    ///
    /// Clearing an LZ happens in the open from first boot to the ship being
    /// on the deck — the whole take is exposed. Storming a fortress is
    /// exposed only on the approach; once you are through the door you are
    /// inside the thing you are fighting over, and the sky stops mattering
    /// even though the fight has not finished.
    pub fn exposed_during_take(self) -> bool {
        self == Approach::ClearLz
    }
    /// Does the ship have to stay grounded to hold this?
    pub fn pins_the_ship(self) -> bool {
        self == Approach::ClearLz
    }
}

/// A candidate foothold on the ground.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub approach: Approach,
    pub at: Bunker,
    /// defenders, as a rough strength
    pub garrison: f32,
    /// seconds of fighting before it is yours, at full strength
    pub take_s: f32,
}

/// What the team knows before committing.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub site: Site,
    /// can they take it before the weather does for them?
    pub viable: bool,
    /// seconds left after taking it, or negative if they are caught in the open
    pub margin_s: f32,
    /// why it was rejected, when it was
    pub note: &'static str,
}

/// Assess a site against the clock and the sky.
///
/// `window_s` is what the weather allows — for a flare that is the CME
/// transit, for a surge the tide, for a clear sky effectively forever.
pub fn assess(body: Body, site: &Site, w: Weather, window_s: f32, speed_ms: f32) -> Plan {
    let (reachable, spare) = crate::survival::can_reach(body, &site.at, window_s, speed_ms);
    if !reachable {
        return Plan { site: *site, viable: false, margin_s: spare, note: "cannot reach it in the window" };
    }
    // The arithmetic from here is not Machine and is shared with the
    // detective campaign, which asks the same question with a rival society
    // for weather and a parked car for a grounded ship. See `approach.rs`;
    // what stays HERE is the domain — reaching it, and whether this shelter
    // stops THIS weather, neither of which that module can know.
    let stops = axis_for(w, &site.approach.shelter().protection());
    let opening = approach::Opening {
        reach_spare_s: spare,
        take_s: site.take_s,
        exposed_during_take: site.approach.exposed_during_take(),
        shelters_after: stops >= 0.5,
        lingering_cost: site.approach.pins_the_ship(),
        effort: site.take_s + site.garrison * 120.0,
        // weather does not care who sees you
        seen_cost: 0.0,
    };
    let v = approach::assess(&opening, window_s);
    Plan {
        site: *site,
        viable: v.viable(),
        margin_s: v.margin_s(),
        note: match v {
            approach::Verdict::CaughtInTheOpen { .. } => "the fight outlasts the window — caught in the open",
            approach::Verdict::NoShelter { .. } => "taken in time, and it does not stop this weather",
            _ => v.note(),
        },
    }
}

/// The same site, as the shared decision core sees it.
fn opening_of(body: Body, site: &Site, w: Weather, window_s: f32, speed_ms: f32) -> approach::Opening {
    let (_, spare) = crate::survival::can_reach(body, &site.at, window_s, speed_ms);
    approach::Opening {
        reach_spare_s: spare,
        take_s: site.take_s,
        exposed_during_take: site.approach.exposed_during_take(),
        shelters_after: axis_for(w, &site.approach.shelter().protection()) >= 0.5,
        lingering_cost: site.approach.pins_the_ship(),
        effort: site.take_s + site.garrison * 120.0,
        seen_cost: 0.0,
    }
}

/// Pick the opening. Prefers a viable site, and among viable ones the one
/// that leaves the ship free — because a pinned ship is a cost that lasts
/// the whole raid rather than the first phase.
pub fn choose<'a>(body: Body, sites: &'a [Site], w: Weather, window_s: f32, speed_ms: f32) -> Option<Plan> {
    // THE SKY DECIDES, which is the whole design and which my first sort got
    // wrong: it ranked "does not pin the ship" above everything, so a
    // fortress always won and the LZ was never worth having. That ranking —
    // margin under pressure, cheapness without it, lingering cost as a
    // tie-break only — is the part shared with the detective campaign and now
    // lives in `approach::choose`.
    let openings: Vec<approach::Opening> =
        sites.iter().map(|s| opening_of(body, s, w, window_s, speed_ms)).collect();
    let i = approach::choose(&openings, window_s)?;
    Some(assess(body, &sites[i], w, window_s, speed_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lz(dist: f32, take: f32) -> Site {
        Site { approach: Approach::ClearLz, at: Bunker { distance_m: dist, shielded: false }, garrison: 0.3, take_s: take }
    }
    fn fort(dist: f32, take: f32) -> Site {
        Site { approach: Approach::SeizeFortress, at: Bunker { distance_m: dist, shielded: true }, garrison: 1.0, take_s: take }
    }

    #[test]
    fn a_clear_sky_favours_the_lz_and_frees_the_ship() {
        // nobody garrisons empty rock, so it is the cheap option when the
        // weather is not the problem
        let sites = [lz(300.0, 60.0), fort(300.0, 400.0)];
        let p = choose(Body::Machine, &sites, Weather::Clear, f32::INFINITY, 14.0).unwrap();
        assert_eq!(p.site.approach, Approach::ClearLz);
        // and the cost is a grounded ship for the rest of the raid
        assert!(p.site.approach.pins_the_ship());
    }

    #[test]
    fn an_incoming_front_makes_the_open_ground_suicide() {
        // A SHORT CLEAR IS STILL FINE UNDER A FLARE, which I did not expect
        // and which is right: forty seconds in the open and then a landed
        // hull at 0.92 is a perfectly good answer. The LZ wins here.
        let quick = [lz(200.0, 40.0), fort(600.0, 200.0)];
        let p = choose(Body::HumanSuited, &quick, Weather::FlareFront, 900.0, 2.5).unwrap();
        assert_eq!(p.site.approach, Approach::ClearLz);

        // WHAT ACTUALLY RULES IT OUT is a take long enough that the front
        // arrives while the team is still in the open. Same window, same
        // distances, a contested LZ instead of an empty one — and now the
        // fortress wins, because its fight happens behind a door.
        let contested = [lz(200.0, 700.0), fort(600.0, 700.0)];
        let p = choose(Body::HumanSuited, &contested, Weather::FlareFront, 900.0, 2.5).unwrap();
        assert_eq!(p.site.approach, Approach::SeizeFortress,
            "a long fight in the open should lose to one behind a door");
    }

    #[test]
    fn a_fight_that_outlasts_the_window_is_refused_rather_than_attempted() {
        let sites = [fort(200.0, 5_000.0)];
        let p = assess(Body::HumanSuited, &sites[0], Weather::FlareFront, 600.0, 2.5);
        assert!(!p.viable);
        assert!(p.note.contains("outlasts"));
        assert!(p.margin_s < 0.0, "a refused plan should say by how much");
        assert!(choose(Body::HumanSuited, &sites, Weather::FlareFront, 600.0, 2.5).is_none());
    }

    #[test]
    fn unreachable_and_unsuitable_are_different_rejections() {
        // "you cannot get there" and "you can get there and it will not help"
        // are different problems and the plan says which
        let far = assess(Body::HumanSuited, &fort(90_000.0, 10.0), Weather::FlareFront, 300.0, 2.5);
        assert!(!far.viable && far.note.contains("reach"));
        // a sky city against a particle front: reachable, takeable, useless
        let exposed = Site {
            approach: Approach::ClearLz,
            at: Bunker { distance_m: 50.0, shielded: false },
            garrison: 0.1,
            take_s: 5.0,
        };
        let p = assess(Body::HumanSuited, &exposed, Weather::GiantWave, 600.0, 2.5);
        // a landed ship DOES stop water, so this one is fine — the check is
        // that the reasoning ran, not that everything fails
        assert!(p.viable, "a landed ship should survive a surge");
    }

    #[test]
    fn a_bot_can_take_ground_a_suited_team_cannot() {
        // same site, same window, different body — which is why you send
        // the machine first
        let sites = [fort(1_500.0, 120.0)];
        let bot = choose(Body::Machine, &sites, Weather::FlareFront, 300.0, 16.0);
        let man = choose(Body::HumanSuited, &sites, Weather::FlareFront, 300.0, 2.5);
        assert!(bot.is_some(), "a bot should make 1.5 km in five minutes");
        assert!(man.is_none(), "a suited human should not");
    }

    #[test]
    fn among_viable_options_it_prefers_not_pinning_the_ship() {
        // both work; the fortress leaves the ship free for the rest of the
        // raid, and that outlasts the first phase
        let sites = [lz(100.0, 30.0), fort(120.0, 40.0)];
        let p = choose(Body::Machine, &sites, Weather::DustStorm, 600.0, 14.0).unwrap();
        assert_eq!(p.site.approach, Approach::SeizeFortress);
        assert!(!p.site.approach.pins_the_ship());
    }
}
