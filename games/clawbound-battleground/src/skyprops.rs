//! skyprops — the things that stand on Starwell Glacier, as voxel meshes.
//!
//! Most are the existing props re-dressed for the spirit world rather than
//! new geometry: the seated cat statue in veined marble, crags turned to
//! glacier ice, pines turned to the golden larches of a high park in
//! autumn, gem clusters as fallen starlight. New geometry only where there
//! was nothing to start from: the clouds that ferry cats across the sky,
//! and the Moonwyrm.

use crate::props;
use crate::voxel::{Grid, VoxelMesh};

fn h3(x: f32, y: f32, z: f32, seed: f32) -> f32 {
    let v = (x * 12.9898 + y * 78.233 + z * 37.719 + seed * 4.1414).sin() * 43758.547;
    v - v.floor()
}

fn luma(c: [f32; 4]) -> f32 {
    c[0] * 0.3 + c[1] * 0.55 + c[2] * 0.15
}

/// Ancient marble: warm white, faint grey veins, a few veins of gold; the
/// eyes and the plinth's inlay hold a little starlight.
pub fn marble(mut m: VoxelMesh, seed: f32) -> VoxelMesh {
    for i in m.inst.iter_mut() {
        let [x, y, z, _] = i.off_ao;
        let l = luma(i.color);
        // veins: thin bands of a slow wobbling plane
        let band = ((x * 1.3 + y * 0.7 + (z * 0.9 + y * 0.4).sin() * 1.7 + seed).sin()).abs();
        let vein = band < 0.07;
        let gold = vein && h3((x * 2.0).floor(), (y * 2.0).floor(), (z * 2.0).floor(), seed) > 0.8;
        let base = 0.93 + (l - 0.44) * 0.5 + 0.04 * h3(x, y, z, seed);
        i.color = if gold {
            [0.86, 0.70, 0.36, 1.0]
        } else if vein {
            [base * 0.78, base * 0.80, base * 0.86, 1.0]
        } else {
            [base, base * 0.985, base * 0.95, 1.0]
        };
        // moonlit marble holds a little light of its own; gold more
        i.misc[2] = if gold { 0.35 } else { 0.07 };
    }
    m
}

/// A seated cat of the old ones, in marble.
pub fn marble_cat(scale: f32, variant: i32) -> VoxelMesh {
    marble(props::statue_cat(scale, variant), variant as f32 * 1.7)
}

/// An owl of the old ones, in marble.
pub fn marble_owl(scale: f32, variant: i32) -> VoxelMesh {
    marble(props::statue_owl(scale, variant), variant as f32 * 2.3 + 0.5)
}

/// A marble arch (the natural stone arch, dressed).
pub fn marble_arch(span: f32, height: f32, yaw: f32, variant: i32) -> VoxelMesh {
    marble(props::stone_arch(span, height, yaw, variant), 3.1 + variant as f32)
}

/// Glacier ice from a crag: deep blue at the root, pale and bright toward
/// the tips, which hold a little light.
pub fn ice(mut m: VoxelMesh) -> VoxelMesh {
    let top = m.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max).max(0.1);
    for i in m.inst.iter_mut() {
        let t = (i.off_ao[1] / top).clamp(0.0, 1.0);
        let l = luma(i.color);
        let n = 0.9 + 0.2 * h3(i.off_ao[0], i.off_ao[1], i.off_ao[2], 7.0);
        let deep = [0.18, 0.40, 0.66];
        let pale = [0.72, 0.90, 0.98];
        let c: Vec<f32> = (0..3).map(|k| (deep[k] + (pale[k] - deep[k]) * (t * 0.7 + l * 0.6).min(1.0)) * n).collect();
        i.color = [c[0], c[1], c[2], 1.0];
        if t > 0.7 {
            i.misc[2] = 0.35 * (t - 0.7) / 0.3;
        }
    }
    m
}

