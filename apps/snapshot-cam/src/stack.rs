//! stack — merging several frames of one scene into one photo.
//!
//! Three merges, one machinery:
//!
//! * **exposure fusion** (HDR) — frames at different shutter times, blended
//!   per pixel by how well exposed, saturated and detailed each is there
//!   (Mertens, Kautz & Van Reeth 2007). It needs no camera response curve
//!   and no RAW, which is why it suits 8-bit frames from a phone's ISP: the
//!   result is a well-exposed 8-bit picture, not a radiance map to tone-map.
//! * **focus stacking** — frames at different focus distances, blended by
//!   local sharpness: the macro shot whose whole subject is in focus.
//! * **burst average** — frames at the same settings, averaged in linear
//!   light: noise falls by √N, which is the handheld night mode.
//!
//! Blending is done on Laplacian pyramids: weights are smoothed at every
//! scale, so the seams a per-pixel pick would leave (a hard edge where one
//! frame hands over to the next) are not there.
//!
//! Frames are **aligned** first — a phone moves between frames. Alignment
//! is global translation, coarse to fine over a luma pyramid. Honest limit:
//! rotation, and anything moving in the scene, is not handled; a person
//! walking through an HDR bracket ghosts.

use crate::develop::{to_linear, to_srgb};

/// A float RGB image, values 0..1 (sRGB-encoded, as Mertens works).
#[derive(Clone, Debug)]
struct Img { w: usize, h: usize, px: Vec<[f32; 3]> }

impl Img {
    fn from_rgba(rgba: &[u8], w: usize, h: usize, shift: (i32, i32)) -> Img {
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let sy = (y as i32 + shift.1).clamp(0, h as i32 - 1) as usize;
            for x in 0..w {
                let sx = (x as i32 + shift.0).clamp(0, w as i32 - 1) as usize;
                let p = &rgba[(sy * w + sx) * 4..];
                px.push([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]);
            }
        }
        Img { w, h, px }
    }
    fn to_rgba(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| [q(p[0]), q(p[1]), q(p[2]), 255]).collect()
    }
}

fn q(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }

/// One channel.
#[derive(Clone, Debug)]
struct Plane { v: Vec<f32> }

// ── pyramids ─────────────────────────────────────────────────────────────

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

fn blur_down<T: Copy + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>>(
    src: &[T], w: usize, h: usize) -> (Vec<T>, usize, usize) {
    // Separable 5-tap, then take every other sample.
    let (nw, nh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut tmp = vec![T::default(); nw * h];
    for y in 0..h {
        for nx in 0..nw {
            let x = nx * 2;
            let mut acc = T::default();
            for (i, k) in K.iter().enumerate() {
                let sx = (x as isize + i as isize - 2).clamp(0, w as isize - 1) as usize;
                acc = acc + src[y * w + sx] * *k;
            }
            tmp[y * nw + nx] = acc;
        }
    }
    let mut out = vec![T::default(); nw * nh];
    for ny in 0..nh {
        let y = ny * 2;
        for x in 0..nw {
            let mut acc = T::default();
            for (i, k) in K.iter().enumerate() {
                let sy = (y as isize + i as isize - 2).clamp(0, h as isize - 1) as usize;
                acc = acc + tmp[sy * nw + x] * *k;
            }
            out[ny * nw + x] = acc;
        }
    }
    (out, nw, nh)
}

fn up<T: Copy + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>>(
    src: &[T], sw: usize, sh: usize, w: usize, h: usize) -> Vec<T> {
    // Bilinear back up to (w, h) — smooth enough for pyramid collapse.
    let mut out = vec![T::default(); w * h];
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let a = src[y0 * sw + x0] * (1.0 - tx) + src[y0 * sw + x1] * tx;
            let b = src[y1 * sw + x0] * (1.0 - tx) + src[y1 * sw + x1] * tx;
            out[y * w + x] = a * (1.0 - ty) + b * ty;
        }
    }
    out
}

/// A small wrapper so `[f32; 3]` can go through the generic filters.
#[derive(Clone, Copy, Default, Debug)]
struct C3([f32; 3]);
impl std::ops::Add for C3 { type Output = C3; fn add(self, o: C3) -> C3 { C3([self.0[0] + o.0[0], self.0[1] + o.0[1], self.0[2] + o.0[2]]) } }
impl std::ops::Mul<f32> for C3 { type Output = C3; fn mul(self, k: f32) -> C3 { C3([self.0[0] * k, self.0[1] * k, self.0[2] * k]) } }
impl std::ops::Sub for C3 { type Output = C3; fn sub(self, o: C3) -> C3 { C3([self.0[0] - o.0[0], self.0[1] - o.0[1], self.0[2] - o.0[2]]) } }

fn levels_for(w: usize, h: usize) -> usize {
    let m = w.min(h) as f32;
    ((m / 8.0).log2().floor() as usize).clamp(1, 8)
}

