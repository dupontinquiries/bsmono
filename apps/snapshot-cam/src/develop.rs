//! develop — the one pass from a camera frame to a photo.
//!
//! What a DSLR's "picture style" and a lens-correction profile do, in one
//! resampling loop over the OUTPUT pixels:
//!
//! 1. place the output pixel in the corrected image (the crop window, the
//!    zoom window, and the anamorphic desqueeze are all just this mapping);
//! 2. undo the lens's radial distortion to find where that point landed on
//!    the sensor (Brown model, `r' = r·(1 + k1·r² + k2·r⁴)`, `r` in units of
//!    half the frame diagonal);
//! 3. sample there, bilinearly, in LINEAR light;
//! 4. undo the lens's vignetting at that sensor radius (lensfun's `pa`
//!    model: the lens multiplied brightness by `1 + v1·r² + v2·r⁴ + v3·r⁶`
//!    — negative coefficients for the usual darkening — so this divides);
//! 5. remove the cover's haze (a per-channel flare offset) and apply the
//!    cover's tint correction, white balance and exposure as per-channel
//!    gains, still linear;
//! 6. encode to sRGB.
//!
//! One pass rather than a chain of images so the preview can run it at
//! preview size every frame (the output is small whatever the zoom) and the
//! shutter can run the SAME function at native size — the file is what the
//! viewfinder showed, by construction rather than by two implementations
//! agreeing.
//!
//! Honest limits: the frames are 8-bit sRGB from the camera's own ISP, not
//! sensor RAW. Exposure compensation here is a gain on those 8 bits — it
//! cannot recover a clipped highlight or invent shadow detail, which is what
//! the zebras and the histogram are for. Real exposure is `hwctl`'s job,
//! where the camera allows it.

use std::sync::OnceLock;

/// Everything `render` needs, resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// The part of the corrected frame to render, `[u0, v0, u1, v1]`.
    pub window: [f32; 4],
    pub out_w: u32,
    pub out_h: u32,
    /// Brown radial distortion `[k1, k2]`; zero is no correction.
    pub distortion: [f32; 2],
    /// Vignetting `[v1, v2, v3]`; zero is no correction.
    pub vignetting: [f32; 3],
    /// Linear per-channel gains: white balance times 2^EV.
    pub gains: [f32; 3],
    /// Veiling glare to remove, linear per channel: a scratched or hazy
    /// cover lifts every pixel by roughly this much, so it is subtracted
    /// and the rest stretched back, `(l − f) / (1 − f)`. Zero is none.
    pub flare: [f32; 3],
}

impl Params {
    /// The whole frame, untouched, at `w × h`.
    pub fn identity(w: u32, h: u32) -> Params {
        Params { window: [0.0, 0.0, 1.0, 1.0], out_w: w, out_h: h,
                 distortion: [0.0; 2], vignetting: [0.0; 3], gains: [1.0; 3], flare: [0.0; 3] }
    }
}

/// The output size at the sensor's native pixel density: the window's
/// pixels, widened by the desqueeze. The only upsampling this app ever
/// does is that widening — an anamorphic lens squeezed the scene
/// horizontally and the file has to be un-squeezed to look right.
pub fn native_size(window: [f32; 4], w: u32, h: u32, desqueeze: f32) -> (u32, u32) {
    let cw = ((window[2] - window[0]).abs() * w as f32 * desqueeze.max(0.1)).round().max(1.0) as u32;
    let ch = ((window[3] - window[1]).abs() * h as f32).round().max(1.0) as u32;
    (cw, ch)
}

/// The same aspect, scaled to fit inside `max_w × max_h` (never up).
pub fn fit_size(native: (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = (native.0 as f32, native.1 as f32);
    let s = (max_w as f32 / w).min(max_h as f32 / h).min(1.0);
    (((w * s).round() as u32).max(1), ((h * s).round() as u32).max(1))
}

fn decode_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

const ENC_N: usize = 4096;
fn encode_lut() -> &'static [u8; ENC_N] {
    static LUT: OnceLock<[u8; ENC_N]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0u8; ENC_N];
        for (i, v) in t.iter_mut().enumerate() {
            let l = i as f32 / (ENC_N - 1) as f32;
            let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
            *v = (c * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        t
    })
}

/// sRGB byte to linear.
pub fn to_linear(c: u8) -> f32 { decode_lut()[c as usize] }
/// Linear to sRGB byte, clipping.
pub fn to_srgb(l: f32) -> u8 { encode_lut()[(l.clamp(0.0, 1.0) * (ENC_N - 1) as f32) as usize] }

