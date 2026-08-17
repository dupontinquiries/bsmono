//! Headless renderer. Not the pretty path — the *provable* path: it draws
//! exactly what `rig::points_at` returns, so a preview PNG is evidence.

use crate::rig::Bot;
use anyhow::Result;
use std::path::Path;

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

/// A camera locked for a whole sequence: fixed center and distance, focal
/// chosen so every listed frame fills `fill` of the tile in its limiting
/// dimension. One `Cam` per bot means no zoom-pop between phases.
pub struct Cam { pub c: [f64; 3], pub dist: f64, pub f: f64, pub ox: f64, pub oy: f64 }

pub fn fit(bot: &Bot, frames: &[(f64, f64, f64)], w: usize, h: usize, fill: f64) -> Cam {
    fit_seq(bot, frames, w, h, fill).into_iter().next().unwrap()
}

/// One camera per frame: shared center, distance, and focal (so world scale is
/// consistent across the sequence), but each frame pans to center its own
/// content — the operator tracks the subject instead of zooming.
pub fn fit_seq(bot: &Bot, frames: &[(f64, f64, f64)], w: usize, h: usize, fill: f64) -> Vec<Cam> {
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    let posed: Vec<(f64, Vec<Vec<[f64; 3]>>)> = frames
        .iter()
        .map(|&(t, _, _)| (t, (0..bot.parts.len()).map(|i| bot.points_at(i, t)).collect()))
        .collect();
    for (_, parts) in &posed {
        for pts in parts {
            for p in pts {
                for a in 0..3 { lo[a] = lo[a].min(p[a]); hi[a] = hi[a].max(p[a]); }
            }
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let radius = (0..3).map(|a| hi[a] - lo[a]).fold(0.0f64, f64::max).max(1e-6);
    let dist = radius * 2.1;
    // per-frame slope spans against each frame's own basis
    let mut spans = Vec::with_capacity(frames.len());
    for ((_, yaw, pitch), (_, parts)) in frames.iter().zip(&posed) {
        let eye = [
            c[0] + dist * pitch.cos() * yaw.sin(),
            c[1] + dist * pitch.sin(),
            c[2] + dist * pitch.cos() * yaw.cos(),
        ];
        let fwd = norm(sub(c, eye));
        let right = norm(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        let (mut sxp, mut sxn, mut syp, mut syn) = (1e-9f64, 1e-9f64, 1e-9f64, 1e-9f64);
        for pts in parts {
            for p in pts {
                let v = sub(*p, eye);
                let z = dot(v, fwd).max(1e-6);
                let (sx, sy) = (dot(v, right) / z, dot(v, up) / z);
                sxp = sxp.max(sx); sxn = sxn.max(-sx);
                syp = syp.max(sy); syn = syn.max(-sy);
            }
        }
        spans.push((sxp, sxn, syp, syn));
    }
    // the focal that fits the widest single frame fits them all at one scale
    let f = spans.iter().fold(f64::MAX, |acc, &(sxp, sxn, syp, syn)| {
        acc.min(fill * (2.0 * (w as f64 / h as f64) / (sxp + sxn)).min(2.0 / (syp + syn)))
    });
    spans.into_iter()
        .map(|(sxp, sxn, syp, syn)| Cam {
            c, dist, f,
            ox: (sxp - sxn) * f / 2.0,
            oy: (syp - syn) * f / 2.0,
        })
        .collect()
}

pub fn render_cam(bot: &Bot, t: f64, cam: &Cam, w: usize, h: usize, yaw: f64, pitch: f64) -> Frame {
    render_inner(bot, t, w, h, yaw, pitch, Some(cam))
}

pub fn render(bot: &Bot, t: f64, w: usize, h: usize, yaw: f64, pitch: f64) -> Frame {
    render_inner(bot, t, w, h, yaw, pitch, None)
}

fn render_inner(bot: &Bot, t: f64, w: usize, h: usize, yaw: f64, pitch: f64, cam: Option<&Cam>) -> Frame {
    let mut rgb = vec![0u8; w * h * 3];
    // backdrop: vertical wash so silhouettes read
    for y in 0..h {
        let k = 14.0 + 26.0 * (y as f64 / h as f64);
        for x in 0..w {
            let o = (y * w + x) * 3;
            rgb[o] = k as u8; rgb[o + 1] = (k + 2.0) as u8; rgb[o + 2] = (k + 6.0) as u8;
        }
    }
    let mut zbuf = vec![f64::INFINITY; w * h];

    // frame the bot: a caller-locked camera, or an exact fit for this frame
    let owned;
    let cam = match cam {
        Some(c) => c,
        None => { owned = fit(bot, &[(t, yaw, pitch)], w, h, 0.92); &owned }
    };
    let c = cam.c;
    let dist = cam.dist;
    let posed: Vec<Vec<[f64; 3]>> = (0..bot.parts.len()).map(|i| bot.points_at(i, t)).collect();
    let eye = [
        c[0] + dist * pitch.cos() * yaw.sin(),
        c[1] + dist * pitch.sin(),
        c[2] + dist * pitch.cos() * yaw.cos(),
    ];
    let fwd = norm(sub(c, eye));
    let right = norm(cross(fwd, [0.0, 1.0, 0.0]));
    let up = cross(right, fwd);
    let f = cam.f;
    let (l1, l2) = (norm([0.6, 0.8, 0.4]), norm([-0.5, 0.2, -0.8]));
    let scene_scale = cam.dist * 0.55;
    let zeps = cam.dist * 0.0012;
    let zbias = |name: &str| -> f64 {
        match name {
            "guts" => 2.0 * zeps,
            "bone" | "link" | "piston" | "coil" | "cap" | "rib" | "gyro" | "cable"
            | "vertebra" | "core" | "module" | "stack" | "brace" | "keel"
            | "telescope" | "muscle" => 1.0 * zeps,
            "clad" | "greeble" => -1.0 * zeps,
            _ => 0.0,
        }
    };

    let proj = |p: [f64; 3]| -> ([f64; 2], f64) {
        let v = sub(p, eye);
        let (x, y, z) = (dot(v, right), dot(v, up), dot(v, fwd));
        ([w as f64 / 2.0 + (x / z * f - cam.ox) * h as f64 / 2.0,
          h as f64 / 2.0 - (y / z * f - cam.oy) * h as f64 / 2.0], z)
    };

    for (pi, pts) in posed.iter().enumerate() {
        let part = &bot.parts[pi];
        for tri in part.idx.chunks_exact(3) {
            let (a, b3, c3) = (pts[tri[0] as usize], pts[tri[1] as usize], pts[tri[2] as usize]);
            let n = norm(cross(sub(b3, a), sub(c3, a)));
            let view = norm(sub(eye, a));
            let lit = 0.34 + 0.52 * dot(n, l1).max(0.0) + 0.28 * dot(n, l2).max(0.0)
                + 0.20 * dot(n, view).max(0.0); // camera fill: no face goes black
            let rim = (1.0 - dot(n, view).abs()).powi(3) * 0.70 + 0.015;
            // upscale pass: metalness from tint desaturation, Blinn specular,
            // fake sky/ground reflection by the reflected ray
            let (tr, tg, tb) = (part.tint[0] as f64, part.tint[1] as f64, part.tint[2] as f64);
            let mx = tr.max(tg).max(tb);
            let mn = tr.min(tg).min(tb);
            let metal = (1.0 - (mx - mn) / mx.max(1e-6)).powi(2);
            let half = norm([view[0]+l1[0], view[1]+l1[1], view[2]+l1[2]]);
            let spec = dot(n, half).max(0.0).powi(24) * (0.25 + 0.75 * metal);
            let refl = {
                let d = [-view[0], -view[1], -view[2]];
                let k = 2.0 * dot(d, n);
                let r = [d[0]-n[0]*k, d[1]-n[1]*k, d[2]-n[2]*k];
                let sky = ((r[1] + 1.0) * 0.5).clamp(0.0, 1.0);
                (0.16 + 0.42 * sky * sky) * metal
                    * (0.35 + 0.65 * (1.0 - dot(n, view).abs()).powi(2))
            };
            let (pa, za) = proj(a); let (pb, zb) = proj(b3); let (pc, zc) = proj(c3);
            if za <= 0.05 || zb <= 0.05 || zc <= 0.05 { continue; }
            let (minx, maxx) = (pa[0].min(pb[0]).min(pc[0]).floor().max(0.0) as usize,
                                (pa[0].max(pb[0]).max(pc[0]).ceil() as usize).min(w - 1));
            let (miny, maxy) = (pa[1].min(pb[1]).min(pc[1]).floor().max(0.0) as usize,
                                (pa[1].max(pb[1]).max(pc[1]).ceil() as usize).min(h - 1));
            let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]);
            if area.abs() < 1e-9 { continue; }
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = ((pb[0] - pa[0]) * (py - pa[1]) - (pb[1] - pa[1]) * (px - pa[0])) / area;
                    let w1 = ((pc[0] - pb[0]) * (py - pb[1]) - (pc[1] - pb[1]) * (px - pb[0])) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                    let z = za * w2 + zb * w1 + zc * w0; // matches winding of edge fns above
                    let o = y * w + x;
                    if z < zbuf[o] {
                        zbuf[o] = z;
                        // procedural panel texture from world position
                        let wp = [
                            a[0] * w2 + b3[0] * w1 + c3[0] * w0,
                            a[1] * w2 + b3[1] * w1 + c3[1] * w0,
                            a[2] * w2 + b3[2] * w1 + c3[2] * w0,
                        ];
                        let cellsz = scene_scale * 0.09;
                        let hcell = {
                            let hx = ((wp[0] / cellsz).floor() * 12.9898
                                + (wp[1] / cellsz).floor() * 78.233
                                + (wp[2] / cellsz).floor() * 37.719).sin() * 43758.5453;
                            hx - hx.floor()
                        };
                        let panel = 0.95 + 0.09 * hcell;
                        let fine = {
                            let f = ((wp[0] * 61.0 + wp[1] * 47.0 + wp[2] * 53.0)
                                / scene_scale).sin() * 0.5 + 0.5;
                            0.98 + 0.04 * f
                        };
                        // facet engraving: crisp seams at triangle edges —
                        // the paneled reference-style read
                        let edge = w0.min(w1).min(w2);
                        let seam = if edge < 0.045 { 0.88 } else { 1.0 };
                        let tex = panel * fine * seam;
                        let shade = |c: f32| ((((c as f64 * tex) * lit + rim + spec + refl)
                            .max(0.22)) * 255.0).min(255.0) as u8;
                        rgb[o * 3] = shade(part.tint[0]);
                        rgb[o * 3 + 1] = shade(part.tint[1]);
                        rgb[o * 3 + 2] = shade(part.tint[2]);
                    }
                }
            }
        }
    }
    Frame { w, h, rgb }
}