/// Blend images by per-pixel weights on Laplacian pyramids. `weights[i]`
/// must already sum to 1 over `i` at every pixel.
fn pyramid_blend(imgs: &[Img], weights: &[Plane]) -> Img {
    let (w, h) = (imgs[0].w, imgs[0].h);
    let levels = levels_for(w, h);
    let mut acc: Vec<(Vec<C3>, usize, usize)> = Vec::new();
    for (img, wt) in imgs.iter().zip(weights) {
        // Gaussian of the image and of the weight, level by level.
        let mut g: Vec<(Vec<C3>, usize, usize)> = vec![(img.px.iter().map(|p| C3(*p)).collect(), w, h)];
        let mut gw: Vec<Vec<f32>> = vec![wt.v.clone()];
        for _ in 1..levels {
            let (prev, pw, ph) = g.last().unwrap();
            let (d, nw, nh) = blur_down(prev, *pw, *ph);
            let (dw, _, _) = blur_down(gw.last().unwrap(), *pw, *ph);
            g.push((d, nw, nh));
            gw.push(dw);
        }
        for l in 0..levels {
            let (gl, lw, lh) = &g[l];
            let lap: Vec<C3> = if l + 1 < levels {
                let (gn, nw, nh) = &g[l + 1];
                let u = up(gn, *nw, *nh, *lw, *lh);
                gl.iter().zip(&u).map(|(a, b)| *a - *b).collect()
            } else { gl.clone() };
            let contrib: Vec<C3> = lap.iter().zip(&gw[l]).map(|(v, k)| *v * *k).collect();
            if acc.len() <= l { acc.push((contrib, *lw, *lh)); }
            else { for (a, c) in acc[l].0.iter_mut().zip(contrib) { *a = *a + c; } }
        }
    }
    // Collapse.
    let mut cur = acc.pop().unwrap();
    while let Some((lap, lw, lh)) = acc.pop() {
        let u = up(&cur.0, cur.1, cur.2, lw, lh);
        cur = (lap.iter().zip(&u).map(|(a, b)| *a + *b).collect(), lw, lh);
    }
    Img { w, h, px: cur.0.into_iter().map(|c| c.0).collect() }
}

fn grey(img: &Img) -> Vec<f32> { img.px.iter().map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).collect() }

fn laplacian_abs(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let at = |dx: isize, dy: isize| g[((y as isize + dy).clamp(0, h as isize - 1) as usize) * w + (x as isize + dx).clamp(0, w as isize - 1) as usize];
            out[y * w + x] = (at(-1, 0) + at(1, 0) + at(0, -1) + at(0, 1) - 4.0 * at(0, 0)).abs();
        }
    }
    out
}

fn normalise_n(ws: &mut [Plane]) {
    let n = ws[0].v.len();
    let k = ws.len() as f32;
    for i in 0..n {
        let s: f32 = ws.iter().map(|p| p.v[i]).sum();
        for p in ws.iter_mut() { p.v[i] = if s <= 1e-12 { 1.0 / k } else { p.v[i] / s }; }
    }
}

// ── alignment ────────────────────────────────────────────────────────────

/// A luma image at reduced size, for alignment.
fn luma_small(rgba: &[u8], w: usize, h: usize, max_w: usize) -> (Vec<f32>, usize, usize, usize) {
    let step = w.div_ceil(max_w).max(1);
    let (sw, sh) = (w / step, h / step);
    let mut v = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let p = &rgba[((y * step) * w + x * step) * 4..];
            v.push(0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32);
        }
    }
    (v, sw, sh, step)
}

/// Mean absolute difference over the overlap of `a` and `b` shifted by (dx, dy).
fn sad(a: &[f32], b: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> f32 {
    let (m, mut s, mut n) = ((w.min(h) / 8) as i32, 0.0f32, 0u32);
    for y in m..h as i32 - m {
        let by = y + dy;
        if by < 0 || by >= h as i32 { continue; }
        for x in m..w as i32 - m {
            let bx = x + dx;
            if bx < 0 || bx >= w as i32 { continue; }
            s += (a[y as usize * w + x as usize] - b[by as usize * w + bx as usize]).abs();
            n += 1;
        }
    }
    if n == 0 { f32::MAX } else { s / n as f32 }
}

/// Where `b` has to be sampled from to line up with `a`: add the result to
/// a pixel's coordinates in `a` to find it in `b`. Pixels, full size.
///
/// Brightness-normalised first so an exposure bracket (the same scene,
/// darker and brighter) still aligns.
pub fn align(a: &[u8], b: &[u8], w: u32, h: u32) -> (i32, i32) {
    let (w, h) = (w as usize, h as usize);
    let (mut la, sw, sh, step) = luma_small(a, w, h, 320);
    let (mut lb, _, _, _) = luma_small(b, w, h, 320);
    for v in [&mut la, &mut lb] {
        let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
        let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len().max(1) as f32).sqrt().max(1e-3);
        for x in v.iter_mut() { *x = (*x - mean) / sd; }
    }
    // Pyramid of both.
    let mut pa = vec![(la, sw, sh)];
    let mut pb = vec![(lb, sw, sh)];
    while pa.last().unwrap().1 > 40 {
        let (v, w0, h0) = pa.last().unwrap();
        let d = blur_down(v, *w0, *h0);
        pa.push(d);
        let (v, w0, h0) = pb.last().unwrap();
        let d = blur_down(v, *w0, *h0);
        pb.push(d);
    }
    let (mut dx, mut dy) = (0i32, 0i32);
    for (lvl, ((va, lw, lh), (vb, _, _))) in pa.iter().zip(&pb).enumerate().rev() {
        let r = if lvl == pa.len() - 1 { 6 } else { 2 };
        let (cx, cy) = (dx, dy);
        let mut best = (f32::MAX, cx, cy);
        for ty in cy - r..=cy + r {
            for tx in cx - r..=cx + r {
                let s = sad(va, vb, *lw, *lh, tx, ty);
                if s < best.0 { best = (s, tx, ty); }
            }
        }
        (dx, dy) = (best.1, best.2);
        if lvl > 0 { dx *= 2; dy *= 2; }
    }
    (dx * step as i32, dy * step as i32)
}

