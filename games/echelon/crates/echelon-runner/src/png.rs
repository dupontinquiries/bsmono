//! Look-dev renderer: applies a TilesetDef's palette + light rig to a map
//! (and optionally a live World) on the CPU, writing PNG. This is the
//! reference for the Bevy pass — same numbers, same intent: dark albedos,
//! broad worn specular, colored key against complementary ambience, rim
//! back-glow, heavy grit. No dependencies; PNG uses stored deflate blocks.

use echelon_data::{Kind, Rgb, TilesetDef};
use echelon_sim::map::{GameMap, ResKind, H_VOID};
use std::sync::OnceLock;

/// sprite atlas baked by `cadscope atlas` — units x 8 facings, RGBA
struct Atlas {
    px: usize,
    facings: usize,
    w: usize,
    rows: std::collections::HashMap<String, usize>,
    data: Vec<u8>,
}

fn atlas() -> Option<&'static Atlas> {
    static A: OnceLock<Option<Atlas>> = OnceLock::new();
    A.get_or_init(|| {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/gfx/echelon-atlas");
        let meta = std::fs::read_to_string(base.with_extension("meta")).ok()?;
        let data = std::fs::read(base.with_extension("raw")).ok()?;
        let mut px = 24;
        let mut facings = 8;
        let mut rows = std::collections::HashMap::new();
        for line in meta.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next()) {
                (Some("sprite"), Some(v)) => px = v.parse().ok()?,
                (Some("facings"), Some(v)) => facings = v.parse().ok()?,
                (Some(name), Some(row)) => {
                    rows.insert(name.to_string(), row.parse().ok()?);
                }
                _ => {}
            }
        }
        Some(Atlas { px, facings, w: px * facings, rows, data })
    })
    .as_ref()
}
use echelon_sim::World;

const PX: i32 = 12; // pixels per tile
const ELEV: f32 = 6.0; // px of relief per terrain level (0..5)

