//! style — make one clip look like another, cheap enough to run live.
//!
//! **Fit once, apply as a lookup.** Measuring two clips and working out the
//! mapping is the expensive part and happens once; the mapping is then baked
//! into a 3D colour lookup table, which is a trilinear fetch per pixel -- fast
//! enough for every preview frame -- and which ffmpeg's `lut3d` applies
//! identically on export (`the_export_matches_the_preview`).
//!
//! **What moves**, in Lab (lightness L, colour axes a and b):
//!
//! * tone: the target's lightness distribution is reshaped onto the
//!   reference's by matching nine quantiles -- contrast, brightness and where
//!   the shadows and highlights sit, all at once;
//! * colour per luminance band: shadows, mids and highlights each take the
//!   reference's tint and saturation. A split tone -- warm shadows under
//!   magenta highlights, the pink look of "All My Life" -- is exactly a
//!   per-band tint, and a single global shift cannot express it.
//!
//! **Stored as numbers, not a file.** The mapping is a few dozen floats on
//! the clip's effect list (kind `"style"`), so it saves, undoes and syncs like
//! any other effect, and the LUT is rebuilt from them wherever it is needed.

use studio_core::Effect;

pub const KIND: &str = "style";
const NQ: usize = 9;

// ── colour conversions (sRGB, D65) ─────────────────────────────────────────

fn to_lin(c: f32) -> f32 { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
fn to_srgb(c: f32) -> f32 { if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } }
fn f(t: f32) -> f32 { if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 } }
fn finv(t: f32) -> f32 { let t3 = t * t * t; if t3 > 0.008_856 { t3 } else { (t - 16.0 / 116.0) / 7.787 } }

/// RGB 0..1 -> Lab (L 0..100).
pub fn rgb_to_lab(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (to_lin(c[0]), to_lin(c[1]), to_lin(c[2]));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Lab -> RGB 0..1, clamped.
pub fn lab_to_rgb(l: [f32; 3]) -> [f32; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let (fx, fz) = (fy + l[1] / 500.0, fy - l[2] / 200.0);
    let (x, y, z) = (finv(fx) * 0.950_47, finv(fy), finv(fz) * 1.088_83);
    let r = 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z;
    let g = -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z;
    let b = 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z;
    [to_srgb(r.max(0.0)).clamp(0.0, 1.0), to_srgb(g.max(0.0)).clamp(0.0, 1.0), to_srgb(b.max(0.0)).clamp(0.0, 1.0)]
}

// ── measuring a look ───────────────────────────────────────────────────────

/// A frame's look: its lightness distribution and the colour of each band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Lightness at the 5%, 16%, 27% ... 95% points.
    pub q: [f32; NQ],
    /// Per band (shadows, mids, highlights, split at the L tertiles):
    /// mean a, mean b, and mean distance from that mean (the band's saturation).
    pub band: [[f32; 3]; 3],
}

fn quantiles(v: &mut [f32]) -> [f32; NQ] {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut q = [0.0; NQ];
    for (i, qi) in q.iter_mut().enumerate() {
        let p = 0.05 + 0.9 * i as f32 / (NQ - 1) as f32;
        *qi = v[((v.len() - 1) as f32 * p).round() as usize];
    }
    q
}

/// Measure an RGB (8-bit) frame. Subsampled: a look is a distribution, and a
/// few thousand pixels describe it as well as a million.
pub fn stats(px: &[u8]) -> Stats {
    let step = ((px.len() / 3) / 8000).max(1) * 3;
    let labs: Vec<[f32; 3]> = px.chunks(3).step_by(step / 3)
        .map(|p| rgb_to_lab([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])).collect();
    let mut ls: Vec<f32> = labs.iter().map(|l| l[0]).collect();
    let q = quantiles(&mut ls);
    let (t1, t2) = (q[2] + (q[3] - q[2]) * 0.6, q[5] + (q[6] - q[5]) * 0.4); // ~33% and ~66%
    let mut band = [[0.0f32; 3]; 3];
    for (k, b) in band.iter_mut().enumerate() {
        let sel: Vec<&[f32; 3]> = labs.iter().filter(|l| match k { 0 => l[0] < t1, 1 => l[0] >= t1 && l[0] < t2, _ => l[0] >= t2 }).collect();
        if sel.is_empty() { continue; }
        let n = sel.len() as f32;
        let (ma, mb) = (sel.iter().map(|l| l[1]).sum::<f32>() / n, sel.iter().map(|l| l[2]).sum::<f32>() / n);
        let spread = sel.iter().map(|l| ((l[1] - ma).powi(2) + (l[2] - mb).powi(2)).sqrt()).sum::<f32>() / n;
        *b = [ma, mb, spread];
    }
    Stats { q, band }
}