// ── the merges ───────────────────────────────────────────────────────────

/// Frames to merge: RGBA, all `w × h`.
pub struct Frames<'a> { pub w: u32, pub h: u32, pub rgba: Vec<&'a [u8]> }

fn aligned(f: &Frames) -> Vec<Img> {
    let (w, h) = (f.w as usize, f.h as usize);
    let base = f.rgba[0];
    f.rgba.iter().map(|b| {
        let s = if std::ptr::eq(*b, base) { (0, 0) } else { align(base, b, f.w, f.h) };
        Img::from_rgba(b, w, h, s)
    }).collect()
}

/// Exposure fusion. Frames in any order.
pub fn fuse_exposures(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut ws: Vec<Plane> = imgs.iter().map(|img| {
        let c = laplacian_abs(&grey(img), w, h);
        let v = img.px.iter().zip(&c).map(|(p, c)| {
            let m = (p[0] + p[1] + p[2]) / 3.0;
            let sat = (((p[0] - m).powi(2) + (p[1] - m).powi(2) + (p[2] - m).powi(2)) / 3.0).sqrt();
            let e: f32 = p.iter().map(|v| (-(v - 0.5).powi(2) / (2.0 * 0.2 * 0.2)).exp()).product();
            (c + 0.02) * (sat + 0.05) * e + 1e-12
        }).collect();
        Plane { v }
    }).collect();
    normalise_n(&mut ws);
    pyramid_blend(&imgs, &ws).to_rgba()
}

/// Focus stacking: each pixel from the frame that is sharpest around it.
pub fn stack_focus(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut ws: Vec<Plane> = imgs.iter().map(|img| {
        // Local energy of the Laplacian, blurred so a textureless patch
        // takes its neighbours' decision, then sharpened to a near-pick.
        let lap = laplacian_abs(&grey(img), w, h);
        let (b1, w1, h1) = blur_down(&lap, w, h);
        let (b2, w2, h2) = blur_down(&b1, w1, h1);
        let e = up(&up(&b2, w2, h2, w1, h1), w1, h1, w, h);
        Plane { v: e.into_iter().map(|x| (x + 1e-4).powi(4)).collect() }
    }).collect();
    normalise_n(&mut ws);
    pyramid_blend(&imgs, &ws).to_rgba()
}

/// Burst average in linear light.
pub fn average(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let n = imgs.len() as f32;
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut out = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        for c in 0..3 {
            let s: f32 = imgs.iter().map(|m| to_linear(q(m.px[i][c]))).sum();
            out.push(to_srgb(s / n));
        }
        out.push(255);
    }
    out
}

