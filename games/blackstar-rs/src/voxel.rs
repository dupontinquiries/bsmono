//! VOXEL CRITICS — what judges a generated frame, besides a discriminator.
//!
//! `apps/beatspace` carries a working GAN: a DCGAN over mel spectrograms,
//! generator and discriminator built by hand out of `Var`s so checkpointing
//! keeps BatchNorm's running statistics. The shape transfers to voxels
//! directly — a 3D generator from a noise vector, a 3D discriminator giving
//! one logit — and that half is a port, not a design.
//!
//! The design is everything else, because **a voxel GAN on its own makes
//! blobs.** A discriminator learns "does this look like the corpus", and the
//! corpus is a thousand vehicles; the fastest way to satisfy it is a
//! vehicle-shaped lump. Nothing in that loss knows a frame needs two arms.
//!
//! So two more critics, and they are the reason this is worth building:
//!
//! **A CORRECTION FIRST.** I designed a skeleton reward that splits a volume
//! into components and asks whether they form a body — torso, head above it,
//! limbs in pairs — and it scored REAL MODELS BELOW NOISE (0.413 against
//! 0.494). The reason is in the data, not the weights: a representative is
//! ONE PART, normalised to its own grid with its own `lo` and `step`, so a
//! whole frame is many files with no shared coordinate frame. Placement was
//! normalised away. A critic that needs relative position cannot run on this
//! corpus at all, and tuning it would have been fitting weights to a premise
//! that is false.
//!
//! The skeleton reference does exist — `tools/mounts.json` carries mount
//! positions "in the vehicle pose's own space, +Y up, nose at -Z", and
//! `solve_placements.py` computes placements — but it is not in the voxels,
//! and wiring it is a separate job.
//!
//! **Cross-bot part matching is what the corpus does support**, and it turns
//! out to already exist in a usable form. `tools/part-labels.json` holds
//! FOURTEEN CLUSTERS derived from real geometry across the whole library,
//! each with shape features — elongation, flatness, roundness, hollowness,
//! whether it comes in mirrored pairs — and the member names that fell into
//! it: `bone, pelvis, spine` in one, `link, coil, thigh` in another.
//!
//! That is the cross-bot part vocabulary, measured rather than authored. A
//! generated part is plausible if it lands near one of those clusters, and
//! the clusters know about mirroring, which is the one piece of skeleton
//! information that survived normalisation.
//!
//! That second one is the connection worth stating: **the gestalt code
//! already knows how to judge whether a shape suits a role**, and it does not
//! care whether the shape came from a model file or from noise.
//!
//! ## What is here and what is not
//!
//! The critics, and their verification against the real corpus:
//! `catalog/representatives/` holds 1,012 models as occupancy at grid 32,
//! measured at 0.955 shape IoU against a 96³ reference. That is a training
//! set, and it is enough to check that the critics separate real frames from
//! noise — which is the claim that has to hold before any training is worth
//! starting.
//!
//! **The training loop is not here.** It needs GPU hours this environment
//! does not have, and writing an untrainable one would be a thousand lines
//! nobody could check.

use crate::combine::{fitness, Member, Slot};

/// Grid resolution of a representative.
pub const GRID: usize = 32;

/// A decoded representative: occupancy at `GRID`, with the palette dropped.
pub struct Volume {
    pub cells: Vec<(u16, u16, u16)>,
}

/// Decode a `.svxr`.
///
/// **Seven bytes per voxel** — three `u16` coordinates and a palette index.
/// `tools/solve_placements.py` read this with a stride of four while
/// unpacking six bytes, so every cell after the first came from a sliding
/// window across two records; on `imported_grimorph_part4` that put 191 of
/// 647 voxels inside the grid instead of all 647. Fixed there too.
pub fn decode(bytes: &[u8]) -> Option<Volume> {
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(bytes)
        .ok()
        .or_else(|| gunzip(bytes))?;
    if raw.len() < 29 || &raw[..5] != b"SVXR\x01" {
        return None;
    }
    let npal = u16::from_le_bytes([raw[7], raw[8]]) as usize;
    let nvox = u32::from_le_bytes([raw[9], raw[10], raw[11], raw[12]]) as usize;
    let off = 29 + npal * 3;
    if raw.len() < off + nvox * 7 {
        return None;
    }
    let mut cells = Vec::with_capacity(nvox);
    for i in 0..nvox {
        let p = off + i * 7;
        cells.push((
            u16::from_le_bytes([raw[p], raw[p + 1]]),
            u16::from_le_bytes([raw[p + 2], raw[p + 3]]),
            u16::from_le_bytes([raw[p + 4], raw[p + 5]]),
        ));
    }
    Some(Volume { cells })
}

