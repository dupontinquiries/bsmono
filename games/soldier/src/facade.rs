//! facade.rs — the **building factory**: reusable machinery that turns a footprint plus a functional-space
//! role into a real facade (a window/door rhythm that visually communicates that role) and a furnished
//! interior (props appropriate to it), backed by a lightweight reachability *sim* that walks the interior
//! from its own doorway before the level ships the building — so "enterable" is checked, not assumed.
//!
//! This generalizes what Desert Town's `clay_building` did by hand into something any map can drive. Where
//! `spaces.rs` says "this open footprint's function implies this cover layout", this module says "this
//! building's function implies this architecture": a Stronghold reads fortified with narrow high windows
//! and thick piers; a Plaza reads open and glassy with wide sills; a Nest reads like a lookout with a firing
//! slit; a Crossing reads like a barricaded gatehouse; a Gauntlet reads utilitarian and industrial; a Muster
//! reads plain and functional. Same skeleton (`clay_wall_run`'s panel rhythm), different parameters — and
//! the interior gets furnished from the same role, not left as a bare box.

use crate::level::{Obstacle, Palette, Rng};
use crate::math::Vec3;
use crate::mesh::{boxx, dither, disc, rgb, rotate_y_about, tube, Material, Mesh3, FACE_ALL};
use crate::spaces::SpaceFunction;
use egui::Color32;

#[inline]
fn scale_rgb(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
}

/// Facade parameters derived from a functional-space role — the single place "this space's function implies
/// this architecture" lives for buildings.
struct FacadeStyle {
    /// Fraction of wall panels that open (window or door) vs solid pier. Low = blind fortress wall.
    open_frac: f32,
    /// Window sill height as a fraction of story height: low = near-floor glass, high = narrow firing slit.
    sill_frac: f32,
    material: Material,
    trim: Color32,
}

impl FacadeStyle {
    fn for_role(role: SpaceFunction, pal: &Palette) -> Self {
        match role {
            SpaceFunction::Stronghold => FacadeStyle { open_frac: 0.28, sill_frac: 0.58, material: Material::Concrete, trim: scale_rgb(pal.trim, 0.75) },
            SpaceFunction::Plaza => FacadeStyle { open_frac: 0.7, sill_frac: 0.12, material: Material::Tile, trim: pal.accent },
            SpaceFunction::Nest => FacadeStyle { open_frac: 0.18, sill_frac: 0.72, material: Material::StoneBlock, trim: scale_rgb(pal.trim, 0.82) },
            SpaceFunction::Crossing => FacadeStyle { open_frac: 0.38, sill_frac: 0.48, material: Material::Brick, trim: pal.trim },
            SpaceFunction::Gauntlet => FacadeStyle { open_frac: 0.42, sill_frac: 0.34, material: Material::Corrugated, trim: scale_rgb(pal.trim, 0.88) },
            SpaceFunction::Muster => FacadeStyle { open_frac: 0.5, sill_frac: 0.28, material: Material::Plaster, trim: pal.trim },
        }
    }
}

/// A generated building: its shell/interior geometry+collision, the world-space footprint (for the sim
/// reachability check and for interior furnishing), the door position, and the story height (for placing
/// floor slabs / roof access elsewhere).
pub struct Building {
    pub meshes: Vec<Mesh3>,
    pub obstacles: Vec<Obstacle>,
    pub door: Vec3,
    pub footprint: (f32, f32, f32, f32), // x0, z0, x1, z1
    pub roof_y: f32,
}

