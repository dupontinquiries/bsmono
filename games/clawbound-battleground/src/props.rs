// Static world props: trees, rocks, crystals, braziers, base gates, fountains.
// All props are merged into one static voxel mesh at startup (single draw).

use crate::voxel::{Grid, VoxelMesh};

fn solid(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

pub fn pine(s: f32) -> VoxelMesh {
    let mut g = Grid::new(12, 26, 12);
    let c = 6;
    g.cyl_y(c as f32, c as f32, 1.6, 0, 5, solid([0.36, 0.26, 0.16]));
    let leaf = [0.14, 0.38, 0.20];
    let leaf2 = [0.10, 0.30, 0.16];
    g.cone_y(c as f32, c as f32, 5.0 * s, 1.2, 4, 11, solid(leaf));
    g.cone_y(c as f32, c as f32, 4.2 * s, 1.0, 10, 17, solid(leaf2));
    g.cone_y(c as f32, c as f32, 3.2 * s, 0.6, 16, 23, solid(leaf));
    g.mesh(0.6, true)
}

pub fn oak(s: f32) -> VoxelMesh {
    let mut g = Grid::new(16, 22, 16);
    let c = 8;
    g.cyl_y(c as f32, c as f32, 1.8, 0, 8, solid([0.4, 0.29, 0.18]));
    g.ellipsoid(c as f32, 13.0, c as f32, 6.5 * s, 4.6 * s, 6.5 * s, solid([0.22, 0.45, 0.18]));
    g.ellipsoid((c - 3) as f32, 11.0, (c + 2) as f32, 4.0 * s, 3.0 * s, 4.0 * s, solid([0.16, 0.38, 0.14]));
    g.ellipsoid((c + 3) as f32, 12.0, (c - 2) as f32, 3.6 * s, 2.8 * s, 3.6 * s, solid([0.26, 0.50, 0.20]));
    g.mesh(0.6, true)
}

pub fn rock(s: f32) -> VoxelMesh {
    let mut g = Grid::new(10, 8, 10);
    g.ellipsoid(5.0, 3.0, 5.0, 4.5 * s, 2.6 * s, 4.2 * s, solid([0.40, 0.38, 0.36]));
    g.ellipsoid(3.5, 4.0, 4.0, 2.4 * s, 1.8 * s, 2.4 * s, solid([0.48, 0.46, 0.44]));
    // moss creeping over the top
    g.ellipsoid(4.0, 5.2, 4.5, 3.0 * s, 0.8, 3.0 * s, solid(MOSS));
    g.mesh(0.6, true)
}

/// A cluster of river-worn boulders: three rounded stones, wet-dark at the
/// base, lichen on the shoulders.
pub fn boulder_cluster(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(18, 10, 18);
    let stone = [0.44, 0.42, 0.40];
    let dark = [0.30, 0.29, 0.30];
    let lichen = [0.55, 0.58, 0.36];
    let offs = [(6.0, 6.0, 1.0), (11.0, 9.0, 0.8), (7.5, 12.0, 0.65), (12.5, 4.5, 0.5)];
    for (i, (x, z, sc)) in offs.iter().enumerate() {
        if (variant + i as i32) % 5 == 4 {
            continue;
        }
        let r = 3.6 * sc * s;
        g.ellipsoid(*x, r * 0.75, *z, r, r * 0.8, r * 1.1, solid(stone));
        g.ellipsoid(*x, r * 0.3, *z, r * 1.05, r * 0.35, r * 1.15, solid(dark));
        g.ellipsoid(*x + 0.6, r * 1.4, *z - 0.4, r * 0.55, 0.6, r * 0.55, solid(lichen));
    }
    g.mesh(0.6, true)
}

/// A curtain of hanging vines from a canopy branch, with leaves and a few
/// trumpet flowers — two poles hold the branch so it reads on its own.
pub fn vine_curtain(variant: i32) -> VoxelMesh {
    let mut g = Grid::new(20, 30, 8);
    let bark = [0.30, 0.215, 0.125];
    let vine = [0.16, 0.42, 0.16];
    let vine_d = [0.12, 0.32, 0.13];
    let leaf = [0.30, 0.55, 0.20];
    let bloom = if variant % 2 == 0 { [0.95, 0.45, 0.55] } else { [0.95, 0.75, 0.3] };
    // branch across the top, on two crooked poles
    g.filled_box(1, 24, 3, 18, 25, 4, solid(bark));
    g.filled_box(1, 0, 3, 2, 24, 4, solid(bark));
    g.filled_box(17, 0, 3, 18, 24, 4, solid(bark));
    for k in 0..9 {
        let x = 2 + k * 2 + (variant + k) % 2;
        let len = 8 + ((variant * 7 + k * 13) % 14);
        for i in 0..len {
            let y = 24 - i;
            let sway = ((i as f32 * 0.5 + k as f32).sin() * 1.2) as i32;
            let z = 3 + sway;
            g.set(x, y, z, solid(if i % 3 == 0 { vine_d } else { vine }));
            if i % 4 == 2 {
                g.set(x + 1, y, z, solid(leaf));
                g.set(x - 1, y - 1, z + 1, solid(leaf));
            }
        }
        if (variant + k) % 3 == 0 {
            g.ellipsoid(x as f32, (24 - len) as f32, 3.5, 1.2, 1.0, 1.2, solid(bloom));
        }
    }
    g.mesh(0.55, true)
}

pub fn crystal(team: [f32; 3]) -> VoxelMesh {
    let mut g = Grid::new(8, 18, 8);
    let glow = mix(team, [1.0, 1.0, 1.0], 0.3);
    g.cone_y(4.0, 4.0, 2.6, 0.7, 2, 13, solid(glow));
    g.cone_y(4.0, 4.0, 2.0, 0.5, 0, 4, solid(mix(team, [0.1, 0.1, 0.2], 0.4)));
    g.cone_y(2.0, 6.0, 1.4, 0.4, 1, 8, solid(mix(glow, [0.2, 0.2, 0.3], 0.3)));
    g.mesh(0.35, true)
}

pub fn brazier() -> VoxelMesh {
    let mut g = Grid::new(8, 12, 8);
    let c = 4;
    g.filled_box(c - 1, 0, c - 1, c + 1, 7, c + 1, solid([0.3, 0.26, 0.24]));
    g.cyl_y(c as f32, c as f32, 2.8, 7, 8, solid([0.42, 0.36, 0.30]));
    g.cyl_y(c as f32, c as f32, 2.4, 8, 9, solid([0.15, 0.13, 0.12]));
    g.mesh(0.12, true)
}

/// Base gate: monumental archway behind the titan.
pub fn base_gate(team: [f32; 3]) -> VoxelMesh {
    let mut g = Grid::new(52, 40, 22);
    let cx = 26;
    let cz = 11;
    let stone = [0.34, 0.33, 0.36];
    let stone_d = [0.24, 0.24, 0.27];
    let trim = mix(team, [0.9, 0.9, 0.95], 0.25);

    // pillars
    g.filled_box(cx - 22, 0, cz - 7, cx - 14, 30, cz + 7, solid(stone));
    g.filled_box(cx + 14, 0, cz - 7, cx + 22, 30, cz + 7, solid(stone));
    for y in (4..30).step_by(4) {
        g.filled_box(cx - 23, y, cz - 8, cx + 23, y, cz + 8, solid(stone_d));
    }
    // arch
    for i in 0..8 {
        let t = i as f32 / 7.0;
        let half = (18.0 * (1.0 - t * t).sqrt()) as i32;
        g.filled_box(cx - half - 1, 30 + i * 2, cz - 7, cx + half + 1, 31 + i * 2, cz + 7, solid(stone));
    }
    // banner drapes
    g.filled_box(cx - 21, 10, cz + 7, cx - 15, 26, cz + 8, solid(team));
    g.filled_box(cx + 15, 10, cz + 7, cx + 21, 26, cz + 8, solid(team));
    g.filled_box(cx - 14, 12, cz + 7, cx - 15, 24, cz + 8, solid(trim));
    g.filled_box(cx + 14, 12, cz + 7, cx + 15, 24, cz + 8, solid(trim));
    // steps
    g.filled_box(cx - 26, 0, cz - 11, cx + 26, 1, cz + 11, solid(stone_d));
    g.mesh(0.24, true)
}

/// Fountain: circular pool with central spire (heal particles spawn here).
pub fn fountain(team: [f32; 3]) -> VoxelMesh {
    let mut g = Grid::new(26, 12, 26);
    let c = 13;
    let water = mix(team, [0.5, 0.8, 1.0], 0.5);
    g.cyl_y(c as f32, c as f32, 11.0, 0, 1, solid([0.35, 0.34, 0.36]));
    g.cyl_y(c as f32, c as f32, 10.0, 1, 2, solid(mix(water, [0.0, 0.0, 0.0], 0.35)));
    g.cyl_y(c as f32, c as f32, 3.0, 2, 8, solid([0.4, 0.39, 0.41]));
    g.cyl_y(c as f32, c as f32, 5.0, 8, 9, solid(water));
    g.cyl_y(c as f32, c as f32, 2.0, 9, 10, solid(mix(team, [1.0, 1.0, 1.0], 0.4)));
    g.mesh(0.2, true)
}

/// Titan pedestal.
pub fn titan_pedestal(team: [f32; 3]) -> VoxelMesh {
    let mut g = Grid::new(30, 8, 30);
    let c = 15;
    g.filled_box(2, 0, 2, 27, 2, 27, solid([0.30, 0.29, 0.31]));
    g.filled_box(5, 2, 5, 24, 4, 24, solid([0.38, 0.37, 0.39]));
    g.cyl_y(c as f32, c as f32, 10.0, 4, 5, solid(mix(team, [0.2, 0.2, 0.25], 0.5)));
    g.cyl_y(c as f32, c as f32, 9.0, 5, 7, solid([0.42, 0.41, 0.43]));
    g.mesh(0.22, true)
}

/// Lane pathway slabs (decorative pads under each lane polyline).
pub fn lane_pad() -> VoxelMesh {
    let mut g = Grid::new(10, 1, 10);
    g.filled_box(0, 0, 0, 9, 0, 9, solid([0.44, 0.42, 0.38]));
    // tiled pattern
    for (x, z) in [(2usize, 2usize), (2, 7), (7, 2), (7, 7)] {
        g.set(x as i32, 0, z as i32, solid([0.36, 0.34, 0.30]));
    }
    g.mesh(0.9, false)
}

// ---------------------------------------------------------------------------
// Indiana-jones jungle flora
// ---------------------------------------------------------------------------

const MOSS: [f32; 3] = [0.20, 0.42, 0.18];
const STONE: [f32; 3] = [0.37, 0.36, 0.34];
const STONE_D: [f32; 3] = [0.28, 0.275, 0.26];

/// Massive broadleaf canopy tree with buttress roots and hanging vines.
/// variant picks canopy shape/vine layout deterministically.
pub fn jungle_tree(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(22, 26, 22);
    let c = 11.0;
    let bark = [0.30, 0.215, 0.125];
    let bark_d = [0.24, 0.17, 0.10];
    // trunk
    g.cyl_y(c, c, 2.6, 0, 9, solid(bark));
    g.cyl_y(c, c, 2.1, 9, 14, solid(bark_d));
    // buttress roots
    for k in 0..4 {
        let a = variant as f32 * 0.9 + k as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
        let dx = a.cos();
        let dz = a.sin();
        for i in 0..5 {
            let r = 2.6 + i as f32 * 1.35;
            let h = (7 - i * 2).max(2);
            let x = (c + dx * r) as i32;
            let z = (c + dz * r) as i32;
            g.filled_box(x - 1, 0, z - 1, x + 1, h, z + 1, solid(bark_d));
        }
        // moss collar at the root base
        g.ellipsoid(c + dx * 4.0, 1.0, c + dz * 4.0, 2.2, 1.0, 2.2, solid(MOSS));
    }
    // canopy: layered blobs, two-tone
    let leaf_a = [0.095, 0.315, 0.115];
    let leaf_b = [0.135, 0.395, 0.145];
    g.ellipsoid(c, 19.5, c, 8.5 * s, 3.6 * s, 8.5 * s, solid(leaf_a));
    g.ellipsoid(c - 4.0, 17.5, c + 3.0, 5.2 * s, 2.8 * s, 5.2 * s, solid(leaf_b));
    g.ellipsoid(c + 4.5, 18.5, c - 3.5, 4.8 * s, 2.6 * s, 4.8 * s, solid(leaf_b));
    g.ellipsoid(c, 22.0, c, 5.5 * s, 2.6 * s, 5.5 * s, solid(leaf_a));
    // hanging vines from the canopy rim
    let vins = [(-7.0, 1.0, 6), (7.5, -2.0, 5), (1.0, 7.5, 7), (-2.0, -7.0, 5)];
    for (i, (vx, vz, len)) in vins.iter().enumerate() {
        if (variant + i as i32) % 2 == 0 {
            continue;
        }
        let x = (c + vx * s) as i32;
        let z = (c + vz * s) as i32;
        for y in 14..14 + len {
            g.set(x, y, z, solid([0.16, 0.40, 0.16]));
            if y % 3 == 0 {
                g.set(x + 1, y, z, solid([0.14, 0.36, 0.14]));
            }
        }
        g.set(x, 14 + len, z, solid([0.30, 0.55, 0.20])); // tip leaf
    }
    g.mesh(0.6, true)
}

/// Curved trunk palm with radiating fronds and coconuts.
pub fn palm(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(16, 30, 16);
    let c = 8.0;
    let bend = if variant % 2 == 0 { 1.0 } else { -1.0 };
    // trunk: progressive offset gives the classic lean
    for y in 0..19 {
        let t = y as f32 / 19.0;
        let x = c + bend * t * t * 3.2;
        g.filled_box(x as i32 - 1, y, c as i32 - 1, x as i32, y + 1, c as i32, solid([0.42, 0.31, 0.18]));
    }
    let top_x = c + bend * 3.2;
    let top_y = 19;
    // fronds
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU + variant as f32 * 0.35;
        let col = if k % 2 == 0 { [0.145, 0.42, 0.17] } else { [0.11, 0.36, 0.14] };
        for i in 1..7 {
            let r = i as f32 * 1.15 * s;
            let y = top_y as f32 + 2.2 - i as f32 * 0.85;
            let x = (top_x + a.cos() * r) as i32;
            let z = (c + a.sin() * r) as i32;
            g.set(x, y as i32, z, solid(col));
            g.set(x, y as i32 - 1, z, solid(col));
        }
    }
    // coconuts
    for (dx, dz) in [(-1.0, 0.0), (1.2, 0.6), (0.2, -1.3)] {
        g.set((top_x + dx) as i32, top_y, (c + dz) as i32, solid([0.30, 0.21, 0.11]));
    }
    g.mesh(0.6, true)
}

