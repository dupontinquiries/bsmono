//! sim::dragon_territory — age-scaled core/influence territory for dragons.
//!
//! From `dragon_energy_lifestyle §3`: a dragon holds a **core** territory it
//! actively excludes others from, plus an outer **influence** zone where it
//! hunts and patrols but tolerates other species. Both radii scale with the
//! dragon's age (a juvenile's 150–300 blocks vs an elder's 2500). `sim::territory`
//! only models a single circular 0→1 intrusion ramp; this adds the two-zone,
//! age-graded structure and the social rules the doc calls out — juveniles defer
//! to adults, and overlapping adult territories mean conflict. Pure, std-only,
//! deterministic; distances are 2D (horizontal), matching the design.

/// Dragon life stage — sets territory size and social standing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DragonAge {
    Juvenile,
    Subadult,
    YoungAdult,
    Mature,
    Elder,
}

impl DragonAge {
    /// Core territory radius (blocks) — actively excluded. Representative midpoints
    /// of the doc's ranges.
    pub fn core_radius(self) -> f32 {
        match self {
            DragonAge::Juvenile => 225.0,
            DragonAge::Subadult => 400.0,
            DragonAge::YoungAdult => 650.0,
            DragonAge::Mature => 1150.0,
            DragonAge::Elder => 2500.0,
        }
    }

    /// Extra band beyond the core forming the influence zone (hunts/patrols,
    /// doesn't exclude).
    pub fn influence_band(self) -> f32 {
        match self {
            DragonAge::Juvenile => 75.0,
            DragonAge::Subadult => 100.0,
            DragonAge::YoungAdult => 200.0,
            DragonAge::Mature => 500.0,
            DragonAge::Elder => 800.0,
        }
    }

    /// Outer influence radius = core + band.
    pub fn influence_radius(self) -> f32 {
        self.core_radius() + self.influence_band()
    }

    /// Adults hold and contest full territories (young adult and up). Juveniles
    /// and subadults are still establishing.
    pub fn is_adult(self) -> bool {
        matches!(self, DragonAge::YoungAdult | DragonAge::Mature | DragonAge::Elder)
    }
}

/// Which zone a point falls in relative to a dragon's den.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Inside the core — actively defended.
    Core,
    /// In the influence band — tolerated but watched.
    Influence,
    /// Beyond the territory entirely.
    Outside,
}

fn dist2d(a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1]];
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

/// Squared horizontal distance — for range comparisons that don't need the
/// actual distance value, so the `sqrt` in [`dist2d`] can be skipped
/// (`d ≤ r ⟺ d² ≤ r²` for non-negative `d`, `r`).
fn dist2d_sq(a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1]];
    d[0] * d[0] + d[1] * d[1]
}

/// Classify a point relative to a dragon's `den` for its `age`.
pub fn zone(age: DragonAge, den: [f32; 2], pos: [f32; 2]) -> Zone {
    // Compare squared distances against squared radii — identical result, no sqrt.
    let d2 = dist2d_sq(den, pos);
    let core = age.core_radius();
    let infl = age.influence_radius();
    if d2 <= core * core {
        Zone::Core
    } else if d2 <= infl * infl {
        Zone::Influence
    } else {
        Zone::Outside
    }
}

/// Does the dragon actively defend (exclude) at this point? Only within the core.
pub fn defends(age: DragonAge, den: [f32; 2], pos: [f32; 2]) -> bool {
    zone(age, den, pos) == Zone::Core
}

/// Intrusion severity in `[0, 1]`: `0` at/beyond the core edge, rising linearly
/// to `1` at the den — how hard to respond to a trespasser inside the core.
/// (The influence band is not an intrusion; it returns 0.)
pub fn intrusion(age: DragonAge, den: [f32; 2], pos: [f32; 2]) -> f32 {
    let r = age.core_radius();
    if r <= 0.0 {
        return 0.0;
    }
    let d = dist2d(den, pos);
    if d >= r {
        0.0
    } else {
        1.0 - d / r
    }
}

/// Does a dragon of `self_age` defer (retreat rather than contest) to a dragon
/// of `other_age`? Per the doc, a juvenile defers to any adult; otherwise
/// standing is contested.
pub fn defers_to(self_age: DragonAge, other_age: DragonAge) -> bool {
    self_age == DragonAge::Juvenile && other_age.is_adult()
}

