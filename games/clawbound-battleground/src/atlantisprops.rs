//! atlantisprops — the Drowned Halls' stonework, glass and sea life, as
//! voxel meshes.
//!
//! The look is logscope's Atlantis scene (scene3d.rs `atlantis()`, and
//! docs/refs/theme-bible.md §7) rebuilt in this game's voxels rather than
//! ported: its smooth meshes need the Ultra-only smooth pipeline here. What
//! carried over is its design and its palette -- teal basalt and pale sea
//! marble, verdigris bronze ribs, a ribbed glass dome with an oculus, fluted
//! columns with gold collars, stained glass in blue and gold, the reading
//! rooms' bookcases, kelp beds and coral on the sea floor.
//!
//! Every mesh is built along +x/+z at the origin, feet at y = 0 unless
//! said; `atlantisworld` places and turns them.

use crate::voxel::{Grid, VoxelMesh};

fn h3(x: f32, y: f32, z: f32, seed: f32) -> f32 {
    let v = (x * 12.9898 + y * 78.233 + z * 37.719 + seed * 4.1414).sin() * 43758.547;
    v - v.floor()
}

fn hex(c: u32, a: f32) -> [f32; 4] {
    // sRGB-ish authoring colours to the linear-ish floats the voxels use
    let f = |v: u32| ((v & 0xff) as f32 / 255.0).powf(2.0);
    [f(c >> 16), f(c >> 8), f(c), a]
}

/// Pale sea marble: cool white with teal veins and a little gold.
pub fn sea_marble(mut m: VoxelMesh, seed: f32) -> VoxelMesh {
    for i in m.inst.iter_mut() {
        let [x, y, z, _] = i.off_ao;
        let band = ((x * 1.1 + y * 0.8 + (z * 0.7 + y * 0.5).sin() * 1.9 + seed).sin()).abs();
        let vein = band < 0.08;
        let gold = vein && h3((x * 2.0).floor(), (y * 2.0).floor(), (z * 2.0).floor(), seed) > 0.82;
        let n = 0.9 + 0.1 * h3(x, y, z, seed);
        i.color = if gold {
            [0.86, 0.68, 0.30, 1.0]
        } else if vein {
            [0.30 * n, 0.58 * n, 0.60 * n, 1.0]
        } else {
            [0.74 * n, 0.80 * n, 0.80 * n, 1.0]
        };
        i.misc[2] = if gold { 0.45 } else { 0.0 };
    }
    m
}

/// A fluted column `h` tall: a square plinth, a fluted shaft, two gold
/// collars, a capital. Voxel 0.25.
pub fn column(h: f32, variant: i32) -> VoxelMesh {
    let k = 4.0;
    let n = (2.2 * k) as usize;
    let ny = (h * k) as usize + 1;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let stone = [0.7, 0.75, 0.75, 1.0];
    let gold = hex(0xd8a840, 1.0);
    g.filled_box(0, 0, 0, n as i32 - 1, (0.5 * k) as i32, n as i32 - 1, stone);
    let top = ny as i32 - 1;
    g.filled_box(0, top - (0.5 * k) as i32, 0, n as i32 - 1, top, n as i32 - 1, stone);
    // the shaft, fluted: every other sixteenth of the round cut in a voxel
    for y in (0.5 * k) as i32..top - (0.5 * k) as i32 {
        for x in 0..n as i32 {
            for z in 0..n as i32 {
                let (dx, dz) = (x as f32 + 0.5 - c, z as f32 + 0.5 - c);
                let r = dx.hypot(dz);
                let a = dz.atan2(dx);
                let flute = ((a * 8.0).sin() > 0.55) as i32 as f32 * 0.45;
                if r <= 0.62 * k - flute {
                    g.set(x, y, z, stone);
                }
            }
        }
    }
    for yy in [(1.0 * k) as i32, top - (1.1 * k) as i32] {
        g.cyl_y(c - 0.5, c - 0.5, 0.78 * k, yy, yy + 1, gold);
    }
    let mut m = sea_marble(g.mesh(0.25, true), variant as f32 * 1.3);
    // the collars stay gold, and glow a little
    for i in m.inst.iter_mut() {
        let y = i.off_ao[1];
        if (y - 1.1).abs() < 0.3 || (y - (h - 1.0)).abs() < 0.3 {
            let r = i.off_ao[0].hypot(i.off_ao[2]);
            if r > 0.7 {
                i.color = gold;
                i.misc[2] = 0.6;
            }
        }
    }
    m
}