/// Strangler fig: host trunk wrapped in aerial roots under a wide flat canopy.
pub fn strangler_fig(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(26, 28, 26);
    let c = 13.0;
    let bark = [0.27, 0.20, 0.13];
    // host trunk
    g.cyl_y(c, c, 2.4, 0, 16, solid(bark));
    // aerial roots arcing outward
    for k in 0..6 {
        let a = k as f32 / 6.0 * std::f32::consts::TAU + variant as f32 * 0.55;
        for y in 0..17 {
            let t = y as f32 / 17.0;
            let r = 2.6 + (1.0 - t) * 5.6 * s;
            let x = (c + a.cos() * r) as i32;
            let z = (c + a.sin() * r) as i32;
            g.set(x, y, z, solid(bark));
            g.set(x + 1, y, z, solid([0.23, 0.17, 0.11]));
        }
        g.ellipsoid(c + a.cos() * 2.8, 0.6, c + a.sin() * 2.8, 1.8, 0.9, 1.8, solid(MOSS));
    }
    // wide flat canopy
    let leaf_a = [0.10, 0.33, 0.12];
    let leaf_b = [0.14, 0.41, 0.15];
    g.ellipsoid(c, 19.0, c, 11.0 * s, 3.0 * s, 11.0 * s, solid(leaf_a));
    g.ellipsoid(c - 5.0, 21.0, c - 2.0, 5.0 * s, 2.2 * s, 5.0 * s, solid(leaf_b));
    g.ellipsoid(c + 4.0, 21.5, c + 4.0, 4.4 * s, 2.2 * s, 4.4 * s, solid(leaf_b));
    g.mesh(0.6, true)
}