/// Render. `src` is RGBA8, `sw × sh`; the result is RGBA8, `out_w × out_h`,
/// opaque. Points that map off the sensor (a strong correction near a
/// corner) are black.
pub fn render(src: &[u8], sw: u32, sh: u32, p: &Params) -> Vec<u8> {
    let (ow, oh) = (p.out_w.max(1) as usize, p.out_h.max(1) as usize);
    let mut out = vec![0u8; ow * oh * 4];
    if sw == 0 || sh == 0 || src.len() < sw as usize * sh as usize * 4 { return out; }
    let dec = decode_lut();
    let (swf, shf) = (sw as f32, sh as f32);
    let (cx, cy) = (swf * 0.5, shf * 0.5);
    let inv_hd2 = 4.0 / (swf * swf + shf * shf);
    let [u0, v0, u1, v1] = p.window;
    let [k1, k2] = p.distortion;
    let [g1, g2, g3] = p.vignetting;
    let distort = k1 != 0.0 || k2 != 0.0;
    let vignette = g1 != 0.0 || g2 != 0.0 || g3 != 0.0;
    let stride = sw as usize * 4;
    let px = |x: usize, y: usize, c: usize| dec[src[y * stride + x * 4 + c] as usize];
    let fl = p.flare.map(|f| f.clamp(0.0, 0.5));
    let fl_k = fl.map(|f| 1.0 / (1.0 - f));
    let dehaze = |l: f32, c: usize| if fl[c] > 0.0 { ((l - fl[c]) * fl_k[c]).max(0.0) } else { l };
    if !distort {
        // SEPARABLE: with no distortion a column's source x does not depend
        // on the row, so the bilinear taps are computed once per column and
        // once per row instead of once per pixel. The common case — crop,
        // zoom, desqueeze, WB, EV, vignetting — and about twice as fast.
        let taps = |n_out: usize, a: f32, b: f32, n_src: u32| -> Vec<(usize, usize, f32, bool, f32)> {
            (0..n_out).map(|o| {
                let s = (a + (o as f32 + 0.5) / n_out as f32 * (b - a)) * n_src as f32;
                let inside = (0.0..n_src as f32).contains(&s);
                let f = (s - 0.5).max(0.0);
                let i0 = (f as usize).min(n_src as usize - 1);
                (i0, (i0 + 1).min(n_src as usize - 1), f - i0 as f32, inside, s)
            }).collect()
        };
        let cols = taps(ow, u0, u1, sw);
        let rows = taps(oh, v0, v1, sh);
        for (oy, &(y0, y1, ty, yin, sy)) in rows.iter().enumerate() {
            let dy2 = (sy - cy) * (sy - cy) * inv_hd2;
            for (ox, &(x0, x1, tx, xin, sx)) in cols.iter().enumerate() {
                let o = (oy * ow + ox) * 4;
                out[o + 3] = 255;
                if !(xin && yin) { continue; }
                let mut gain = 1.0;
                if vignette {
                    let r2 = (sx - cx) * (sx - cx) * inv_hd2 + dy2;
                    gain = 1.0 / (1.0 + g1 * r2 + g2 * r2 * r2 + g3 * r2 * r2 * r2).max(0.05);
                }
                for c in 0..3 {
                    let a = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                    let b = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                    out[o + c] = to_srgb(dehaze(a * (1.0 - ty) + b * ty, c) * gain * p.gains[c]);
                }
            }
        }
        return out;
    }
    for oy in 0..oh {
        let y = (v0 + (oy as f32 + 0.5) / oh as f32 * (v1 - v0)) * shf;
        for ox in 0..ow {
            let x = (u0 + (ox as f32 + 0.5) / ow as f32 * (u1 - u0)) * swf;
            let (mut sx, mut sy) = (x, y);
            if distort {
                let (dx, dy) = (x - cx, y - cy);
                let r2 = (dx * dx + dy * dy) * inv_hd2;
                let f = 1.0 + k1 * r2 + k2 * r2 * r2;
                sx = cx + dx * f;
                sy = cy + dy * f;
            }
            let o = (oy * ow + ox) * 4;
            out[o + 3] = 255;
            if !(0.0..swf).contains(&sx) || !(0.0..shf).contains(&sy) { continue; }
            // Bilinear, texel centres at +0.5.
            let fx = (sx - 0.5).max(0.0);
            let fy = (sy - 0.5).max(0.0);
            let x0 = (fx as usize).min(sw as usize - 1);
            let y0 = (fy as usize).min(sh as usize - 1);
            let x1 = (x0 + 1).min(sw as usize - 1);
            let y1 = (y0 + 1).min(sh as usize - 1);
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let mut gain = 1.0;
            if vignette {
                let (dx, dy) = (sx - cx, sy - cy);
                let r2 = (dx * dx + dy * dy) * inv_hd2;
                gain = 1.0 / (1.0 + g1 * r2 + g2 * r2 * r2 + g3 * r2 * r2 * r2).max(0.05);
            }
            for c in 0..3 {
                let a = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                let b = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                let l = dehaze(a * (1.0 - ty) + b * ty, c) * gain * p.gains[c];
                out[o + c] = to_srgb(l);
            }
        }
    }
    out
}

