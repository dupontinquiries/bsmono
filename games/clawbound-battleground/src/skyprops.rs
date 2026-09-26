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

/// Voxel size of the Moonwyrm's head, and of its body's segments (coarser:
/// there are ~150 of them, each ~7 u thick).
pub const WYRM_VOX: f32 = 0.3;
pub const WYRM_SEG_VOX: f32 = 0.6;

/// The Moonwyrm's head, in world units: a cat's head on a sky-serpent --
/// ears, a short muzzle, gold eyes, horns swept back, long whiskers, a
/// glowing mane down the back of the neck, and two small clawed forelegs
/// under the chest. Origin at the base of the skull (where the body
/// joins), facing +z. Its body is `wyrm_segment`s laid along
/// `wyrmbody::segments`.
pub fn moonwyrm() -> VoxelMesh {
    let k = 1.0 / WYRM_VOX;
    let hs = 2.7f32; // the old head's proportions, at this scale
    let (nx, ny, nz) = ((22.0 * k) as usize, (18.0 * k) as usize, (24.0 * k) as usize);
    let mut g = Grid::new(nx, ny, nz);
    // grid origin: the skull base sits here
    let (ox, oy, oz) = (nx as f32 * 0.5, 7.5 * k, 11.0 * k);
    let at = |x: f32, y: f32, z: f32| (ox + x * hs * k, oy + y * hs * k, oz + z * hs * k);
    let r = |v: f32| v * hs * k;
    let pearl = [0.88, 0.91, 0.98, 1.0];
    let belly = [0.66, 0.78, 0.95, 1.0];
    let mane = [0.45, 0.85, 1.0, 1.0];
    let horn = [0.93, 0.93, 0.98, 1.0];
    let eye = [1.0, 0.82, 0.30, 1.0];
    let claw = [0.95, 0.86, 0.60, 1.0];
    // skull and muzzle
    let (hx, hy, hz) = at(0.0, 0.3, 0.9);
    g.ellipsoid(hx, hy, hz, r(1.55), r(1.35), r(1.5), pearl);
    let (mx, my, mz) = at(0.0, -0.15, 2.3);
    g.ellipsoid(mx, my, mz, r(0.85), r(0.65), r(0.8), belly);
    // the throat, joining the body
    let (tx, ty, tz) = at(0.0, -0.1, -0.2);
    g.ellipsoid(tx, ty, tz, r(1.05), r(1.05), r(1.2), pearl);
    for side in [-1.0f32, 1.0] {
        // ears
        for t in 0..10 {
            let u = t as f32 / 9.0;
            let (ex, ey, ez) = at(side * (0.9 + 0.1 * u), 1.3 + 1.4 * u, 0.5);
            g.ellipsoid(ex, ey, ez, r(0.55 * (1.0 - u) + 0.05), r(0.2), r(0.35 * (1.0 - u) + 0.05), pearl);
        }
        // horns, swept back
        for t in 0..20 {
            let u = t as f32 / 19.0;
            let (qx, qy, qz) = at(side * (0.6 + 0.4 * u), 1.2 + 1.0 * u, 0.1 - 2.2 * u);
            let rr = r(0.3 - 0.2 * u);
            g.ellipsoid(qx, qy, qz, rr, rr, rr, horn);
        }
        // eyes
        let (ix, iy, iz) = at(side * 0.65, 0.55, 2.05);
        g.ellipsoid(ix, iy, iz, r(0.24), r(0.2), r(0.12), eye);
        // whiskers: long, trailing back and down, a little wavy
        for t in 0..40 {
            let u = t as f32 / 39.0;
            let (bx, by, bz) = at(side * (0.8 + 2.6 * u), -0.2 - 1.6 * u + 0.35 * (u * 7.0).sin(), 2.4 - 1.2 * u);
            g.ellipsoid(bx, by, bz, r(0.07), r(0.07), r(0.07), mane);
        }
        // forelegs under the chest, reaching forward, claws out
        for t in 0..10 {
            let u = t as f32 / 9.0;
            let (lx, ly, lz) = at(side * (0.8 + 0.5 * u), -0.8 - 1.2 * u, -1.6 + 1.0 * u);
            g.ellipsoid(lx, ly, lz, r(0.32), r(0.32), r(0.32), pearl);
        }
        for c in -1..=1 {
            let (lx, ly, lz) = at(side * 1.3 + c as f32 * 0.18, -2.1, -0.4);
            g.ellipsoid(lx, ly, lz, r(0.1), r(0.1), r(0.25), claw);
        }
    }
    // a mane down the back of the neck
    for i in 0..12 {
        let u = i as f32 / 11.0;
        let (qx, qy, qz) = at(0.0, 1.0 - 0.3 * u, -0.3 - 1.4 * u);
        g.ellipsoid(qx, qy, qz, r(0.45), r(0.5), r(0.4), mane);
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
    for i in m.inst.iter_mut() {
        let cx = (i.off_ao[0] / WYRM_VOX + nx as f32 * 0.5 - 0.5).round() as i32;
        let cy = (i.off_ao[1] / WYRM_VOX - 0.5).round() as i32;
        let cz = (i.off_ao[2] / WYRM_VOX + nz as f32 * 0.5 - 0.5).round() as i32;
        if let Some(&v) = glow.get(&(cx, cy, cz)) {
            i.misc[2] = v;
        }
        // move the origin to the skull base
        i.off_ao[1] -= oy * WYRM_VOX;
        i.off_ao[2] -= (oz - nz as f32 * 0.5) * WYRM_VOX;
    }
    m
}

/// A piece of the Moonwyrm's body, `wyrmbody::BODY_R` thick and a little
/// longer than `wyrmbody::SEG_STEP` so neighbours overlap: pearl, a pale
/// belly, a scatter of star-bright scales, and (with `fin`) a glowing crest
/// fin along the spine. Centred, running along z, back up.
pub fn wyrm_segment(fin: bool, variant: i32) -> VoxelMesh {
    let k = 1.0 / WYRM_SEG_VOX;
    let rad = crate::wyrmbody::BODY_R;
    let half = crate::wyrmbody::SEG_STEP * 0.8;
    let (nx, ny, nz) = (((rad * 2.0 + 1.0) * k) as usize, ((rad * 2.0 + 3.2) * k) as usize, ((half * 2.0 + 1.0) * k) as usize);
    let mut g = Grid::new(nx, ny, nz);
    let (cx, cy, cz) = (nx as f32 * 0.5, (rad + 0.5) * k, nz as f32 * 0.5);
    let pearl = [0.88, 0.91, 0.98, 1.0];
    let belly = [0.66, 0.78, 0.95, 1.0];
    let mane = [0.45, 0.85, 1.0, 1.0];
    g.ellipsoid(cx, cy, cz, rad * k, rad * k, half * k, pearl);
    g.ellipsoid(cx, cy - rad * 0.45 * k, cz, rad * 0.75 * k, rad * 0.6 * k, half * 0.95 * k, belly);
    if fin {
        for t in 0..8 {
            let u = t as f32 / 7.0;
            g.ellipsoid(cx, cy + (rad * 0.85 + 2.4 * u) * k, cz - u * 0.8 * k, 0.35 * k, 0.45 * k, (1.3 - 0.9 * u) * k, mane);
        }
    }
    let mut m = g.mesh(WYRM_SEG_VOX, false);
    let v = variant as f32;
    // the mesher centres the grid; put the origin on the body's axis
    let lift = (cy - ny as f32 * 0.5) * WYRM_SEG_VOX;
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= lift;
        let [x, y, z, _] = i.off_ao;
        if fin && y > rad * 0.85 && x.abs() < 0.6 {
            i.misc[2] = 0.9; // the crest
        } else if h3((x * 3.0).floor(), (y * 3.0).floor(), (z * 3.0).floor(), 11.0 + v) > 0.96 && i.color[2] > 0.9 && i.color[0] > 0.8 {
            i.color = [1.0, 1.0, 1.0, 1.0];
            i.misc[2] = 1.2; // a star-bright scale
        } else {
            i.misc[2] = 0.04;
        }
    }
    m
}