/// An ice spire: a spiky crag in glacier ice.
pub fn ice_spire(size: f32, variant: i32, lean: f32, yaw: f32) -> VoxelMesh {
    ice(props::crag(size, variant, lean, 0.6, yaw, true))
}

/// A spirit larch: a pine gone to autumn gold, the needle-tips glowing
/// faintly -- the high park's larches in September, in the spirit world.
pub fn spirit_larch(scale: f32, variant: i32) -> VoxelMesh {
    let mut m = props::pine(scale);
    let top = m.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max).max(0.1);
    for i in m.inst.iter_mut() {
        let [r, g, b, a] = i.color;
        if g > r * 1.2 {
            // foliage: gold, deeper toward the trunk, a few ember needles
            let n = h3(i.off_ao[0], i.off_ao[1], i.off_ao[2], variant as f32);
            let t = i.off_ao[1] / top;
            let gold = [0.95, 0.72, 0.22];
            let deep = [0.62, 0.36, 0.10];
            let k = (0.4 + 0.6 * t) * (0.85 + 0.3 * n);
            i.color = [deep[0] + (gold[0] - deep[0]) * k, deep[1] + (gold[1] - deep[1]) * k, deep[2] + (gold[2] - deep[2]) * k, a];
            if n > 0.86 {
                i.misc[2] = 0.45;
            }
        } else {
            // the trunk: silver-grey bark
            let l = (r + g + b) / 3.0;
            i.color = [l * 0.9 + 0.12, l * 0.9 + 0.12, l + 0.16, a];
        }
    }
    m
}

/// Fallen starlight: a gem cluster, pale blue or violet.
pub fn star_shard(size: f32, variant: i32) -> VoxelMesh {
    props::gem_cluster(size, variant, (variant % 2) as u8)
}

/// Voxel size of a ferry cloud.
pub const CLOUD_VOX: f32 = 0.5;

/// A cloud to ride: a heap of billows round a flat top of radius `r`, the
/// top at the mesh's y = `top` (feet origin at the bottom). Lit from within
/// a little -- it is a spirit cloud -- and bluer underneath.
pub fn ferry_cloud(r: f32, variant: i32) -> (VoxelMesh, f32) {
    let k = 1.0 / CLOUD_VOX;
    let n = ((r * 2.0 + 5.0) * k) as usize;
    let ny = (3.4 * k) as usize;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let top_cells = ny as f32 - 1.2 * k;
    let white = [0.95, 0.95, 1.0, 1.0];
    // the deck: a flat-topped disc
    g.cyl_y(c, c, r * k, (top_cells - 1.0 * k) as i32, top_cells as i32, white);
    // billows round the rim and under
    let puffs = 11 + variant.rem_euclid(4);
    for p in 0..puffs {
        let a = p as f32 / puffs as f32 * std::f32::consts::TAU + h3(p as f32, 0.0, 0.0, variant as f32) * 0.5;
        let rr = (r - 0.3 + h3(p as f32, 1.0, 0.0, variant as f32) * 1.2) * k;
        let pr = (1.2 + h3(p as f32, 2.0, 0.0, variant as f32) * 0.9) * k;
        let y = top_cells - 0.4 * k + h3(p as f32, 3.0, 0.0, variant as f32) * 0.8 * k;
        g.ellipsoid(c + a.cos() * rr, y, c + a.sin() * rr, pr, pr * 0.8, pr, white);
    }
    g.ellipsoid(c, top_cells - 1.4 * k, c, r * k * 0.9, 1.3 * k, r * k * 0.9, white);
    let mut m = g.mesh(CLOUD_VOX, true);
    let top_y = top_cells * CLOUD_VOX;
    for i in m.inst.iter_mut() {
        // bluer and dimmer underneath, bright on the deck
        let t = (i.off_ao[1] / top_y).clamp(0.0, 1.0);
        let under = [0.60, 0.64, 0.86];
        let over = [0.97, 0.97, 1.0];
        i.color = [under[0] + (over[0] - under[0]) * t, under[1] + (over[1] - under[1]) * t, under[2] + (over[2] - under[2]) * t, 1.0];
        i.misc[2] = 0.12 + 0.1 * t;
    }
    (m, top_y)
}