// ── white balance and exposure ──────────────────────────────────────────

/// How colour is corrected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteBalance {
    /// The camera's own choice, untouched.
    AsShot,
    /// Grey-world from the picture itself, re-measured as the scene changes.
    Auto,
    /// The light is this colour temperature; correct it to daylight.
    Kelvin(u32),
}

/// The presets a camera body puts on its WB dial.
pub const WB_PRESETS: [(&str, u32); 5] = [
    ("tungsten", 3200), ("fluorescent", 4000), ("daylight", 5500), ("cloudy", 6500), ("shade", 7500),
];

impl WhiteBalance {
    pub fn label(&self) -> String {
        match self {
            WhiteBalance::AsShot => "as shot".into(),
            WhiteBalance::Auto => "auto".into(),
            WhiteBalance::Kelvin(k) => match WB_PRESETS.iter().find(|(_, pk)| pk == k) {
                Some((n, _)) => format!("{n} {k}K"),
                None => format!("{k}K"),
            },
        }
    }
    pub fn to_conf(&self) -> String {
        match self { WhiteBalance::AsShot => "as-shot".into(), WhiteBalance::Auto => "auto".into(),
                     WhiteBalance::Kelvin(k) => k.to_string() }
    }
    pub fn from_conf(s: &str) -> WhiteBalance {
        match s.trim() {
            "auto" => WhiteBalance::Auto,
            k => k.parse::<u32>().ok().filter(|k| (1500..=15000).contains(k))
                .map(WhiteBalance::Kelvin).unwrap_or(WhiteBalance::AsShot),
        }
    }
}

/// The colour of a black body at `k` kelvin, linear RGB, peak 1 (Tanner
/// Helland's fit to the CIE data — good to a few percent over 1000–40000K,
/// which is far better than a WB dial needs).
pub fn kelvin_rgb(k: u32) -> [f32; 3] {
    let t = (k as f32).clamp(1000.0, 40000.0) / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let g = if t <= 66.0 { 99.470_8 * t.ln() - 161.119_57 } else { 288.122_16 * (t - 60.0).powf(-0.075_514_85) };
    let b = if t >= 66.0 { 255.0 } else if t <= 19.0 { 0.0 } else { 138.517_73 * (t - 10.0).ln() - 305.044_8 };
    let f = |v: f32| to_linear(v.clamp(0.0, 255.0) as u8).max(1e-3);
    [f(r), f(g), f(b)]
}

/// Gains that turn light of temperature `k` into neutral, green held at 1
/// so brightness stays put: tungsten light is orange, so blue goes up.
pub fn kelvin_gains(k: u32) -> [f32; 3] {
    let c = kelvin_rgb(k);
    let d = kelvin_rgb(6500);
    let raw = [d[0] / c[0], d[1] / c[1], d[2] / c[2]];
    [raw[0] / raw[1], 1.0, raw[2] / raw[1]]
}

/// Grey-world gains from an image: the mean of the lit pixels is assumed
/// grey. Clamped, because a frame that is all one colour (a red wall) is not
/// grey and the clamp is what keeps it from being painted cyan.
pub fn grey_world(rgba: &[u8]) -> [f32; 3] {
    let mut s = [0.0f64; 3];
    let mut n = 0u32;
    for px in rgba.chunks_exact(4) {
        let luma = (px[0] as u32 * 3 + px[1] as u32 * 6 + px[2] as u32) / 10;
        if !(16..=245).contains(&luma) { continue; }
        for c in 0..3 { s[c] += to_linear(px[c]) as f64; }
        n += 1;
    }
    if n < 16 { return [1.0; 3]; }
    let g = s[1].max(1e-6);
    [(g / s[0].max(1e-6)).clamp(0.4, 2.5) as f32, 1.0, (g / s[2].max(1e-6)).clamp(0.4, 2.5) as f32]
}