/// Sharpness of a frame: variance of the Laplacian of its middle, small.
/// What the primed shutter uses to pick the steadiest frame.
pub fn sharpness(rgba: &[u8], w: u32, h: u32) -> f32 {
    let (l, sw, sh, _) = luma_small(rgba, w as usize, h as usize, 240);
    if sw < 8 || sh < 8 { return 0.0; }
    // The SIGNED Laplacian's variance (the usual focus measure): a fine,
    // regular texture has a constant |Laplacian| but a large signed spread.
    let (x0, x1, y0, y1) = (sw / 4, sw * 3 / 4, sh / 4, sh * 3 / 4);
    let at = |x: usize, y: usize| l[y * sw + x];
    let vals: Vec<f32> = (y0.max(1)..y1.min(sh - 1)).flat_map(|y| (x0.max(1)..x1.min(sw - 1)).map(move |x| (x, y)))
        .map(|(x, y)| at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y)).collect();
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene with detail everywhere and a large dynamic range: a dim room
    /// (left) and a bright window (right), in LINEAR radiance.
    fn radiance(w: u32, h: u32) -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w {
            let tex = 0.5 + 0.5 * (((x / 3 + y / 3) % 2) as f32);
            let base = if x < w / 2 { 0.02 } else { 0.9 };
            v.push([base * tex, base * tex * 0.9, base * tex * 0.8]);
        } }
        v
    }

    /// The scene through a shutter of `ev` stops, clipped to 8 bits.
    fn expose(r: &[[f32; 3]], ev: f32) -> Vec<u8> {
        let k = 2f32.powf(ev);
        r.iter().flat_map(|p| [to_srgb(p[0] * k), to_srgb(p[1] * k), to_srgb(p[2] * k), 255]).collect()
    }

    /// Detail visible in a half: the spread of values there.
    fn detail(img: &[u8], w: u32, h: u32, right: bool) -> f32 {
        let xs = if right { w / 2 + 2..w - 2 } else { 2..w / 2 - 2 };
        let vals: Vec<f32> = (2..h - 2).flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| img[((y * w + x) * 4) as usize] as f32).collect();
        let m = vals.iter().sum::<f32>() / vals.len() as f32;
        (vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32).sqrt()
    }

    #[test]
    fn exposure_fusion_keeps_detail_in_the_shadows_and_the_highlights() {
        let (w, h) = (96u32, 64u32);
        let r = radiance(w, h);
        let (dark, mid, bright) = (expose(&r, -1.0), expose(&r, 1.5), expose(&r, 4.0));
        let fused = fuse_exposures(&Frames { w, h, rgba: vec![&dark, &mid, &bright] });
        // Each single exposure loses one side: the bright one clips the
        // window, the dark one buries the room.
        assert!(detail(&bright, w, h, true) < 5.0, "bright frame should clip the window");
        assert!(detail(&dark, w, h, false) < 8.0, "dark frame should bury the room");
        let (l, rr) = (detail(&fused, w, h, false), detail(&fused, w, h, true));
        assert!(l > 12.0 && rr > 12.0, "fused keeps both: room {l}, window {rr}");
    }

    #[test]
    fn a_shifted_frame_is_found_and_the_merge_does_not_double_the_edges() {
        let (w, h) = (160u32, 120u32);
        let mut a = Vec::new();
        for y in 0..h { for x in 0..w {
            let v = if ((x as i32 - 70).pow(2) + (y as i32 - 60).pow(2)) < 900 { 230 } else { ((x * 7 + y * 13) % 40 + 30) as u8 };
            a.extend_from_slice(&[v, v, v, 255]);
        } }
        // b is a moved 5 right, 3 down: pixel (x, y) of a is at (x+5, y+3) in b.
        let mut b = vec![0u8; a.len()];
        for y in 0..h as i32 { for x in 0..w as i32 {
            let (sx, sy) = ((x - 5).clamp(0, w as i32 - 1), (y - 3).clamp(0, h as i32 - 1));
            let (i, j) = (((y as u32 * w + x as u32) * 4) as usize, ((sy as u32 * w + sx as u32) * 4) as usize);
            b[i..i + 4].copy_from_slice(&a[j..j + 4]);
        } }
        assert_eq!(align(&a, &b, w, h), (5, 3));
        // Brightness does not fool it.
        let darker: Vec<u8> = b.iter().enumerate().map(|(i, v)| if i % 4 == 3 { 255 } else { v / 2 }).collect();
        assert_eq!(align(&a, &darker, w, h), (5, 3));
        let avg = average(&Frames { w, h, rgba: vec![&a, &b] });
        // The disc edge at x=100 (right of centre 70, r=30) is still one edge.
        let row = 60u32;
        let inside = avg[((row * w + 95) * 4) as usize];
        let outside = avg[((row * w + 105) * 4) as usize];
        assert!(inside > 200 && outside < 100, "ghosted: {inside} / {outside}");
    }

    #[test]
    fn focus_stacking_takes_each_half_from_the_frame_where_it_is_sharp() {
        let (w, h) = (96u32, 64u32);
        let sharp: Vec<u8> = (0..w * h).flat_map(|i| { let (x, y) = (i % w, i / w); let v = if (x / 2 + y / 2) % 2 == 0 { 200 } else { 50 }; [v, v, v, 255] }).collect();
        let blurred: Vec<u8> = vec![125u8; (w * h * 4) as usize].chunks(4).flat_map(|_| [125u8, 125, 125, 255]).collect();
        // Near-focus frame: left sharp, right blurred; far-focus: the opposite.
        let half = |left_sharp: bool| -> Vec<u8> {
            (0..(w * h) as usize).flat_map(|i| {
                let x = i as u32 % w;
                let src = if (x < w / 2) == left_sharp { &sharp } else { &blurred };
                [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], 255]
            }).collect()
        };
        let (near, far) = (half(true), half(false));
        let out = stack_focus(&Frames { w, h, rgba: vec![&near, &far] });
        let l = detail(&out, w, h, false);
        let r = detail(&out, w, h, true);
        assert!(l > 50.0 && r > 50.0, "both halves sharp: {l} {r}");
    }

    #[test]
    fn averaging_a_noisy_burst_reduces_the_noise() {
        let (w, h) = (64u32, 48u32);
        let mut frames = Vec::new();
        let mut seed = 12345u32;
        for _ in 0..6 {
            frames.push((0..w * h).flat_map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let n = ((seed >> 16) % 41) as i32 - 20;
                let v = (100 + n) as u8;
                [v, v, v, 255]
            }).collect::<Vec<u8>>());
        }
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        let out = average(&Frames { w, h, rgba: refs });
        let sd = |img: &[u8]| { let v: Vec<f32> = img.chunks(4).map(|p| p[0] as f32).collect(); let m = v.iter().sum::<f32>() / v.len() as f32; (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt() };
        assert!(sd(&out) < sd(&frames[0]) * 0.6, "{} vs {}", sd(&out), sd(&frames[0]));
    }

    #[test]
    fn a_sharp_frame_scores_above_a_blurred_one() {
        let (w, h) = (120u32, 80u32);
        let sharp: Vec<u8> = (0..w * h).flat_map(|i| { let v = if (i % w / 3 + i / w / 3) % 2 == 0 { 220 } else { 30 }; [v, v, v, 255] }).collect();
        let soft = crate::develop::render(&sharp, w, h, &crate::develop::Params::identity(w / 4, h / 4));
        let soft = crate::develop::render(&soft, w / 4, h / 4, &crate::develop::Params::identity(w, h));
        assert!(sharpness(&sharp, w, h) > sharpness(&soft, w, h) * 2.0);
    }
}