/// Bamboo clump: thin segmented culms with leaf tufts.
pub fn bamboo(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(12, 30, 12);
    let c = 6.0;
    let culm = [0.34, 0.52, 0.21];
    let ring = [0.44, 0.62, 0.28];
    for k in 0..7 {
        let a = k as f32 * 2.4 + variant as f32;
        let r = 0.8 + (k % 3) as f32 * 1.1;
        let bx = (c + a.cos() * r) as i32;
        let bz = (c + a.sin() * r) as i32;
        let top = 16 + ((k * 5 + variant) % 9);
        for y in 0..top {
            let cc = if y % 3 == 2 { ring } else { culm };
            g.set(bx, y, bz, solid(cc));
        }
        // leaf tufts
        for (dx, dz, dy) in [(1, 0, 0), (-1, 1, -1), (0, -1, 1)] {
            g.set(bx + dx, top + dy, bz + dz, solid([0.24, 0.46, 0.16]));
        }
    }
    let _ = s;
    g.mesh(0.6, true)
}

/// Ground fern rosette.
pub fn fern(variant: i32) -> VoxelMesh {
    let mut g = Grid::new(11, 6, 11);
    let c = 5.0;
    for k in 0..7 {
        let a = k as f32 / 7.0 * std::f32::consts::TAU + variant as f32 * 0.8;
        let col = if k % 2 == 0 { [0.13, 0.38, 0.13] } else { [0.17, 0.45, 0.16] };
        for i in 1..5 {
            let r = i as f32 * 1.05;
            let y = (2.2 - i as f32 * 0.35) as i32;
            g.set((c + a.cos() * r) as i32, y, (c + a.sin() * r) as i32, solid(col));
        }
    }
    g.set(c as i32, 2, c as i32, solid([0.30, 0.52, 0.18]));
    g.mesh(0.35, true)
}

/// Leafy undergrowth bush with occasional berries/blooms.
pub fn bush(variant: i32) -> VoxelMesh {
    let mut g = Grid::new(10, 7, 10);
    let c = 5.0;
    g.ellipsoid(c, 2.6, c, 4.4, 2.6, 4.4, solid([0.11, 0.31, 0.12]));
    g.ellipsoid(c - 1.5, 3.4, c + 1.0, 2.6, 1.8, 2.6, solid([0.15, 0.38, 0.15]));
    if variant % 3 == 0 {
        for (bx, bz) in [(3, 4), (6, 5), (5, 7), (7, 3)] {
            g.set(bx, 3, bz, solid([0.75, 0.30, 0.20])); // red bloom
        }
    } else if variant % 3 == 1 {
        for (bx, bz) in [(4, 6), (6, 4)] {
            g.set(bx, 4, bz, solid([0.85, 0.72, 0.30])); // pale flower
        }
    }
    g.mesh(0.6, true)
}

/// Mossy fallen log.
/// The river-crossing log. Shape constants come from `logs`, which also
/// computes footing from them. Bark with ridges, a PALE WORN STRIP down the
/// spine where paws go, end grain, a few knots -- and moss only in patches
/// low on the flanks, where a log that gets walked on keeps it.
pub fn crossing_log() -> VoxelMesh {
    use crate::logs::{LOG_HALF_LEN, LOG_VOXEL, SECTION_A, SECTION_B};
    let half = (LOG_HALF_LEN / LOG_VOXEL) as i32; // 22
    // ODD in y and z, so one column is dead centre. With an even count the
    // mesh centres BETWEEN two columns and is drawn 0.15 u to one side of the
    // footing computed for it.
    let (nx, ny, nz) = (half * 2, (SECTION_B as usize) * 2 + 3, (SECTION_A as usize) * 2 + 3);
    let mut g = Grid::new(nx as usize, ny, nz);
    let (cy, cz) = ((ny / 2) as i32, (nz / 2) as i32);
    let h = |a: i32, b: i32| ((a.wrapping_mul(73856093) ^ b.wrapping_mul(19349663)) as u32 % 1000) as f32 / 1000.0;
    for x in 0..nx {
        for j in -(SECTION_B as i32 + 1)..=(SECTION_B as i32 + 1) {
            for k in -(SECTION_A as i32 + 1)..=(SECTION_A as i32 + 1) {
                let (fj, fk) = (j as f32 / SECTION_B, k as f32 / SECTION_A);
                let r2 = fj * fj + fk * fk;
                if r2 > 1.0 {
                    continue;
                }
                let end = x < 1 || x >= nx - 1;
                let ridge = 0.86 + 0.22 * h(x / 3, k * 7 + j);
                let mut c = [0.33 * ridge, 0.245 * ridge, 0.15 * ridge];
                if end {
                    // end grain: rings
                    let ring = ((r2.sqrt() * 5.0) as i32) % 2 == 0;
                    c = if ring { [0.62, 0.50, 0.33] } else { [0.50, 0.39, 0.25] };
                } else if j >= (SECTION_B * 0.8) as i32 && k.abs() <= 1 && h(x, k) > 0.18 {
                    // The worn path down the spine: narrow, broken, and only a
                    // shade paler than the bark. Five voxels wide and much
                    // paler, it read as a sawn plank.
                    let w = 0.9 + 0.15 * h(x, k);
                    c = [0.45 * w, 0.35 * w, 0.22 * w];
                } else if j <= 0 && k.abs() >= 4 && h(x / 5, k.signum() * 3) > 0.62 {
                    // moss, low on a flank, in patches
                    c = [MOSS[0] * 0.9, MOSS[1] * 0.9, MOSS[2] * 0.9];
                } else if h(x / 2, j * 5 + k) > 0.965 {
                    c = [0.20, 0.145, 0.09]; // a knot
                }
                g.set(x, cy + j, cz + k, solid(c));
            }
        }
    }
    g.mesh(LOG_VOXEL, true)
}