/// White balance times exposure, as `render` wants them.
pub fn gains(wb: WhiteBalance, auto: [f32; 3], ev: f32) -> [f32; 3] {
    let w = match wb { WhiteBalance::AsShot => [1.0; 3], WhiteBalance::Auto => auto, WhiteBalance::Kelvin(k) => kelvin_gains(k) };
    let e = 2f32.powf(ev.clamp(-5.0, 5.0));
    [w[0] * e, w[1] * e, w[2] * e]
}

// ── what a DSLR shows you about the picture ─────────────────────────────

/// Histogram bins. 64 is what fits a thumbnail-sized box legibly.
pub const BINS: usize = 64;

/// What the viewfinder overlays need, measured on what is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// `[luma, r, g, b]`, each `BINS` counts.
    pub hist: [[u32; BINS]; 4],
    /// Mean scene brightness against middle grey (18%), in stops: 0 is a
    /// "correct" average exposure, +1 is one stop over.
    pub meter_ev: f32,
    /// Fraction of pixels with a channel at 255 — clipped highlights.
    pub clipped: f32,
    /// RGBA, same size as the input, transparent except where the zebra
    /// stripes (clipping) and the focus peaking (edges) are painted.
    pub overlay: Option<Vec<u8>>,
}

/// Measure `rgba` (`w × h`). `zebra` stripes channels at or above
/// `ZEBRA_AT`; `peaking` paints strong local contrast — what is in focus.
pub fn analyse(rgba: &[u8], w: u32, h: u32, zebra: bool, peaking: bool) -> Analysis {
    let (w, h) = (w as usize, h as usize);
    let mut hist = [[0u32; BINS]; 4];
    let mut lin_sum = 0.0f64;
    let mut clipped = 0u32;
    let n = (w * h).max(1);
    let mut luma = vec![0u8; w * h];
    for (i, px) in rgba.chunks_exact(4).take(w * h).enumerate() {
        let l = ((px[0] as u32 * 54 + px[1] as u32 * 183 + px[2] as u32 * 19) >> 8) as u8;
        luma[i] = l;
        hist[0][l as usize * BINS / 256] += 1;
        for c in 0..3 { hist[c + 1][px[c] as usize * BINS / 256] += 1; }
        lin_sum += to_linear(l) as f64;
        if px[0] == 255 || px[1] == 255 || px[2] == 255 { clipped += 1; }
    }
    let mean = (lin_sum / n as f64).max(1e-5) as f32;
    let meter_ev = (mean / 0.18).log2();
    let overlay = (zebra || peaking).then(|| {
        let mut o = vec![0u8; w * h * 4];
        if peaking && w > 2 && h > 2 {
            // Sobel magnitude over luma; the threshold is relative to the
            // frame's own strongest edges so a soft, dim scene still shows
            // where its focus is.
            let mut mag = vec![0u16; w * h];
            let mut peak = 0u16;
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let l = |dx: isize, dy: isize| luma[((y as isize + dy) as usize) * w + (x as isize + dx) as usize] as i32;
                    let gx = -l(-1, -1) - 2 * l(-1, 0) - l(-1, 1) + l(1, -1) + 2 * l(1, 0) + l(1, 1);
                    let gy = -l(-1, -1) - 2 * l(0, -1) - l(1, -1) + l(-1, 1) + 2 * l(0, 1) + l(1, 1);
                    let m = ((gx.abs() + gy.abs()) as u16).min(2040);
                    mag[y * w + x] = m;
                    peak = peak.max(m);
                }
            }
            let thr = (peak as f32 * PEAK_FRACTION).max(PEAK_FLOOR) as u16;
            for (i, m) in mag.iter().enumerate() {
                if *m >= thr { o[i * 4..i * 4 + 4].copy_from_slice(&PEAK_COLOUR); }
            }
        }
        if zebra {
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let px = &rgba[i * 4..i * 4 + 3];
                    if px.iter().any(|c| *c >= ZEBRA_AT) && ((x + y) / 4) % 2 == 0 {
                        o[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 200]);
                    }
                }
            }
        }
        o
    });
    Analysis { hist, meter_ev, clipped: clipped as f32 / n as f32, overlay }
}