pub fn save_png(f: &Frame, path: &Path) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.w as u32, f.h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&f.rgb)?;
    Ok(())
}

/// Paste `src` into `dst` at (x,y). Both RGB.
pub fn blit(dst: &mut Frame, src: &Frame, x: usize, y: usize) {
    for r in 0..src.h.min(dst.h.saturating_sub(y)) {
        let (d0, s0) = (((y + r) * dst.w + x) * 3, r * src.w * 3);
        let n = src.w.min(dst.w.saturating_sub(x)) * 3;
        dst.rgb[d0..d0 + n].copy_from_slice(&src.rgb[s0..s0 + n]);
    }
}

/// Box-filter downsample by an integer factor (supersampling support).
pub fn downsample(f: &Frame, ss: usize) -> Frame {
    if ss <= 1 { return Frame { w: f.w, h: f.h, rgb: f.rgb.clone() }; }
    let (w, h) = (f.w / ss, f.h / ss);
    let mut out = Frame { w, h, rgb: vec![0; w * h * 3] };
    for y in 0..h { for x in 0..w {
        let mut acc = [0u32; 3];
        for dy in 0..ss { for dx in 0..ss {
            let o = ((y * ss + dy) * f.w + x * ss + dx) * 3;
            for c in 0..3 { acc[c] += f.rgb[o + c] as u32; }
        }}
        let o2 = (y * w + x) * 3;
        for c in 0..3 { out.rgb[o2 + c] = (acc[c] / (ss * ss) as u32) as u8; }
    }}
    out
}