/// Voxel size of a cliff nook (the same as `props::diorama`'s, which it
/// holds).
pub const NOOK_VOX: f32 = 0.15;

/// A nook in an island's cliff face, under the lip: a shelf of stone
/// jutting out of the rock with a hood of rock over it, icicles hanging
/// from its underside, and on it a tiny scene -- `kind` 0 a marble shrine
/// with its cat idol and two starlit stones, 1 a frosted moss garden with
/// its pine, 2 a split geode of starlight. Faces +z; origin at the middle
/// of the shelf's top, which is where the scene stands.
pub fn cliff_nook(kind: u8, variant: i32) -> VoxelMesh {
    let k = 1.0 / NOOK_VOX;
    let (nx, ny, nz) = ((4.4 * k) as usize, (6.0 * k) as usize, (3.6 * k) as usize);
    let mut g = Grid::new(nx, ny, nz);
    let c = nx as f32 * 0.5;
    let shelf_top = 2.2 * k; // grid cells above the grid floor: room for icicles
    let back = 0.3 * k;
    let rock = [0.30, 0.34, 0.46, 1.0];
    let rock_d = [0.22, 0.25, 0.36, 1.0];
    let icicle = [0.62, 0.84, 0.97, 1.0];
    let snow = [0.90, 0.92, 0.98, 1.0];
    // the shelf, its front edge a little rounded
    g.filled_box((c - 1.9 * k) as i32, (shelf_top - 0.5 * k) as i32, back as i32, (c + 1.9 * k) as i32, shelf_top as i32 - 1, (nz as f32 - 0.3 * k) as i32, rock);
    g.filled_box((c - 1.8 * k) as i32, shelf_top as i32 - 1, back as i32, (c + 1.8 * k) as i32, shelf_top as i32, (nz as f32 - 0.5 * k) as i32, snow);
    // the hood: a shell of rock over the back half, open to the front
    g.ellipsoid(c, shelf_top, back, 2.2 * k, 3.3 * k, 1.9 * k, rock_d);
    g.carve_sphere(c, shelf_top + 0.2 * k, back + 1.2 * k, 1.9 * k);
    g.carve_sphere(c, shelf_top + 0.6 * k, back + 2.2 * k, 1.8 * k);
    // snow on the hood's brow
    g.ellipsoid(c, shelf_top + 2.8 * k, back + 0.9 * k, 1.8 * k, 0.35 * k, 0.7 * k, snow);
    // icicles hanging from the shelf's front
    for q in 0..7 {
        let x = c + (q as f32 / 6.0 * 3.2 - 1.6) * k;
        let len = (0.6 + h3(q as f32, 0.0, 0.0, variant as f32) * 1.4) * k;
        let z = nz as f32 - (0.6 + h3(q as f32, 1.0, 0.0, variant as f32) * 0.6) * k;
        g.cone_y(x, z, 0.22 * k, 0.03 * k, (shelf_top - 0.5 * k - len) as i32, (shelf_top - 0.5 * k) as i32, icicle);
    }
    let mut m = g.mesh(NOOK_VOX, true);
    let top = shelf_top * NOOK_VOX;
    let zc = (nz as f32 * 0.5 - (back + 1.6 * k)) * NOOK_VOX; // shelf middle sits at z = 0
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= top;
        i.off_ao[2] += zc;
        if i.color[2] > 0.95 && i.color[0] < 0.7 {
            i.misc[2] = 0.3; // icicles catch the light
        }
    }
    // the scene on the shelf, dressed for the glacier
    let mut d = props::diorama(kind, variant);
    match kind % 3 {
        0 => d = marble(d, variant as f32),
        1 => {
            let hi = d.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max).max(0.1);
            for i in d.inst.iter_mut() {
                if i.off_ao[1] > hi * 0.3 && h3(i.off_ao[0], i.off_ao[1], i.off_ao[2], 5.0) < 0.5 {
                    i.color = [0.90, 0.92, 0.98, 1.0];
                }
            }
        }
        _ => {}
    }
    crate::voxel::merge(&[m, d], &[glam::Vec3::ZERO, glam::Vec3::new(0.0, 0.0, 0.2)])
}

