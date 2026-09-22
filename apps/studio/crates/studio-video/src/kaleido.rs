//! kaleido — a kaleidoscope, as a clip effect.
//!
//! The picture is folded into `segments` mirrored wedges around a centre: each
//! output pixel is turned into polar form around the centre, its angle folded
//! into the first wedge (and mirrored within it, as real kaleidoscope mirrors
//! do), and the picture is sampled there. Two segments is a mirror, six is the
//! classic look, twelve is lace.
//!
//! ## One formula, two places
//!
//! The preview runs [`apply`] on each frame it shows; the export cannot run
//! Rust per pixel inside ffmpeg, so [`geq_filter`] writes the same maths as an
//! ffmpeg `geq` expression. Two implementations of one idea drift unless
//! something holds them together; the test `the_export_matches_the_preview`
//! renders a frame both ways and compares them. Studio's export ignored clip
//! effects entirely until recently, and the way it was fixed -- one parser, a
//! parity test -- is the way this is built.
//!
//! Carried as an effect of kind `"kaleidoscope"` on the clip (params
//! `segments`, `angle` in degrees, `cx`/`cy` as 0..1 fractions of the frame),
//! so it syncs, undoes and saves like any other effect, and a build that does
//! not know it simply plays the clip plain.

use studio_core::Effect;

pub const KIND: &str = "kaleidoscope";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kaleido {
    /// Mirrored wedges around the centre; 2 is a single mirror.
    pub segments: u32,
    /// Rotation of the wedge pattern, degrees.
    pub angle: f32,
    /// Centre, as fractions of the frame.
    pub cx: f32,
    pub cy: f32,
}

impl Default for Kaleido {
    fn default() -> Kaleido { Kaleido { segments: 6, angle: 0.0, cx: 0.5, cy: 0.5 } }
}

impl Kaleido {
    /// The last enabled kaleidoscope on a clip, if any.
    pub fn of(effects: &[Effect]) -> Option<Kaleido> {
        effects.iter().filter(|e| e.enabled && e.kind == KIND).last().map(|e| {
            let g = |k: &str, d: f32| e.params.get(k).copied().unwrap_or(d);
            Kaleido {
                segments: (g("segments", 6.0).round() as i64).clamp(2, 48) as u32,
                angle: g("angle", 0.0),
                cx: g("cx", 0.5).clamp(0.0, 1.0),
                cy: g("cy", 0.5).clamp(0.0, 1.0),
            }
        })
    }

    pub fn effect(&self) -> Effect {
        Effect {
            kind: KIND.into(),
            enabled: true,
            params: [("segments", self.segments as f32), ("angle", self.angle), ("cx", self.cx), ("cy", self.cy)]
                .into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    /// Where output pixel (x, y) samples from, in a `w`x`h` frame.
    #[inline]
    pub fn source_of(&self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
        let (cx, cy) = (self.cx * w, self.cy * h);
        let phi = self.angle.to_radians();
        let wedge = std::f32::consts::TAU / self.segments as f32;
        let (dx, dy) = (x - cx, y - cy);
        let r = dx.hypot(dy);
        let a = dy.atan2(dx) - phi;
        let mut m = a.rem_euclid(wedge);
        if m > wedge / 2.0 { m = wedge - m; }  // the mirror inside each wedge
        let a2 = m + phi;
        (cx + r * a2.cos(), cy + r * a2.sin())
    }
}

/// Fold an 8-bit RGB frame. Nearest-neighbour, like `geq`'s own sampling, so
/// the preview and the export pick the same source pixels.
pub fn apply(px: &[u8], w: u32, h: u32, k: &Kaleido) -> Vec<u8> {
    let mut out = vec![0u8; px.len()];
    let (wf, hf) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = k.source_of(x as f32, y as f32, wf, hf);
            let sx = (sx.round() as i64).clamp(0, w as i64 - 1) as usize;
            let sy = (sy.round() as i64).clamp(0, h as i64 - 1) as usize;
            let (o, s) = (((y * w + x) * 3) as usize, (sy * w as usize + sx) * 3);
            out[o..o + 3].copy_from_slice(&px[s..s + 3]);
        }
    }
    out
}