pub fn fallen_log(s: f32) -> VoxelMesh {
    let mut g = Grid::new(24, 7, 9);
    let bark = [0.31, 0.23, 0.14];
    g.ellipsoid(12.0, 2.6, 4.5, 11.0 * s, 2.3, 3.4, solid(bark));
    g.ellipsoid(12.0, 4.6, 4.5, 9.5 * s, 0.9, 2.9, solid(MOSS));
    // hollow ends
    g.ellipsoid(1.5, 2.6, 4.5, 1.2, 1.4, 1.8, solid([0.14, 0.10, 0.07]));
    g.ellipsoid(22.5, 2.6, 4.5, 1.2, 1.4, 1.8, solid([0.14, 0.10, 0.07]));
    // shelf fungus
    g.ellipsoid(8.0, 4.4, 7.6, 1.4, 0.7, 1.0, solid([0.62, 0.48, 0.28]));
    g.ellipsoid(17.0, 4.4, 1.6, 1.3, 0.7, 1.0, solid([0.60, 0.44, 0.24]));
    g.mesh(0.6, true)
}

// ---------------------------------------------------------------------------
// Overgrown ruins
// ---------------------------------------------------------------------------

/// Broken mossy column.
pub fn ruin_pillar(_s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(9, 26, 9);
    let c = 4.5;
    g.filled_box(1, 0, 1, 7, 1, 7, solid(STONE_D));
    let top = 8 + variant % 4 * 3;
    let mut y = 2;
    while y < top {
        let seg_h = 3.min(top - y);
        let lean = ((y / 3) % 2) as f32 * 0.4 * if variant % 2 == 0 { 1.0 } else { -1.0 };
        g.cyl_y(c + lean, c, 2.2, y, y + seg_h, solid(STONE));
        y += seg_h;
    }
    // tilted fallen top piece
    g.cyl_y(c + 1.6, c + 0.8, 2.0, top, top + 2, solid(STONE_D));
    // moss creeping up
    for yy in (2..top).step_by(2) {
        g.set(2, yy, 3, solid(MOSS));
        g.set(6, yy + 1, 6, solid([0.17, 0.38, 0.15]));
    }
    g.mesh(0.6, true)
}

/// Vine-draped stone archway (walk-under).
pub fn ruin_arch(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(34, 26, 10);
    let mossy = variant % 2 == 0;
    // pillars
    g.filled_box(3, 0, 2, 8, 15, 7, solid(STONE));
    g.filled_box(25, 0, 2, 30, 15, 7, solid(STONE));
    // arch courses
    for i in 0..10 {
        let t = i as f32 / 9.0;
        let half = (11.0 * (1.0 - t * t).sqrt()) as i32;
        g.filled_box(16 - half - 1, 15 + i, 2, 16 + half + 1, 16 + i, 7, solid(if i % 3 == 0 { STONE_D } else { STONE }));
    }
    // keystone
    g.filled_box(15, 24, 2, 18, 25, 7, solid(STONE_D));
    // vines hanging from the arch
    for k in 0..7 {
        let vx = 6 + (k * 4 + variant) % 22;
        let len = 3 + (k * 3 + variant) % 7;
        for y in 0..len {
            let yy = 23 - y;
            if yy < 4 {
                break;
            }
            g.set(vx, yy, 7, solid([0.17, 0.40, 0.15]));
            if k % 2 == 0 {
                g.set(vx, yy, 2, solid([0.14, 0.36, 0.13]));
            }
        }
    }
    if mossy {
        for xx in (3..9).step_by(2) {
            g.set(xx, 1, 2, solid(MOSS));
            g.set(31 - (xx - 3), 2, 7, solid([0.16, 0.37, 0.14]));
        }
    }
    let _ = s;
    g.mesh(0.6, true)
}

/// Crumbled L-shaped overgrown wall.
pub fn ruin_wall(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(24, 12, 20);
    let _ = variant;
    for x in 0..20 {
        let h = 6 - ((x * 5) % 4);
        for y in 0..h {
            for z in 0..3 {
                let moss = y < 2 || (x % 5 == 0 && y < h - 1);
                g.set(x, y, z, solid(if moss { MOSS } else { STONE }));
            }
        }
    }
    for z in 3..16 {
        let h = 5 - ((z * 3) % 3);
        for y in 0..h {
            for x in 0..3 {
                let moss = y < 2 || (z % 4 == 0);
                g.set(x, y, z, solid(if moss { [0.18, 0.40, 0.16] } else { STONE_D }));
            }
        }
    }
    let _ = s;
    g.mesh(0.6, true)
}

/// Small weathered stone head (mini colossus), moss-streaked.
pub fn stone_head(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(14, 20, 12);
    let _ = variant;
    // shoulders + head
    g.filled_box(2, 0, 2, 11, 3, 9, solid(STONE_D));
    g.ellipsoid(7.0, 9.0, 5.5, 4.4, 6.0, 3.4, solid(STONE));
    // brow + nose
    g.filled_box(3, 11, 2, 10, 12, 4, solid(STONE_D));
    g.filled_box(6, 7, 1, 7, 11, 3, solid(STONE_D));
    // eyes
    g.set(4, 10, 2, solid([0.10, 0.10, 0.12]));
    g.set(9, 10, 2, solid([0.10, 0.10, 0.12]));
    // mouth
    g.filled_box(5, 5, 1, 8, 5, 2, solid(STONE_D));
    // moss streaks
    g.set(3, 8, 2, solid(MOSS));
    g.set(10, 6, 2, solid([0.17, 0.38, 0.15]));
    g.set(5, 1, 8, solid(MOSS));
    let _ = s;
    g.mesh(0.6, true)
}

/// Wooden plank bridge over the river (deck spans +X, arches slightly).
pub fn bridge(len_cells: usize) -> VoxelMesh {
    let mut g = Grid::new(len_cells, 9, 11);
    let n = len_cells as i32;
    let plank_a = [0.45, 0.34, 0.20];
    let plank_b = [0.38, 0.28, 0.16];
    for x in 0..n {
        let t = x as f32 / (n - 1).max(1) as f32;
        let arch = (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0) * 2.2;
        let y = 2 + arch as i32;
        let col = if x % 2 == 0 { plank_a } else { plank_b };
        g.filled_box(x, y, 3, x, y, 7, solid(col));
        // support posts near the banks
        if x == 1 || x == n - 2 {
            g.filled_box(x, 0, 3, x, y, 3, solid([0.34, 0.25, 0.15]));
            g.filled_box(x, 0, 7, x, y, 7, solid([0.34, 0.25, 0.15]));
        }
        // rails + rope
        if x % 3 != 2 {
            g.set(x, y + 3, 3, solid([0.30, 0.22, 0.13]));
            g.set(x, y + 3, 7, solid([0.30, 0.22, 0.13]));
        }
        g.set(x, y + 2, 3, solid([0.52, 0.44, 0.30]));
        g.set(x, y + 2, 7, solid([0.52, 0.44, 0.30]));
    }
    g.mesh(0.6, true)
}

