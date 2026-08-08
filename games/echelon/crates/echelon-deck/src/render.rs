//! Frame renderer for the Command Deck: the look-dev renderer's terrain
//! language (level-lerped albedo, lit bridge decks, chasm fog) plus the
//! player layer — fog of war, last-seen ghosts, selection rings, hp
//! bars, rally flags, and a HUD strip drawn with the 4x6 font.

use crate::font;
use echelon_data::{Kind, Mover, Rgb, TilesetDef};
use echelon_sim::map::{ResKind, H_VOID};
use echelon_sim::World;

pub const PX: i32 = 10;
pub const HUD_H: usize = 84;

pub struct Ghost {
    pub tx: i32,
    pub ty: i32,
    pub fp: i32,
    pub player: u8,
}

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub buf: Vec<u8>,
}

fn c3(c: Rgb) -> [f32; 3] {
    [c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0]
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn put(buf: &mut [u8], w: usize, x: i32, y: i32, c: [f32; 3]) {
    if x < 0 || y < 0 {
        return;
    }
    let i = (y as usize * w + x as usize) * 3;
    if i + 2 < buf.len() {
        buf[i] = (c[0].clamp(0.0, 1.0) * 255.0) as u8;
        buf[i + 1] = (c[1].clamp(0.0, 1.0) * 255.0) as u8;
        buf[i + 2] = (c[2].clamp(0.0, 1.0) * 255.0) as u8;
    }
}

fn scale_px(buf: &mut [u8], w: usize, x: i32, y: i32, k: f32) {
    if x < 0 || y < 0 {
        return;
    }
    let i = (y as usize * w + x as usize) * 3;
    if i + 2 < buf.len() {
        buf[i] = (buf[i] as f32 * k) as u8;
        buf[i + 1] = (buf[i + 1] as f32 * k) as u8;
        buf[i + 2] = (buf[i + 2] as f32 * k) as u8;
    }
}

fn disc(buf: &mut [u8], w: usize, cx: i32, cy: i32, r: i32, c: [f32; 3]) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                put(buf, w, cx + dx, cy + dy, c);
            }
        }
    }
}

fn ring(buf: &mut [u8], w: usize, cx: i32, cy: i32, r: i32, c: [f32; 3]) {
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 <= r * r && d2 >= (r - 1) * (r - 1) {
                put(buf, w, cx + dx, cy + dy, c);
            }
        }
    }
}