#[cfg(test)]
mod timing {
    /// `cargo test --release --no-default-features --lib timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn merge_cost_at_1280x720() {
        let (w, h) = (1280u32, 720u32);
        let frames: Vec<Vec<u8>> = (0..3).map(|k| (0..w * h).flat_map(|i| { let v = ((i % w + i / w * 3 + k * 40) % 256) as u8; [v, v / 2, 255 - v, 255] }).collect()).collect();
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        let f = super::Frames { w, h, rgba: refs };
        for (name, run) in [("fuse 3", super::fuse_exposures as fn(&super::Frames) -> Vec<u8>), ("focus 3", super::stack_focus), ("average 3", super::average)] {
            let t = std::time::Instant::now();
            let _ = run(&f);
            eprintln!("{name}: {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
        }
    }
}
//! stack — merging several frames of one scene into one photo.
//!
//! Three merges, one machinery:
//!
//! * **exposure fusion** (HDR) — frames at different shutter times, blended
//!   per pixel by how well exposed, saturated and detailed each is there
//!   (Mertens, Kautz & Van Reeth 2007). It needs no camera response curve
//!   and no RAW, which is why it suits 8-bit frames from a phone's ISP: the
//!   result is a well-exposed 8-bit picture, not a radiance map to tone-map.
//! * **focus stacking** — frames at different focus distances, blended by
//!   local sharpness: the macro shot whose whole subject is in focus.
//! * **burst average** — frames at the same settings, averaged in linear
//!   light: noise falls by √N, which is the handheld night mode.
//!
//! Blending is done on Laplacian pyramids: weights are smoothed at every
//! scale, so the seams a per-pixel pick would leave (a hard edge where one
//! frame hands over to the next) are not there.
//!
//! Frames are **aligned** first — a phone moves between frames. Alignment
//! is global translation, coarse to fine over a luma pyramid. Honest limit:
//! rotation, and anything moving in the scene, is not handled; a person
//! walking through an HDR bracket ghosts.

use crate::develop::{to_linear, to_srgb};

/// A float RGB image, values 0..1 (sRGB-encoded, as Mertens works).
#[derive(Clone, Debug)]
struct Img { w: usize, h: usize, px: Vec<[f32; 3]> }

impl Img {
    fn from_rgba(rgba: &[u8], w: usize, h: usize, shift: (i32, i32)) -> Img {
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let sy = (y as i32 + shift.1).clamp(0, h as i32 - 1) as usize;
            for x in 0..w {
                let sx = (x as i32 + shift.0).clamp(0, w as i32 - 1) as usize;
                let p = &rgba[(sy * w + sx) * 4..];
                px.push([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]);
            }
        }
        Img { w, h, px }
    }
    fn to_rgba(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| [q(p[0]), q(p[1]), q(p[2]), 255]).collect()
    }
}

fn q(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }

/// One channel.
#[derive(Clone, Debug)]
struct Plane { v: Vec<f32> }

// ── pyramids ─────────────────────────────────────────────────────────────

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

fn blur_down<T: Copy + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>>(
    src: &[T], w: usize, h: usize) -> (Vec<T>, usize, usize) {
    // Separable 5-tap, then take every other sample.
    let (nw, nh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut tmp = vec![T::default(); nw * h];
    for y in 0..h {
        for nx in 0..nw {
            let x = nx * 2;
            let mut acc = T::default();
            for (i, k) in K.iter().enumerate() {
                let sx = (x as isize + i as isize - 2).clamp(0, w as isize - 1) as usize;
                acc = acc + src[y * w + sx] * *k;
            }
            tmp[y * nw + nx] = acc;
        }
    }
    let mut out = vec![T::default(); nw * nh];
    for ny in 0..nh {
        let y = ny * 2;
        for x in 0..nw {
            let mut acc = T::default();
            for (i, k) in K.iter().enumerate() {
                let sy = (y as isize + i as isize - 2).clamp(0, h as isize - 1) as usize;
                acc = acc + tmp[sy * nw + x] * *k;
            }
            out[ny * nw + x] = acc;
        }
    }
    (out, nw, nh)
}

fn up<T: Copy + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>>(
    src: &[T], sw: usize, sh: usize, w: usize, h: usize) -> Vec<T> {
    // Bilinear back up to (w, h) — smooth enough for pyramid collapse.
    let mut out = vec![T::default(); w * h];
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let a = src[y0 * sw + x0] * (1.0 - tx) + src[y0 * sw + x1] * tx;
            let b = src[y1 * sw + x0] * (1.0 - tx) + src[y1 * sw + x1] * tx;
            out[y * w + x] = a * (1.0 - ty) + b * ty;
        }
    }
    out
}

/// A small wrapper so `[f32; 3]` can go through the generic filters.
#[derive(Clone, Copy, Default, Debug)]
struct C3([f32; 3]);
impl std::ops::Add for C3 { type Output = C3; fn add(self, o: C3) -> C3 { C3([self.0[0] + o.0[0], self.0[1] + o.0[1], self.0[2] + o.0[2]]) } }
impl std::ops::Mul<f32> for C3 { type Output = C3; fn mul(self, k: f32) -> C3 { C3([self.0[0] * k, self.0[1] * k, self.0[2] * k]) } }
impl std::ops::Sub for C3 { type Output = C3; fn sub(self, o: C3) -> C3 { C3([self.0[0] - o.0[0], self.0[1] - o.0[1], self.0[2] - o.0[2]]) } }

fn levels_for(w: usize, h: usize) -> usize {
    let m = w.min(h) as f32;
    ((m / 8.0).log2().floor() as usize).clamp(1, 8)
}