// ---------------------------------------------------------------------------
// Beyond-the-bounds colossal silhouettes
// ---------------------------------------------------------------------------

/// Colossal jungle giant rising from the outer hills.
pub fn mega_tree(s: f32, variant: i32) -> VoxelMesh {
    let mut g = Grid::new(34, 46, 34);
    let c = 17.0;
    let bark = [0.27, 0.20, 0.12];
    g.cyl_y(c, c, 5.0, 0, 16, solid(bark));
    g.cyl_y(c, c, 4.0, 16, 24, solid([0.23, 0.17, 0.10]));
    // flaring roots
    for k in 0..5 {
        let a = k as f32 * 1.26 + variant as f32 * 0.7;
        for i in 0..6 {
            let r = 5.0 + i as f32 * 1.7;
            let h = (12 - i * 2).max(3);
            let x = (c + a.cos() * r) as i32;
            let z = (c + a.sin() * r) as i32;
            g.filled_box(x - 1, 0, z - 1, x + 2, h, z + 2, solid(bark));
        }
    }
    // enormous canopy
    let leaf_a = [0.075, 0.26, 0.095];
    let leaf_b = [0.11, 0.33, 0.12];
    g.ellipsoid(c, 32.0, c, 13.0 * s, 5.0 * s, 13.0 * s, solid(leaf_a));
    g.ellipsoid(c - 7.0, 28.5, c + 4.0, 7.0 * s, 3.4 * s, 7.0 * s, solid(leaf_b));
    g.ellipsoid(c + 6.0, 29.5, c - 6.0, 6.4 * s, 3.2 * s, 6.4 * s, solid(leaf_b));
    g.ellipsoid(c, 37.0, c, 7.5 * s, 3.4 * s, 7.5 * s, solid(leaf_a));
    // hollow the underside for shape + voxel savings
    g.carve_sphere(c, 26.0, c, 9.0);
    // long hanging vines
    for k in 0..6 {
        let a = k as f32 * 1.05 + variant as f32 * 0.4;
        let x = (c + a.cos() * 10.0) as i32;
        let z = (c + a.sin() * 10.0) as i32;
        let len = 6 + (k * 3 + variant) % 6;
        for y in 0..len {
            let yy = 27 - y;
            if yy < 14 {
                break;
            }
            g.set(x, yy, z, solid([0.13, 0.33, 0.12]));
        }
    }
    g.mesh(1.2, true)
}

/// Great stepped temple (hollow shell) — the "what's out there" landmark.
pub fn ziggurat() -> VoxelMesh {
    let mut g = Grid::new(60, 42, 60);
    let c = 30;
    let tiers = 5;
    for i in 0..tiers {
        let half = 27 - i * 5;
        let y0 = 1 + i * 7;
        let stone = if i % 2 == 0 { STONE } else { STONE_D };
        // perimeter walls (hollow inside)
        g.filled_box(c - half, y0, c - half, c + half, y0 + 6, c - half + 1, solid(stone));
        g.filled_box(c - half, y0, c + half - 1, c + half, y0 + 6, c + half, solid(stone));
        g.filled_box(c - half, y0, c - half, c - half + 1, y0 + 6, c + half, solid(stone));
        g.filled_box(c + half - 1, y0, c - half, c + half, y0 + 6, c + half, solid(stone));
        // ledge slab
        g.filled_box(c - half, y0 + 6, c - half, c + half, y0 + 6, c + half, solid(STONE_D));
        // moss on the lower courses
        if i < 3 {
            for x in (c - half..=c + half).step_by(3) {
                g.set(x, y0, c - half, solid(MOSS));
                g.set(x, y0 + 1, c + half, solid([0.17, 0.38, 0.15]));
            }
        }
        // shrubs on the ledges
        g.ellipsoid((c - half + 3) as f32, (y0 + 7) as f32, (c - half + 4) as f32, 2.0, 1.4, 2.0, solid([0.10, 0.30, 0.11]));
        g.ellipsoid((c + half - 4) as f32, (y0 + 7) as f32, (c + half - 5) as f32, 1.8, 1.2, 1.8, solid([0.13, 0.35, 0.13]));
    }
    // grand stair (south face)
    for i in 0..tiers * 7 {
        let w = 3;
        let y = 1 + tiers * 7 - i - 1;
        let step_back = i / 7 * 5;
        let zz = c + 27 - step_back;
        if zz <= c + 28 && zz < 60 {
            g.filled_box(c - w, y, zz, c + w, y + 1, zz + 1, solid(STONE_D));
        }
    }
    // summit shrine
    let top_y = 1 + tiers * 7;
    g.filled_box(c - 5, top_y, c - 5, c + 5, top_y + 5, c + 5, solid(STONE));
    g.filled_box(c - 2, top_y, c + 4, c + 2, top_y + 3, c + 5, solid([0.12, 0.11, 0.10])); // doorway
    g.mesh(1.2, true)
}

/// Colossal moai-like head, half sunk into the hillside.
pub fn colossus_head() -> VoxelMesh {
    let mut g = Grid::new(26, 40, 24);
    let stone = [0.33, 0.325, 0.31];
    let stone_d = [0.25, 0.25, 0.24];
    // head mass
    g.ellipsoid(13.0, 20.0, 11.0, 9.5, 13.5, 8.5, solid(stone));
    // brow ridge
    g.filled_box(4, 26, 2, 21, 28, 6, solid(stone_d));
    // nose
    g.filled_box(11, 16, 1, 14, 25, 4, solid(stone_d));
    // eyes (deep set)
    g.filled_box(6, 24, 2, 9, 25, 4, solid([0.08, 0.08, 0.10]));
    g.filled_box(16, 24, 2, 19, 25, 4, solid([0.08, 0.08, 0.10]));
    // mouth
    g.filled_box(8, 10, 1, 17, 11, 3, solid(stone_d));
    // headdress slab
    g.filled_box(3, 33, 3, 22, 36, 19, solid([0.42, 0.30, 0.20]));
    // buried base
    g.filled_box(2, 0, 2, 23, 6, 20, solid(stone_d));
    // moss streaks down the face
    for (mx, my) in [(5, 22), (20, 18), (13, 8), (7, 12)] {
        for y in 0..6 {
            let yy = my - y;
            if yy > 1 {
                g.set(mx, yy, 1, solid(MOSS));
            }
        }
    }
    g.mesh(1.2, true)
}

