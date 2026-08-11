//! CLONES — the seven elemental bodies, as models.
//!
//! The watch-bearer is a human ICE-CLONE shapeshifter, and the ice clone
//! has a real reference: the C engine's model catalog carries
//! `catalog/representatives/imported_ice_clone_body_reference.svxr`
//! (occupancy at grid 32, 1,431 voxels; a skin variant sits beside it,
//! and `imported_reference_bot_watch_engineer_vine_clone_transformer_
//! hybrid.svxr` sketches the vine cousin). Decoded — records are seven
//! bytes, `<3H` coords plus a palette byte, per `tools/voxel_rep.py`'s
//! `encode`; note `solve_placements.py::load_representative` reads them
//! at stride 4, which is a bug in that tool, not in the files — the
//! reference is a FIGURE: split-hoofed double-column legs, a tall
//! blade-thin torso carrying its mass high, and twin horns sweeping off
//! the crown. Alien reads exactly right.
//!
//! What this module does with it:
//! * embeds the MEASURED silhouette (eight height-bands of mass
//!   fraction, width, depth) as `ICE_REF`, provenance above;
//! * regenerates the ice clone procedurally to match that profile — a
//!   TEST holds the generated body to the reference, so the asset
//!   constrains the code instead of decorating a comment;
//! * derives the other six clones as principled mutations of the same
//!   anatomy: one skeleton, seven natures, every one distinct (also a
//!   test). Fire flares low and crowns in plumes; Air is a lantern of
//!   winglets; Electric antennas past its own head; Vine slumps
//!   asymmetric (the hybrid reference's silhouette); Rock is the squat
//!   heavy cousin; Water tapers smooth and hornless.
//!
//! Output is a point cloud (`Vec<Vec3>`, metres, feet at y=0) — the
//! same shape `model::Model::cloud` feeds the reflection-SDF bake, so
//! the renderer can pick these up without a new path.

use glam::Vec3;

use crate::raidgen::{strhash, Rng};
use crate::weapons::Element;

/// One height-band of a silhouette: mass fraction, width and depth in
/// reference voxels (grid 32).
#[derive(Clone, Copy)]
pub struct Band {
    pub frac: f32,
    pub w: f32,
    pub d: f32,
}

const fn band(frac: f32, w: f32, d: f32) -> Band {
    Band { frac, w, d }
}

/// The ice-clone reference, measured 2026-08-10 from the catalog file
/// named in the module doc: 1,431 voxels, bounds 19 x 32 x 14, eight
/// bands bottom-to-top.
pub const ICE_REF: [Band; 8] = [
    band(0.103, 12.0, 8.0),
    band(0.125, 18.0, 6.0),
    band(0.144, 19.0, 6.0),
    band(0.175, 19.0, 8.0),
    band(0.180, 18.0, 8.0),
    band(0.185, 16.0, 9.0),
    band(0.069, 10.0, 10.0),
    band(0.020, 14.0, 4.0),
];
pub const ICE_REF_HEIGHT_V: f32 = 32.0;

/// A clone's nature: the knobs one anatomy is bent by.
pub struct CloneSpec {
    pub element: Element,
    pub name: &'static str,
    /// standing height in metres
    pub height_m: f32,
    /// band widths/depths multiply the reference by these
    pub breadth: f32,
    pub thickness: f32,
    /// where the mass sits: >0 shifts it up the body, <0 down
    pub mass_shift: f32,
    /// crown protrusions: (count, length in bands, outward sweep)
    pub horns: (u8, f32, f32),
    /// side asymmetry, 0 = mirrored (vine slumps)
    pub skew: f32,
    /// surface break-up: spikes for ice, plates for rock, ripple for water
    pub burr: f32,
}

/// Seven natures on one skeleton. Ice IS the reference; the others are
/// stated as differences from it, which is how a family reads.
pub const CLONES: [CloneSpec; 7] = [
    CloneSpec { element: Element::Ice, name: "veilhorn", height_m: 2.6, breadth: 1.0, thickness: 1.0, mass_shift: 0.0, horns: (2, 1.6, 1.4), skew: 0.0, burr: 0.35 },
    CloneSpec { element: Element::Fire, name: "cindermaw", height_m: 2.4, breadth: 1.15, thickness: 1.1, mass_shift: -0.20, horns: (3, 1.0, 0.6), skew: 0.0, burr: 0.55 },
    CloneSpec { element: Element::Air, name: "lanternkite", height_m: 2.8, breadth: 0.7, thickness: 0.6, mass_shift: 0.25, horns: (4, 2.2, 2.4), skew: 0.0, burr: 0.15 },
    CloneSpec { element: Element::Electric, name: "arcwrit", height_m: 2.5, breadth: 0.85, thickness: 0.8, mass_shift: 0.10, horns: (5, 2.6, 0.4), skew: 0.0, burr: 0.25 },
    CloneSpec { element: Element::Vine, name: "loomshank", height_m: 2.7, breadth: 0.9, thickness: 1.0, mass_shift: -0.10, horns: (1, 1.8, 1.0), skew: 0.45, burr: 0.45 },
    CloneSpec { element: Element::Rock, name: "cairnback", height_m: 2.1, breadth: 1.45, thickness: 1.5, mass_shift: -0.30, horns: (2, 0.6, 0.8), skew: 0.0, burr: 0.65 },
    CloneSpec { element: Element::Water, name: "slipmere", height_m: 2.5, breadth: 0.95, thickness: 0.9, mass_shift: 0.05, horns: (0, 0.0, 0.0), skew: 0.0, burr: 0.05 },
];

