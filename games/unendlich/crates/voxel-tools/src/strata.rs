//! Strata: what the ground is made of, by depth.
//!
//! A canyon cut through grassland exposed GRASS all the way down,
//! because nothing knew the ground had a structure. `wall=sandstone`
//! painted the whole face one colour, which is a wall, not a cliff.
//! What makes badlands read as badlands is the BANDING -- ochre over
//! red over pale, thin bands and thick ones, the same bands on the far
//! side of the valley because they were laid down together.
//!
//! So the ground gets a profile: a list of layers from the surface
//! down, each with a thickness and a block, deterministic from the
//! world's seed and the position. Two things use it:
//!
//! * every CARVE dresses its cut face from the profile rather than
//!   from one `wall=` block, so a canyon shows its geology and the
//!   bands line up across it
//! * a SITE can be judged by what it is made of: "somewhere with rock
//!   near the surface" is a query against the profile, which is how a
//!   feature finds a place to be rather than being told a coordinate
//!
//! The profile is a function, not a store: nothing is persisted and
//! any column can be asked at any time.

/// One band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub block: u16,
    /// Blocks thick at its thickest; the profile varies it by position.
    pub thickness: i32,
    /// How much the thickness varies, 0..1.
    pub variance: f32,
}

/// A named sequence of bands from the surface down. The last band is
/// the bedrock and continues forever.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    pub name: &'static str,
    pub bands: &'static [Band],
    /// How much the whole sequence rises and falls across the world:
    /// bands are not level, and a canyon that shows them tilting is
    /// the thing that reads as real. In BLOCKS PER BLOCK, so 0.01 is
    /// one block of shift per hundred travelled -- a gentle dip. The
    /// first values were ten times that, and the bands sheared a whole
    /// layer every fourteen blocks: two columns four apart no longer
    /// agreed, which is not a stratum, it is noise.
    pub dip: f32,
}

const fn b(block: u16, thickness: i32, variance: f32) -> Band { Band { block, thickness, variance } }

pub const PROFILES: &[Profile] = &[
    // grass, dirt, stone: the default everywhere temperate
    Profile { name: "temperate", dip: 0.004,
              bands: &[b(2, 1, 0.0), b(3, 4, 0.5), b(1, 40, 0.2), b(1, 999, 0.0)] },
    // the banded one: what badlands are for
    Profile { name: "sedimentary", dip: 0.012,
              bands: &[b(12, 2, 0.4), b(24, 6, 0.6), b(172, 4, 0.5), b(24, 9, 0.4),
                       b(179, 3, 0.7), b(24, 14, 0.3), b(1, 999, 0.0)] },
    // desert: sand over sandstone over stone
    Profile { name: "desert", dip: 0.005,
              bands: &[b(12, 4, 0.5), b(24, 18, 0.3), b(1, 999, 0.0)] },
    // volcanic: ash, basalt, obsidian at depth
    Profile { name: "volcanic", dip: 0.008,
              bands: &[b(13, 2, 0.6), b(87, 10, 0.4), b(1, 30, 0.3), b(49, 999, 0.0)] },
    // snowy: snow, frozen dirt, stone
    Profile { name: "snowy", dip: 0.004,
              bands: &[b(80, 2, 0.5), b(3, 3, 0.4), b(1, 999, 0.0)] },
    // coastal: sand, gravel, stone
    Profile { name: "coastal", dip: 0.006,
              bands: &[b(12, 3, 0.5), b(13, 5, 0.5), b(1, 999, 0.0)] },
];

pub fn lookup(name: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// The profile a site suggests, from its palette.
pub fn for_palette(palette: &str) -> &'static Profile {
    let name = match palette {
        "desert" => "desert",
        "mesa" => "sedimentary",
        "volcanic" => "volcanic",
        "snowy" | "frozen" => "snowy",
        _ => "temperate",
    };
    lookup(name).expect("every palette maps to a real profile")
}