/// One wall run of a facade: a row of panels (piers / window bays / one door bay) whose rhythm is driven by
/// `style` instead of being hand-set per map. Mirrors `level.rs`'s `clay_wall_run` but style-parameterized.
#[allow(clippy::too_many_arguments)]
fn facade_wall_run(
    axis_x: bool,
    fixed: f32,
    a: f32,
    b: f32,
    base_y: f32,
    height: f32,
    th: f32,
    door_panel: i32,
    style: &FacadeStyle,
    seed: u32,
    out_m: &mut Vec<Mesh3>,
    out_o: &mut Vec<Obstacle>,
) {
    let span = (b - a).abs();
    let n = ((span / 1.8).round() as i32).max(1);
    let pw = span / n as f32;
    let sill = height * style.sill_frac;
    let lintel = (sill + height * 0.32).min(height - 0.3);
    let wall = scale_rgb(rgb(180, 178, 176), 1.0); // overwritten by material tint below; base tone only
    for i in 0..n {
        let c0 = a + pw * i as f32;
        let cc = c0 + pw * 0.5;
        let hw = pw * 0.5 * 0.98;
        let is_door = i == door_panel;
        // window rhythm driven by style: higher open_frac → more panels become windows
        let is_window = !is_door && (dither_bool(seed, i, style.open_frac));
        let mk = |lo: f32, hi: f32, key: i32, om: &mut Vec<Mesh3>, oo: &mut Vec<Obstacle>| {
            let (center, half) = if axis_x {
                (Vec3::new(cc, base_y + (lo + hi) * 0.5, fixed), Vec3::new(hw, (hi - lo) * 0.5, th))
            } else {
                (Vec3::new(fixed, base_y + (lo + hi) * 0.5, cc), Vec3::new(th, (hi - lo) * 0.5, hw))
            };
            let mut mesh = boxx(center, half, dither(wall, key, 0.07), FACE_ALL);
            mesh.material = Some(style.material);
            om.push(mesh);
            oo.push(Obstacle { center, half });
        };
        if is_door {
            mk(lintel, height, i * 3, out_m, out_o); // open doorway, lintel above only
        } else if is_window {
            if sill > 0.05 {
                mk(0.0, sill, i * 3 + 1, out_m, out_o); // sill you vault onto
            }
            mk(lintel, height, i * 3 + 2, out_m, out_o); // lintel + upper wall, window gap between
        } else {
            mk(0.0, height, i * 7, out_m, out_o); // solid pier
        }
    }
}

/// Deterministic "should this panel be a window" roll, independent of an RNG so the same (seed,i) always
/// gives the same answer regardless of call order — panels stay stable if a caller re-generates a building.
fn dither_bool(seed: u32, i: i32, frac: f32) -> bool {
    let h = crate::mesh::hash01(seed as i32 ^ (i.wrapping_mul(0x9E37)), 7);
    h < frac
}

/// Build a facade + hollow interior shell for a footprint, styled by `role`. One doorway on `door_side`
/// (0=N 1=S 2=W 3=E). Returns a `Building` ready for `furnish_interior` and the reachability sim.
#[allow(clippy::too_many_arguments)]
pub fn build_facade(cx: f32, cz: f32, hx: f32, hz: f32, stories: i32, door_side: u8, base_y: f32, role: SpaceFunction, pal: &Palette, seed: u32) -> Building {
    let style = FacadeStyle::for_role(role, pal);
    let mut m = Vec::new();
    let mut o = Vec::new();
    let story_h = 3.0;
    let height = stories as f32 * story_h;
    let th = 0.32;
    let (x0, x1, z0, z1) = (cx - hx, cx + hx, cz - hz, cz + hz);
    let dn = |side: u8| -> i32 {
        if door_side == side {
            1
        } else {
            -1
        }
    };
    facade_wall_run(true, z0, x0, x1, base_y, height, th, dn(0), &style, seed, &mut m, &mut o);
    facade_wall_run(true, z1, x0, x1, base_y, height, th, dn(1), &style, seed ^ 0x11, &mut m, &mut o);
    facade_wall_run(false, x0, z0, z1, base_y, height, th, dn(2), &style, seed ^ 0x22, &mut m, &mut o);
    facade_wall_run(false, x1, z0, z1, base_y, height, th, dn(3), &style, seed ^ 0x33, &mut m, &mut o);

    let roof_y = base_y + height;
    let mut roof = boxx(Vec3::new(cx, roof_y + 0.14, cz), Vec3::new(hx + th, 0.14, hz + th), scale_rgb(style.trim, 1.05), FACE_ALL);
    roof.material = Some(Material::Concrete);
    m.push(roof.clone());
    o.push(Obstacle { center: Vec3::new(cx, roof_y + 0.14, cz), half: Vec3::new(hx + th, 0.14, hz + th) });
    // low parapet — standable rooftop cover. Textured (not raw trim vertex colour): a few roles' trim comes
    // straight from the palette's accent, which can be a saturated colour (teal, cyan) that reads as an odd
    // tinted stripe wrapping the whole roofline when left as flat vertex colour instead of a real material.
    for (px, pz, phx, phz) in [(cx, z0, hx, 0.16f32), (cx, z1, hx, 0.16), (x0, cz, 0.16, hz), (x1, cz, 0.16, hz)] {
        let pc = Vec3::new(px, roof_y + 0.5, pz);
        let ph = Vec3::new(phx, 0.34, phz);
        let mut par = boxx(pc, ph, scale_rgb(style.trim, 0.9), FACE_ALL);
        par.material = Some(Material::StoneBlock);
        m.push(par);
        o.push(Obstacle { center: pc, half: ph });
    }
    let door_pos = match door_side {
        0 => Vec3::new(cx, base_y, z0 - 0.6),
        1 => Vec3::new(cx, base_y, z1 + 0.6),
        2 => Vec3::new(x0 - 0.6, base_y, cz),
        _ => Vec3::new(x1 + 0.6, base_y, cz),
    };
    Building { meshes: m, obstacles: o, door: door_pos, footprint: (x0, z0, x1, z1), roof_y }
}