/// Blend images by per-pixel weights on Laplacian pyramids. `weights[i]`
/// must already sum to 1 over `i` at every pixel.
fn pyramid_blend(imgs: &[Img], weights: &[Plane]) -> Img {
    let (w, h) = (imgs[0].w, imgs[0].h);
    let levels = levels_for(w, h);
    let mut acc: Vec<(Vec<C3>, usize, usize)> = Vec::new();
    for (img, wt) in imgs.iter().zip(weights) {
        // Gaussian of the image and of the weight, level by level.
        let mut g: Vec<(Vec<C3>, usize, usize)> = vec![(img.px.iter().map(|p| C3(*p)).collect(), w, h)];
        let mut gw: Vec<Vec<f32>> = vec![wt.v.clone()];
        for _ in 1..levels {
            let (prev, pw, ph) = g.last().unwrap();
            let (d, nw, nh) = blur_down(prev, *pw, *ph);
            let (dw, _, _) = blur_down(gw.last().unwrap(), *pw, *ph);
            g.push((d, nw, nh));
            gw.push(dw);
        }
        for l in 0..levels {
            let (gl, lw, lh) = &g[l];
            let lap: Vec<C3> = if l + 1 < levels {
                let (gn, nw, nh) = &g[l + 1];
                let u = up(gn, *nw, *nh, *lw, *lh);
                gl.iter().zip(&u).map(|(a, b)| *a - *b).collect()
            } else { gl.clone() };
            let contrib: Vec<C3> = lap.iter().zip(&gw[l]).map(|(v, k)| *v * *k).collect();
            if acc.len() <= l { acc.push((contrib, *lw, *lh)); }
            else { for (a, c) in acc[l].0.iter_mut().zip(contrib) { *a = *a + c; } }
        }
    }
    // Collapse.
    let mut cur = acc.pop().unwrap();
    while let Some((lap, lw, lh)) = acc.pop() {
        let u = up(&cur.0, cur.1, cur.2, lw, lh);
        cur = (lap.iter().zip(&u).map(|(a, b)| *a + *b).collect(), lw, lh);
    }
    Img { w, h, px: cur.0.into_iter().map(|c| c.0).collect() }
}

fn grey(img: &Img) -> Vec<f32> { img.px.iter().map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).collect() }

fn laplacian_abs(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let at = |dx: isize, dy: isize| g[((y as isize + dy).clamp(0, h as isize - 1) as usize) * w + (x as isize + dx).clamp(0, w as isize - 1) as usize];
            out[y * w + x] = (at(-1, 0) + at(1, 0) + at(0, -1) + at(0, 1) - 4.0 * at(0, 0)).abs();
        }
    }
    out
}

fn normalise_n(ws: &mut [Plane]) {
    let n = ws[0].v.len();
    let k = ws.len() as f32;
    for i in 0..n {
        let s: f32 = ws.iter().map(|p| p.v[i]).sum();
        for p in ws.iter_mut() { p.v[i] = if s <= 1e-12 { 1.0 / k } else { p.v[i] / s }; }
    }
}

// ── alignment ────────────────────────────────────────────────────────────

/// A luma image at reduced size, for alignment.
fn luma_small(rgba: &[u8], w: usize, h: usize, max_w: usize) -> (Vec<f32>, usize, usize, usize) {
    let step = w.div_ceil(max_w).max(1);
    let (sw, sh) = (w / step, h / step);
    let mut v = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let p = &rgba[((y * step) * w + x * step) * 4..];
            v.push(0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32);
        }
    }
    (v, sw, sh, step)
}

/// Mean absolute difference over the overlap of `a` and `b` shifted by (dx, dy).
fn sad(a: &[f32], b: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> f32 {
    let (m, mut s, mut n) = ((w.min(h) / 8) as i32, 0.0f32, 0u32);
    for y in m..h as i32 - m {
        let by = y + dy;
        if by < 0 || by >= h as i32 { continue; }
        for x in m..w as i32 - m {
            let bx = x + dx;
            if bx < 0 || bx >= w as i32 { continue; }
            s += (a[y as usize * w + x as usize] - b[by as usize * w + bx as usize]).abs();
            n += 1;
        }
    }
    if n == 0 { f32::MAX } else { s / n as f32 }
}

/// Where `b` has to be sampled from to line up with `a`: add the result to
/// a pixel's coordinates in `a` to find it in `b`. Pixels, full size.
///
/// Brightness-normalised first so an exposure bracket (the same scene,
/// darker and brighter) still aligns.
pub fn align(a: &[u8], b: &[u8], w: u32, h: u32) -> (i32, i32) {
    let (w, h) = (w as usize, h as usize);
    let (mut la, sw, sh, step) = luma_small(a, w, h, 320);
    let (mut lb, _, _, _) = luma_small(b, w, h, 320);
    for v in [&mut la, &mut lb] {
        let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
        let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len().max(1) as f32).sqrt().max(1e-3);
        for x in v.iter_mut() { *x = (*x - mean) / sd; }
    }
    // Pyramid of both.
    let mut pa = vec![(la, sw, sh)];
    let mut pb = vec![(lb, sw, sh)];
    while pa.last().unwrap().1 > 40 {
        let (v, w0, h0) = pa.last().unwrap();
        let d = blur_down(v, *w0, *h0);
        pa.push(d);
        let (v, w0, h0) = pb.last().unwrap();
        let d = blur_down(v, *w0, *h0);
        pb.push(d);
    }
    let (mut dx, mut dy) = (0i32, 0i32);
    for (lvl, ((va, lw, lh), (vb, _, _))) in pa.iter().zip(&pb).enumerate().rev() {
        let r = if lvl == pa.len() - 1 { 6 } else { 2 };
        let (cx, cy) = (dx, dy);
        let mut best = (f32::MAX, cx, cy);
        for ty in cy - r..=cy + r {
            for tx in cx - r..=cx + r {
                let s = sad(va, vb, *lw, *lh, tx, ty);
                if s < best.0 { best = (s, tx, ty); }
            }
        }
        (dx, dy) = (best.1, best.2);
        if lvl > 0 { dx *= 2; dy *= 2; }
    }
    (dx * step as i32, dy * step as i32)
}

// ── the merges ───────────────────────────────────────────────────────────