pub fn spec(e: Element) -> Option<&'static CloneSpec> {
    CLONES.iter().find(|c| c.element == e)
}

/// Interpolate the reference bands at height fraction `t` (0..1), with
/// the spec's mass shift bending where the body carries itself. Returns
/// width, depth, AND the reference's mass fraction there — the density
/// a generated ring should honor, because occupancy is the thing the
/// reference actually measures (its upper bands are hollower than their
/// extents suggest, and w x d misses that).
fn silhouette(s: &CloneSpec, t: f32) -> (f32, f32, f32) {
    let shifted = (t - s.mass_shift * 0.3).clamp(0.0, 0.999);
    // bands are 1/8 slices with their values at band CENTERS, so the
    // interpolation axis is t*8 - 1/2, not t*7 — the off-by-one-grid
    // that stretched band five's bulk into band six's territory
    let f = (shifted * 8.0 - 0.5).clamp(0.0, 7.0);
    let (i, u) = (f as usize, f.fract());
    // NEAREST band, not lerp: the reference is an occupancy histogram
    // with a genuine cliff at the shoulder (0.185 -> 0.069), and
    // interpolation smears exactly the feature that makes the body read
    let n = if u < 0.5 { ICE_REF[i] } else { ICE_REF[(i + 1).min(7)] };
    (n.w * s.breadth, n.d * s.thickness, n.frac)
}

/// Build a clone's body as a point cloud: metres, centred on x/z, feet
/// at y = 0. Deterministic per name. Point budget lands inside what the
/// SDF bake calls affordable (~2,600).
pub fn model(e: Element) -> Vec<Vec3> {
    let s = spec(e).expect("every element has a clone");
    let mut rng = Rng::new(strhash(s.name));
    let mut pts: Vec<Vec3> = Vec::with_capacity(2200);
    let vox = s.height_m / ICE_REF_HEIGHT_V; // metres per reference voxel
    let rings = 44; // height slices
    let leg_top = 0.16; // the split ends where the reference's does
    // the torso stops short of the crown: the reference's top band is
    // horn span, not shoulder — bulk that reaches it reads as a hood.
    // A hornless nature keeps rising instead; nothing else will.
    let torso_top = if s.horns.0 == 0 { 1.0 } else { 0.86 };
    // the torso's point budget, dealt out ring by ring in proportion to
    // the reference's mass fraction at that height
    let torso_pts = 1150.0;
    let frac_sum: f32 = (0..rings)
        .map(|r| silhouette(s, (r as f32 + 0.5) / rings as f32 * torso_top).2)
        .sum();
    for r in 0..rings {
        let t = (r as f32 + 0.5) / rings as f32 * torso_top;
        let (w, d, frac) = silhouette(s, t);
        let (hw, hd) = (0.5 * w * vox, 0.5 * d * vox);
        let y = t * s.height_m;
        let n = ((frac / frac_sum) * torso_pts).round().max(2.0) as usize;
        for k in 0..n {
            let ang = k as f32 / n as f32 * std::f32::consts::TAU + rng.f(0.0, 0.25);
            let (mut cx, cz) = (ang.cos() * hw, ang.sin() * hd);
            // the split stance: below the hips the ring collapses into
            // two columns, the reference's `####..####`
            if t < leg_top {
                let side = if cx >= 0.0 { 1.0 } else { -1.0 };
                cx = side * (hw * 0.55) + cx * 0.35;
            }
            // vine slumps: one shoulder low, mass dragged sideways
            let skew = s.skew * (t - 0.4).max(0.0) * hw;
            // burr breaks the surface inward-biased, so the nature's
            // texture never inflates the stated extents
            let burr = 1.0 + rng.f(-0.55, 0.10) * s.burr;
            pts.push(Vec3::new((cx + skew) * burr, y, cz * burr));
        }
    }
    // horns: from the crown, swept out and up, INSIDE the stated height
    // — the reference keeps its horns in frame, so the family does too
    let (count, len_bands, sweep) = s.horns;
    // horns rise from the crown to EXACTLY the stated height, so the
    // profile's band grid and the torso's t axis agree by construction
    // — horn length shows in bead density and sweep, not in overshoot
    let crown = s.height_m * 0.80;
    let (cw, _, _) = silhouette(s, 0.82);
    let reach = s.height_m - crown;
    for h in 0..count {
        let side = if count == 1 { 1.0 } else { (h as f32 / (count.max(2) - 1) as f32) * 2.0 - 1.0 };
        let beads = (8.0 + len_bands * 4.0) as usize;
        for b in 0..beads {
            let u = b as f32 / beads as f32;
            let rise = u * reach;
            let out = side * (0.42 * cw * vox + u * sweep * 2.2 * vox);
            let jitter = rng.f(-0.015, 0.015);
            pts.push(Vec3::new(out + jitter, crown + rise, rng.f(-0.03, 0.03)));
        }
    }
    pts
}