/// Voxel size of the Moonwyrm.
pub const WYRM_VOX: f32 = 0.3;

/// The Moonwyrm: a sky-serpent with a cat's head, made of moonlight. Its
/// body rises out of the well in a great climbing coil into the air --
/// the tail's tip still in the water, the neck arching out over it -- with
/// a crest of glowing fins down the spine, two small clawed forelegs under
/// the neck, a cat's head held out (ears, a short muzzle, gold eyes),
/// swept-back horns, long trailing whiskers, and stars in its hide. An
/// original design in the moon's palette: pearl, silver-blue, starlight
/// gold. Faces +z; the water line at y = 0.
pub fn moonwyrm() -> VoxelMesh {
    let k = 1.0 / WYRM_VOX;
    let (nx, ny, nz) = ((13.0 * k) as usize, (16.0 * k) as usize, (14.0 * k) as usize);
    let mut g = Grid::new(nx, ny, nz);
    let (cx, cz0) = (nx as f32 * 0.5, nz as f32 * 0.5);
    let at = |x: f32, y: f32, z: f32| (cx + x * k, y * k, cz0 + z * k);
    let pearl = [0.88, 0.91, 0.98, 1.0];
    let belly = [0.66, 0.78, 0.95, 1.0];
    let mane = [0.45, 0.85, 1.0, 1.0];
    let horn = [0.93, 0.93, 0.98, 1.0];
    let eye = [1.0, 0.82, 0.30, 1.0];
    let claw = [0.95, 0.86, 0.60, 1.0];
    // the body's path, tail tip (s = 0) to the base of the skull (s = 1):
    // a coil climbing round a vertical axis, then the neck bending out
    let path = |s: f32| -> (f32, f32, f32) {
        let turns = 1.6;
        let a = s * turns * std::f32::consts::TAU - 1.2;
        let r = 3.6 * (1.0 - 0.35 * s);
        let (x, z) = (a.cos() * r, a.sin() * r - 1.0);
        let y = 0.3 + s.powf(0.9) * 11.0;
        // the last stretch leans out toward +z: the neck over the water
        let lean = ((s - 0.85) / 0.15).clamp(0.0, 1.0);
        (x * (1.0 - lean), y + lean * 0.8, z * (1.0 - lean) + lean * 3.2)
    };
    let steps = 360;
    for i in 0..=steps {
        let s = i as f32 / steps as f32;
        let (x, y, z) = path(s);
        // thick through the middle, a thin tail, a slimmer neck
        let r = 0.25 + 1.05 * (s * 3.0).min(1.0) * (1.0 - 0.3 * ((s - 0.7) / 0.3).clamp(0.0, 1.0));
        let (px, py, pz) = at(x, y, z);
        g.ellipsoid(px, py, pz, r * k, r * k, r * k, pearl);
        // the belly: a paler stripe on the coil's inner side
        let (bx, by, bz) = at(x * 0.82, y - r * 0.35, z * 0.82 - 0.18);
        g.ellipsoid(bx, by, bz, r * 0.7 * k, r * 0.55 * k, r * 0.7 * k, belly);
        // the crest: glowing fins along the spine, every so often
        if i % 9 == 0 && s > 0.12 {
            let h = 0.5 + 0.9 * (1.0 - (s - 0.6).abs());
            for u in 0..6 {
                let f = u as f32 / 5.0;
                let (qx, qy, qz) = at(x * (1.0 + 0.08 * f), y + r * 0.85 + h * f, z * (1.0 + 0.08 * f) - 0.15 * f);
                g.ellipsoid(qx, qy, qz, 0.22 * k * (1.0 - f * 0.6), 0.3 * k, 0.35 * k * (1.0 - f * 0.5), mane);
            }
        }
    }
    let (hx0, hy0, hz0) = path(1.0);
    // head
    let (hx, hy, hz) = at(hx0, hy0 + 0.3, hz0 + 0.9);
    g.ellipsoid(hx, hy, hz, 1.55 * k, 1.35 * k, 1.5 * k, pearl);
    let (mx, my, mz) = at(hx0, hy0 - 0.15, hz0 + 2.3);
    g.ellipsoid(mx, my, mz, 0.85 * k, 0.65 * k, 0.8 * k, belly);
    for side in [-1.0f32, 1.0] {
        // ears
        let (ex, _, ez) = at(hx0 + side * 0.9, 0.0, hz0 + 0.5);
        g.cone_y(ex, ez, 0.6 * k, 0.05 * k, ((hy0 + 1.3) * k) as i32, ((hy0 + 2.7) * k) as i32, pearl);
        // horns, swept back
        for t in 0..16 {
            let u = t as f32 / 15.0;
            let (qx, qy, qz) = at(hx0 + side * (0.6 + 0.4 * u), hy0 + 1.2 + 1.0 * u, hz0 + 0.1 - 2.2 * u);
            let rr = (0.3 - 0.2 * u) * k;
            g.ellipsoid(qx, qy, qz, rr, rr, rr, horn);
        }
        // eyes
        let (ix, iy, iz) = at(hx0 + side * 0.65, hy0 + 0.55, hz0 + 2.05);
        g.ellipsoid(ix, iy, iz, 0.24 * k, 0.2 * k, 0.12 * k, eye);
        // whiskers: long, trailing back and down, a little wavy
        for t in 0..30 {
            let u = t as f32 / 29.0;
            let (bx, by, bz) = at(hx0 + side * (0.8 + 2.6 * u), hy0 - 0.2 - 1.6 * u + 0.35 * (u * 7.0).sin(), hz0 + 2.4 - 1.2 * u);
            g.set(bx as i32, by as i32, bz as i32, mane);
        }
        // forelegs under the neck, reaching forward, claws out
        let (sx, sy, sz) = path(0.9);
        for t in 0..10 {
            let u = t as f32 / 9.0;
            let (lx, ly, lz) = at(sx + side * (0.8 + 0.5 * u), sy - 0.6 - 1.2 * u, sz + 0.5 + 1.0 * u);
            g.ellipsoid(lx, ly, lz, 0.32 * k, 0.32 * k, 0.32 * k, pearl);
        }
        for c in -1..=1 {
            let (lx, ly, lz) = at(sx + side * 1.3 + c as f32 * 0.18, sy - 1.9, sz + 1.7);
            g.ellipsoid(lx, ly, lz, 0.1 * k, 0.1 * k, 0.25 * k, claw);
        }
    }
    // a mane down the back of the neck
    for i in 0..40 {
        let s = 0.8 + 0.2 * i as f32 / 39.0;
        let (x, y, z) = path(s);
        let (qx, qy, qz) = at(x, y + 1.0, z - 0.3);
        g.ellipsoid(qx, qy, qz, 0.45 * k, 0.5 * k, 0.4 * k, mane);
    }
    // what glows, by grid cell (the mesh shades its colours, so they cannot
    // be matched after meshing -- `props::gem_cluster` does the same)
    let same = |c: [f32; 4], a: [f32; 4]| (c[0] - a[0]).abs() < 1e-4 && (c[1] - a[1]).abs() < 1e-4 && (c[2] - a[2]).abs() < 1e-4;
    let mut glow: std::collections::HashMap<(i32, i32, i32), f32> = Default::default();
    for x in 0..nx as i32 {
        for y in 0..ny as i32 {
            for z in 0..nz as i32 {
                if let Some(c) = g.get(x, y, z) {
                    let v = if same(c, mane) {
                        0.9
                    } else if same(c, eye) {
                        1.4
                    } else if same(c, pearl) && h3(x as f32, y as f32, z as f32, 11.0) > 0.93 {
                        -1.2 // a star: white and bright
                    } else if same(c, pearl) {
                        0.04
                    } else {
                        continue;
                    };
                    glow.insert((x, y, z), v);
                }
            }
        }
    }
    let mut m = g.mesh(WYRM_VOX, true);
    let off = (nx as f32 * 0.5, 0.0, nz as f32 * 0.5);
    for i in m.inst.iter_mut() {
        let cx = (i.off_ao[0] / WYRM_VOX + off.0 - 0.5).round() as i32;
        let cy = (i.off_ao[1] / WYRM_VOX - 0.5).round() as i32;
        let cz = (i.off_ao[2] / WYRM_VOX + off.2 - 0.5).round() as i32;
        if let Some(&v) = glow.get(&(cx, cy, cz)) {
            if v < 0.0 {
                i.color = [1.0, 1.0, 1.0, 1.0];
            }
            i.misc[2] = v.abs();
        }
    }
    m
}