/// A star lantern: a marble post, a cage of four gold ribs at its head,
/// and in it a caught star. About 2.8 u tall, feet at y = 0.
pub fn star_lantern(variant: i32) -> VoxelMesh {
    let k = 1.0 / 0.12;
    let n = (1.4 * k) as usize;
    let mut g = Grid::new(n, (3.0 * k) as usize, n);
    let c = n as f32 * 0.5;
    let white = [0.93, 0.92, 0.88, 1.0];
    let gold = [0.86, 0.70, 0.36, 1.0];
    let star = [1.0, 0.95, 0.80, 1.0];
    g.cyl_y(c, c, 0.5 * k, 0, (0.25 * k) as i32, white);
    g.cyl_y(c, c, 0.16 * k, (0.25 * k) as i32, (2.1 * k) as i32, white);
    g.cyl_y(c, c, 0.35 * k, (2.1 * k) as i32, (2.25 * k) as i32, gold);
    for (dx, dz) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        for y in (2.25 * k) as i32..(2.85 * k) as i32 {
            g.set((c + dx * 0.32 * k) as i32, y, (c + dz * 0.32 * k) as i32, gold);
        }
    }
    g.ellipsoid(c, 2.55 * k, c, 0.2 * k, 0.2 * k, 0.2 * k, star);
    let mut m = marble(g.mesh(0.12, true), variant as f32);
    // the marble pass recolours everything: put the star and the gold back
    for i in m.inst.iter_mut() {
        let y = i.off_ao[1];
        let r = i.off_ao[0].hypot(i.off_ao[2]);
        if y > 2.35 && y < 2.75 && r < 0.25 {
            i.color = star;
            i.misc[2] = 2.0;
        } else if y > 2.1 && r > 0.25 {
            i.color = gold;
            i.misc[2] = 0.3;
        }
    }
    m
}