/// Do two dragons' territories conflict? Their influence zones overlap **and**
/// neither simply defers. A juvenile beside an adult doesn't "conflict" — it
/// yields; two adults (or two subadults) whose zones overlap do.
pub fn territories_conflict(a_age: DragonAge, a_den: [f32; 2], b_age: DragonAge, b_den: [f32; 2]) -> bool {
    let overlap = {
        // squared-distance comparison — no sqrt needed for a threshold test
        let sum = a_age.influence_radius() + b_age.influence_radius();
        dist2d_sq(a_den, b_den) < sum * sum
    };
    if !overlap {
        return false;
    }
    // If either yields to the other, it's resolved by deference, not conflict.
    !(defers_to(a_age, b_age) || defers_to(b_age, a_age))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn territory_grows_with_age() {
        let ages = [
            DragonAge::Juvenile,
            DragonAge::Subadult,
            DragonAge::YoungAdult,
            DragonAge::Mature,
            DragonAge::Elder,
        ];
        for w in ages.windows(2) {
            assert!(w[0].core_radius() < w[1].core_radius(), "{:?} core < {:?}", w[0], w[1]);
            assert!(w[0].influence_radius() < w[1].influence_radius());
        }
        // influence always beyond core
        for a in ages {
            assert!(a.influence_radius() > a.core_radius());
        }
    }

    #[test]
    fn zones_classify_by_distance() {
        let age = DragonAge::YoungAdult; // core 650, influence 850
        let den = [0.0, 0.0];
        assert_eq!(zone(age, den, [100.0, 0.0]), Zone::Core);
        assert_eq!(zone(age, den, [700.0, 0.0]), Zone::Influence);
        assert_eq!(zone(age, den, [1000.0, 0.0]), Zone::Outside);
    }

    #[test]
    fn defends_only_inside_the_core() {
        let age = DragonAge::Mature; // core 1150
        let den = [0.0, 0.0];
        assert!(defends(age, den, [500.0, 0.0]));
        assert!(!defends(age, den, [1300.0, 0.0]), "influence band is not actively excluded");
    }

    #[test]
    fn intrusion_ramps_from_core_edge_to_den() {
        let age = DragonAge::Subadult; // core 400
        let den = [0.0, 0.0];
        assert!((intrusion(age, den, [0.0, 0.0]) - 1.0).abs() < 1e-4, "max at the den");
        assert!((intrusion(age, den, [200.0, 0.0]) - 0.5).abs() < 1e-4, "halfway in");
        assert_eq!(intrusion(age, den, [400.0, 0.0]), 0.0, "0 at the core edge");
        assert_eq!(intrusion(age, den, [700.0, 0.0]), 0.0, "influence band ⇒ no intrusion");
    }

    #[test]
    fn a_juvenile_defers_to_adults_but_not_vice_versa() {
        assert!(defers_to(DragonAge::Juvenile, DragonAge::Mature));
        assert!(defers_to(DragonAge::Juvenile, DragonAge::YoungAdult));
        assert!(!defers_to(DragonAge::Juvenile, DragonAge::Subadult), "subadult isn't an adult");
        assert!(!defers_to(DragonAge::Mature, DragonAge::Juvenile));
    }

    #[test]
    fn overlapping_adult_territories_conflict() {
        // two matures 1000 apart: influence 1650 each ⇒ overlap ⇒ conflict
        assert!(territories_conflict(DragonAge::Mature, [0.0, 0.0], DragonAge::Mature, [1000.0, 0.0]));
    }

    #[test]
    fn distant_territories_do_not_conflict() {
        assert!(!territories_conflict(DragonAge::Mature, [0.0, 0.0], DragonAge::Mature, [5000.0, 0.0]));
    }

    #[test]
    fn a_juvenile_yields_instead_of_conflicting() {
        // juvenile right next to a mature: overlap, but the juvenile defers
        assert!(!territories_conflict(DragonAge::Juvenile, [0.0, 0.0], DragonAge::Mature, [100.0, 0.0]));
    }

    #[test]
    fn classification_is_deterministic() {
        let age = DragonAge::Elder;
        assert_eq!(zone(age, [0.0, 0.0], [10.0, 10.0]), zone(age, [0.0, 0.0], [10.0, 10.0]));
        assert_eq!(intrusion(age, [0.0, 0.0], [10.0, 10.0]), intrusion(age, [0.0, 0.0], [10.0, 10.0]));
    }
}