/// Frames to merge: RGBA, all `w × h`.
pub struct Frames<'a> { pub w: u32, pub h: u32, pub rgba: Vec<&'a [u8]> }

fn aligned(f: &Frames) -> Vec<Img> {
    let (w, h) = (f.w as usize, f.h as usize);
    let base = f.rgba[0];
    f.rgba.iter().map(|b| {
        let s = if std::ptr::eq(*b, base) { (0, 0) } else { align(base, b, f.w, f.h) };
        Img::from_rgba(b, w, h, s)
    }).collect()
}

/// Exposure fusion. Frames in any order.
pub fn fuse_exposures(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut ws: Vec<Plane> = imgs.iter().map(|img| {
        let c = laplacian_abs(&grey(img), w, h);
        let v = img.px.iter().zip(&c).map(|(p, c)| {
            let m = (p[0] + p[1] + p[2]) / 3.0;
            let sat = (((p[0] - m).powi(2) + (p[1] - m).powi(2) + (p[2] - m).powi(2)) / 3.0).sqrt();
            let e: f32 = p.iter().map(|v| (-(v - 0.5).powi(2) / (2.0 * 0.2 * 0.2)).exp()).product();
            (c + 0.02) * (sat + 0.05) * e + 1e-12
        }).collect();
        Plane { v }
    }).collect();
    normalise_n(&mut ws);
    pyramid_blend(&imgs, &ws).to_rgba()
}

/// Focus stacking: each pixel from the frame that is sharpest around it.
pub fn stack_focus(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut ws: Vec<Plane> = imgs.iter().map(|img| {
        // Local energy of the Laplacian, blurred so a textureless patch
        // takes its neighbours' decision, then sharpened to a near-pick.
        let lap = laplacian_abs(&grey(img), w, h);
        let (b1, w1, h1) = blur_down(&lap, w, h);
        let (b2, w2, h2) = blur_down(&b1, w1, h1);
        let e = up(&up(&b2, w2, h2, w1, h1), w1, h1, w, h);
        Plane { v: e.into_iter().map(|x| (x + 1e-4).powi(4)).collect() }
    }).collect();
    normalise_n(&mut ws);
    pyramid_blend(&imgs, &ws).to_rgba()
}

/// Burst average in linear light.
pub fn average(f: &Frames) -> Vec<u8> {
    let imgs = aligned(f);
    let n = imgs.len() as f32;
    let (w, h) = (imgs[0].w, imgs[0].h);
    let mut out = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        for c in 0..3 {
            let s: f32 = imgs.iter().map(|m| to_linear(q(m.px[i][c]))).sum();
            out.push(to_srgb(s / n));
        }
        out.push(255);
    }
    out
}