/// A clone's measured silhouette, in the reference's terms — the same
/// eight bands, so a generated body and the catalog file can be held to
/// each other.
pub fn profile(pts: &[Vec3]) -> [Band; 8] {
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    for p in pts {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    let h = (ymax - ymin).max(1e-6);
    let mut bands: [Vec<&Vec3>; 8] = Default::default();
    for p in pts {
        let i = (((p.y - ymin) / h) * 8.0).min(7.99) as usize;
        bands[i].push(p);
    }
    let vox = h / ICE_REF_HEIGHT_V;
    core::array::from_fn(|i| {
        let b = &bands[i];
        if b.is_empty() {
            return band(0.0, 0.0, 0.0);
        }
        let (mut xmin, mut xmax, mut zmin, mut zmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in b {
            xmin = xmin.min(p.x);
            xmax = xmax.max(p.x);
            zmin = zmin.min(p.z);
            zmax = zmax.max(p.z);
        }
        Band {
            frac: b.len() as f32 / pts.len() as f32,
            w: (xmax - xmin) / vox,
            d: (zmax - zmin) / vox,
        }
    })
}

/// How far apart two silhouettes are: mass-distribution L1 plus a small
/// breadth term. Used to hold ice to the reference and the seven apart.
pub fn distance(a: &[Band; 8], b: &[Band; 8]) -> f32 {
    let mut d = 0.0;
    for i in 0..8 {
        d += (a[i].frac - b[i].frac).abs();
        d += (a[i].w - b[i].w).abs() / 64.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ice_clone_answers_to_its_reference() {
        // the generated veilhorn's mass distribution sits close to the
        // catalog file's — the asset constrains the generator
        let p = profile(&model(Element::Ice));
        let d = distance(&p, &ICE_REF);
        assert!(d < 0.30, "the ice clone drifted from its reference: distance {d:.3}");
        // and the signature traits survive generation: the crown band is
        // thin (horns, not bulk), the mid-body is where the mass lives
        assert!(p[7].frac < 0.06, "the crown grew bulk: {:.3}", p[7].frac);
        assert!(p[3].frac + p[4].frac + p[5].frac > 0.35, "the high-carried mass sank");
    }

    #[test]
    fn seven_natures_and_no_two_alike() {
        let profiles: Vec<_> = CLONES.iter().map(|c| profile(&model(c.element))).collect();
        for i in 0..7 {
            for j in (i + 1)..7 {
                let d = distance(&profiles[i], &profiles[j]);
                assert!(
                    d > 0.10,
                    "{} and {} read as the same body: distance {d:.3}",
                    CLONES[i].name,
                    CLONES[j].name
                );
            }
        }
        // and the stated natures hold: rock is the squattest and
        // broadest, air the tallest reach for its breadth
        let rock = spec(Element::Rock).unwrap();
        let air = spec(Element::Air).unwrap();
        assert!(rock.height_m < air.height_m && rock.breadth > air.breadth);
    }

    #[test]
    fn every_body_is_deterministic_and_affordable() {
        for c in &CLONES {
            let a = model(c.element);
            let b = model(c.element);
            assert_eq!(a.len(), b.len(), "{}", c.name);
            assert!(a.iter().zip(&b).all(|(p, q)| p == q), "{} regenerates differently", c.name);
            assert!(a.len() >= 700 && a.len() <= 2600, "{} cloud is {} points", c.name, a.len());
            // feet at zero, standing its stated height
            let ymax = a.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            let ymin = a.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            assert!(ymin.abs() < 0.15 && ymax > c.height_m * 0.9, "{} does not stand right", c.name);
        }
    }
}