/// Voxel size of a Moonwyrm star.
pub const STAR_VOX: f32 = 0.14;

/// One of the Moonwyrm's stars: a bright core with four points, a little
/// warm at the heart. Centred on its origin.
pub fn star_mote() -> VoxelMesh {
    let n = 15usize;
    let mut g = Grid::new(n, n, n);
    let c = n as f32 * 0.5;
    let core = [1.0, 0.95, 0.80, 1.0];
    let ray = [0.85, 0.92, 1.0, 1.0];
    g.ellipsoid(c, c, c, 2.2, 2.2, 2.2, core);
    for t in 0..7 {
        let f = t as i32;
        for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
            g.set(c as i32 + dx * f, c as i32 + dy * f, c as i32 + dz * f, ray);
        }
    }
    let mut m = g.mesh(STAR_VOX, false);
    for i in m.inst.iter_mut() {
        i.misc[2] = 2.2;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ferry_cloud_is_flat_on_top_and_as_wide_as_asked() {
        let (m, top) = ferry_cloud(5.0, 1);
        assert!(!m.inst.is_empty());
        let max_y = m.inst.iter().map(|i| i.off_ao[1]).fold(f32::MIN, f32::max);
        assert!((max_y - top).abs() < 1.2, "the deck is the top: {max_y} vs {top}");
        let reach = m.inst.iter().filter(|i| (i.off_ao[1] - top).abs() < 1.0).map(|i| i.off_ao[0].hypot(i.off_ao[2])).fold(0.0f32, f32::max);
        assert!((4.5..8.5).contains(&reach), "deck reach {reach} (the billows overhang the walkable disc)");
    }

    #[test]
    fn the_moonwyrm_is_big_and_glows() {
        let m = moonwyrm();
        let (lo, hi) = m.inst.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), i| {
            let p = glam::Vec3::new(i.off_ao[0], i.off_ao[1], i.off_ao[2]);
            (lo.min(p), hi.max(p))
        });
        let size = hi - lo;
        assert!(size.z > 11.0 && size.y > 6.0, "a long beast with its head held high: {size}");
        let glowing = m.inst.iter().filter(|i| i.misc[2] > 0.5).count();
        assert!(glowing > 50, "mane, eyes and stars glow: {glowing}");
    }

    #[test]
    fn the_dressing_changes_colour_not_shape() {
        let a = props::statue_cat(1.0, 1);
        let b = marble_cat(1.0, 1);
        assert_eq!(a.inst.len(), b.inst.len());
        let pale = b.inst.iter().filter(|i| luma(i.color) > 0.7).count();
        assert!(pale * 2 > b.inst.len(), "marble is mostly white: {pale}/{}", b.inst.len());
        let larch = spirit_larch(1.0, 2);
        let gold = larch.inst.iter().filter(|i| i.color[0] > i.color[2] * 2.0).count();
        assert!(gold * 2 > larch.inst.len(), "a larch is mostly gold");
    }
}