/// A channel at or above this is striped.
pub const ZEBRA_AT: u8 = 250;
/// Edges within this fraction of the frame's strongest are peaked.
const PEAK_FRACTION: f32 = 0.35;
const PEAK_FLOOR: f32 = 120.0;
const PEAK_COLOUR: [u8; 4] = [255, 40, 200, 230];

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..w * h { v.extend_from_slice(&[c[0], c[1], c[2], 255]); }
        v
    }

    fn ramp(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w { v.extend_from_slice(&[(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, 128, 255]); } }
        v
    }

    #[test]
    fn preview_cost_is_printed() {
        // Not an assertion on speed (that depends on the machine and the
        // profile); the number, so a regression shows up in the log.
        let src = vec![128u8; 1280 * 720 * 4];
        let p = Params { window: [0.0, 0.0, 1.0, 1.0], out_w: 640, out_h: 271, distortion: [0.05, 0.0],
                         vignetting: [-0.3, 0.0, 0.0], gains: [1.1, 1.0, 0.9], flare: [0.02; 3] };
        let t0 = std::time::Instant::now();
        let out = render(&src, 1280, 720, &p);
        let t1 = t0.elapsed();
        let _ = analyse(&out, 640, 271, true, true);
        eprintln!("render 640×271: {:.1} ms, analyse: {:.1} ms", t1.as_secs_f32() * 1e3, (t0.elapsed() - t1).as_secs_f32() * 1e3);
    }

    #[test]
    fn the_fast_path_and_the_general_path_agree() {
        // A distortion too small to move a sample forces the general loop;
        // the two must give the same picture.
        let src = ramp(64, 36);
        let base = Params { window: [0.1, 0.2, 0.9, 0.8], out_w: 50, out_h: 20, distortion: [0.0; 2],
                            vignetting: [-0.3, 0.1, 0.0], gains: [1.2, 1.0, 0.8], flare: [0.03, 0.02, 0.01] };
        let fast = render(&src, 64, 36, &base);
        let slow = render(&src, 64, 36, &Params { distortion: [1e-12, 0.0], ..base });
        let worst = fast.iter().zip(&slow).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "{worst}");
    }

    #[test]
    fn the_identity_render_is_the_frame() {
        let src = ramp(32, 16);
        let out = render(&src, 32, 16, &Params::identity(32, 16));
        let worst = src.iter().zip(&out).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "an identity pass changed a pixel by {worst}");
    }

    #[test]
    fn a_window_renders_that_part_and_a_desqueeze_widens_it() {
        let src = ramp(40, 20);
        let win = [0.5, 0.0, 1.0, 1.0];
        let (w, h) = native_size(win, 40, 20, 1.0);
        assert_eq!((w, h), (20, 20));
        let out = render(&src, 40, 20, &Params { window: win, out_w: w, out_h: h, ..Params::identity(w, h) });
        // The first output column is source column 20.
        assert!((out[0] as i32 - (20 * 255 / 39) as i32).abs() <= 2, "{}", out[0]);
        // Anamorphic 1.33×: the same window, a third wider, same height.
        assert_eq!(native_size(win, 40, 20, 1.33), (27, 20));
        assert_eq!(native_size([0.0, 0.0, 1.0, 1.0], 1280, 720, 1.33), (1702, 720));
    }

    #[test]
    fn exposure_is_a_linear_gain_and_clips_at_white() {
        let src = flat(4, 4, [118, 118, 118]); // ~18% grey
        let one_up = render(&src, 4, 4, &Params { gains: gains(WhiteBalance::AsShot, [1.0; 3], 1.0), ..Params::identity(4, 4) });
        // One stop in linear light: 0.18 → 0.36, which is sRGB ~161.
        assert!((one_up[0] as i32 - 161).abs() <= 2, "{}", one_up[0]);
        let five_up = render(&src, 4, 4, &Params { gains: gains(WhiteBalance::AsShot, [1.0; 3], 5.0), ..Params::identity(4, 4) });
        assert_eq!(five_up[0], 255);
    }

    #[test]
    fn tungsten_correction_cools_and_shade_warms_and_daylight_is_nearly_neutral() {
        let t = kelvin_gains(3200);
        assert!(t[2] > 1.3 && t[0] < 1.0, "tungsten: blue up, red down: {t:?}");
        let s = kelvin_gains(7500);
        assert!(s[0] > 1.0 && s[2] < 1.0, "shade: red up, blue down: {s:?}");
        let d = kelvin_gains(6500);
        assert!((d[0] - 1.0).abs() < 1e-3 && (d[2] - 1.0).abs() < 1e-3, "{d:?}");
        assert_eq!(t[1], 1.0, "green is the anchor");
    }

    #[test]
    fn grey_world_neutralises_a_cast_and_refuses_to_repaint_a_red_wall() {
        let cast = flat(8, 8, [180, 140, 100]);
        let g = grey_world(&cast);
        let out = render(&cast, 8, 8, &Params { gains: g, ..Params::identity(8, 8) });
        let (r, gg, b) = (out[0] as i32, out[1] as i32, out[2] as i32);
        assert!((r - gg).abs() <= 3 && (b - gg).abs() <= 3, "not neutral: {r} {gg} {b}");
        let wall = flat(8, 8, [220, 30, 30]);
        let g = grey_world(&wall);
        assert!(g[0] >= 0.4 && g[2] <= 2.5, "clamped: {g:?}");
    }

    #[test]
    fn vignetting_correction_brightens_the_corners_not_the_centre() {
        let src = flat(64, 36, [100, 100, 100]);
        let p = Params { vignetting: [-0.4, 0.0, 0.0], ..Params::identity(64, 36) };
        let out = render(&src, 64, 36, &p);
        let at = |x: usize, y: usize| out[(y * 64 + x) * 4];
        assert!(at(0, 0) > at(32, 18) + 20, "corner {} vs centre {}", at(0, 0), at(32, 18));
        assert!((at(32, 18) as i32 - 100).abs() <= 2);
    }

    #[test]
    fn barrel_correction_pulls_from_further_out_and_leaves_the_centre() {
        // A frame whose left half is dark and right half bright: with k1 > 0
        // an output point near the edge samples further out, so the centre
        // column is unchanged and nothing panics near the corners.
        let mut src = flat(64, 36, [20, 20, 20]);
        for y in 0..36 { for x in 32..64 { let i = (y * 64 + x) * 4; src[i..i + 3].copy_from_slice(&[230, 230, 230]); } }
        let p = Params { distortion: [0.3, 0.0], ..Params::identity(64, 36) };
        let out = render(&src, 64, 36, &p);
        assert_eq!(out.len(), 64 * 36 * 4);
        // Corners map off the sensor: black, not garbage.
        assert_eq!(&out[..3], &[0, 0, 0]);
        assert!(out[(18 * 64 + 33) * 4] > 200 && out[(18 * 64 + 30) * 4] < 40, "the centre moved");
    }

    #[test]
    fn the_meter_and_histogram_read_the_picture() {
        let grey = flat(16, 16, [118, 118, 118]);
        let a = analyse(&grey, 16, 16, false, false);
        assert!(a.meter_ev.abs() < 0.1, "middle grey meters at 0: {}", a.meter_ev);
        assert_eq!(a.hist[0].iter().sum::<u32>(), 256);
        assert!(a.overlay.is_none());
        let white = flat(16, 16, [255, 255, 255]);
        let a = analyse(&white, 16, 16, true, false);
        assert!(a.meter_ev > 2.0);
        assert_eq!(a.clipped, 1.0);
        let o = a.overlay.unwrap();
        let striped = o.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(striped > 64 && striped < 256, "zebras are stripes, not a fill: {striped}");
    }

    #[test]
    fn peaking_marks_the_sharp_edge_and_not_the_flat_field() {
        let mut src = flat(32, 32, [40, 40, 40]);
        for y in 0..32 { for x in 16..32 { let i = (y * 32 + x) * 4; src[i..i + 3].copy_from_slice(&[220, 220, 220]); } }
        let a = analyse(&src, 32, 32, false, true);
        let o = a.overlay.unwrap();
        let marked = |x: usize, y: usize| o[(y * 32 + x) * 4 + 3] > 0;
        assert!(marked(16, 10) || marked(15, 10), "the edge is peaked");
        assert!(!marked(5, 10) && !marked(26, 10), "the flat field is not");
    }

    #[test]
    fn wb_round_trips_through_the_settings_file() {
        for wb in [WhiteBalance::AsShot, WhiteBalance::Auto, WhiteBalance::Kelvin(3200), WhiteBalance::Kelvin(5100)] {
            assert_eq!(WhiteBalance::from_conf(&wb.to_conf()), wb);
        }
        assert_eq!(WhiteBalance::from_conf("99"), WhiteBalance::AsShot, "a nonsense temperature is refused");
        assert_eq!(WhiteBalance::Kelvin(3200).label(), "tungsten 3200K");
    }
}