/// A bronze rib spanning `span` across (along x, centred), springing from
/// y = 0 at each end and rising `rise` at the middle, `depth` thick along
/// z. Verdigris, with a gold keystone that glows. Voxel 0.3.
pub fn rib(span: f32, rise: f32, depth: f32) -> VoxelMesh {
    let v = 0.3;
    let nx = (span / v) as usize + 2;
    let ny = ((rise + 1.0) / v) as usize + 2;
    let nz = (depth / v).max(1.0) as usize;
    let mut g = Grid::new(nx, ny, nz);
    let bronze = hex(0x2f7a66, 1.0);
    let dark = hex(0x1c4a40, 1.0);
    let gold = hex(0xf0c85a, 1.0);
    let half = span * 0.5;
    for xi in 0..nx as i32 {
        let x = xi as f32 * v - half;
        let t = (x / half).clamp(-1.0, 1.0);
        let y = rise * (1.0 - t * t).max(0.0).sqrt();
        let yi = (y / v) as i32;
        for zi in 0..nz as i32 {
            for dy in 0..3 {
                let c = if dy == 0 { dark } else if t.abs() < 0.06 { gold } else { bronze };
                g.set(xi, yi + dy, zi, c);
            }
        }
    }
    let mut m = g.mesh(v, false);
    // `mesh` centres a non-feet grid; put the springing at y = 0
    let low = m.inst.iter().map(|i| i.off_ao[1]).fold(f32::MAX, f32::min);
    for i in m.inst.iter_mut() {
        i.off_ao[1] -= low;
        if i.color[0] > 0.7 {
            i.misc[2] = 1.2;
        }
    }
    m
}

/// A length of glass barrel vault: `len` along z, spanning `span` along x
/// (as [`rib`], same rise), 0.7 u voxels of sea-tinted glass, clear enough
/// to see the halls through from above and the sea through from below.
pub fn vault_glass(span: f32, rise: f32, len: f32) -> VoxelMesh {
    let v = 0.7;
    let mut m = VoxelMesh::default();
    let half = span * 0.5;
    let steps = (std::f32::consts::PI * half / v).ceil() as i32;
    let nz = (len / v).ceil() as i32;
    for s in 0..=steps {
        let a = std::f32::consts::PI * s as f32 / steps as f32;
        let (x, y) = (-a.cos() * half, a.sin() * rise);
        for zi in 0..nz {
            let z = zi as f32 * v - len * 0.5;
            let tint = 0.9 + 0.1 * h3(x, y, z, 3.0);
            m.inst.push(crate::voxel::VoxInst { off_ao: [x, y, z, 1.0], color: [0.30 * tint, 0.72 * tint, 0.80 * tint, 0.16], misc: [1.0, 0.0, 0.05, 0.0] });
        }
    }
    m
}

/// A ribbed glass dome of radius `r` rising `rise`, its base ring at y = 0:
/// sea glass between twelve bronze meridian ribs and two latitude rings,
/// an oculus at the crown ringed in gold, the skylight in it glowing.
pub fn dome(r: f32, rise: f32) -> VoxelMesh {
    let v = 0.7;
    let mut m = VoxelMesh::default();
    let oculus = r * 0.16;
    let rings = (std::f32::consts::FRAC_PI_2 * r / v).ceil() as i32;
    let bronze = hex(0x2f7a66, 1.0);
    let gold = hex(0xf0c85a, 1.0);
    for li in 0..=rings {
        let phi = std::f32::consts::FRAC_PI_2 * li as f32 / rings as f32;
        let rr = r * phi.cos();
        let y = rise * phi.sin();
        if rr < oculus {
            continue;
        }
        let around = (std::f32::consts::TAU * rr / v).ceil().max(8.0) as i32;
        let lat = li == rings / 3 || li == 2 * rings / 3;
        for ai in 0..around {
            let a = std::f32::consts::TAU * ai as f32 / around as f32;
            let (x, z) = (a.cos() * rr, a.sin() * rr);
            let meridian = ((a / std::f32::consts::TAU * 12.0).fract() < v / (std::f32::consts::TAU * rr / 12.0).max(v)) || lat;
            let crown = rr < oculus + v * 1.5;
            let (color, glow) = if crown {
                (gold, 1.0)
            } else if meridian {
                (bronze, 0.0)
            } else {
                let t = 0.9 + 0.1 * h3(x, y, z, 5.0);
                ([0.28 * t, 0.70 * t, 0.80 * t, 0.14], 0.05)
            };
            m.inst.push(crate::voxel::VoxInst { off_ao: [x, y, z, 1.0], color, misc: [1.0, 0.0, glow, 0.0] });
        }
    }
    // the skylight: a glowing disc in the oculus
    let n = (oculus / v) as i32;
    for xi in -n..=n {
        for zi in -n..=n {
            let (x, z) = (xi as f32 * v, zi as f32 * v);
            if x.hypot(z) < oculus {
                m.inst.push(crate::voxel::VoxInst { off_ao: [x, rise, z, 1.0], color: [0.75, 0.96, 1.0, 0.55], misc: [1.0, 0.0, 1.6, 0.0] });
            }
        }
    }
    m
}