// ── the mapping ────────────────────────────────────────────────────────────

/// How to turn a target clip's look into a reference's: fitted once, applied
/// as a LUT.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    pub src: Stats,
    pub dst: Stats,
    /// 0 leaves the clip as it is, 1 is the full reference look.
    pub strength: f32,
}

impl Transfer {
    pub fn fit(target: &Stats, reference: &Stats, strength: f32) -> Transfer {
        Transfer { src: *target, dst: *reference, strength: strength.clamp(0.0, 1.0) }
    }

    fn tone(&self, l: f32) -> f32 {
        let (s, d) = (&self.src.q, &self.dst.q);
        // piecewise-linear through the quantile pairs, extended at the ends
        let seg = |i: usize, l: f32| {
            let (a, b) = (s[i], s[i + 1]);
            let t = if (b - a).abs() < 1e-3 { 0.5 } else { (l - a) / (b - a) };
            d[i] + t * (d[i + 1] - d[i])
        };
        let out = if l <= s[0] { if s[0] > 1e-3 { l * d[0] / s[0] } else { d[0] } }
            else if l >= s[NQ - 1] { let (a, b) = (s[NQ - 1], d[NQ - 1]); b + (l - a) * (100.0 - b) / (100.0 - a).max(1e-3) }
            else { let i = (0..NQ - 1).find(|&i| l < s[i + 1]).unwrap_or(NQ - 2); seg(i, l) };
        out.clamp(0.0, 100.0)
    }

    /// One colour through the mapping (RGB 0..1).
    pub fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        let lab = rgb_to_lab(c);
        let l2 = self.tone(lab[0]);
        // band weights by the NEW lightness, smooth across band edges (no banding)
        let centres = [(self.dst.q[1] + self.dst.q[2]) / 2.0, self.dst.q[4], (self.dst.q[6] + self.dst.q[7]) / 2.0];
        let mut w = [0.0f32; 3];
        for k in 0..3 { let d = (l2 - centres[k]) / 18.0; w[k] = (-d * d).exp(); }
        let ws: f32 = w.iter().sum::<f32>().max(1e-6);
        let (mut a2, mut b2) = (0.0, 0.0);
        for k in 0..3 {
            let (sb, db) = (self.src.band[k], self.dst.band[k]);
            let scale = if sb[2] > 0.5 { (db[2] / sb[2]).clamp(0.25, 4.0) } else { 1.0 };
            a2 += w[k] / ws * (db[0] + (lab[1] - sb[0]) * scale);
            b2 += w[k] / ws * (db[1] + (lab[2] - sb[1]) * scale);
        }
        // Hold the ends: true black and true white stay neutral. The first run
        // on "All My Life" turned its black letterbox bars dark purple -- the
        // shadow band's tint carried all the way down to L = 0.
        let ends = |l: f32| { let lo = (l / 8.0).clamp(0.0, 1.0); let hi = ((100.0 - l) / 6.0).clamp(0.0, 1.0);
                              lo * lo * (3.0 - 2.0 * lo) * hi * hi * (3.0 - 2.0 * hi) };
        let k = ends(l2).min(ends(lab[0]));
        let s = self.strength;
        lab_to_rgb([lab[0] + (l2 - lab[0]) * s, lab[1] + (a2 - lab[1]) * s * k, lab[2] + (b2 - lab[2]) * s * k])
    }

    /// The mapping baked into an `n`^3 LUT, red fastest (the `.cube` order).
    pub fn lut(&self, n: usize) -> Vec<[f32; 3]> {
        let mut v = Vec::with_capacity(n * n * n);
        let d = (n - 1) as f32;
        for b in 0..n { for g in 0..n { for r in 0..n {
            v.push(self.apply_rgb([r as f32 / d, g as f32 / d, b as f32 / d]));
        }}}
        v
    }

    /// The LUT as an Adobe/Resolve `.cube` file, which ffmpeg's `lut3d` reads.
    pub fn cube(&self, n: usize) -> String {
        let mut s = format!("TITLE \"studio style\"\nLUT_3D_SIZE {n}\n");
        for c in self.lut(n) { s.push_str(&format!("{:.6} {:.6} {:.6}\n", c[0], c[1], c[2])); }
        s
    }

    pub fn effect(&self) -> Effect {
        let mut p: std::collections::BTreeMap<String, f32> = Default::default();
        p.insert("strength".into(), self.strength);
        for (tag, st) in [("s", &self.src), ("d", &self.dst)] {
            for i in 0..NQ { p.insert(format!("{tag}q{i}"), st.q[i]); }
            for k in 0..3 { for j in 0..3 { p.insert(format!("{tag}b{k}{j}"), st.band[k][j]); } }
        }
        Effect { kind: KIND.into(), enabled: true, params: p.into_iter().collect() }
    }

    /// The last enabled style on a clip, if any.
    pub fn of(effects: &[Effect]) -> Option<Transfer> {
        let e = effects.iter().filter(|e| e.enabled && e.kind == KIND).last()?;
        let g = |k: String| e.params.get(&k).copied();
        let read = |tag: &str| -> Option<Stats> {
            let mut st = Stats { q: [0.0; NQ], band: [[0.0; 3]; 3] };
            for i in 0..NQ { st.q[i] = g(format!("{tag}q{i}"))?; }
            for k in 0..3 { for j in 0..3 { st.band[k][j] = g(format!("{tag}b{k}{j}"))?; } }
            Some(st)
        };
        Some(Transfer { src: read("s")?, dst: read("d")?, strength: g("strength".into()).unwrap_or(1.0) })
    }
}