/// A lift pad (`lift`): a low marble dais `r` across, a gold ring round
/// its rim and a four-pointed star of starlight set in it -- the mark of an
/// updraft. Origin at the centre of its top face.
pub fn lift_pad(r: f32, variant: i32) -> VoxelMesh {
    let v = 0.15;
    let k = 1.0 / v;
    let n = (2.0 * (r + 0.5) * k) as usize;
    let mut g = Grid::new(n, (0.6 * k) as usize, n);
    let c = n as f32 * 0.5;
    let white = [0.93, 0.92, 0.88, 1.0];
    g.cyl_y(c, c, (r + 0.4) * k, 0, (0.3 * k) as i32, white);
    g.cyl_y(c, c, r * k, (0.3 * k) as i32, (0.45 * k) as i32, white);
    let mut m = marble(g.mesh(v, true), variant as f32);
    let top = m.inst.iter().map(|i| i.off_ao[1]).fold(f32::MIN, f32::max);
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= top;
        let (x, z) = (i.off_ao[0], i.off_ao[2]);
        let rr = x.hypot(z);
        // the star: four arms narrowing to points along the axes
        let (ax, az) = (x.abs(), z.abs());
        let arm = (ax < 0.35 * (1.0 - az / r).max(0.0) + 0.05) || (az < 0.35 * (1.0 - ax / r).max(0.0) + 0.05);
        if i.off_ao[1] > -0.2 && arm && rr < r * 0.9 {
            i.color = [0.80, 0.90, 1.0, 1.0];
            i.misc[2] = 2.0;
        } else if (rr - r).abs() < 0.2 || (rr > r + 0.1 && rr < r + 0.35) {
            i.color = [0.86, 0.70, 0.36, 1.0];
            i.misc[2] = 0.3;
        }
    }
    m
}