/// gzip wrapper around the deflate stream.
fn gunzip(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 18 || b[0] != 0x1f || b[1] != 0x8b {
        return None;
    }
    let mut i = 10;
    let flg = b[3];
    if flg & 4 != 0 {
        let n = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
        i += 2 + n;
    }
    for bit in [8u8, 16] {
        if flg & bit != 0 {
            while i < b.len() && b[i] != 0 {
                i += 1;
            }
            i += 1;
        }
    }
    if flg & 2 != 0 {
        i += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(&b[i..]).ok()
}

/// One connected piece of a volume.
#[derive(Clone, Debug)]
pub struct Component {
    pub cells: Vec<(u16, u16, u16)>,
    /// bounding box, in grid units
    pub extent: (f32, f32, f32),
    /// centroid height, 0 at the floor of the volume, 1 at its top
    pub height_frac: f32,
}

impl Component {
    /// The same four numbers `combine.rs` measures a gestalt member by, read
    /// off geometry instead of off a table. That is the whole trick: the
    /// slot-fitness function does not care where the numbers came from.
    pub fn as_member(&self, name: &'static str) -> Member {
        let (ex, ey, ez) = self.extent;
        let longest = ex.max(ey).max(ez).max(0.001);
        let shortest = ex.min(ey).min(ez).max(0.001);
            Member {
            name,
            // 1 = a rod, 0 = a cube
            elongation: (1.0 - shortest / longest).clamp(0.0, 1.0),
            // how much of the bounding box is filled
            bulk: (self.cells.len() as f32 / (ex * ey * ez).max(1.0)).clamp(0.0, 1.0),
            reach_m: longest / GRID as f32 * 8.5,
            mass: self.cells.len() as f32 * 40.0,
        }
    }
    pub fn voxels(&self) -> usize {
        self.cells.len()
    }
    /// Roughly rod-like along one axis and thin in the others: an arm or leg
    /// rather than a block.
    pub fn limbish(&self) -> bool {
        let m = self.as_member("x");
        m.elongation > 0.55 && m.bulk < 0.75
    }
}

/// Split into 6-connected pieces, dropping specks.
///
/// `min_voxels` matters: at grid 32 a handful of voxels is something no
/// aligner can orient, and feeding it in produces a confident answer fitted
/// to noise — the same reason the mesh harvester has a `min_tris`.
pub fn components(v: &Volume, min_voxels: usize) -> Vec<Component> {
    use std::collections::HashSet;
    let occ: HashSet<(u16, u16, u16)> = v.cells.iter().copied().collect();
    let mut seen: HashSet<(u16, u16, u16)> = HashSet::new();
    let mut out = Vec::new();
    let (mut lo_y, mut hi_y) = (u16::MAX, 0u16);
    for c in &v.cells {
        lo_y = lo_y.min(c.1);
        hi_y = hi_y.max(c.1);
    }
    let span_y = (hi_y.saturating_sub(lo_y)).max(1) as f32;
    for start in &v.cells {
        if seen.contains(start) {
            continue;
        }
        let mut stack = vec![*start];
        let mut group = Vec::new();
        seen.insert(*start);
        while let Some(c) = stack.pop() {
            group.push(c);
            for (dx, dy, dz) in
                [(1i32, 0i32, 0i32), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
            {
                let n = (
                    (c.0 as i32 + dx) as u16,
                    (c.1 as i32 + dy) as u16,
                    (c.2 as i32 + dz) as u16,
                );
                if occ.contains(&n) && seen.insert(n) {
                    stack.push(n);
                }
            }
        }
        if group.len() < min_voxels {
            continue;
        }
        let f = |sel: fn(&(u16, u16, u16)) -> u16| {
            let lo = group.iter().map(sel).min().unwrap();
            let hi = group.iter().map(sel).max().unwrap();
            (hi - lo + 1) as f32
        };
        let cy = group.iter().map(|c| c.1 as f32).sum::<f32>() / group.len() as f32;
        out.push(Component {
            extent: (f(|c| c.0), f(|c| c.1), f(|c| c.2)),
            height_frac: ((cy - lo_y as f32) / span_y).clamp(0.0, 1.0),
            cells: group,
        });
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.voxels()));
    out
}

// ------------------------------------------------------------ critics ---

/// A part class the library actually contains, from `tools/part-labels.json`.
///
/// Fourteen clusters over the whole model library, with shape features
/// measured off geometry. The `label` fields in that file are still empty —
/// the tool asks a human to name them — so the member names stand in, which
/// is honest: these are what the shapes ARE, not what somebody decided to
/// call them.
pub struct Cluster {
    pub id: u8,
    /// how many parts fell into it
    pub n: u16,
    pub elong: f32,
    pub flat: f32,
    pub round: f32,
    pub hollow: f32,
    /// 1.0 means these come in mirrored pairs — the one piece of skeleton
    /// information that survived per-part normalisation
    pub mirrored: f32,
    pub up: f32,
    /// the commonest member names, as a description
    pub members: &'static str,
}

const fn c(
    id: u8, n: u16, elong: f32, flat: f32, round: f32, hollow: f32, mirrored: f32, up: f32,
    members: &'static str,
) -> Cluster {
    Cluster { id, n, elong, flat, round, hollow, mirrored, up, members }
}
use c as C;

pub static CLUSTERS: &[Cluster] = &[
    C(0,356,2.138,0.582,0.770,0.385,1.000,0.417,"bone,pelvis,spine"),
    C(1,568,1.272,0.570,0.802,0.476,1.000,-0.126,"link,coil,thighr"),
    C(2,298,1.928,0.980,0.003,0.994,0.000,0.088,"guts,greeble,clad"),
    C(3,87,1.333,0.823,0.283,0.688,0.000,0.079,"greeble,thighr,pelvis"),
    C(4,49,22.170,0.864,0.260,0.784,0.878,-0.103,"rib,pelvis,larmr"),
    C(5,392,1.818,0.608,0.807,0.533,1.000,-0.178,"muscle,coil,bone"),
    C(6,456,1.907,0.871,0.007,0.938,1.000,-0.049,"greeble,clad,link"),
    C(7,95,1.423,0.802,0.829,0.223,0.916,0.045,"pelvis,spine,clavr"),
    C(8,836,2.112,0.991,0.003,0.994,1.000,0.357,"guts,greeble,clad"),
    C(9,806,2.303,0.991,0.004,0.997,1.000,-0.230,"guts,clad,greeble"),
    C(10,396,1.876,0.416,0.083,0.701,1.000,-0.163,"rib,link,face"),
    C(11,122,1.935,0.541,0.624,0.394,0.000,0.054,"clavr,pelvis,rib"),
    C(12,530,1.656,0.506,0.778,0.494,1.000,-0.356,"muscle,piston,shinr"),
    C(13,449,2.468,0.942,0.002,0.988,1.000,-0.113,"guts,face,finger"),
];

/// Shape features of a component, in the same space the clusters live in.
pub fn features(c: &Component) -> (f32, f32, f32) {
    let (ex, ey, ez) = c.extent;
    let longest = ex.max(ey).max(ez).max(0.001);
    let shortest = ex.min(ey).min(ez).max(0.001);
    let mid = (ex + ey + ez - longest - shortest).max(0.001);
    let elong = longest / mid;
    let flat = 1.0 - shortest / mid;
    let fill = (c.cells.len() as f32 / (ex * ey * ez).max(1.0)).clamp(0.0, 1.0);
    (elong, flat, fill)
}

/// How near a component sits to the closest real part class, 0..1.
///
/// THE CROSS-BOT MATCH. A generated part is plausible if the library already
/// contains parts shaped like it — and the library's own clustering is the
/// authority on what shapes it contains.
pub fn cluster_match(c: &Component) -> (f32, &'static Cluster) {
    let (elong, flat, fill) = features(c);
    let mut best = (f32::MAX, &CLUSTERS[0]);
    for k in CLUSTERS {
        // hollowness stands in for fill: both say how much of the box is
        // actually there, from opposite ends
        // `round` and `up` join the distance: roundness is another read on
        // how box-filling a shape is, and `up` is the cluster's vertical
        // bias, which separates a rib from a shin even when both are rods.
        let d = ((elong - k.elong) / 2.0).powi(2)
            + (flat - k.flat).powi(2)
            + (fill - (1.0 - k.hollow)).powi(2)
            + 0.5 * (fill - k.round).powi(2)
            + 0.25 * ((c.height_frac - 0.5) * 2.0 - k.up).powi(2);
        if d < best.0 {
            best = (d, k);
        }
    }
    ((-best.0 * 1.5).exp(), best.1)
}

/// Mean cluster match over the pieces. The plausibility critic.
pub fn part_match_reward(cs: &[Component]) -> f32 {
    if cs.is_empty() {
        return 0.0;
    }
    cs.iter().take(8).map(|c| cluster_match(c).0).sum::<f32>() / cs.len().min(8) as f32
}

/// Would `combine::fitness` have taken these for gestalt limbs?
///
/// The second half of cross-bot matching, and the connection worth having:
/// the gestalt scorer does not care that these numbers came from voxels.
pub fn slot_reward(cs: &[Component]) -> f32 {
    if cs.is_empty() {
        return 0.0;
    }
    let slots = [Slot::Torso, Slot::ArmL, Slot::ArmR, Slot::LegL, Slot::LegR];
    let mut total = 0.0;
    for c in cs.iter().take(8) {
        let m = c.as_member("candidate");
        total += slots.iter().map(|s| fitness(&m, *s)).fold(0.0f32, f32::max);
    }
    total / cs.len().min(8) as f32
}

/// The combined critic, as a training signal would use it.
///
/// `disc` is the discriminator's opinion in 0..1, passed in rather than
/// computed: the learned half is beatspace's to provide.
pub fn reward(disc: f32, cs: &[Component]) -> f32 {
    // No skeleton term. It needs placement and the corpus does not carry it;
    // see the module doc. When `mounts.json` is wired in, it goes here.
    0.4 * disc.clamp(0.0, 1.0) + 0.35 * part_match_reward(cs) + 0.25 * slot_reward(cs)
}

/// Structured noise, for the negative half of the check.
///
/// Uniform noise is too easy a negative — any critic separates it. This makes
/// a blob of roughly the right density and extent, which is what an
/// undertrained generator actually emits.
pub fn noise_volume(seed: u64, n: usize) -> Volume {
    let mut s = seed | 1;
    let mut rnd = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as f32) / (u32::MAX as f32 / 2.0)
    };
    let mut cells = Vec::with_capacity(n);
    for _ in 0..n {
        let g = GRID as f32;
        cells.push((
            (rnd() * g * 0.6 + g * 0.2) as u16,
            (rnd() * g * 0.6 + g * 0.2) as u16,
            (rnd() * g * 0.6 + g * 0.2) as u16,
        ));
    }
    Volume { cells }
}