/// Broken aqueduct arc — towering ruined channel on the rim.
pub fn aqueduct_arch() -> VoxelMesh {
    let mut g = Grid::new(22, 42, 10);
    let stone = [0.35, 0.345, 0.33];
    let stone_d = [0.27, 0.265, 0.255];
    // two pillars
    g.filled_box(2, 0, 2, 6, 30, 7, solid(stone));
    g.filled_box(15, 0, 2, 19, 30, 7, solid(stone));
    // double arch
    for i in 0..9 {
        let t = i as f32 / 8.0;
        let half = (7.0 * (1.0 - t * t).sqrt()) as i32;
        g.filled_box(10 - half - 1, 30 + i, 2, 11 + half + 1, 31 + i, 7, solid(if i % 2 == 0 { stone_d } else { stone }));
        let half2 = (half as f32 * 0.55) as i32;
        g.filled_box(10 - half2 - 1, 21 + i, 3, 11 + half2 + 1, 22 + i, 6, solid(stone_d));
    }
    // broken channel on top
    g.filled_box(2, 39, 2, 12, 41, 7, solid(stone_d));
    // vines
    for k in 0..5 {
        let vx = 3 + k * 4;
        let len = 4 + k % 5;
        for y in 0..len {
            let yy = 38 - y;
            if yy < 10 {
                break;
            }
            g.set(vx, yy, 2, solid([0.15, 0.37, 0.14]));
        }
    }
    // moss
    for yy in (2..12).step_by(3) {
        g.set(2, yy, 7, solid(MOSS));
        g.set(19, yy + 1, 2, solid([0.16, 0.38, 0.15]));
    }
    g.mesh(1.2, true)
}

/// A small mountain waterfall, `h` world units tall, falling down a cliff
/// face onto the floor: a narrow sheet of pale water, streaked, fraying as
/// it falls, with a skirt of white spray where it lands. Built at the small
/// voxel size (0.35) so the streaks read. `feet_origin`: y = 0 is the pool.
pub fn waterfall(h: f32, variant: i32) -> VoxelMesh {
    const VOX: f32 = 0.35;
    let ny = ((h / VOX).ceil() as usize).clamp(6, 64);
    let mut g = Grid::new(9, ny + 2, 7);
    let pale = [0.72, 0.84, 0.92];
    let deep = [0.46, 0.62, 0.76];
    let foam = [0.93, 0.96, 0.98];
    for y in 0..ny as i32 {
        let t = y as f32 / ny as f32; // 0 at the pool, 1 at the lip
        // narrow at the lip, fraying wider toward the pool
        let half = 1.0 + (1.0 - t) * 1.6;
        for x in 0..9 {
            let dx = x as f32 - 4.0;
            if dx.abs() > half {
                continue;
            }
            // streaks: columns alternate pale and deep, drifting with height
            let streak = ((x as i32 + variant + y / 3) % 3) == 0;
            let c = if streak { deep } else { pale };
            // the sheet is one voxel thick against the cliff (z = 1), with a
            // second layer where it frays so it is not paper from the side
            g.set(x, y, 1, solid(c));
            if t < 0.45 && ((x + y) % 2 == 0) {
                g.set(x, y, 2, solid(mix(c, foam, 0.5)));
            }
        }
    }
    // spray where it lands
    g.ellipsoid(4.0, 0.8, 2.5, 3.6, 1.4, 2.6, solid(foam));
    g.mesh(VOX, true)
}

/// A crag: a cluster of broken rock shards, the way a cliff breaks into
/// strata and blades rather than being one smooth face. 2-4 shards, each a
/// tapering prism with an IRREGULAR polygon section (so it has flat facets
/// and sharp edges, not a round cone), leaning toward local +z -- the
/// direction the placement turns to face out of the wall -- by `lean`
/// (0 = upright spire, 1 = jutting out at ~50 degrees). Lighter on the top
/// facets, dark in the creases at the root, cold grey like the cliffs.
/// `size` ~0.6..1.6. Built with feet at y = 0 and the lean side at +z.
///
/// `vox` is the voxel size: 0.6 for crags seen close, 1.2 for distant
/// spires (same silhouette, an eighth of the instances by volume).
///
/// `yaw` is the lean direction in the model's xz plane (0 = +z, pi/2 = +x):
/// built in, not applied as a quarter turn, so a crag leans exactly out of
/// a curved wall. Shards root at the BACK of the grid (inside the wall the
/// placement puts it against) and grow forward, so the crag emerges from
/// the rock instead of hanging on it.
///
/// `spiky`: the wide, bristling cluster -- more spread, rooted further back,
/// thicker shards. It frames a choke like a gate (preferred there over the
/// slim ribs on a side-by-side look); the ribs suit the long walls.
pub fn crag(size: f32, variant: i32, lean: f32, vox: f32, yaw: f32, spiky: bool) -> VoxelMesh {
    let k = 0.6 / vox.max(0.1);
    let n = ((24.0 * size * k).clamp(6.0, 44.0)) as usize;
    let ny = ((34.0 * size * k).clamp(8.0, 60.0)) as usize;
    let mut g = Grid::new(n, ny, n);
    let h = |k: i32| {
        let x = ((variant * 7919 + k * 104729) as f32 * 0.6180339).sin() * 43758.547;
        x - x.floor()
    };
    let c = n as f32 * 0.5;
    let shards = if spiky { 3 + (h(1) * 2.0) as i32 } else { 2 + (h(1) * 3.0) as i32 };
    for s in 0..shards {
        let k = 10 + s * 13;
        // base near the centre, spread a little; the main shard biggest
        let main = s == 0;
        // spread across the wall (perpendicular to the lean); rooted behind
        // the centre along it -- inside the rock
        let (fx, fz) = (yaw.sin(), yaw.cos());
        // ribs, not boulders: little spread along the wall (a curved wall
        // under a wide crag buries one end and bares the other) and rooted
        // close to the face line
        let across = (h(k) - 0.5) * n as f32 * if spiky { 0.45 } else { 0.32 };
        let back = -(n as f32) * if spiky { 0.18 + 0.12 * h(k + 1) } else { 0.04 + 0.06 * h(k + 1) };
        let bx = c + fz * across + fx * back;
        let bz = c - fx * across + fz * back;
        let len = ny as f32 * if main { 0.9 } else { 0.45 + h(k + 2) * 0.35 };
        let tilt = (0.15 + lean * 0.75) * (0.7 + h(k + 3) * 0.6);
        let yaw_s = yaw + (h(k + 4) - 0.5) * 0.6; // out of the wall, give or take
        let dir = glam::Vec3::new(tilt.sin() * yaw_s.sin(), tilt.cos(), tilt.sin() * yaw_s.cos()).normalize();
        let r0 = n as f32 * match (spiky, main) {
            (true, true) => 0.22,
            (true, false) => 0.14 + h(k + 5) * 0.08,
            (false, true) => 0.15,
            (false, false) => 0.09 + h(k + 5) * 0.05,
        };
        // the section: 4..7 corners at irregular angles and radii
        let corners = 4 + (h(k + 6) * 4.0) as usize;
        let poly: Vec<(f32, f32)> = (0..corners)
            .map(|i| {
                let a = (i as f32 + (h(k + 7 + i as i32) - 0.5) * 0.6) / corners as f32 * std::f32::consts::TAU;
                (a, 0.65 + h(k + 20 + i as i32) * 0.5)
            })
            .collect();
        // a frame round the axis for the section's angle
        let up = if dir.y.abs() < 0.95 { glam::Vec3::Y } else { glam::Vec3::X };
        let e1 = up.cross(dir).normalize();
        let e2 = dir.cross(e1);
        let base = glam::Vec3::new(bx, -1.0, bz);
        for x in 0..n as i32 {
            for y in 0..ny as i32 {
                for z in 0..n as i32 {
                    let p = glam::Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) - base;
                    let t = p.dot(dir) / len;
                    if !(0.0..=1.0).contains(&t) {
                        continue;
                    }
                    let q = p - dir * p.dot(dir);
                    let (u, v) = (q.dot(e1), q.dot(e2));
                    let ang = v.atan2(u).rem_euclid(std::f32::consts::TAU);
                    // radius of the polygon edge at this angle
                    let mut edge = f32::MAX;
                    for i in 0..corners {
                        let (a0, r_a) = poly[i];
                        let (a1, r_b) = poly[(i + 1) % corners];
                        let a1 = if a1 < a0 { a1 + std::f32::consts::TAU } else { a1 };
                        let aa = if ang < a0 { ang + std::f32::consts::TAU } else { ang };
                        if aa >= a0 && aa <= a1 {
                            // point on the chord between the two corners, along this angle
                            let pa = glam::Vec2::new(a0.cos(), a0.sin()) * r_a;
                            let pb = glam::Vec2::new(a1.cos(), a1.sin()) * r_b;
                            let dirv = glam::Vec2::new(aa.cos(), aa.sin());
                            let e = pb - pa;
                            let den = dirv.x * e.y - dirv.y * e.x;
                            if den.abs() > 1e-5 {
                                edge = (pa.x * e.y - pa.y * e.x) / den;
                            }
                            break;
                        }
                    }
                    if edge == f32::MAX {
                        continue;
                    }
                    // taper to a broken tip, not a point: sharp but blunt-ended
                    let taper = (1.0 - t).powf(0.75) * 0.9 + 0.1 * (1.0 - t);
                    if (u * u + v * v).sqrt() > edge * r0 * taper {
                        continue;
                    }
                    // colour: facets lighter up and on the lit side, creases dark
                    let facet = ((ang * corners as f32 / std::f32::consts::TAU).floor() as i32 + variant) % 3;
                    // the root is the cliff face's own grey (render's
                    // pass_ground_color: ~0.25-0.29, bluer), lightening only
                    // toward the tips: no step in colour where crag meets face
                    let lift = 0.265 + 0.10 * t * t + [0.0, 0.02, -0.02][facet as usize] * t;
                    let v0 = lift.clamp(0.18, 0.5);
                    g.set(x, y, z, solid([v0, v0 * 1.02, v0 * 1.10]));
                }
            }
        }
    }
    g.mesh(vox, true)
}