/// Sharpness of a frame: variance of the Laplacian of its middle, small.
/// What the primed shutter uses to pick the steadiest frame.
pub fn sharpness(rgba: &[u8], w: u32, h: u32) -> f32 {
    let (l, sw, sh, _) = luma_small(rgba, w as usize, h as usize, 240);
    if sw < 8 || sh < 8 { return 0.0; }
    // The SIGNED Laplacian's variance (the usual focus measure): a fine,
    // regular texture has a constant |Laplacian| but a large signed spread.
    let (x0, x1, y0, y1) = (sw / 4, sw * 3 / 4, sh / 4, sh * 3 / 4);
    let at = |x: usize, y: usize| l[y * sw + x];
    let vals: Vec<f32> = (y0.max(1)..y1.min(sh - 1)).flat_map(|y| (x0.max(1)..x1.min(sw - 1)).map(move |x| (x, y)))
        .map(|(x, y)| at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y)).collect();
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene with detail everywhere and a large dynamic range: a dim room
    /// (left) and a bright window (right), in LINEAR radiance.
    fn radiance(w: u32, h: u32) -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w {
            let tex = 0.5 + 0.5 * (((x / 3 + y / 3) % 2) as f32);
            let base = if x < w / 2 { 0.02 } else { 0.9 };
            v.push([base * tex, base * tex * 0.9, base * tex * 0.8]);
        } }
        v
    }

    /// The scene through a shutter of `ev` stops, clipped to 8 bits.
    fn expose(r: &[[f32; 3]], ev: f32) -> Vec<u8> {
        let k = 2f32.powf(ev);
        r.iter().flat_map(|p| [to_srgb(p[0] * k), to_srgb(p[1] * k), to_srgb(p[2] * k), 255]).collect()
    }

    /// Detail visible in a half: the spread of values there.
    fn detail(img: &[u8], w: u32, h: u32, right: bool) -> f32 {
        let xs = if right { w / 2 + 2..w - 2 } else { 2..w / 2 - 2 };
        let vals: Vec<f32> = (2..h - 2).flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| img[((y * w + x) * 4) as usize] as f32).collect();
        let m = vals.iter().sum::<f32>() / vals.len() as f32;
        (vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32).sqrt()
    }

    #[test]
    fn exposure_fusion_keeps_detail_in_the_shadows_and_the_highlights() {
        let (w, h) = (96u32, 64u32);
        let r = radiance(w, h);
        let (dark, mid, bright) = (expose(&r, -1.0), expose(&r, 1.5), expose(&r, 4.0));
        let fused = fuse_exposures(&Frames { w, h, rgba: vec![&dark, &mid, &bright] });
        // Each single exposure loses one side: the bright one clips the
        // window, the dark one buries the room.
        assert!(detail(&bright, w, h, true) < 5.0, "bright frame should clip the window");
        assert!(detail(&dark, w, h, false) < 8.0, "dark frame should bury the room");
        let (l, rr) = (detail(&fused, w, h, false), detail(&fused, w, h, true));
        assert!(l > 12.0 && rr > 12.0, "fused keeps both: room {l}, window {rr}");
    }

    #[test]
    fn a_shifted_frame_is_found_and_the_merge_does_not_double_the_edges() {
        let (w, h) = (160u32, 120u32);
        let mut a = Vec::new();
        for y in 0..h { for x in 0..w {
            let v = if ((x as i32 - 70).pow(2) + (y as i32 - 60).pow(2)) < 900 { 230 } else { ((x * 7 + y * 13) % 40 + 30) as u8 };
            a.extend_from_slice(&[v, v, v, 255]);
        } }
        // b is a moved 5 right, 3 down: pixel (x, y) of a is at (x+5, y+3) in b.
        let mut b = vec![0u8; a.len()];
        for y in 0..h as i32 { for x in 0..w as i32 {
            let (sx, sy) = ((x - 5).clamp(0, w as i32 - 1), (y - 3).clamp(0, h as i32 - 1));
            let (i, j) = (((y as u32 * w + x as u32) * 4) as usize, ((sy as u32 * w + sx as u32) * 4) as usize);
            b[i..i + 4].copy_from_slice(&a[j..j + 4]);
        } }
        assert_eq!(align(&a, &b, w, h), (5, 3));
        // Brightness does not fool it.
        let darker: Vec<u8> = b.iter().enumerate().map(|(i, v)| if i % 4 == 3 { 255 } else { v / 2 }).collect();
        assert_eq!(align(&a, &darker, w, h), (5, 3));
        let avg = average(&Frames { w, h, rgba: vec![&a, &b] });
        // The disc edge at x=100 (right of centre 70, r=30) is still one edge.
        let row = 60u32;
        let inside = avg[((row * w + 95) * 4) as usize];
        let outside = avg[((row * w + 105) * 4) as usize];
        assert!(inside > 200 && outside < 100, "ghosted: {inside} / {outside}");
    }

    #[test]
    fn focus_stacking_takes_each_half_from_the_frame_where_it_is_sharp() {
        let (w, h) = (96u32, 64u32);
        let sharp: Vec<u8> = (0..w * h).flat_map(|i| { let (x, y) = (i % w, i / w); let v = if (x / 2 + y / 2) % 2 == 0 { 200 } else { 50 }; [v, v, v, 255] }).collect();
        let blurred: Vec<u8> = vec![125u8; (w * h * 4) as usize].chunks(4).flat_map(|_| [125u8, 125, 125, 255]).collect();
        // Near-focus frame: left sharp, right blurred; far-focus: the opposite.
        let half = |left_sharp: bool| -> Vec<u8> {
            (0..(w * h) as usize).flat_map(|i| {
                let x = i as u32 % w;
                let src = if (x < w / 2) == left_sharp { &sharp } else { &blurred };
                [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], 255]
            }).collect()
        };
        let (near, far) = (half(true), half(false));
        let out = stack_focus(&Frames { w, h, rgba: vec![&near, &far] });
        let l = detail(&out, w, h, false);
        let r = detail(&out, w, h, true);
        assert!(l > 50.0 && r > 50.0, "both halves sharp: {l} {r}");
    }

    #[test]
    fn averaging_a_noisy_burst_reduces_the_noise() {
        let (w, h) = (64u32, 48u32);
        let mut frames = Vec::new();
        let mut seed = 12345u32;
        for _ in 0..6 {
            frames.push((0..w * h).flat_map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let n = ((seed >> 16) % 41) as i32 - 20;
                let v = (100 + n) as u8;
                [v, v, v, 255]
            }).collect::<Vec<u8>>());
        }
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        let out = average(&Frames { w, h, rgba: refs });
        let sd = |img: &[u8]| { let v: Vec<f32> = img.chunks(4).map(|p| p[0] as f32).collect(); let m = v.iter().sum::<f32>() / v.len() as f32; (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt() };
        assert!(sd(&out) < sd(&frames[0]) * 0.6, "{} vs {}", sd(&out), sd(&frames[0]));
    }

    #[test]
    fn a_sharp_frame_scores_above_a_blurred_one() {
        let (w, h) = (120u32, 80u32);
        let sharp: Vec<u8> = (0..w * h).flat_map(|i| { let v = if (i % w / 3 + i / w / 3) % 2 == 0 { 220 } else { 30 }; [v, v, v, 255] }).collect();
        let soft = crate::develop::render(&sharp, w, h, &crate::develop::Params::identity(w / 4, h / 4));
        let soft = crate::develop::render(&soft, w / 4, h / 4, &crate::develop::Params::identity(w, h));
        assert!(sharpness(&sharp, w, h) > sharpness(&soft, w, h) * 2.0);
    }
}

#[cfg(test)]
mod timing {
    /// `cargo test --release --no-default-features --lib timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn merge_cost_at_1280x720() {
        let (w, h) = (1280u32, 720u32);
        let frames: Vec<Vec<u8>> = (0..3).map(|k| (0..w * h).flat_map(|i| { let v = ((i % w + i / w * 3 + k * 40) % 256) as u8; [v, v / 2, 255 - v, 255] }).collect()).collect();
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        let f = super::Frames { w, h, rgba: refs };
        for (name, run) in [("fuse 3", super::fuse_exposures as fn(&super::Frames) -> Vec<u8>), ("focus 3", super::stack_focus), ("average 3", super::average)] {
            let t = std::time::Instant::now();
            let _ = run(&f);
            eprintln!("{name}: {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
        }
    }
}