/// Furnish a building's interior with props appropriate to its role — the same "function implies structure"
/// idea `spaces.rs` uses for open cover, applied to what's actually sitting inside a room. Returns aligned
/// (meshes, obstacles); props are kept clear of the footprint's edges so they don't collide with the shell.
pub fn furnish_interior(b: &Building, base_y: f32, pal: &Palette, role: SpaceFunction, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let (x0, z0, x1, z1) = b.footprint;
    let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
    let (hx, hz) = ((x1 - x0) * 0.5 - 0.6, (z1 - z0) * 0.5 - 0.6);
    if hx < 0.6 || hz < 0.6 {
        return (Vec::new(), Vec::new()); // too small to furnish without choking the doorway
    }
    let mut rng = Rng::new(seed as u64 ^ 0xF0E5);
    let mut m = Vec::new();
    let mut o = Vec::new();
    let spot = |r: &mut Rng| -> (f32, f32) { (cx + (r.f32() - 0.5) * hx * 1.6, cz + (r.f32() - 0.5) * hz * 1.6) };
    match role {
        SpaceFunction::Stronghold => {
            // crates + sandbag mounds — a defended interior
            for _ in 0..4 {
                let (px, pz) = spot(&mut rng);
                let s = 0.4 + rng.f32() * 0.25;
                let mut c = boxx(Vec3::new(px, base_y + s, pz), Vec3::splat(s), scale_rgb(pal.cover, 0.9), FACE_ALL);
                c.material = Some(Material::Planks);
                m.push(c);
                o.push(Obstacle { center: Vec3::new(px, base_y + s, pz), half: Vec3::splat(s) });
            }
        }
        SpaceFunction::Plaza => {
            // a table/bench and a couple of stools — a public gathering interior
            let (px, pz) = (cx, cz);
            let mut table = boxx(Vec3::new(px, base_y + 0.42, pz), Vec3::new(1.1, 0.42, 0.6), scale_rgb(pal.trim, 0.85), FACE_ALL);
            table.material = Some(Material::Planks);
            m.push(table);
            o.push(Obstacle { center: Vec3::new(px, base_y + 0.42, pz), half: Vec3::new(1.1, 0.42, 0.6) });
            for s in [-1.6f32, 1.6] {
                let sx = px + s;
                let mut stool = boxx(Vec3::new(sx, base_y + 0.22, pz), Vec3::splat(0.22), scale_rgb(pal.trim, 0.7), FACE_ALL);
                stool.material = Some(Material::Planks);
                m.push(stool);
                o.push(Obstacle { center: Vec3::new(sx, base_y + 0.22, pz), half: Vec3::splat(0.22) });
            }
        }
        SpaceFunction::Nest => {
            // a single raised firing perch facing the door
            let (px, pz) = (cx, cz);
            let mut perch = boxx(Vec3::new(px, base_y + 0.5, pz), Vec3::new(0.7, 0.5, 0.7), scale_rgb(pal.cover, 0.95), FACE_ALL);
            perch.material = Some(Material::StoneBlock);
            m.push(perch);
            o.push(Obstacle { center: Vec3::new(px, base_y + 0.5, pz), half: Vec3::new(0.7, 0.5, 0.7) });
        }
        SpaceFunction::Crossing => {
            // a barricade just inside the door — you fight through the doorway, not stand in the open
            let (px, pz) = (cx, cz);
            let mut bar = boxx(Vec3::new(px, base_y + 0.5, pz), Vec3::new(hx.min(1.2), 0.5, 0.3), scale_rgb(pal.wall, 0.8), FACE_ALL);
            bar.material = Some(Material::Concrete);
            m.push(bar);
            o.push(Obstacle { center: Vec3::new(px, base_y + 0.5, pz), half: Vec3::new(hx.min(1.2), 0.5, 0.3) });
        }
        SpaceFunction::Gauntlet => {
            // staggered pipe/machinery clutter down the long axis
            let long_x = hx > hz;
            let n = 3;
            for i in 0..n {
                let t = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let side = if i % 2 == 0 { -0.5 } else { 0.5 };
                let (px, pz) = if long_x { (cx + t * hx, cz + side * hz) } else { (cx + side * hx, cz + t * hz) };
                m.push(tube(Vec3::new(px, base_y, pz), Vec3::new(px, base_y + 1.1, pz), 0.18, 6, scale_rgb(pal.trim, 0.8)));
                o.push(Obstacle { center: Vec3::new(px, base_y + 0.55, pz), half: Vec3::new(0.2, 0.55, 0.2) });
            }
        }
        SpaceFunction::Muster => {
            // a couple of supply crates near a wall, kept low so the room stays open
            for _ in 0..2 {
                let (px, pz) = spot(&mut rng);
                let mut c = boxx(Vec3::new(px, base_y + 0.3, pz), Vec3::splat(0.3), scale_rgb(pal.cover, 0.85), FACE_ALL);
                c.material = Some(Material::Planks);
                m.push(c);
                o.push(Obstacle { center: Vec3::new(px, base_y + 0.3, pz), half: Vec3::splat(0.3) });
            }
        }
    }
    // A lit fixture so the interior isn't pitch dark — a small hanging lamp near the ceiling, not a floor
    // disc. (An earlier version put a room-radius emissive disc at floor level and blended too little of its
    // colour away from the palette's accent, which reads blue/teal on any theme whose accent leans cool —
    // exactly the "weird blue floor" a real interior shouldn't have. Warm light, small fixture, up high.)
    let lamp_y = base_y + (b.roof_y - base_y).min(3.0) - 0.3;
    let warm = rgb(255, 214, 150);
    let mut lamp = disc(Vec3::new(cx, lamp_y, cz), 0.14, 6, warm);
    lamp.emissive = 0.8;
    m.push(lamp);
    let mut fixture = crate::mesh::tube(Vec3::new(cx, lamp_y + 0.02, cz), Vec3::new(cx, lamp_y + 0.18, cz), 0.03, 5, rgb(60, 56, 50));
    fixture.emissive = 0.0;
    m.push(fixture);
    let _ = rotate_y_about; // kept available for callers that want to rotate furnished sets
    (m, o)
}