/// A stained-glass window `w` wide and `h` tall in the x-y plane, facing
/// +z: a dark leading grid, tiles from `palette`, an arched head of wedges.
/// It holds light of its own (the sea behind it). Voxel 0.2.
pub fn stained_glass(w: f32, h: f32, gold: bool, seed: f32) -> VoxelMesh {
    let v = 0.2;
    let blue = [0x102c66, 0x1c52b0, 0x2f8fd8, 0x6ac8f0, 0x146e88, 0xd8a840];
    let warm = [0x6e4e12, 0xb88622, 0xf0c85a, 0xf6e0a0, 0xc0702a, 0x2f8fd8];
    let pal = if gold { warm } else { blue };
    let nx = (w / v) as i32;
    let ny = (h / v) as i32;
    let mut m = VoxelMesh::default();
    let lead = [0.05, 0.06, 0.07, 1.0];
    let cx = nx as f32 * 0.5;
    for xi in 0..nx {
        for yi in 0..ny {
            let (x, y) = (xi as f32 * v - w * 0.5, yi as f32 * v);
            // the arched head: above h - w/2, inside the semicircle
            let head = h - w * 0.5;
            if y > head && ((xi as f32 - cx) * v).hypot(y - head) > w * 0.5 {
                continue;
            }
            let (cxi, cyi) = (xi / 4, yi / 5);
            let edge = xi == 0 || xi == nx - 1 || yi == 0 || xi % 4 == 0 || yi % 5 == 0;
            let (color, glow) = if edge {
                (lead, 0.0)
            } else {
                let k = (h3(cxi as f32, cyi as f32, seed, 1.0) * pal.len() as f32) as usize % pal.len();
                (hex(pal[k], 1.0), 0.9)
            };
            m.inst.push(crate::voxel::VoxInst { off_ao: [x, y, 0.0, 1.0], color, misc: [1.0, 0.0, glow, 0.0] });
        }
    }
    m
}

/// A bookcase of the Drowned Library, `w` wide, about 4 u tall, backed on
/// -z and open to +z: dark wood, five shelves of spines in faded colours,
/// here and there a gap or a book leaning. Voxel 0.2.
pub fn bookcase(w: f32, seed: f32) -> VoxelMesh {
    let v = 0.2;
    let (nx, ny, nz) = ((w / v) as usize, 21usize, 4usize);
    let mut g = Grid::new(nx, ny, nz);
    let wood = hex(0x3a2616, 1.0);
    let spines = [0x7a2e2a, 0x2e4e6e, 0x5e6e2e, 0x8a6a2a, 0x4a2e5e, 0x2e6e5e, 0xa0906a];
    for x in 0..nx as i32 {
        for y in 0..ny as i32 {
            g.set(x, y, 0, wood);
        }
    }
    for y in 0..ny as i32 {
        for z in 0..nz as i32 {
            g.set(0, y, z, wood);
            g.set(nx as i32 - 1, y, z, wood);
        }
    }
    for shelf in 0..5 {
        let y0 = shelf * 4;
        for x in 0..nx as i32 {
            for z in 0..nz as i32 {
                g.set(x, y0, z, wood);
            }
        }
        // books
        let mut x = 1;
        while x < nx as i32 - 1 {
            let r = h3(x as f32, shelf as f32, seed, 2.0);
            if r < 0.1 {
                x += 2;
                continue;
            }
            let c = hex(spines[(r * 97.0) as usize % spines.len()], 1.0);
            let tall = 2 + (r * 3.0) as i32 % 2;
            for y in y0 + 1..=y0 + tall {
                for z in 1..nz as i32 - 1 {
                    g.set(x, y, z, c);
                }
            }
            x += 1;
        }
    }
    g.mesh(v, true)
}