/// The same fold as an ffmpeg filter fragment, for a frame of `w`x`h`. Expects
/// and leaves RGB (`gbrp` inside, as `geq`'s RGB mode needs).
pub fn geq_filter(k: &Kaleido, w: u32, h: u32) -> String {
    let (cx, cy) = (k.cx * w as f32, k.cy * h as f32);
    let phi = k.angle.to_radians();
    let wedge = std::f32::consts::TAU / k.segments as f32;
    // st/ld registers: 0 dx, 1 dy, 2 r, 3 angle, 4 folded angle, 5 final angle
    let e = |ch: &str| format!(
        "st(0,X-{cx});st(1,Y-{cy});st(2,hypot(ld(0),ld(1)));st(3,atan2(ld(1),ld(0))-{phi});\
st(4,mod(mod(ld(3),{wedge})+{wedge},{wedge}));st(4,if(gt(ld(4),{half}),{wedge}-ld(4),ld(4)));\
st(5,ld(4)+{phi});{ch}(clip(round({cx}+ld(2)*cos(ld(5))),0,{mx}),clip(round({cy}+ld(2)*sin(ld(5))),0,{my}))",
        half = wedge / 2.0, mx = w - 1, my = h - 1);
    format!("format=gbrp,geq=r='{}':g='{}':b='{}',format=rgb24", e("r"), e("g"), e("b"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with no symmetry of its own: a colour ramp plus a bright off-centre block.
    fn frame(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 3) as usize];
        for y in 0..h { for x in 0..w {
            let i = ((y * w + x) * 3) as usize;
            v[i] = (x * 255 / w) as u8; v[i + 1] = (y * 255 / h) as u8; v[i + 2] = ((x ^ y) & 0xff) as u8;
            if x > w * 3 / 5 && x < w * 4 / 5 && y > h / 5 && y < h * 2 / 5 { v[i] = 255; v[i + 1] = 255; v[i + 2] = 40; }
        }}
        v
    }

    #[test]
    fn the_folded_picture_repeats_every_wedge() {
        // Rotating the output by one wedge about the centre lands on the same
        // picture: that IS a kaleidoscope. Checked on points away from the
        // edges, where clamping does not apply.
        let (w, h) = (200u32, 200u32);
        let k = Kaleido { segments: 6, angle: 10.0, cx: 0.5, cy: 0.5 };
        let out = apply(&frame(w, h), w, h, &k);
        let wedge = std::f32::consts::TAU / 6.0;
        let mut same = 0; let mut total = 0;
        for &(r, a) in &[(30.0f32, 0.3f32), (50.0, 1.1), (70.0, 2.0), (40.0, 4.0), (60.0, 5.5)] {
            let p = |a: f32| {
                let (x, y) = (100.0 + r * a.cos(), 100.0 + r * a.sin());
                let i = ((y.round() as u32 * w + x.round() as u32) * 3) as usize; [out[i], out[i + 1], out[i + 2]]
            };
            total += 1;
            let (p0, p1) = (p(a), p(a + wedge));
            if p0.iter().zip(&p1).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 12) { same += 1; }
        }
        assert!(same >= total - 1, "{same}/{total} points repeat one wedge round");
    }

    #[test]
    fn two_segments_is_a_mirror() {
        let (w, h) = (120u32, 80u32);
        let k = Kaleido { segments: 2, angle: 90.0, cx: 0.5, cy: 0.5 };
        let out = apply(&frame(w, h), w, h, &k);
        // mirrored about the vertical centre line: column x and w-1-x agree
        let mut bad = 0;
        for y in 5..h - 5 { for x in 5..w / 2 - 2 {
            let (a, b) = (((y * w + x) * 3) as usize, ((y * w + (w - 1 - x)) * 3) as usize);
            if (out[a] as i32 - out[b] as i32).abs() > 12 { bad += 1; }
        }}
        assert!(bad < 40, "{bad} pixels break the mirror");
    }

    #[test]
    fn settings_read_back_and_are_clamped() {
        let k = Kaleido { segments: 8, angle: 22.5, cx: 0.3, cy: 0.6 };
        assert_eq!(Kaleido::of(&[k.effect()]), Some(k));
        let mut wild = k.effect();
        wild.params.insert("segments".into(), 900.0);
        wild.params.insert("cx".into(), -2.0);
        let r = Kaleido::of(&[wild]).unwrap();
        assert_eq!((r.segments, r.cx), (48, 0.0));
        let mut off = k.effect(); off.enabled = false;
        assert_eq!(Kaleido::of(&[off]), None, "disabled means plain");
    }

    /// The export runs the geq expression; the preview runs `apply`. Render one
    /// frame both ways and compare. Nearest sampling on both sides, so the
    /// only disagreements are pixels exactly on a fold, where float rounding
    /// can pick the neighbour.
    #[test]
    fn the_export_matches_the_preview() {
        if !crate::testkit::ffmpeg_or_skip("the_export_matches_the_preview") { return; }
        let (w, h) = (160u32, 90u32);
        let src = frame(w, h);
        for k in [Kaleido::default(), Kaleido { segments: 8, angle: 17.0, cx: 0.4, cy: 0.55 },
                  Kaleido { segments: 3, angle: -40.0, cx: 0.6, cy: 0.4 }] {
            let want = apply(&src, w, h, &k);
            let mut child = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{w}x{h}"), "-i", "-",
                       "-vf", &geq_filter(&k, w, h), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
                .spawn().expect("ffmpeg");
            use std::io::Write;
            child.stdin.take().unwrap().write_all(&src).unwrap();
            let got = child.wait_with_output().unwrap().stdout;
            assert_eq!(got.len(), want.len(), "ffmpeg returned a frame");
            let off = got.chunks(3).zip(want.chunks(3))
                .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| (*x as i32 - *y as i32).abs() > 8)).count();
            let share = off as f32 / (w * h) as f32;
            assert!(share < 0.02, "{k:?}: {:.1}% of pixels differ between export and preview", share * 100.0);
        }
    }
}