#[derive(Clone, Copy)]
struct V3(f32, f32, f32);
impl V3 {
    fn dot(self, o: V3) -> f32 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    fn norm(self) -> V3 {
        let l = self.dot(self).sqrt().max(1e-6);
        V3(self.0 / l, self.1 / l, self.2 / l)
    }
    fn scale(self, k: f32) -> V3 {
        V3(self.0 * k, self.1 * k, self.2 * k)
    }
    fn add(self, o: V3) -> V3 {
        V3(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
    fn mul(self, o: V3) -> V3 {
        V3(self.0 * o.0, self.1 * o.1, self.2 * o.2)
    }
}
fn c(r: Rgb) -> V3 {
    // sRGB -> linear-ish
    let f = |v: u8| (v as f32 / 255.0).powf(2.2);
    V3(f(r.0), f(r.1), f(r.2))
}
fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    a.scale(1.0 - t).add(b.scale(t))
}

fn hash2(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E3779B1) ^ (y as u32).wrapping_mul(0x85EBCA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2AE3D);
    h ^= h >> 13;
    h
}
fn val(x: i32, y: i32) -> f32 {
    (hash2(x, y) & 0xffff) as f32 / 65535.0
}
/// bilinear value noise at lattice scale s
fn noise(px: f32, py: f32, s: f32) -> f32 {
    let (gx, gy) = (px / s, py / s);
    let (x0, y0) = (gx.floor() as i32, gy.floor() as i32);
    let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
    let (fx, fy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = val(x0, y0);
    let b = val(x0 + 1, y0);
    let d = val(x0, y0 + 1);
    let e = val(x0 + 1, y0 + 1);
    a + (b - a) * fx + (d - a) * fy + (a - b - d + e) * fx * fy
}

fn hlevel(h: u8) -> f32 {
    if h == H_VOID {
        -2.4 // bottomless chasm
    } else {
        h as f32
    }
}

pub fn render(map: &GameMap, world: Option<&World>, ts: &TilesetDef, path: &str) -> std::io::Result<()> {
    let (tw, th) = (map.w, map.h);
    let (w, h) = ((tw * PX) as usize, (th * PX) as usize);

    // ---- heightfield (bilinear across tile centers + micro noise) ----
    let tile_h = |tx: i32, ty: i32| -> f32 {
        let tx = tx.clamp(0, tw - 1);
        let ty = ty.clamp(0, th - 1);
        let h = hlevel(map.hgt(tx, ty));
        match map.ramp(tx, ty) {
            Some(lo) => lo as f32 + 0.5,
            None => h,
        }
    };
    let mut hf = vec![0f32; w * h];
    for py in 0..h as i32 {
        for px in 0..w as i32 {
            let fx = (px as f32 + 0.5) / PX as f32 - 0.5;
            let fy = (py as f32 + 0.5) / PX as f32 - 0.5;
            let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
            let (u, v) = (fx - x0 as f32, fy - y0 as f32);
            let a = tile_h(x0, y0);
            let b = tile_h(x0 + 1, y0);
            let d = tile_h(x0, y0 + 1);
            let e = tile_h(x0 + 1, y0 + 1);
            let base = a + (b - a) * u + (d - a) * v + (a - b - d + e) * u * v;
            let micro = (noise(px as f32, py as f32, 7.0) - 0.5) * 0.16
                + (noise(px as f32, py as f32, 2.3) - 0.5) * 0.07;
            hf[(py as usize) * w + px as usize] = base * ELEV + micro * ELEV;
        }
    }
    let hat = |x: i32, y: i32| -> f32 {
        hf[(y.clamp(0, h as i32 - 1) as usize) * w + x.clamp(0, w as i32 - 1) as usize]
    };

    // ---- light rig ----
    let l = V3(
        -(ts.sun_dir.0 as f32),
        -(ts.sun_dir.1 as f32),
        ts.sun_dir.2 as f32,
    )
    .norm();
    let rim_dir = V3(-l.0, -l.1, 0.22).norm();
    let view = V3(0.0, 0.0, 1.0);
    let half = l.add(view).norm();
    let sun = c(ts.sun).scale(1.35);
    let amb = c(ts.ambient).scale(0.9);
    let rim = c(ts.rim);
    let spec_k = ts.spec as f32 / 100.0;
    let grit_k = ts.grit as f32 / 100.0;

    let team = |p: u8| -> V3 {
        if p == 0 {
            V3(0.10, 0.34, 0.95)
        } else {
            V3(0.95, 0.16, 0.09)
        }
    };

    let mut img = vec![0u8; w * h * 3];
    for py in 0..h as i32 {
        for px in 0..w as i32 {
            let (tx, ty) = (px / PX, py / PX);
            let hg = map.hgt(tx, ty);

            // normal from heightfield
            let dzx = hat(px + 1, py) - hat(px - 1, py);
            let dzy = hat(px + 1, py + 1) * 0.0 + hat(px, py + 1) - hat(px, py - 1);
            let n = V3(-dzx, -dzy, 2.6).norm();

            // ---- albedo ----
            let macro_t = noise(px as f32, py as f32, 34.0);
            let deck = map.bridge(tx, ty);
            let mut alb = if map.ramp(tx, ty).is_some() {
                c(ts.ramp)
            } else if hg == H_VOID {
                c(ts.rock).scale(0.7)
            } else {
                let lt = lerp3(c(ts.low), c(ts.low_var), macro_t);
                let ht = lerp3(c(ts.high), c(ts.high_var), macro_t);
                lerp3(lt, ht, hg as f32 / 5.0)
            };
            // grit: two-octave grime multiply, SC1-style busy surface
            let g = noise(px as f32 * 1.7, py as f32 * 1.7, 3.1) * 0.65
                + noise(px as f32, py as f32, 9.0) * 0.35;
            alb = alb.scale(1.0 - grit_k * 0.55 * (1.0 - g));
            // cliff faces: strong slope -> cliff material with strata bands
            let slope = (dzx * dzx + dzy * dzy).sqrt();
            let cliff_t = ((slope - 0.55) * 1.4).clamp(0.0, 1.0);
            if cliff_t > 0.0 {
                let strata = 0.85 + 0.15 * ((hat(px, py) * 1.9).sin() * 0.5 + 0.5);
                alb = lerp3(alb, c(ts.cliff).scale(strata), cliff_t);
            }

            // ---- shading ----
            let ndl = n.dot(l).max(0.0);
            // soft contact occlusion under cliffs: lower ground near a
            // higher neighbor tile sits in grime shadow
            let mut occ = 1.0f32;
            for (ox, oy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nh = hlevel(map.hgt(tx + ox, ty + oy));
                let mh = hlevel(hg);
                if nh > mh + 0.4 {
                    let fx = px - tx * PX;
                    let fy = py - ty * PX;
                    let d = match (ox, oy) {
                        (1, 0) => PX - 1 - fx,
                        (-1, 0) => fx,
                        (0, 1) => PX - 1 - fy,
                        _ => fy,
                    } as f32;
                    occ = occ.min(0.45 + 0.55 * (d / PX as f32).clamp(0.0, 1.0));
                }
            }
            let mut col = alb.mul(amb.add(sun.scale(ndl)).scale(occ));
            // broad, worn specular — the "specular but not glossy" knob.
            // Gated by slope so flats carry broken sparkle while lips,
            // ramps and cliff faces catch the real sheen.
            let gate = 0.15 + 0.85 * (slope * 2.2).clamp(0.0, 1.0);
            let sp = n.dot(half).max(0.0).powf(ts.spec_pow as f32) * spec_k * ndl * gate;
            col = col.add(sun.scale(sp * occ));
            // rim back-glow on silhouettes facing away from key
            let rimt = n.dot(rim_dir).max(0.0);
            col = col.add(rim.mul(alb.add(V3(0.05, 0.05, 0.05))).scale(rimt * rimt * 0.9));

            img[((py as usize) * w + px as usize) * 3] = 0; // filled after fx
            if hg == H_VOID && deck == 0 {
                col = col.scale(0.20); // depth fog in the chasm
            }
            // ---- bridge deck: flat lit slab riding at its own level ----
            if deck > 0 {
                let fxp = px - tx * PX;
                let fyp = py - ty * PX;
                let edge_n = map.bridge(tx, ty - 1) == 0 && fyp < 2;
                let edge_s = map.bridge(tx, ty + 1) == 0 && fyp >= PX - 2;
                let edge_w = map.bridge(tx - 1, ty) == 0 && fxp < 2;
                let edge_e = map.bridge(tx + 1, ty) == 0 && fxp >= PX - 2;
                let jitter = (noise(px as f32 * 2.1, py as f32 * 2.1, 2.6) - 0.5) * 0.5;
                let dn = V3(jitter * 0.4, jitter * 0.3, 1.0).norm();
                let plank = 0.9 + 0.1 * ((px as f32 * 0.9).sin() * 0.5 + 0.5);
                let mut da = c(ts.bridge).scale(plank * 1.35);
                let g2 = noise(px as f32 * 1.5, py as f32 * 1.5, 3.4);
                da = da.scale(1.0 - grit_k * 0.45 * (1.0 - g2));
                let ndl2 = dn.dot(l).max(0.0);
                let mut dc = da.mul(amb.add(sun.scale(ndl2)));
                let sp2 = dn.dot(half).max(0.0).powf(ts.spec_pow as f32) * spec_k * ndl2;
                dc = dc.add(sun.scale(sp2));
                if edge_n || edge_s || edge_w || edge_e {
                    dc = dc.scale(0.22); // dark rail/underside lip
                }
                col = dc;
            }
            let i = ((py as usize) * w + px as usize) * 3;
            let tm = |v: f32| {
                let v = v / (1.0 + v);
                ((v.powf(1.0 / 2.2)) * 255.0).clamp(0.0, 255.0) as u8
            };
            img[i] = tm(col.0);
            img[i + 1] = tm(col.1);
            img[i + 2] = tm(col.2);
        }
    }

    // ---- bridge drop shadows onto the ground beneath/beside ----
    for ty in 0..th {
        for tx in 0..tw {
            let b = map.bridge(tx, ty);
            if b == 0 {
                continue;
            }
            let drop = (b as f32 - hlevel(map.hgt(tx, ty)).max(-2.4)) * 0.9;
            let (sx, sy) = ((drop * 1.6) as i32 + 2, (drop * 1.1) as i32 + 1);
            for oy in 0..PX {
                for px_ in 0..PX {
                    let (x, y) = (tx * PX + px_ + sx, ty * PX + oy + sy);
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                        continue;
                    }
                    if map.bridge(x / PX, y / PX) > 0 {
                        continue; // decks don't self-shadow here
                    }
                    let i = (y as usize * w + x as usize) * 3;
                    img[i] = (img[i] as f32 * 0.55) as u8;
                    img[i + 1] = (img[i + 1] as f32 * 0.55) as u8;
                    img[i + 2] = (img[i + 2] as f32 * 0.55) as u8;
                }
            }
        }
    }

    // ---- emissive resources (additive glow) ----
    let add_glow = |cx: f32, cy: f32, color: V3, radius: f32, img: &mut Vec<u8>| {
        let r = radius as i32 + 1;
        for oy in -r..=r {
            for ox in -r..=r {
                let (x, y) = (cx as i32 + ox, cy as i32 + oy);
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let d2 = (ox * ox + oy * oy) as f32;
                let g = (-d2 / (radius * radius * 0.45)).exp();
                if g < 0.02 {
                    continue;
                }
                let i = (y as usize * w + x as usize) * 3;
                let addch = |cur: u8, e: f32| -> u8 {
                    let lin = (cur as f32 / 255.0).powf(2.2) + e;
                    let v = lin / (1.0 + lin * 0.35);
                    (v.powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8
                };
                img[i] = addch(img[i], color.0 * g);
                img[i + 1] = addch(img[i + 1], color.1 * g);
                img[i + 2] = addch(img[i + 2], color.2 * g);
            }
        }
    };
    for n in &map.nodes {
        if n.amount <= 0 {
            continue;
        }
        let cx = (n.tx * PX + PX / 2) as f32;
        let cy = (n.ty * PX + PX / 2) as f32;
        let (col, r) = match n.kind {
            ResKind::Alloy => (c(ts.alloy_glow).scale(0.85), 6.5),
            ResKind::Flux => (c(ts.flux_glow).scale(1.15), 8.5),
        };
        add_glow(cx, cy, col, r, &mut img);
    }

    // ---- units & structures (lit team-color forms) ----
    if let Some(wo) = world {
        for e in &wo.ents {
            if !e.alive || e.def == u16::MAX {
                continue;
            }
            let d = wo.pack.def(e.def);
            let cx = (e.pos.x.to_milli() * PX as i64 / 1000) as i32;
            let cy = (e.pos.y.to_milli() * PX as i64 / 1000) as i32;
            let tcol = team(e.player);
            if d.kind == Kind::Structure {
                let s = d.fp() * PX * 9 / 20; // half-side
                for oy in -s..=s {
                    for ox in -s..=s {
                        let (x, y) = (cx + ox, cy + oy);
                        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                            continue;
                        }
                        let edge = ox.abs().max(oy.abs());
                        // faceted top: fake bevel normal
                        let bn = V3(ox as f32 / s as f32, oy as f32 / s as f32, 1.6).norm();
                        let lt = 0.25 + 0.85 * bn.dot(l).max(0.0);
                        let sp = bn.dot(half).max(0.0).powf(ts.spec_pow as f32) * spec_k;
                        let mut cc = tcol.scale(0.5 * lt).add(sun.scale(sp * 0.6));
                        if e.progress > 0 {
                            cc = cc.scale(0.45);
                        }
                        if edge > s - 2 {
                            cc = cc.scale(0.25);
                        }
                        let i = (y as usize * w + x as usize) * 3;
                        let tm = |v: f32| {
                            let v = v / (1.0 + v);
                            (v.powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8
                        };
                        img[i] = tm(cc.0);
                        img[i + 1] = tm(cc.1);
                        img[i + 2] = tm(cc.2);
                    }
                }
            } else if let (Some(at), Some(&row)) =
                (atlas(), atlas().and_then(|a| a.rows.get(&d.name)))
            {
                let air = d.mover() == echelon_data::Mover::Air;
                let ei = wo
                    .ents
                    .iter()
                    .position(|x| std::ptr::eq(x, e))
                    .unwrap_or(0);
                // facing from the live path, else toward the held target,
                // else a per-player resting heading
                let (dx, dy) = if e.path_i < e.path.len() {
                    let wp = e.path[e.path_i];
                    (
                        wp.0 as f32 + 0.5 - e.pos.x.to_milli() as f32 / 1000.0,
                        wp.1 as f32 + 0.5 - e.pos.y.to_milli() as f32 / 1000.0,
                    )
                } else if e.target > 0 && (e.target as usize - 1) < wo.ents.len() {
                    let t = &wo.ents[e.target as usize - 1];
                    (
                        (t.pos.x.to_milli() - e.pos.x.to_milli()) as f32,
                        (t.pos.y.to_milli() - e.pos.y.to_milli()) as f32,
                    )
                } else if e.player == 0 {
                    (1.0, 1.0)
                } else {
                    (-1.0, -1.0)
                };
                let ang = (-dy).atan2(dx);
                let sector = ((ang / std::f32::consts::TAU * at.facings as f32).round()
                    as i32)
                    .rem_euclid(at.facings as i32) as usize;
                let bob = if air {
                    ((wo.tick as f32 + ei as f32 * 1.7) * 0.28).sin() * 1.4
                } else {
                    0.0
                };
                // altitude shadow for flyers
                if air {
                    let lvl = wo.fly_level(ei) as i32;
                    let ground = hlevel(map.hgt(cx / PX, cy / PX)).max(0.0) as i32;
                    let drop = (lvl - 6).max(0) + 2 + (5 - ground.min(5)) / 2;
                    let rr = 5;
                    for oy in -rr..=rr {
                        for ox in -rr..=rr {
                            if ox * ox + oy * oy > rr * rr {
                                continue;
                            }
                            let (x, y) = (cx + drop * 2 + ox, cy + drop + oy);
                            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                                continue;
                            }
                            let i = (y as usize * w + x as usize) * 3;
                            img[i] = (img[i] as f32 * 0.72) as u8;
                            img[i + 1] = (img[i + 1] as f32 * 0.72) as u8;
                            img[i + 2] = (img[i + 2] as f32 * 0.72) as u8;
                        }
                    }
                }
                // team underlay ring
                let rr = 4;
                for oy in -rr..=rr {
                    for ox in -rr..=rr {
                        let d2 = ox * ox + oy * oy;
                        if d2 > rr * rr || d2 < (rr - 2) * (rr - 2) {
                            continue;
                        }
                        let (x, y) = (cx + ox, cy + 3 + oy);
                        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                            continue;
                        }
                        let i = (y as usize * w + x as usize) * 3;
                        let t = tcol.scale(0.9);
                        img[i] = ((t.0 / (1.0 + t.0)).powf(1.0 / 2.2) * 255.0) as u8;
                        img[i + 1] = ((t.1 / (1.0 + t.1)).powf(1.0 / 2.2) * 255.0) as u8;
                        img[i + 2] = ((t.2 / (1.0 + t.2)).powf(1.0 / 2.2) * 255.0) as u8;
                    }
                }
                // sprite blit
                let sp = at.px as i32;
                for sy in 0..sp {
                    for sx in 0..sp {
                        let src = ((row * at.px + sy as usize) * at.w + sector * at.px
                            + sx as usize)
                            * 4;
                        let a = at.data[src + 3];
                        if a == 0 {
                            continue;
                        }
                        let (x, y) = (
                            cx - sp / 2 + sx,
                            cy - sp * 2 / 3 + sy + bob as i32,
                        );
                        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                            continue;
                        }
                        let i = (y as usize * w + x as usize) * 3;
                        img[i] = at.data[src];
                        img[i + 1] = at.data[src + 1];
                        img[i + 2] = at.data[src + 2];
                    }
                }
                // muzzle flash the instant a weapon cycles
                if let Some(wp) = &d.wpn {
                    if e.cd >= wp.cd - 1 && wp.cd > 2 {
                        let fx = cx + ((ang.cos()) * 9.0) as i32;
                        let fy = cy - ((ang.sin()) * 9.0) as i32;
                        for (ox, oy) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let (x, y) = (fx + ox, fy + oy);
                            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                                continue;
                            }
                            let i = (y as usize * w + x as usize) * 3;
                            img[i] = 255;
                            img[i + 1] = 240;
                            img[i + 2] = 170;
                        }
                    }
                }
            } else {
                let air = d.mover() == echelon_data::Mover::Air;
                let r = if d.kind == Kind::Worker {
                    2
                } else if d.supply2 >= 8 {
                    4
                } else {
                    3
                } + if air && d.depth_eff() >= 2 { 1 } else { 0 };
                // altitude shadow: offset grows with the band it rides
                if air {
                    let ei = wo
                        .ents
                        .iter()
                        .position(|x| std::ptr::eq(x, e))
                        .unwrap_or(0);
                    let lvl = wo.fly_level(ei) as i32;
                    let ground = hlevel(map.hgt(cx / PX, cy / PX)).max(0.0) as i32;
                    let drop = (lvl - 6).max(0) + 2 + (5 - ground.min(5)) / 2;
                    let (sx, sy) = (cx + drop * 2, cy + drop);
                    for oy in -r..=r {
                        for ox in -r..=r {
                            if ox * ox + oy * oy > r * r {
                                continue;
                            }
                            let (x, y) = (sx + ox, sy + oy);
                            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                                continue;
                            }
                            let i = (y as usize * w + x as usize) * 3;
                            img[i] = (img[i] as f32 * 0.7) as u8;
                            img[i + 1] = (img[i + 1] as f32 * 0.7) as u8;
                            img[i + 2] = (img[i + 2] as f32 * 0.7) as u8;
                        }
                    }
                }
                for oy in -r..=r {
                    for ox in -r..=r {
                        if ox * ox + oy * oy > r * r {
                            continue;
                        }
                        let (x, y) = (cx + ox, cy + oy);
                        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                            continue;
                        }
                        // sphere-ish normal for a specular ping
                        let sn = V3(ox as f32 / r as f32, oy as f32 / r as f32, 0.9).norm();
                        let lt = 0.3 + 0.8 * sn.dot(l).max(0.0);
                        let sp = sn.dot(half).max(0.0).powf((ts.spec_pow + 8) as f32) * 0.8;
                        let cc = tcol.scale(0.6 * lt).add(sun.scale(sp));
                        let i = (y as usize * w + x as usize) * 3;
                        let tm = |v: f32| {
                            let v = v / (1.0 + v);
                            (v.powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8
                        };
                        img[i] = tm(cc.0);
                        img[i + 1] = tm(cc.1);
                        img[i + 2] = tm(cc.2);
                    }
                }
            }
        }
    }

    write_png(path, w as u32, h as u32, &img)
}

// ---------------- minimal PNG (stored deflate) ----------------

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut c = Vec::with_capacity(4 + data.len());
    c.extend_from_slice(tag);
    c.extend_from_slice(data);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

fn write_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    // raw scanlines with filter byte 0
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        let s = y * w as usize * 3;
        raw.extend_from_slice(&rgb[s..s + w as usize * 3]);
    }
    // zlib stream: header + stored deflate blocks + adler
    let mut z = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() {
        let n = (raw.len() - off).min(65535);
        let last = off + n == raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[off..off + n]);
        off += n;
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}