/// Voxel size of the floating stone and the drifting cloud.
pub const DRIFT_VOX: f32 = 0.3;
pub const PUFF_VOX: f32 = 0.6;

/// An anchor stone: a flat, broken tile of old flagstone drifting in the
/// sky, about 4 u across -- snow on top, blue rock and a hanging root of
/// ice beneath, and on some a stub of marble column from whatever stood
/// there once. Origin at the centre of its top face.
pub fn anchor_tile(variant: i32) -> VoxelMesh {
    let k = 1.0 / DRIFT_VOX;
    let (n, ny) = ((8.0 * k) as usize, (7.0 * k) as usize);
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let top = ny as f32 - 2.2 * k; // leave room above for a column stub
    let v = variant as f32;
    let stone = [0.56, 0.60, 0.70, 1.0];
    let snow = [0.90, 0.92, 0.98, 1.0];
    let rock = [0.50, 0.55, 0.68, 1.0];
    let ice = [0.55, 0.78, 0.95, 1.0];
    // the tile: two or three overlapping slabs, not quite level
    for s in 0..3 {
        let a = h3(v, s as f32, 0.0, 1.0) * std::f32::consts::TAU;
        let off = h3(v, s as f32, 1.0, 1.0) * 0.8 * k;
        let (rx, rz) = ((2.2 + h3(v, s as f32, 2.0, 1.0) * 0.9) * k, (2.0 + h3(v, s as f32, 3.0, 1.0) * 0.8) * k);
        g.ellipsoid(c + a.cos() * off, top - 0.45 * k, c + a.sin() * off, rx, 0.5 * k, rz, stone);
        g.ellipsoid(c + a.cos() * off, top - 0.05 * k, c + a.sin() * off, rx * 0.85, 0.22 * k, rz * 0.85, snow);
    }
    // the root beneath: rock tapering to an icicle
    g.cone_y(c, c, 1.0 * k, 0.15 * k, (top - 2.0 * k) as i32, (top - 0.7 * k) as i32, rock);
    g.cone_y(c + 0.3 * k, c - 0.2 * k, 0.3 * k, 0.05 * k, (top - 3.2 * k) as i32, (top - 1.9 * k) as i32, ice);
    // a broken column on every third
    if variant.rem_euclid(3) == 0 {
        g.cyl_y(c + 0.5 * k, c - 0.3 * k, 0.45 * k, top as i32, (top + 1.2 * k + h3(v, 9.0, 0.0, 1.0) * 0.9 * k) as i32, [0.93, 0.92, 0.88, 1.0]);
    }
    // (the mesher centres x and z already; y from the grid's floor)
    let mut m = g.mesh(DRIFT_VOX, true);
    let top_y = top * DRIFT_VOX;
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= top_y;
        let l = luma(i.color);
        if l > 0.85 {
            i.misc[2] = 0.08; // snow and marble, moonlit
        } else if i.color[2] > 0.9 {
            i.misc[2] = 0.3; // the icicle holds a little light
        }
    }
    m
}