fn noise3(x: i32, y: i32, z: i32, seed: i32) -> f32 {
    // wrapping: a hash, and debug builds panic on the overflow it relies on
    let v = (x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663) ^ z.wrapping_mul(83492791) ^ seed.wrapping_mul(2654435)) as f32 * 0.000_000_1;
    let s = (v.sin() * 43758.547).fract();
    s.abs()
}

/// Glowing gems: 3-6 hexagonal crystal prisms growing out of a small rock
/// base, tilted outward. `hue` 0 = ice cyan, 1 = violet. The crystal voxels
/// carry their glow in misc.z (the voxel shader adds it to the draw's
/// emissive), brightest at each crystal's core and tip.
pub fn gem_cluster(size: f32, variant: i32, hue: u8) -> VoxelMesh {
    let n = ((20.0 * size).clamp(10.0, 32.0)) as usize;
    let ny = ((34.0 * size).clamp(14.0, 48.0)) as usize;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let h = |k: i32| ((variant * 131 + k * 977) as f32 * 0.7548776).sin().abs().fract();
    // the rock it grows from
    g.ellipsoid(c, 1.0, c, n as f32 * 0.38, 2.2, n as f32 * 0.34, solid([0.26, 0.27, 0.30]));
    let (core, rim) = match hue {
        0 => ([0.62, 0.92, 1.0], [0.30, 0.62, 0.86]),
        _ => ([0.86, 0.66, 1.0], [0.50, 0.32, 0.80]),
    };
    // few, long and thin: at this voxel size a fat or crowded prism is a
    // blob (the first render), a thin one is a crystal
    let crystals = 3 + (h(1) * 2.0) as i32;
    let mut glow_cells: Vec<(i32, i32, i32, f32)> = Vec::new();
    for k in 0..crystals {
        let a = h(10 + k) * std::f32::consts::TAU;
        let tilt = 0.15 + h(20 + k) * 0.55;
        let dir = glam::Vec3::new(tilt.sin() * a.cos(), tilt.cos(), tilt.sin() * a.sin()).normalize();
        let base = glam::Vec3::new(c + a.cos() * n as f32 * 0.2, 1.5, c + a.sin() * n as f32 * 0.2);
        let len = ny as f32 * (0.6 + h(30 + k) * 0.35) * if k == 0 { 1.0 } else { 0.75 };
        let r = 1.6 + h(40 + k) * 0.9;
        let up = if dir.y.abs() < 0.95 { glam::Vec3::Y } else { glam::Vec3::X };
        let e1 = up.cross(dir).normalize();
        let e2 = dir.cross(e1);
        for x in 0..n as i32 {
            for y in 0..ny as i32 {
                for z in 0..n as i32 {
                    let p = glam::Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) - base;
                    let t = p.dot(dir) / len;
                    if !(0.0..=1.0).contains(&t) {
                        continue;
                    }
                    let q = p - dir * p.dot(dir);
                    let (u, v) = (q.dot(e1), q.dot(e2));
                    // hexagon: max over three axes of |projection|
                    let hex = [0.0f32, 1.047, 2.094].iter().map(|&th| (u * th.cos() + v * th.sin()).abs()).fold(0.0, f32::max);
                    // straight prism, then a pointed six-sided tip
                    let taper = if t < 0.72 { 1.0 } else { 1.0 - (t - 0.72) / 0.28 };
                    if hex > r * taper {
                        continue;
                    }
                    let edge = hex / (r * taper).max(1e-3);
                    let col = mix(core, rim, edge.powf(1.5));
                    g.set(x, y, z, solid(col));
                    glow_cells.push((x, y, z, (1.0 - edge * 0.6) * (0.7 + 0.6 * t)));
                }
            }
        }
    }
    let mut m = g.mesh(0.35, true);
    // bake the glow: the mesh's voxel offsets are grid cell * 0.35 (feet at
    // y = 0, centred in x/z), so find each instance's cell
    let glow_at: std::collections::HashMap<(i32, i32, i32), f32> = glow_cells.into_iter().map(|(x, y, z, gl)| ((x, y, z), gl)).collect();
    let off = (n as f32 * 0.5, 0.0, n as f32 * 0.5);
    for i in m.inst.iter_mut() {
        let cx = (i.off_ao[0] / 0.35 + off.0 - 0.5).round() as i32;
        let cy = (i.off_ao[1] / 0.35 - 0.5).round() as i32;
        let cz = (i.off_ao[2] / 0.35 + off.2 - 0.5).round() as i32;
        if let Some(&gl) = glow_at.get(&(cx, cy, cz)) {
            i.misc[2] = 1.1 * gl;
        }
    }
    m
}