/// **Sim**: verify a building's interior is actually reachable from its own doorway — a cheap flood-fill
/// over a fine grid inside the footprint, stepping around (not through) the furnished obstacles. Run this
/// before a building ships; a building that fails is a bug (a piece of furniture blocked its only door),
/// not a design choice, so callers should either drop the failing furniture set or leave the room empty.
pub fn sim_interior_reachable(b: &Building, obstacles: &[Obstacle], base_y: f32) -> bool {
    let (x0, z0, x1, z1) = b.footprint;
    let step = 0.35;
    let nx = (((x1 - x0) / step) as i32).max(1);
    let nz = (((z1 - z0) / step) as i32).max(1);
    let blocked = |wx: f32, wz: f32| -> bool {
        for ob in obstacles {
            let dx = (wx - ob.center.x).abs();
            let dz = (wz - ob.center.z).abs();
            if dx <= ob.half.x + 0.18 && dz <= ob.half.z + 0.18 && ob.half.y > 0.05 {
                // only chest-height+ obstacles actually block a walking player; low sills/steps don't
                if ob.center.y - base_y < 1.5 {
                    return true;
                }
            }
        }
        false
    };
    let idx = |i: i32, j: i32| -> usize { (j * (nx + 1) + i) as usize };
    let mut seen = vec![false; ((nx + 1) * (nz + 1)) as usize];
    let start = ((b.door.x - x0) / step).round().clamp(0.0, nx as f32) as i32;
    let startz = ((b.door.z - z0) / step).round().clamp(0.0, nz as f32) as i32;
    let mut stack = vec![(start, startz)];
    seen[idx(start, startz)] = true;
    let mut reached_cells = 0usize;
    while let Some((i, j)) = stack.pop() {
        reached_cells += 1;
        for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = (i + di, j + dj);
            if ni < 0 || nj < 0 || ni > nx || nj > nz {
                continue;
            }
            if seen[idx(ni, nj)] {
                continue;
            }
            let wx = x0 + ni as f32 * step;
            let wz = z0 + nj as f32 * step;
            if blocked(wx, wz) {
                continue;
            }
            seen[idx(ni, nj)] = true;
            stack.push((ni, nj));
        }
    }
    // reachable if the flood covers a meaningful fraction of the interior, not just the doorway cell
    let total = ((nx + 1) * (nz + 1)) as usize;
    reached_cells as f32 / total.max(1) as f32 > 0.25
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::rgb;

    fn pal() -> Palette {
        Palette {
            floor: rgb(150, 130, 102),
            wall: rgb(176, 138, 100),
            trim: rgb(196, 170, 140),
            ceil: rgb(70, 64, 58),
            cover: rgb(168, 150, 120),
            sky: rgb(206, 188, 150),
            fog: rgb(196, 176, 146),
            accent: rgb(120, 150, 150),
            rock: rgb(150, 134, 112),
        }
    }

    #[test]
    fn every_role_produces_a_reachable_furnished_interior() {
        let p = pal();
        for (i, role) in [
            SpaceFunction::Stronghold,
            SpaceFunction::Plaza,
            SpaceFunction::Nest,
            SpaceFunction::Crossing,
            SpaceFunction::Gauntlet,
            SpaceFunction::Muster,
        ]
        .into_iter()
        .enumerate()
        {
            let b = build_facade(20.0, 20.0, 5.0, 5.0, 2, 0, 0.0, role, &p, 100 + i as u32);
            assert!(b.meshes.iter().map(|m| m.tri_count()).sum::<usize>() > 0, "{role:?} facade should have geometry");
            let (fm, fo) = furnish_interior(&b, 0.0, &p, role, 100 + i as u32);
            assert!(!fm.is_empty(), "{role:?} interior should be furnished with something");
            let mut all_obs = b.obstacles.clone();
            all_obs.extend(fo.iter().copied());
            assert!(sim_interior_reachable(&b, &all_obs, 0.0), "{role:?} interior should be reachable from its own door — furniture blocked it");
        }
    }

    #[test]
    fn different_roles_read_as_different_facades() {
        let p = pal();
        let stronghold = FacadeStyle::for_role(SpaceFunction::Stronghold, &p);
        let plaza = FacadeStyle::for_role(SpaceFunction::Plaza, &p);
        assert!(plaza.open_frac > stronghold.open_frac, "a plaza should read more open than a fortified stronghold");
        assert!(plaza.sill_frac < stronghold.sill_frac, "a plaza's glass should run lower than a stronghold's firing-slit windows");
    }
}