/// tile visibility for player p: any completed friendly with the tile
/// center in sight
pub fn tile_vis(world: &World, p: u8) -> Vec<bool> {
    let map = &world.map;
    let n = (map.w * map.h) as usize;
    let mut vis = vec![false; n];
    for (i, e) in world.ents.iter().enumerate() {
        if !e.alive || e.def == u16::MAX || e.player != p || e.progress != 0 {
            continue;
        }
        let s = world.sight(i);
        let r = s.to_milli() / 1000 + 1;
        let (cx, cy) = e.pos.tile();
        for ty in (cy - r as i32).max(0)..(cy + r as i32 + 1).min(map.h) {
            for tx in (cx - r as i32).max(0)..(cx + r as i32 + 1).min(map.w) {
                let dx = tx - cx;
                let dy = ty - cy;
                if ((dx * dx + dy * dy) as i64) <= (r * r) as i64 {
                    vis[(ty * map.w + tx) as usize] = true;
                }
            }
        }
    }
    vis
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    world: &World,
    ts: &TilesetDef,
    me: u8,
    selection: &[u32],
    ghosts: &[Ghost],
    banner: Option<&str>,
    hud_lines: &[String],
) -> Frame {
    let map = &world.map;
    let w = (map.w * PX) as usize;
    let map_h = (map.h * PX) as usize;
    let h = map_h + HUD_H;
    let mut buf = vec![0u8; w * h * 3];
    let vis = tile_vis(world, me);
    let emask = world.visible_mask(me);
    let low = c3(ts.low);
    let high = c3(ts.high);
    let rock = c3(ts.rock);
    let rampc = c3(ts.ramp);
    let bridgec = c3(ts.bridge);

    // ---- terrain ----
    for ty in 0..map.h {
        for tx in 0..map.w {
            let hg = map.hgt(tx, ty);
            let base = if map.bridge(tx, ty) > 0 {
                [bridgec[0] * 1.1, bridgec[1] * 1.1, bridgec[2] * 1.1]
            } else if map.ramp(tx, ty).is_some() {
                rampc
            } else if hg == H_VOID {
                [rock[0] * 0.25, rock[1] * 0.25, rock[2] * 0.3]
            } else {
                let t = hg as f32 / 5.0;
                let c = lerp(low, high, t);
                [c[0] * (0.55 + 0.5 * t), c[1] * (0.55 + 0.5 * t), c[2] * (0.55 + 0.5 * t)]
            };
            let fog = if vis[(ty * map.w + tx) as usize] { 1.0 } else { 0.38 };
            let col = [base[0] * fog, base[1] * fog, base[2] * fog];
            for py in 0..PX {
                for px in 0..PX {
                    let edge = px == 0 || py == 0;
                    let c = if edge {
                        [col[0] * 0.92, col[1] * 0.92, col[2] * 0.92]
                    } else {
                        col
                    };
                    put(&mut buf, w, tx * PX + px, ty * PX + py, c);
                }
            }
        }
    }

    // ---- resources ----
    for n in &map.nodes {
        if n.amount <= 0 {
            continue;
        }
        let seen = vis[(n.ty * map.w + n.tx) as usize];
        let glow = match n.kind {
            ResKind::Alloy => [0.22, 0.84, 0.73],
            ResKind::Flux => [0.69, 0.42, 0.95],
        };
        let k = if seen { 1.0 } else { 0.4 };
        disc(
            &mut buf,
            w,
            n.tx * PX + PX / 2,
            n.ty * PX + PX / 2,
            3,
            [glow[0] * k, glow[1] * k, glow[2] * k],
        );
    }

    // ---- ghosts (last-seen enemy structures) ----
    for g in ghosts {
        let c = if g.player == 0 { [0.25, 0.4, 0.6] } else { [0.55, 0.28, 0.24] };
        for ty in g.ty..g.ty + g.fp {
            for tx in g.tx..g.tx + g.fp {
                if vis[(ty * map.w + tx) as usize] {
                    continue; // live view supersedes memory
                }
                for py in 1..PX - 1 {
                    for px in 1..PX - 1 {
                        if (px + py) % 3 == 0 {
                            put(&mut buf, w, tx * PX + px, ty * PX + py, c);
                        }
                    }
                }
            }
        }
    }

    // ---- entities ----
    let p_cols = [[0.30, 0.62, 0.95f32], [0.95, 0.42, 0.32]];
    for (i, e) in world.ents.iter().enumerate() {
        if !e.alive || e.def == u16::MAX || !emask[i] {
            continue;
        }
        let d = world.pack.def(e.def);
        let (mx, my) = (e.pos.x.to_milli(), e.pos.y.to_milli());
        let cx = (mx * PX as i64 / 1000) as i32;
        let cy = (my * PX as i64 / 1000) as i32;
        let mut col = p_cols[e.player as usize];
        if e.progress != 0 {
            col = [col[0] * 0.5, col[1] * 0.5, col[2] * 0.5];
        }
        if d.kind == Kind::Structure {
            let fp = d.fp();
            let (tx, ty) = e.pos.tile();
            let x0 = (tx - fp / 2) * PX + 1;
            let y0 = (ty - fp / 2) * PX + 1;
            for py in 0..fp * PX - 2 {
                for px in 0..fp * PX - 2 {
                    let edge = px == 0 || py == 0 || px == fp * PX - 3 || py == fp * PX - 3;
                    let c = if edge {
                        [col[0] * 0.6, col[1] * 0.6, col[2] * 0.6]
                    } else {
                        [col[0] * 0.85, col[1] * 0.85, col[2] * 0.85]
                    };
                    put(&mut buf, w, x0 + px, y0 + py, c);
                }
            }
            if e.rally != (0, 0) {
                let rx = (e.rally.0 as i64 * PX as i64 / 1000) as i32;
                let ry = (e.rally.1 as i64 * PX as i64 / 1000) as i32;
                for k in 0..5 {
                    put(&mut buf, w, rx, ry - k, [1.0, 0.9, 0.3]);
                }
                put(&mut buf, w, rx + 1, ry - 4, [1.0, 0.9, 0.3]);
                put(&mut buf, w, rx + 2, ry - 4, [1.0, 0.9, 0.3]);
            }
        } else {
            let air = d.mover() == Mover::Air;
            let r = if d.kind == Kind::Worker { 2 } else { 3 } + if d.depth_eff() >= 2 { 1 } else { 0 };
            if air {
                let lvl = world.fly_level(i) as i32 - 5;
                for dy in 0..3 {
                    for dx in 0..4 {
                        scale_px(&mut buf, w, cx + lvl + dx, cy + lvl + dy, 0.7);
                    }
                }
            }
            disc(&mut buf, w, cx, cy, r, col);
            if air {
                ring(&mut buf, w, cx, cy, r + 1, [col[0] * 0.7, col[1] * 0.7, 1.0]);
            }
        }
        // hp / shield bars
        let rt_hp = world.max_hp(e.def);
        if e.hp < rt_hp {
            let frac = (e.hp.to_milli() as f32 / rt_hp.to_milli().max(1) as f32).clamp(0.0, 1.0);
            let bw = 8;
            let x0 = cx - bw / 2;
            let y0 = cy - 7;
            for px in 0..bw {
                let on = (px as f32) < frac * bw as f32;
                put(
                    &mut buf,
                    w,
                    x0 + px,
                    y0,
                    if on { [0.2, 0.9, 0.3] } else { [0.25, 0.08, 0.08] },
                );
            }
        }
        if selection.contains(&(i as u32)) {
            ring(&mut buf, w, cx, cy, 6, [0.5, 1.0, 0.55]);
        }
    }

    // ---- HUD ----
    for y in map_h..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            buf[i] = 16;
            buf[i + 1] = 18;
            buf[i + 2] = 22;
        }
    }
    for x in 0..w {
        put(&mut buf, w, x as i32, map_h as i32, [0.5, 0.44, 0.24]);
    }
    for (li, line) in hud_lines.iter().enumerate() {
        font::draw(
            &mut buf,
            w,
            10,
            map_h + 8 + li * 18,
            line,
            [214, 218, 224],
            2,
        );
    }
    if let Some(b) = banner {
        let bw = b.len() * (font::GW + 1) * 4;
        let x0 = (w.saturating_sub(bw)) / 2;
        for y in map_h / 2 - 24..map_h / 2 + 24 {
            for x in x0.saturating_sub(16)..(x0 + bw + 16).min(w) {
                scale_px(&mut buf, w, x as i32, y as i32, 0.3);
            }
        }
        font::draw(&mut buf, w, x0, map_h / 2 - 12, b, [238, 214, 140], 4);
    }
    Frame { w, h, buf }
}

pub fn write_png(f: &Frame, path: &str) {
    let mut raw = Vec::with_capacity(f.h * (f.w * 3 + 1));
    for y in 0..f.h {
        raw.push(0u8);
        raw.extend_from_slice(&f.buf[y * f.w * 3..(y + 1) * f.w * 3]);
    }
    let mut idat = vec![0x78u8, 0x01];
    for (i, chunk) in raw.chunks(65530).enumerate() {
        let last = (i + 1) * 65530 >= raw.len();
        idat.push(if last { 1 } else { 0 });
        idat.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        idat.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    idat.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let crc = |data: &[u8]| -> u32 {
        let mut c = 0xffff_ffffu32;
        for &byte in data {
            c ^= byte as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    };
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut push = |tag: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut full = tag.to_vec();
        full.extend_from_slice(body);
        out.extend_from_slice(&full);
        out.extend_from_slice(&crc(&full).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(f.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(f.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    push(b"IHDR", &ihdr);
    push(b"IDAT", &idat);
    push(b"IEND", &[]);
    std::fs::write(path, out).expect("write frame");
}