fn hash(x: i64, z: i64, s: u64) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ s;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn unit(x: i64, z: i64, s: u64) -> f32 { (hash(x, z, s) % 10_000) as f32 / 10_000.0 }
/// Smooth noise over a grid, so band thickness varies gradually rather
/// than per column -- a band that jitters block to block is gravel.
fn smooth(x: f32, z: f32, s: u64) -> f32 {
    let (xi, zi) = (x.floor() as i64, z.floor() as i64);
    let (fx, fz) = (x - xi as f32, z - zi as f32);
    let (fx, fz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let c = |dx: i64, dz: i64| unit(xi + dx, zi + dz, s);
    let top = c(0, 0) + (c(1, 0) - c(0, 0)) * fx;
    let bot = c(0, 1) + (c(1, 1) - c(0, 1)) * fx;
    top + (bot - top) * fz
}

impl Profile {
    /// The block at a depth below the surface of this column. `depth`
    /// is 0 at the surface block itself.
    pub fn at(&self, x: i32, z: i32, depth: i32, seed: u64) -> u16 {
        if self.bands.is_empty() { return 1; }
        // the whole sequence tilts, so a wide cut shows the bands
        // running at an angle rather than sitting level
        let tilt = ((x as f32 * 0.7 + z as f32 * 0.3) * self.dip) as i32;
        let mut d = depth + tilt;
        if d < 0 { d = 0; }
        for (i, band) in self.bands.iter().enumerate() {
            let n = smooth(x as f32 / 24.0, z as f32 / 24.0, seed ^ (i as u64).wrapping_mul(7919));
            let t = (band.thickness as f32 * (1.0 - band.variance * 0.5 + band.variance * n))
                .round().max(1.0) as i32;
            if d < t { return band.block; }
            d -= t;
        }
        self.bands.last().map(|b| b.block).unwrap_or(1)
    }

    /// Depth to the first band of `block`, or None within `limit`.
    pub fn depth_to(&self, x: i32, z: i32, block: u16, seed: u64, limit: i32) -> Option<i32> {
        (0..limit).find(|&d| self.at(x, z, d, seed) == block)
    }

    /// The bands a cut from `top` down to `floor` would expose, as
    /// (depth, block) runs. What the preview shows the player.
    pub fn exposed(&self, x: i32, z: i32, depth: i32, seed: u64) -> Vec<(i32, u16)> {
        let mut out: Vec<(i32, u16)> = Vec::new();
        for d in 0..=depth.max(0) {
            let bl = self.at(x, z, d, seed);
            if out.last().map(|(_, b)| *b != bl).unwrap_or(true) { out.push((d, bl)); }
        }
        out
    }

    pub fn describe(&self, x: i32, z: i32, depth: i32, seed: u64) -> String {
        let runs = self.exposed(x, z, depth, seed);
        format!("{}: {}", self.name, runs.iter()
            .map(|(d, b)| format!("{d}+ block {b}")).collect::<Vec<_>>().join(", "))
    }
}

/// What a site is made of and whether it suits a purpose. This is how
/// a feature finds a place to be: not a coordinate, a QUERY.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteQuery {
    /// The block that must appear within `within` of the surface.
    pub wants: Option<u16>,
    pub within: i32,
    /// Surface height band.
    pub y_range: Option<(i32, i32)>,
    /// Maximum height difference over the sample.
    pub max_relief: i32,
    /// Minimum height difference: a cliff feature wants relief.
    pub min_relief: i32,
    /// Prefer near water, avoid it, or neither.
    pub water: Option<bool>,
}