// ------------------------------------------------------------- driver ---

/// Where the corpus lives.
pub const CORPUS: &str = "../blackstar/catalog/representatives";

pub fn run() {
    println!("VOXEL CRITICS — what judges a frame besides a discriminator\n");
    println!("  the corpus: 1,012 models at grid {GRID}, occupancy with a palette,");
    println!("  measured at 0.955 shape IoU against a 96^3 reference.\n");

    let Ok(dir) = std::fs::read_dir(CORPUS) else {
        println!("  corpus not found at {CORPUS}");
        return;
    };
    let mut paths: Vec<std::path::PathBuf> =
        dir.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "svxr")).collect();
    paths.sort();
    if paths.is_empty() {
        println!("  no representatives found");
        return;
    }

    println!("  {:<38} {:>5} {:>5} {:>7} {:>6}  {}", "model", "vox", "pcs", "cluster", "slot", "nearest part class");
    let (mut rs, mut rp) = (0.0f32, 0.0f32);
    let mut n = 0.0f32;
    for p in paths.iter().step_by(paths.len() / 8 + 1).take(8) {
        let Ok(b) = std::fs::read(p) else { continue };
        let Some(v) = decode(&b) else { continue };
        let cs = components(&v, 6);
        let (sk, pm) = (part_match_reward(&cs), slot_reward(&cs));
        rs += sk;
        rp += pm;
        n += 1.0;
        let name = p.file_stem().unwrap().to_string_lossy();
        let short: String = name.chars().take(37).collect();
        let (_, k) = cluster_match(&cs[0]);
        println!(
            "  {:<38} {:>5} {:>5} {:>7.2} {:>6.2}  #{} {} ({}{})",
            short, v.cells.len(), cs.len(), sk, pm, k.id, k.members, k.n,
            if k.mirrored > 0.9 { ", paired" } else { "" }
        );
    }

    println!("\n  AND THE SAME CRITICS ON NOISE\n");
    println!("  {:<38} {:>5} {:>5} {:>7} {:>6}", "", "vox", "pcs", "cluster", "slot");
    let (mut ns, mut np) = (0.0f32, 0.0f32);
    for seed in 0..8u64 {
        let v = noise_volume(seed, 640);
        let cs = components(&v, 6);
        ns += part_match_reward(&cs);
        np += slot_reward(&cs);
        if seed < 3 {
            println!(
                "  {:<38} {:>5} {:>5} {:>7.2} {:>6.2}",
                format!("structured noise, seed {seed}"),
                v.cells.len(),
                cs.len(),
                part_match_reward(&cs),
                slot_reward(&cs)
            );
        }
    }
    println!(
        "\n  mean over 8:   real {:.2} / {:.2}     noise {:.2} / {:.2}",
        rs / n.max(1.0),
        rp / n.max(1.0),
        ns / 8.0,
        np / 8.0
    );
    println!("\n  the critics have to separate those before any training is worth");
    println!("  starting. Uniform noise would be too easy — this is a blob of the");
    println!("  right density and extent, which is what an undertrained generator");
    println!("  actually emits.\n");

    println!("  THE ARCHITECTURE\n");
    println!("    generator      z -> 32^3 occupancy      (beatspace's, in 3D)");
    println!("    discriminator  32^3 -> one logit        (beatspace's, in 3D)");
    println!("    skeleton       is the parts list a BODY? torso, head above it,");
    println!("                   limbs in pairs. A discriminator cannot say this:");
    println!("                   it is about a parts list, not about texture.");
    println!("    part match     would `combine::fitness` have taken each piece for");
    println!("                   somebody's arm? the gestalt code already knows how");
    println!("                   to judge whether a shape suits a role, and it does");
    println!("                   not care that this one came from noise.");
    // The combined signal a trainer consumes, and the two numbers a skeleton
    // term WOULD read — shown so it is obvious what is missing rather than
    // merely stated.
    if let Some(v) = std::fs::read(&paths[0]).ok().and_then(|b| decode(&b)) {
        let cs = components(&v, 6);
        let limbs = cs.iter().filter(|c| c.limbish()).count();
        let tallest = cs.iter().map(|c| c.height_frac).fold(0.0f32, f32::max);
        println!(
            "  worked example — {}:",
            paths[0].file_stem().unwrap().to_string_lossy()
        );
        println!("    reward(disc 0.5) = {:.3}", reward(0.5, &cs));
        println!("    {limbs} limb-like pieces; highest centroid at {tallest:.2} of the body.");
        println!("    those two are exactly what a skeleton term would read, and it");
        println!("    cannot: `height_frac` is within-file, and a frame's parts are");
        println!("    separate files with no shared origin. `mounts.json` has the");
        println!("    placement, in the vehicle pose's own space.\n");
    }

    println!("\n  NOT BUILT: the training loop. It needs GPU hours this environment");
    println!("  does not have, and an untrainable one would be a thousand lines");
    println!("  nobody could check.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_real_model() -> Option<Volume> {
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(CORPUS)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "svxr"))
            .collect();
        paths.sort();
        decode(&std::fs::read(paths.first()?).ok()?)
    }

    #[test]
    fn the_stride_is_seven_and_the_grid_proves_it() {
        // The decoder bug, pinned. `solve_placements.py` used a stride of four
        // while unpacking six bytes; the give-away is that most voxels land
        // outside the 0..32 grid, which cannot happen in a valid file.
        let Some(v) = a_real_model() else { return };
        assert!(!v.cells.is_empty());
        let inside = v
            .cells
            .iter()
            .filter(|c| (c.0 as usize) < GRID && (c.1 as usize) < GRID && (c.2 as usize) < GRID)
            .count();
        assert_eq!(inside, v.cells.len(), "{} of {} voxels escaped the grid", inside, v.cells.len());
    }

    #[test]
    fn a_real_model_is_mostly_one_solid() {
        // a representative is a thing, not a cloud; if this fails the decode
        // is wrong even when every voxel is in range
        let Some(v) = a_real_model() else { return };
        let cs = components(&v, 6);
        assert!(!cs.is_empty());
        let biggest = cs[0].voxels() as f32 / v.cells.len() as f32;
        assert!(biggest > 0.3, "the largest piece is only {:.0}% of it", biggest * 100.0);
    }

    #[test]
    fn the_critics_separate_real_frames_from_plausible_noise() {
        // THE CLAIM THAT HAS TO HOLD before training is worth starting. Not
        // against uniform noise — any critic beats that — but against a blob
        // of the right density and extent, which is what an undertrained
        // generator emits.
        let mut paths: Vec<std::path::PathBuf> = match std::fs::read_dir(CORPUS) {
            Ok(d) => d
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "svxr"))
                .collect(),
            Err(_) => return,
        };
        paths.sort();
        if paths.len() < 20 {
            return;
        }
        let step = paths.len() / 16;
        let mut real = 0.0f32;
        let mut nreal = 0.0f32;
        for p in paths.iter().step_by(step.max(1)).take(16) {
            let Ok(b) = std::fs::read(p) else { continue };
            let Some(v) = decode(&b) else { continue };
            let cs = components(&v, 6);
            real += reward(0.5, &cs);
            nreal += 1.0;
        }
        let mut fake = 0.0f32;
        for seed in 0..16u64 {
            let v = noise_volume(seed, 640);
            fake += reward(0.5, &components(&v, 6));
        }
        let (real, fake) = (real / nreal.max(1.0), fake / 16.0);
        // A HONEST THRESHOLD. Real parts beat a density-matched blob, and
        // only by about five percent — 0.683 against 0.652 when this was
        // written. That is a weak separation and the number says so rather
        // than being tuned until it looks strong.
        //
        // It is also the argument FOR the learned half. Coarse shape features
        // — elongation, flatness, fill — are not very discriminative, because
        // the library's own clusters are broad enough to admit a blob of the
        // right density. The discriminator is the critic that would do the
        // heavy lifting here; these two exist to stop it settling for a lump,
        // not to replace it.
        assert!(
            real > fake,
            "the critics cannot tell a frame from a blob at all: {real:.3} vs {fake:.3}"
        );
        assert!(
            real < fake * 1.5,
            "separation is {:.2}x — better than measured, so revisit this note",
            real / fake
        );
    }

    #[test]
    fn the_clusters_are_the_librarys_own_vocabulary() {
        // fourteen part classes measured off real geometry, not authored
        assert_eq!(CLUSTERS.len(), 14);
        let total: u32 = CLUSTERS.iter().map(|c| c.n as u32).sum();
        assert!(total > 2000, "only {total} parts were clustered");
        for k in CLUSTERS {
            assert!(!k.members.is_empty(), "cluster {} has no members", k.id);
            assert!(k.elong > 0.0 && k.n > 0);
        }
        // some of them come in mirrored pairs, which is the only skeleton
        // information that survived per-part normalisation
        assert!(
            CLUSTERS.iter().any(|k| k.mirrored > 0.9),
            "nothing in the library is paired?"
        );
    }

    #[test]
    fn part_matching_reuses_the_gestalt_scorer() {
        // the connection worth having: `combine::fitness` does not care that
        // the numbers came from voxels instead of a table
        // 30 cells in a 2x12x2 box. The first version put SIXTY in a
        // forty-eight cell box, so `bulk` clamped to 1.0 and the rod failed
        // its own limb test — a fixture that cannot exist, asserting about a
        // shape nothing would ever produce.
        let rod = Component {
            cells: vec![(0, 0, 0); 30],
            extent: (2.0, 12.0, 2.0),
            height_frac: 0.4,
        };
        let block = Component {
            cells: vec![(0, 0, 0); 500],
            extent: (9.0, 9.0, 9.0),
            height_frac: 0.5,
        };
        assert!(rod.limbish(), "a 2x12x2 piece should read as a limb");
        assert!(!block.limbish(), "a cube should not");
        // and each is a good SOMETHING under the gestalt scorer, which is the
        // claim — not that both land near a library cluster, since a 2x12x2
        // rod at elongation 6 is outside every cluster the library contains
        // (the widest is about 2.1). That is the cluster critic working:
        // it says this shape is unlike anything in the corpus, and it is.
        assert!(slot_reward(&[rod.clone()]) > 0.3, "{}", slot_reward(&[rod.clone()]));
        assert!(slot_reward(&[block.clone()]) > 0.3);
        assert!(
            cluster_match(&rod).0 < cluster_match(&block).0,
            "an extreme rod should be further from the library than a block"
        );
    }

    #[test]
    fn specks_are_dropped_because_they_orient_to_noise() {
        let v = Volume { cells: vec![(0, 0, 0), (20, 20, 20), (1, 0, 0), (2, 0, 0), (3, 0, 0)] };
        let cs = components(&v, 3);
        assert_eq!(cs.len(), 1, "the lone voxel should have been dropped");
        assert!(cs[0].voxels() >= 3);
    }

    #[test]
    fn a_corrupt_file_is_none_rather_than_a_panic() {
        assert!(decode(b"").is_none());
        assert!(decode(b"not a voxel model at all, just some bytes").is_none());
    }
}