/// A natural stone arch spanning `span` world units, `height` tall, built
/// in any orientation: `yaw` is the direction the corridor runs (0 = +z), so
/// the arch spans across it. Two jagged legs thickening to the ground, a
/// rough lintel, weathered grey with lighter tops. Feet at y = 0.
pub fn stone_arch(span: f32, height: f32, yaw: f32, variant: i32) -> VoxelMesh {
    const VOX: f32 = 0.6;
    let half = span * 0.5 + 3.0;
    let n = ((half * 2.0 / VOX).ceil() as usize + 4).min(80);
    let ny = ((height + 3.0) / VOX).ceil() as usize + 2;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let (sn, cs) = yaw.sin_cos();
    for x in 0..n as i32 {
        for y in 0..ny as i32 {
            for z in 0..n as i32 {
                // world-ish units, centred; u across the corridor, w along it
                let px = (x as f32 + 0.5 - c) * VOX;
                let pz = (z as f32 + 0.5 - c) * VOX;
                let py = (y as f32 + 0.5) * VOX;
                let u = px * cs - pz * sn;
                let w = px * sn + pz * cs;
                let rough = (noise3(x / 2, y / 2, z / 2, variant) - 0.5) * 1.1;
                let a_out = span * 0.5 + 2.6 + rough;
                let a_in = span * 0.5 - 0.2 + rough * 0.5;
                let b_out = height + rough;
                let b_in = height - 3.2 + rough * 0.6;
                let e = |a: f32, b: f32| (u / a).powi(2) + (py / b).powi(2);
                let in_band = e(a_out, b_out) <= 1.0 && e(a_in, b_in) > 1.0;
                // thicker along the corridor near the ground (the legs)
                let thick = 1.6 + (1.0 - (py / height).clamp(0.0, 1.0)) * 1.4 + rough * 0.4;
                if !in_band || w.abs() > thick {
                    continue;
                }
                let v = 0.30 + 0.10 * (py / height).clamp(0.0, 1.0) + (noise3(x, y, z, variant + 7) - 0.5) * 0.06;
                g.set(x, y, z, solid([v, v * 1.01, v * 1.06]));
            }
        }
    }
    g.mesh(VOX, true)
}

/// Weathered statue stone: grey with lichen patches and dark cracks.
fn statue_stone(x: i32, y: i32, z: i32, seed: i32) -> [f32; 4] {
    let n = noise3(x, y, z, seed);
    let lichen = noise3(x / 3, y / 3, z / 3, seed + 11) > 0.78;
    let crack = noise3(x, y / 2, z, seed + 5) > 0.93;
    if crack {
        solid([0.20, 0.20, 0.22])
    } else if lichen {
        solid([0.42, 0.46, 0.34])
    } else {
        let v = 0.44 + n * 0.08;
        solid([v, v * 0.99, v * 0.96])
    }
}

fn restyle(g: &mut Grid, seed: i32) {
    for x in 0..g.nx as i32 {
        for y in 0..g.ny as i32 {
            for z in 0..g.nz as i32 {
                if g.get(x, y, z).is_some() {
                    g.set(x, y, z, statue_stone(x, y, z, seed));
                }
            }
        }
    }
}

/// An animal god: a great cat seated on a plinth, head up, tail wrapped
/// round its forepaws, facing +z. ~15 u tall at scale 1.
pub fn statue_cat(s: f32, variant: i32) -> VoxelMesh {
    let k = s / 0.6; // grid cells per world unit
    let n = (14.0 * k) as usize;
    let ny = (17.0 * k) as usize;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let u = |v: f32| v * k;
    let st = solid([0.44, 0.44, 0.43]);
    // plinth
    g.filled_box((c - u(6.0)) as i32, 0, (c - u(6.0)) as i32, (c + u(6.0)) as i32, u(1.6) as i32, (c + u(6.0)) as i32, st);
    // haunches, chest, forelegs
    g.ellipsoid(c, u(4.2), c - u(1.8), u(4.2), u(3.0), u(3.6), st);
    g.ellipsoid(c, u(7.4), c + u(0.6), u(2.8), u(4.2), u(2.6), st);
    for side in [-1.0f32, 1.0] {
        g.cyl_y(c + side * u(1.5), c + u(2.4), u(0.9), u(1.6) as i32, u(7.0) as i32, st);
        g.ellipsoid(c + side * u(1.5), u(2.0), c + u(3.0), u(1.1), u(0.6), u(1.4), st);
    }
    // head, muzzle, ears
    g.ellipsoid(c, u(12.0), c + u(1.6), u(2.2), u(2.0), u(2.1), st);
    g.ellipsoid(c, u(11.3), c + u(3.4), u(1.1), u(0.9), u(1.0), st);
    for side in [-1.0f32, 1.0] {
        g.cone_y(c + side * u(1.3), c + u(1.2), u(0.9), u(0.1), u(13.4) as i32, u(15.6) as i32, st);
    }
    // tail wrapped round the front
    for t in 0..20 {
        let a = t as f32 / 19.0 * std::f32::consts::PI * 1.1 - 0.2;
        g.ellipsoid(c + a.cos() * u(4.0), u(2.2), c - u(0.5) + a.sin() * u(3.8), u(0.7), u(0.7), u(0.7), st);
    }
    restyle(&mut g, variant);
    g.mesh(0.6, true)
}

/// An animal god: an owl with folded wings on a plinth, ear tufts up,
/// facing +z. ~14 u tall at scale 1.
pub fn statue_owl(s: f32, variant: i32) -> VoxelMesh {
    let k = s / 0.6;
    let n = (11.0 * k) as usize;
    let ny = (16.0 * k) as usize;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let u = |v: f32| v * k;
    let st = solid([0.44, 0.44, 0.43]);
    g.filled_box((c - u(4.5)) as i32, 0, (c - u(4.5)) as i32, (c + u(4.5)) as i32, u(1.6) as i32, (c + u(4.5)) as i32, st);
    // body, folded wings, head
    g.ellipsoid(c, u(6.6), c, u(3.2), u(5.0), u(2.9), st);
    for side in [-1.0f32, 1.0] {
        g.ellipsoid(c + side * u(2.8), u(6.2), c - u(0.4), u(1.0), u(4.6), u(2.6), st);
        // feet
        g.ellipsoid(c + side * u(1.2), u(1.9), c + u(1.2), u(0.8), u(0.4), u(1.0), st);
        // ear tufts
        g.cone_y(c + side * u(1.6), c + u(0.2), u(0.7), u(0.1), u(13.2) as i32, u(15.2) as i32, st);
    }
    g.ellipsoid(c, u(11.6), c + u(0.3), u(2.9), u(2.3), u(2.5), st);
    restyle(&mut g, variant + 3);
    // the face disc and deep eyes, carved after the restyle
    for side in [-1.0f32, 1.0] {
        g.carve_sphere(c + side * u(1.0), u(11.8), c + u(2.6), u(0.6));
    }
    // the beak
    g.ellipsoid(c, u(11.0), c + u(2.7), u(0.35), u(0.6), u(0.5), solid([0.34, 0.33, 0.32]));
    g.mesh(0.6, true)
}