/// A strand of kelp `h` tall: two or three stalks swaying in a slow wave,
/// with blades off them, olive-brown to green-gold at the tips. Voxel 0.25.
/// Give it sway when placing.
pub fn kelp(h: f32, variant: i32) -> VoxelMesh {
    let v = 0.25;
    let mut m = VoxelMesh::default();
    let stalks = 2 + (variant % 2);
    for s in 0..stalks {
        let ph = s as f32 * 2.1 + variant as f32;
        let (ox, oz) = ((s as f32 - 1.0) * 0.35, (h3(s as f32, 0.0, variant as f32, 1.0) - 0.5) * 0.6);
        let n = ((h * (0.75 + 0.25 * h3(s as f32, 1.0, variant as f32, 2.0))) / v) as i32;
        for i in 0..n {
            let y = i as f32 * v;
            let t = y / h;
            let x = ox + (y * 0.45 + ph).sin() * 0.5 * t;
            let z = oz + (y * 0.31 + ph * 0.7).cos() * 0.35 * t;
            let c = [0.12 + 0.20 * t, 0.20 + 0.30 * t, 0.06 + 0.05 * t, 1.0];
            m.inst.push(crate::voxel::VoxInst { off_ao: [x, y, z, 0.9], color: c, misc: [1.0, 0.0, 0.0, 0.0] });
            // a blade every few voxels, out to one side
            if i % 4 == 2 {
                let side = if (i / 4) % 2 == 0 { 1.0 } else { -1.0 };
                for b in 1..4 {
                    let bx = x + side * b as f32 * v;
                    m.inst.push(crate::voxel::VoxInst { off_ao: [bx, y + b as f32 * v * 0.6, z, 0.95], color: [c[0] * 1.1, c[1] * 1.15, c[2], 1.0], misc: [1.0, 0.0, 0.0, 0.0] });
                }
            }
        }
    }
    m
}

/// A coral head about `size` across: branching fingers from a knobbed
/// base, the tips glowing faintly. Voxel 0.2.
pub fn coral(size: f32, variant: i32) -> VoxelMesh {
    let k = 5.0 / size.max(0.3) * size; // cells per unit (voxel 0.2)
    let n = (size * 5.0 * 1.4) as usize + 2;
    let ny = (size * 5.0 * 1.3) as usize + 2;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let pal = [[0xc45e6e, 0xf2a28c], [0xd8783a, 0xf6c070], [0x7a4ea0, 0xd8a0f0], [0x2e8a7a, 0x9af0d8]];
    let [lo, hi] = pal[(variant as usize) % pal.len()];
    let (lo, hi) = (hex(lo, 1.0), hex(hi, 1.0));
    let _ = k;
    g.ellipsoid(c, 1.5, c, size * 1.6, 1.8, size * 1.6, lo);
    let fingers = 5 + variant % 4;
    for f in 0..fingers {
        let a = f as f32 / fingers as f32 * std::f32::consts::TAU + variant as f32;
        let lean = 0.35 + 0.3 * h3(f as f32, 0.0, variant as f32, 3.0);
        let len = ny as f32 * (0.55 + 0.4 * h3(f as f32, 1.0, variant as f32, 4.0));
        for i in 0..len as i32 {
            let t = i as f32 / len;
            let (x, z) = (c + a.cos() * lean * i as f32 * 0.6, c + a.sin() * lean * i as f32 * 0.6);
            let col = if t > 0.8 { hi } else { lo };
            g.set(x as i32, 1 + i, z as i32, col);
            g.set(x as i32 + 1, 1 + i, z as i32, col);
        }
    }
    let mut m = g.mesh(0.2, true);
    let top = m.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max).max(0.1);
    for i in m.inst.iter_mut() {
        if i.off_ao[1] > top * 0.75 {
            i.misc[2] = 0.5;
        }
    }
    m
}