/// A drifting cloud: a loose cluster of billows about 10 u across, pearl
/// on top, lavender beneath. Origin at its centre. Drawn scaled and
/// breathing (`skydrift`), so it changes shape as it goes.
pub fn cloud_puff(variant: i32) -> VoxelMesh {
    let k = 1.0 / PUFF_VOX;
    let (n, ny) = ((11.0 * k) as usize, (5.0 * k) as usize);
    let mut g = Grid::new(n, ny, n);
    let (c, cy) = (n as f32 * 0.5, ny as f32 * 0.45);
    let v = variant as f32;
    let white = [0.95, 0.95, 1.0, 1.0];
    let blobs = 6 + variant.rem_euclid(4);
    for b in 0..blobs {
        let a = h3(v, b as f32, 0.0, 3.0) * std::f32::consts::TAU;
        let r = h3(v, b as f32, 1.0, 3.0) * 3.2 * k;
        let size = (1.3 + h3(v, b as f32, 2.0, 3.0) * 1.3) * k;
        g.ellipsoid(c + a.cos() * r, cy + (h3(v, b as f32, 3.0, 3.0) - 0.4) * 1.2 * k, c + a.sin() * r, size, size * 0.7, size, white);
    }
    // (centred by the mesher)
    let mut m = g.mesh(PUFF_VOX, false);
    let top = 2.4f32;
    for i in m.inst.iter_mut() {
        let t = ((i.off_ao[1] + top) / (2.0 * top)).clamp(0.0, 1.0);
        let under = [0.62, 0.64, 0.86];
        let over = [0.97, 0.97, 1.0];
        i.color = [under[0] + (over[0] - under[0]) * t, under[1] + (over[1] - under[1]) * t, under[2] + (over[2] - under[2]) * t, 1.0];
        i.misc[2] = 0.15 + 0.15 * t;
    }
    m
}

/// A fragment of an island's edge, broken off and hanging in the air
/// beside it: a chunk of glacier ice and rock with snow on it, 2-4 u.
/// Origin at the centre of its top.
pub fn edge_fragment(variant: i32) -> VoxelMesh {
    let k = 1.0 / DRIFT_VOX;
    let n = (4.4 * k) as usize;
    let mut g = Grid::new(n, n, n);
    let c = n as f32 * 0.5;
    let top = n as f32 - 0.6 * k;
    let v = variant as f32;
    let rock = [0.22, 0.26, 0.38, 1.0];
    let ice = [0.50, 0.74, 0.92, 1.0];
    let snow = [0.90, 0.92, 0.98, 1.0];
    let body = if variant.rem_euclid(2) == 0 { ice } else { rock };
    let (rx, rz) = ((1.0 + h3(v, 0.0, 0.0, 5.0) * 0.8) * k, (0.9 + h3(v, 1.0, 0.0, 5.0) * 0.8) * k);
    g.ellipsoid(c, top - 0.9 * k, c, rx, 0.9 * k, rz, body);
    g.cone_y(c, c, rx.min(rz) * 0.9, 0.1 * k, (top - 3.8 * k).max(0.0) as i32, (top - 0.8 * k) as i32, body);
    g.ellipsoid(c, top - 0.1 * k, c, rx * 0.8, 0.3 * k, rz * 0.8, snow);
    let mut m = g.mesh(DRIFT_VOX, true);
    let ty = top * DRIFT_VOX;
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= ty;
        if i.color[2] > 0.9 && i.color[0] < 0.6 {
            i.misc[2] = 0.2;
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
        let seg = wyrm_segment(true, 0);
        let lo = seg.inst.iter().map(|i| i.off_ao[0]).fold(f32::MAX, f32::min);
        let hi = seg.inst.iter().map(|i| i.off_ao[0]).fold(f32::MIN, f32::max);
        assert!((hi - lo - crate::wyrmbody::BODY_R * 2.0).abs() < 1.5, "a segment is the body's thickness: {}", hi - lo);
        assert!(seg.inst.iter().any(|i| i.misc[2] > 0.5), "the crest glows");
        let m = moonwyrm();
        let (lo, hi) = m.inst.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), i| {
            let p = glam::Vec3::new(i.off_ao[0], i.off_ao[1], i.off_ao[2]);
            (lo.min(p), hi.max(p))
        });
        let size = hi - lo;
        assert!(size.x > 14.0 && size.y > 10.0 && size.z > 10.0, "a head for a 400 u body: {size}");
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
