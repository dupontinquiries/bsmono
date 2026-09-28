//! The bending fields underground (`cave_biomes.docx`, each biome's PROPERTY
//! SIGNATURE). "Underground spaces have dramatically modified H, R, W, P, L,
//! S, and B values compared to the surface. A water bender in a limestone
//! cavern is at near-peak power (H 0.90). A fire bender in a CO₂ basin cannot
//! maintain a flame. An information bender in any cave is operating
//! near-blind."
//!
//! `sim::bending_field` samples the fields from climate and weather, which say
//! little about a cave. This gives each cave zone the docx's signature and
//! blends it in by how closed-in the player is (`world::cave_climate::
//! enclosure`), the same blend the cave's air temperature uses.
//!
//! Zones map to docx biomes: Limestone -> Twilight Limestone Caverns, Crystal
//! -> Crystal Growth Chambers, Abyssal -> Abyssal Chasm Systems, Geothermal ->
//! Geothermal Deep World. The docx's river networks and steam caverns have no
//! zone yet; CO₂ basins are handled by `sim::cave_air::gas_fields`.
//!
//! Pressure: the docx gives P in atmospheres (1 at the surface, 5-50 in the
//! geothermal deep); the engine's P field is 0..1 with the surface at 0.5. The
//! mapping here is `0.5 + 0.25 * log2(atm)`, clamped: 1 atm -> 0.5, 2 -> 0.75,
//! 4 and more -> 1.0.
use crate::sim::affinity_access::Fields;
use crate::world::underground::Zone;

/// Engine P field for a pressure in atmospheres.
pub fn p_field(atm: f32) -> f32 {
    (0.5 + 0.25 * atm.max(1e-3).log2()).clamp(0.0, 1.0)
}

/// The docx signature for a zone as (low, high) per channel, in `Fields`
/// order h, r, w, p, l, s, b (P already mapped to the engine's scale).
pub fn signature(zone: Zone) -> Option<[(f32, f32); 7]> {
    let p = |lo: f32, hi: f32| (p_field(lo), p_field(hi));
    Some(match zone {
        Zone::Soil => return None,
        //                h             r             w             p            l             s             b
        Zone::Limestone => [(0.85, 0.95), (0.05, 0.12), (0.00, 0.05), p(0.95, 1.05), (0.00, 0.05), (0.50, 0.75), (0.10, 0.35)],
        Zone::Crystal => [(0.45, 0.70), (0.20, 0.45), (0.00, 0.05), p(1.0, 3.0), (0.05, 0.20), (0.65, 0.90), (0.05, 0.15)],
        Zone::Abyssal => [(0.20, 0.50), (0.10, 0.30), (0.10, 0.35), p(1.0, 5.0), (0.00, 0.02), (0.70, 0.95), (0.01, 0.08)],
        Zone::Geothermal => [(0.70, 0.95), (0.85, 1.00), (0.15, 0.40), p(5.0, 50.0), (0.02, 0.10), (0.90, 1.00), (0.05, 0.15)],
    })
}

/// A zone's fields at `t01` within its signature (the game passes a slow
/// map noise, so two limestone caves are not identical).
pub fn cave_fields(zone: Zone, t01: f32) -> Option<Fields> {
    let s = signature(zone)?;
    let t = t01.clamp(0.0, 1.0);
    let v = |i: usize| s[i].0 + (s[i].1 - s[i].0) * t;
    Some(Fields { h: v(0), r: v(1), w: v(2), p: v(3), l: v(4), s: v(5), b: v(6) })
}