/// A sea lantern: a marble post with a bronze cage and a caught glow of
/// the deep, cyan. About 2.8 u tall.
pub fn sea_lantern(variant: i32) -> VoxelMesh {
    let mut m = crate::skyprops::star_lantern(variant);
    for i in m.inst.iter_mut() {
        if i.misc[2] >= 1.5 {
            i.color = [0.55, 0.98, 1.0, 1.0];
            i.misc[2] = 2.4;
        } else if i.misc[2] > 0.2 {
            i.color = hex(0x2f7a66, 1.0);
            i.misc[2] = 0.2;
        } else {
            let l = i.color[0];
            i.color = [l * 0.8, l * 0.88, l * 0.88, 1.0];
        }
    }
    m
}

/// A cat of the old ones in sea marble, `scale` x the jungle statue.
pub fn statue(scale: f32, variant: i32) -> VoxelMesh {
    sea_marble(crate::props::statue_cat(scale, variant), 2.0 + variant as f32)
}

/// The orichalcum heart at the Library's centre: a tall glowing crystal
/// on a stepped plinth. Voxel 0.25.
pub fn orichalcum(variant: i32) -> VoxelMesh {
    let k = 4.0;
    let n = (5.0 * k) as usize;
    let ny = (7.0 * k) as usize;
    let mut g = Grid::new(n, ny, n);
    let c = n as f32 * 0.5;
    let stone = [0.7, 0.75, 0.75, 1.0];
    g.cyl_y(c, c, 2.4 * k, 0, (0.4 * k) as i32, stone);
    g.cyl_y(c, c, 1.8 * k, (0.4 * k) as i32, (0.8 * k) as i32, stone);
    let crystal = [0.55, 0.95, 0.88, 1.0];
    g.cone_y(c, c, 0.9 * k, 0.05 * k, (0.8 * k) as i32, (6.8 * k) as i32, crystal);
    for s in 0..3 {
        let a = s as f32 * 2.1 + variant as f32;
        g.cone_y(c + a.cos() * 1.1 * k, c + a.sin() * 1.1 * k, 0.45 * k, 0.03 * k, (0.8 * k) as i32, (3.6 * k) as i32, crystal);
    }
    let mut m = g.mesh(0.25, true);
    for i in m.inst.iter_mut() {
        if i.color[1] > 0.9 {
            i.misc[2] = 1.4 + 0.6 * (i.off_ao[1] / 6.8);
        }
    }
    let marble: Vec<usize> = (0..m.inst.len()).filter(|&j| m.inst[j].color[1] < 0.9).collect();
    for j in marble {
        let l = 0.72 + 0.08 * h3(m.inst[j].off_ao[0], m.inst[j].off_ao[1], m.inst[j].off_ao[2], 1.0);
        m.inst[j].color = [l, l * 1.05, l * 1.05, 1.0];
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pieces_are_built_the_way_the_halls_need_them() {
        let c = column(10.0, 1);
        let top = c.inst.iter().map(|i| i.off_ao[1]).fold(0.0f32, f32::max);
        assert!((9.0..10.6).contains(&top), "column top {top}");
        assert!(c.inst.iter().any(|i| i.misc[2] > 0.5), "gold collars glow");
        let r = rib(16.0, 3.0, 0.9);
        let (lo, hi) = r.inst.iter().fold((f32::MAX, f32::MIN), |(a, b), i| (a.min(i.off_ao[1]), b.max(i.off_ao[1])));
        assert!(lo.abs() < 0.01 && (2.5..4.5).contains(&hi), "rib {lo}..{hi}");
        let d = dome(20.0, 12.0);
        let glass = d.inst.iter().filter(|i| i.color[3] < 0.5).count();
        let solid = d.inst.iter().filter(|i| i.color[3] >= 0.99).count();
        assert!(glass > solid * 2 && solid > 100, "a glass dome with ribs: glass {glass}, ribs {solid}");
        assert!(d.inst.iter().all(|i| i.off_ao[1] <= 12.01 && i.off_ao[0].hypot(i.off_ao[2]) <= 20.01));
        let v = vault_glass(16.0, 3.0, 10.0);
        assert!(v.inst.iter().all(|i| i.color[3] < 0.5), "all glass");
        let s = stained_glass(2.0, 4.0, false, 1.0);
        assert!(s.inst.iter().filter(|i| i.misc[2] > 0.5).count() > s.inst.len() / 3, "the glass holds light");
        assert!(!kelp(6.0, 1).inst.is_empty() && !coral(1.2, 2).inst.is_empty() && !bookcase(3.0, 1.0).inst.is_empty());
        assert!(orichalcum(0).inst.iter().any(|i| i.misc[2] > 1.4));
    }
}