/// Apply an `n`^3 LUT to an 8-bit RGB frame, trilinear -- the per-frame cost
/// the preview pays.
///
/// Each channel value has only 256 possibilities, so its cell index and
/// fraction are looked up from a table built once per call rather than
/// recomputed per pixel with a divide and a floor. Rows are split across
/// threads when the frame is big enough to be worth it; the result is
/// identical to a single-threaded pass (pinned by a test), because every
/// pixel is computed independently.
pub fn apply_lut(px: &mut [u8], lut: &[[f32; 3]], n: usize) {
    let d = (n - 1) as f32;
    let mut idx = [(0usize, 0usize, 0.0f32); 256];
    for (v, e) in idx.iter_mut().enumerate() {
        let f = v as f32 / 255.0 * d;
        let i0 = (f.floor() as usize).min(n - 1);
        *e = (i0, (i0 + 1).min(n - 1), f - i0 as f32);
    }
    let pixels = px.len() / 3;
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(1).min(8);
    if threads <= 1 || pixels < 200_000 {
        apply_rows(px, lut, n, &idx);
        return;
    }
    let chunk = pixels.div_ceil(threads) * 3;
    std::thread::scope(|sc| {
        for part in px.chunks_mut(chunk) {
            let idx = &idx;
            sc.spawn(move || apply_rows(part, lut, n, idx));
        }
    });
}