/// The fields a player draws on: the surface sample blended into the cave's
/// by `enclosure` (0 = open sky, 1 = deep and roofed).
pub fn blend(surface: Fields, cave: Option<Fields>, enclosure: f32) -> Fields {
    let Some(c) = cave else { return surface };
    let k = enclosure.clamp(0.0, 1.0);
    // Written so the ends are exact: k = 0 is the surface, k = 1 the cave.
    let m = |a: f32, b: f32| a * (1.0 - k) + b * k;
    Fields {
        h: m(surface.h, c.h),
        r: m(surface.r, c.r),
        w: m(surface.w, c.w),
        p: m(surface.p, c.p),
        l: m(surface.l, c.l),
        s: m(surface.s, c.s),
        b: m(surface.b, c.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::affinity_access::{access, THRESHOLDS};

    // affinity_access channel order: h, r, w, p, l, s, b.
    const WATER: usize = 0;
    const FIRE: usize = 1;
    const WIND: usize = 2;
    const LIGHT: usize = 4;
    const EARTH: usize = 5;

    fn unlocked(f: &Fields, ch: usize) -> bool {
        access(f).disciplines & (1 << ch) != 0
    }

    #[test]
    fn pressure_maps_one_atmosphere_to_the_surface_value() {
        assert!((p_field(1.0) - 0.5).abs() < 1e-6);
        assert!((p_field(2.0) - 0.75).abs() < 1e-6);
        assert_eq!(p_field(4.0), 1.0);
        assert_eq!(p_field(50.0), 1.0);
        assert!(p_field(0.95) < 0.5);
    }

    #[test]
    fn the_signatures_are_the_docx_ones() {
        let l = signature(Zone::Limestone).unwrap();
        assert_eq!((l[0], l[1], l[2], l[4], l[5], l[6]), ((0.85, 0.95), (0.05, 0.12), (0.00, 0.05), (0.00, 0.05), (0.50, 0.75), (0.10, 0.35)));
        let g = signature(Zone::Geothermal).unwrap();
        assert_eq!((g[1], g[5]), ((0.85, 1.00), (0.90, 1.00)));
        let c = signature(Zone::Crystal).unwrap();
        assert_eq!(c[4], (0.05, 0.20));
        let a = signature(Zone::Abyssal).unwrap();
        assert_eq!(a[2], (0.10, 0.35));
        assert_eq!(signature(Zone::Soil), None);
    }

    #[test]
    fn a_water_bender_peaks_in_limestone_and_fire_is_locked_there() {
        for t in [0.0, 0.5, 1.0] {
            let f = cave_fields(Zone::Limestone, t).unwrap();
            assert!(unlocked(&f, WATER), "water locked in limestone at {t}");
            assert!(access(&f).strength[WATER] > 0.7, "water weak: {}", access(&f).strength[WATER]);
            assert!(!unlocked(&f, FIRE), "fire in a wet cave at {t}");
            assert!(!unlocked(&f, LIGHT) && !unlocked(&f, WIND));
        }
    }

    #[test]
    fn the_deep_is_earth_and_fire_country_and_every_cave_is_near_blind() {
        let g = cave_fields(Zone::Geothermal, 0.5).unwrap();
        assert!(unlocked(&g, EARTH) && unlocked(&g, FIRE));
        for z in [Zone::Limestone, Zone::Crystal, Zone::Abyssal, Zone::Geothermal] {
            for t in [0.0, 1.0] {
                let f = cave_fields(z, t).unwrap();
                assert!(f.l < THRESHOLDS[LIGHT], "{z:?} is not near-blind: L {}", f.l);
            }
        }
        // ...and the crystal chamber is the least blind of them.
        let crystal = cave_fields(Zone::Crystal, 0.5).unwrap().l;
        for z in [Zone::Limestone, Zone::Abyssal, Zone::Geothermal] {
            assert!(crystal > cave_fields(z, 0.5).unwrap().l, "{z:?}");
        }
    }

    #[test]
    fn the_blend_follows_enclosure_and_leaves_the_surface_alone() {
        let surface = Fields { h: 0.3, r: 0.8, w: 0.9, p: 0.5, l: 0.9, s: 0.1, b: 0.6 };
        let cave = cave_fields(Zone::Limestone, 0.5);
        assert_eq!(blend(surface, cave, 0.0), surface, "open sky: the weather");
        assert_eq!(blend(surface, None, 1.0), surface, "no cave: the weather");
        let deep = blend(surface, cave, 1.0);
        assert_eq!(deep, cave.unwrap(), "deep and roofed: the cave");
        let half = blend(surface, cave, 0.5);
        assert!(half.h > surface.h && half.h < deep.h && half.w < surface.w && half.w > deep.w);
    }
}