impl Default for SiteQuery {
    fn default() -> Self {
        Self { wants: None, within: 8, y_range: None, max_relief: 200, min_relief: 0, water: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteScore {
    pub at: (i32, i32),
    /// 0 is a perfect fit; larger is worse. None of the hard rules
    /// are violated by a scored site -- those are rejected outright.
    pub cost: f32,
    pub surface: i32,
    pub relief: i32,
}

/// Search a region for the site that best fits a query, sampling on a
/// grid. Returns the best few, so a caller can spread several features
/// without stacking them.
pub fn find_sites<W: crate::VoxelWorld>(w: &W, min: (i32, i32), max: (i32, i32), step: i32,
                                        radius: i32, q: &SiteQuery, profile: &Profile, seed: u64,
                                        want: usize) -> (Vec<SiteScore>, Vec<String>) {
    let step = step.clamp(4, 256);
    let mut scored: Vec<SiteScore> = Vec::new();
    let mut notes = Vec::new();
    let (mut rej_relief, mut rej_band, mut rej_y, mut rej_water) = (0, 0, 0, 0);
    let surface = |x: i32, z: i32| (0..=320).rev().find(|&y| w.get(x, y, z) != 0).unwrap_or(0);
    let is_water = |x: i32, y: i32, z: i32| matches!(w.get(x, y, z), 8 | 9);
    for x in (min.0..=max.0).step_by(step as usize) {
        for z in (min.1..=max.1).step_by(step as usize) {
            let sy = surface(x, z);
            if let Some((lo, hi)) = q.y_range { if sy < lo || sy > hi { rej_y += 1; continue; } }
            // relief over the footprint, sampled at its corners and edges
            let probe: Vec<i32> = [(-radius, 0), (radius, 0), (0, -radius), (0, radius),
                                   (-radius, -radius), (radius, radius), (0, 0)]
                .iter().map(|(dx, dz)| surface(x + dx, z + dz)).collect();
            let relief = probe.iter().max().unwrap() - probe.iter().min().unwrap();
            if relief > q.max_relief { rej_relief += 1; continue; }
            if relief < q.min_relief { rej_relief += 1; continue; }
            if let Some(block) = q.wants {
                if profile.depth_to(x, z, block, seed, q.within.max(1)).is_none() { rej_band += 1; continue; }
            }
            let near_water = probe.iter().enumerate().any(|(i, y)| {
                let (dx, dz) = [(-radius, 0), (radius, 0), (0, -radius), (0, radius),
                                (-radius, -radius), (radius, radius), (0, 0)][i];
                is_water(x + dx, *y, z + dz)
            });
            match q.water { Some(true) if !near_water => { rej_water += 1; continue; }
                            Some(false) if near_water => { rej_water += 1; continue; }
                            _ => {} }
            // cost: how far from the middle of what was asked for
            let mut cost = 0.0;
            if q.min_relief > 0 { cost += (q.min_relief as f32 * 2.0 - relief as f32).max(0.0) / 20.0; }
            if q.max_relief < 200 { cost += relief as f32 / q.max_relief.max(1) as f32; }
            if let Some(block) = q.wants {
                cost += profile.depth_to(x, z, block, seed, q.within.max(1)).unwrap_or(q.within) as f32
                    / q.within.max(1) as f32;
            }
            scored.push(SiteScore { at: (x, z), cost, surface: sy, relief });
        }
    }
    scored.sort_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal));
    // spread them: no two within a diameter of each other
    let mut out: Vec<SiteScore> = Vec::new();
    for s in scored {
        if out.len() >= want { break; }
        if out.iter().any(|o| (o.at.0 - s.at.0).abs() < radius * 2 && (o.at.1 - s.at.1).abs() < radius * 2) { continue; }
        out.push(s);
    }
    if out.len() < want { notes.push(format!("{} of {want} site(s) found", out.len())); }
    for (n, why) in [(rej_relief, "relief"), (rej_band, "no such rock near the surface"),
                     (rej_y, "height"), (rej_water, "water")] {
        if n > 0 { notes.push(format!("{n} rejected: {why}")); }
    }
    (out, notes)
}

/// The query a feature wants: where each kind of place belongs.
pub fn query_for(feature: &str) -> SiteQuery {
    match feature {
        // badlands want flat ground and banded rock under it
        "badlands" => SiteQuery { wants: Some(24), within: 12, max_relief: 12, ..Default::default() },
        // a gorge wants something to cut through
        "gorge" => SiteQuery { max_relief: 40, min_relief: 6, water: Some(false), ..Default::default() },
        // a caldera wants an open flat: it makes its own relief
        "caldera" => SiteQuery { max_relief: 10, water: Some(false), ..Default::default() },
        // cliffs want the coast
        "cliffs" => SiteQuery { water: Some(true), max_relief: 30, ..Default::default() },
        // highlands want to be high already
        "highlands" => SiteQuery { min_relief: 8, max_relief: 60, ..Default::default() },
        // a sinkhole wants deep rock and flat ground over it
        "sinkhole" => SiteQuery { wants: Some(1), within: 10, max_relief: 8, ..Default::default() },
        _ => SiteQuery::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Land(BTreeMap<(i32, i32, i32), u16>);
    impl Land {
        fn h(x: i32, z: i32) -> i32 {
            // a plain in the west, a ridge in the middle, a lake east
            if x > 120 { 60 } else { (70 + (x / 6).min(30) + ((z as f32 / 30.0).sin() * 5.0) as i32).clamp(60, 104) }
        }
    }
    impl crate::VoxelWorld for Land {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            let h = Land::h(x, z);
            if x > 120 && y == h { return 9; }        // water surface east
            if y <= h { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    #[test]
    fn a_profile_bands_by_depth_and_the_bands_are_thick_not_jittery() {
        let p = lookup("sedimentary").unwrap();
        let col: Vec<u16> = (0..40).map(|d| p.at(0, 0, d, 7)).collect();
        // several distinct bands
        let distinct: std::collections::BTreeSet<u16> = col.iter().copied().collect();
        assert!(distinct.len() >= 4, "banded: {col:?}");
        // each band is a RUN, not alternating
        let switches = col.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(switches <= 8, "thick bands, not stripes: {switches} switches in 40");
        // the top is the surface block, the bottom is bedrock
        assert_eq!(col[0], 12, "sand at the surface");
        assert_eq!(p.at(0, 0, 500, 7), 1, "stone at depth");
    }

    #[test]
    fn bands_line_up_across_a_cut_and_tilt_across_the_world() {
        let p = lookup("sedimentary").unwrap();
        // two columns four blocks apart show the same band at the same
        // depth: they were laid down together
        let a: Vec<u16> = (0..25).map(|d| p.at(60, 0, d, 3)).collect();
        let b: Vec<u16> = (0..25).map(|d| p.at(64, 0, d, 3)).collect();
        let agree = a.iter().zip(&b).filter(|(x, y)| x == y).count();
        assert!(agree >= 20, "the bands line up nearby: {agree}/25");
        // but far apart they have tilted -- gently, so it takes distance
        let far: Vec<u16> = (0..25).map(|d| p.at(2000, 0, d, 3)).collect();
        assert!(a.iter().zip(&far).filter(|(x, y)| x == y).count() < 20, "the sequence dips across the world");
    }

    #[test]
    fn exposed_reads_as_a_geology_the_player_could_be_shown() {
        let p = lookup("sedimentary").unwrap();
        let runs = p.exposed(0, 0, 30, 5);
        assert!(runs.len() >= 4 && runs[0].0 == 0);
        // strictly increasing depths, each a different block from the last
        for w in runs.windows(2) { assert!(w[1].0 > w[0].0 && w[1].1 != w[0].1, "{runs:?}"); }
        let d = p.describe(0, 0, 30, 5);
        assert!(d.starts_with("sedimentary:") && d.contains("0+ block"));
    }

    #[test]
    fn a_site_query_finds_the_flat_and_rejects_the_steep() {
        let w = Land(BTreeMap::new());
        let p = lookup("temperate").unwrap();
        // badlands want flat: the western plain, not the ridge
        let q = SiteQuery { max_relief: 6, ..Default::default() };
        let (sites, notes) = find_sites(&w, (0, -60), (110, 60), 20, 10, &q, p, 1, 3);
        assert!(!sites.is_empty(), "{notes:?}");
        assert!(sites.iter().all(|s| s.relief <= 6), "{:?}", sites);
        assert!(notes.iter().any(|n| n.contains("relief")), "it says what it rejected: {notes:?}");
        // and they are spread, not stacked
        for (i, a) in sites.iter().enumerate() { for b in &sites[i + 1..] {
            assert!((a.at.0 - b.at.0).abs() >= 20 || (a.at.1 - b.at.1).abs() >= 20, "{a:?} {b:?}");
        }}
    }

    #[test]
    fn a_query_can_want_relief_or_water_and_gets_them() {
        let w = Land(BTreeMap::new());
        let p = lookup("temperate").unwrap();
        let hilly = SiteQuery { min_relief: 8, ..Default::default() };
        let (s, _) = find_sites(&w, (0, -60), (110, 60), 16, 20, &hilly, p, 1, 2);
        assert!(s.iter().all(|s| s.relief >= 8), "{s:?}");
        let coastal = SiteQuery { water: Some(true), ..Default::default() };
        let (c, notes) = find_sites(&w, (60, -60), (180, 60), 16, 12, &coastal, p, 1, 2);
        assert!(!c.is_empty(), "the lake shore: {notes:?}");
        assert!(c.iter().all(|s| s.at.0 > 100), "near the water in the east: {c:?}");
        let inland = SiteQuery { water: Some(false), ..Default::default() };
        let (i, _) = find_sites(&w, (60, -60), (180, 60), 16, 12, &inland, p, 1, 2);
        assert!(i.iter().all(|s| s.at.0 <= 130), "away from it: {i:?}");
    }

    #[test]
    fn a_band_query_wants_rock_near_the_surface() {
        let w = Land(BTreeMap::new());
        let sed = lookup("sedimentary").unwrap();
        // sandstone (24) is 2-8 down in this profile: found
        let near = SiteQuery { wants: Some(24), within: 12, ..Default::default() };
        let (s, notes) = find_sites(&w, (0, -40), (100, 40), 20, 16, &near, sed, 4, 2);
        assert!(!s.is_empty(), "sandstone is near the surface here: {notes:?}");
        // obsidian (49) is not in this profile at all
        let absent = SiteQuery { wants: Some(49), within: 12, ..Default::default() };
        let (none, why) = find_sites(&w, (0, -40), (100, 40), 20, 16, &absent, sed, 4, 2);
        assert!(none.is_empty());
        assert!(why.iter().any(|n| n.contains("no such rock")), "{why:?}");
    }

    #[test]
    fn every_feature_has_a_query_and_they_differ() {
        let names = ["badlands", "gorge", "caldera", "cliffs", "highlands", "sinkhole"];
        let qs: Vec<SiteQuery> = names.iter().map(|n| query_for(n)).collect();
        assert_eq!(query_for("cliffs").water, Some(true), "cliffs want the coast");
        assert_eq!(query_for("highlands").min_relief, 8, "highlands want to be high");
        assert!(query_for("badlands").wants.is_some(), "badlands want banded rock");
        // they are not all the same
        assert!(qs.windows(2).any(|w| w[0] != w[1]));
        assert_eq!(query_for("nonesuch"), SiteQuery::default());
    }
}