fn apply_rows(px: &mut [u8], lut: &[[f32; 3]], n: usize, idx: &[(usize, usize, f32); 256]) {
    let nn = n * n;
    for p in px.chunks_mut(3) {
        let (r0, r1, tr) = idx[p[0] as usize];
        let (g0, g1, tg) = idx[p[1] as usize];
        let (b0, b1, tb) = idx[p[2] as usize];
        let (c000, c100) = (lut[b0 * nn + g0 * n + r0], lut[b0 * nn + g0 * n + r1]);
        let (c010, c110) = (lut[b0 * nn + g1 * n + r0], lut[b0 * nn + g1 * n + r1]);
        let (c001, c101) = (lut[b1 * nn + g0 * n + r0], lut[b1 * nn + g0 * n + r1]);
        let (c011, c111) = (lut[b1 * nn + g1 * n + r0], lut[b1 * nn + g1 * n + r1]);
        for c in 0..3 {
            let x00 = c000[c] + (c100[c] - c000[c]) * tr;
            let x10 = c010[c] + (c110[c] - c010[c]) * tr;
            let x01 = c001[c] + (c101[c] - c001[c]) * tr;
            let x11 = c011[c] + (c111[c] - c011[c]) * tr;
            let y0 = x00 + (x10 - x00) * tg;
            let y1 = x01 + (x11 - x01) * tg;
            p[c] = ((y0 + (y1 - y0) * tb) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
}

/// The straightforward per-pixel version, kept as the reference the fast
/// path is checked against.
#[cfg(test)]
fn apply_lut_reference(px: &mut [u8], lut: &[[f32; 3]], n: usize) {
    let d = (n - 1) as f32;
    let at = |r: usize, g: usize, b: usize| lut[(b * n + g) * n + r];
    for p in px.chunks_mut(3) {
        let (fr, fg, fb) = (p[0] as f32 / 255.0 * d, p[1] as f32 / 255.0 * d, p[2] as f32 / 255.0 * d);
        let (r0, g0, b0) = (fr.floor() as usize, fg.floor() as usize, fb.floor() as usize);
        let (r1, g1, b1) = ((r0 + 1).min(n - 1), (g0 + 1).min(n - 1), (b0 + 1).min(n - 1));
        let (tr, tg, tb) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);
        for c in 0..3 {
            let l = |r, g, b| at(r, g, b)[c];
            let x00 = l(r0, g0, b0) * (1.0 - tr) + l(r1, g0, b0) * tr;
            let x10 = l(r0, g1, b0) * (1.0 - tr) + l(r1, g1, b0) * tr;
            let x01 = l(r0, g0, b1) * (1.0 - tr) + l(r1, g0, b1) * tr;
            let x11 = l(r0, g1, b1) * (1.0 - tr) + l(r1, g1, b1) * tr;
            let y0 = x00 * (1.0 - tg) + x10 * tg;
            let y1 = x01 * (1.0 - tg) + x11 * tg;
            p[c] = ((y0 * (1.0 - tb) + y1 * tb) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
}

/// LUT size used everywhere: 33 is the common grading size, fine enough that
/// trilinear steps are invisible.
pub const LUT_N: usize = 33;

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene-like test frame: sky gradient top, darker ground bottom.
    fn scene(tint: [f32; 3], shadow_tint: [f32; 3], lift: f32) -> Vec<u8> {
        let (w, h) = (160usize, 90usize);
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h { for x in 0..w {
            let base = if y < h / 2 { 0.55 + 0.4 * (1.0 - y as f32 / (h / 2) as f32) } else { 0.08 + 0.3 * ((x as f32 / 9.0).sin() * 0.5 + 0.5) };
            let bright = base > 0.5;
            let t = if bright { tint } else { shadow_tint };
            for c in 0..3 { v.push(((base * t[c] + lift).clamp(0.0, 1.0) * 255.0) as u8); }
        }}
        v
    }

    #[test]
    fn lab_round_trips() {
        for c in [[0.0f32, 0.0, 0.0], [1.0, 1.0, 1.0], [0.8, 0.2, 0.5], [0.1, 0.6, 0.9]] {
            let back = lab_to_rgb(rgb_to_lab(c));
            for k in 0..3 { assert!((back[k] - c[k]).abs() < 2e-3, "{c:?} -> {back:?}"); }
        }
    }

    #[test]
    fn a_clip_matched_to_itself_is_unchanged() {
        let px = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let s = stats(&px);
        let t = Transfer::fit(&s, &s, 1.0);
        let mut out = px.clone();
        apply_lut(&mut out, &t.lut(LUT_N), LUT_N);
        let worst = px.iter().zip(&out).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 3, "identity drifted by {worst} levels");
    }

    #[test]
    fn a_split_tone_travels_band_by_band() {
        // reference: magenta highlights over warm shadows, brighter -- the pink look
        let reference = scene([1.0, 0.78, 1.0], [1.0, 0.8, 0.6], 0.08);
        let target = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let (sr, st) = (stats(&reference), stats(&target));
        let t = Transfer::fit(&st, &sr, 1.0);
        let mut out = target.clone();
        apply_lut(&mut out, &t.lut(LUT_N), LUT_N);
        let so = stats(&out);
        // highlights move toward magenta (a up, b down), shadows toward warm (a and b up)
        assert!(so.band[2][0] > st.band[2][0] + 3.0 && so.band[2][1] < st.band[2][1] - 1.0,
                "highlights: {:?} -> {:?} (reference {:?})", st.band[2], so.band[2], sr.band[2]);
        assert!(so.band[0][0] > st.band[0][0] + 1.0 && so.band[0][1] > st.band[0][1] + 2.0,
                "shadows: {:?} -> {:?} (reference {:?})", st.band[0], so.band[0], sr.band[0]);
        // and the tone follows: median lightness moves most of the way to the reference
        let gap0 = (sr.q[4] - st.q[4]).abs();
        let gap1 = (sr.q[4] - so.q[4]).abs();
        assert!(gap1 < gap0 * 0.4, "median L: target {} ref {} result {}", st.q[4], sr.q[4], so.q[4]);
    }

    #[test]
    fn black_and_white_stay_neutral() {
        let reference = scene([1.0, 0.78, 1.0], [1.0, 0.8, 0.6], 0.08);
        let target = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let t = Transfer::fit(&stats(&target), &stats(&reference), 1.0);
        for c in [[0.0f32, 0.0, 0.0], [1.0, 1.0, 1.0]] {
            let o = t.apply_rgb(c);
            let lab = rgb_to_lab(o);
            assert!(lab[1].abs() < 1.0 && lab[2].abs() < 1.0, "{c:?} picked up a tint: {o:?} (Lab {lab:?})");
        }
    }

    #[test]
    fn strength_blends_and_the_effect_round_trips() {
        let reference = scene([1.0, 0.78, 1.0], [1.0, 0.8, 0.6], 0.08);
        let target = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let t = Transfer::fit(&stats(&target), &stats(&reference), 0.5);
        assert_eq!(Transfer::of(&[t.effect()]), Some(t));
        let zero = Transfer { strength: 0.0, ..t };
        let mut out = target.clone();
        apply_lut(&mut out, &zero.lut(LUT_N), LUT_N);
        assert!(target.iter().zip(&out).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 3), "strength 0 is the clip as it was");
    }

    #[test]
    fn the_fast_lut_path_matches_the_reference_on_every_colour() {
        let reference = scene([1.0, 0.78, 1.0], [1.0, 0.8, 0.6], 0.08);
        let target = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let lut = Transfer::fit(&stats(&target), &stats(&reference), 1.0).lut(LUT_N);
        // every channel value at least once, and a frame big enough to take
        // the threaded path where there are cores
        let mut px: Vec<u8> = (0..700_000u32).flat_map(|i| [(i % 256) as u8, ((i / 7) % 256) as u8, ((i / 49) % 256) as u8]).collect();
        let mut want = px.clone();
        apply_lut_reference(&mut want, &lut, LUT_N);
        apply_lut(&mut px, &lut, LUT_N);
        let worst = px.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "fast path differs from the reference by {worst} levels");
    }

    #[test]
    fn the_export_matches_the_preview() {
        if !crate::testkit::ffmpeg_or_skip("style::the_export_matches_the_preview") { return; }
        let reference = scene([1.0, 0.78, 1.0], [1.0, 0.8, 0.6], 0.08);
        let target = scene([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.0);
        let t = Transfer::fit(&stats(&target), &stats(&reference), 1.0);
        let mut want = target.clone();
        apply_lut(&mut want, &t.lut(LUT_N), LUT_N);
        let cube = std::env::temp_dir().join(format!("style-{}.cube", std::process::id()));
        std::fs::write(&cube, t.cube(LUT_N)).unwrap();
        let mut child = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "160x90", "-i", "-",
                   "-vf", &format!("lut3d=file='{}':interp=trilinear", cube.display()),
                   "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&target).unwrap();
        let got = child.wait_with_output().unwrap().stdout;
        let _ = std::fs::remove_file(&cube);
        assert_eq!(got.len(), want.len());
        let worst = got.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 2, "export and preview differ by up to {worst} levels");
    }
}
