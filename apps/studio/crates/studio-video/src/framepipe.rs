//! framepipe — decode, touch every pixel, encode, one frame at a time.
//!
//! Studio's picture work so far is either a filter string handed to ffmpeg
//! ([`crate::edit`]) or a single frame pulled for the viewport. Neither can
//! express the things a film actually needs from a touch-up pass: a
//! stabilizer's correction is a DIFFERENT warp on every frame, and a
//! temporal clean-plate reads several frames to write one. Both need frames
//! in memory, in order, under Rust's control.
//!
//! That is all this is: ffmpeg decodes into a pipe, a closure sees each
//! frame, ffmpeg encodes from another pipe. It is deliberately not a
//! "renderer" — no timeline, no effects graph — because its whole value is
//! being the one place where per-frame pixel work happens, so stabilize,
//! the interruption filter and grain all get the same streaming, bounded,
//! interruptible loop instead of three private ones.
//!
//! ## Memory is bounded on purpose
//!
//! One frame in flight for [`Pipe::map`]; `radius` frames for
//! [`Pipe::map_window`], which is what a temporal filter needs. A 4K RGB
//! frame is 24 MB, so a window of 13 is 300 MB — the reason the window is a
//! parameter and the reason analysis work should run at
//! [`Size::analysis`] rather than full size. Nothing here ever holds a whole
//! shot.
//!
//! ## Speed
//!
//! Decoding dominates. Measured in this repo's sandbox (one core, AV1 4K
//! proxies): the motion search runs at ~210 fps on 960×540 grey frames while
//! decode delivers far fewer, which is why [`Pipe::gray`] exists — grey
//! analysis frames at analysis size, decoded once, shared by every pass that
//! only needs to MEASURE.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::slate_ocr::Gray;

/// A decoded frame: 8-bit RGB, row-major, `w * h * 3` bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Frame {
    pub fn black(w: u32, h: u32) -> Frame { Frame { w, h, px: vec![0; (w * h * 3) as usize] } }

    #[inline]
    pub fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y.min(self.h - 1) * self.w + x.min(self.w - 1)) * 3) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, v: [u8; 3]) {
        let i = ((y * self.w + x) * 3) as usize;
        self.px[i..i + 3].copy_from_slice(&v);
    }

    /// Rec.709 luma, for analysis passes that only need structure.
    pub fn to_gray(&self) -> Gray {
        let mut g = Gray::new(self.w as usize, self.h as usize);
        for (o, p) in g.px.iter_mut().zip(self.px.chunks(3)) {
            *o = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) as u8;
        }
        g
    }

    /// Shift by (dx, dy) pixels and zoom about the centre, bilinear, edges
    /// held. This is what a [`crate::stabilize::Correction`] means in pixels.
    ///
    /// The hot loop of a stabilize render: at 960×540 the naive version
    /// (a bounds-checked `at()` per corner, per channel) ran the whole
    /// render at 29 fps against 73 fps for decode-and-encode alone, so the
    /// warp cost more than the codecs. This walks each output row with the
    /// source x stepping incrementally, hoists the row clamping, and takes
    /// a straight copy when the warp is a whole-pixel shift at zoom 1.
    pub fn warp(&self, dx: f32, dy: f32, zoom: f32) -> Frame {
        self.warp_pose(dx, dy, zoom, 0.0)
    }

    /// [`Frame::warp`] with a roll about the centre (radians, positive
    /// anticlockwise) — what a [`crate::stabilize::Correction`] carries when
    /// the path was measured with roll. Kept separate so the common,
    /// rotation-free case keeps its row-stepping fast path.
    pub fn warp_pose(&self, dx: f32, dy: f32, zoom: f32, rot: f32) -> Frame {
        if rot.abs() > 1e-6 {
            let (w, h) = (self.w as usize, self.h as usize);
            let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
            let z = if zoom.abs() < 1e-6 { 1.0 } else { zoom };
            let (cos, sin) = ((-rot).cos(), (-rot).sin());
            let mut out = Frame::black(self.w, self.h);
            for y in 0..h {
                for x in 0..w {
                    // destination -> source: unzoom, unrotate, unshift
                    let (ux, uy) = ((x as f32 + 0.5 - cx) / z, (y as f32 + 0.5 - cy) / z);
                    let sx = ux * cos - uy * sin - dx + cx - 0.5;
                    let sy = ux * sin + uy * cos - dy + cy - 0.5;
                    let (x0f, y0f) = (sx.floor(), sy.floor());
                    let (fx, fy) = (sx - x0f, sy - y0f);
                    let cl = |v: f32, hi: usize| (v as isize).clamp(0, hi as isize - 1) as usize;
                    let (xa, xb) = (cl(x0f, w) * 3, cl(x0f + 1.0, w) * 3);
                    let (ya, yb) = (cl(y0f, h) * w * 3, cl(y0f + 1.0, h) * w * 3);
                    let o = (y * w + x) * 3;
                    for c in 0..3 {
                        let top = self.px[ya + xa + c] as f32 * (1.0 - fx) + self.px[ya + xb + c] as f32 * fx;
                        let bot = self.px[yb + xa + c] as f32 * (1.0 - fx) + self.px[yb + xb + c] as f32 * fx;
                        out.px[o + c] = (top * (1.0 - fy) + bot * fy + 0.5) as u8;
                    }
                }
            }
            return out;
        }
        self.warp_translate(dx, dy, zoom)
    }

    fn warp_translate(&self, dx: f32, dy: f32, zoom: f32) -> Frame {
        let mut out = Frame::black(self.w, self.h);
        let (w, h) = (self.w as usize, self.h as usize);
        let z = if zoom.abs() < 1e-6 { 1.0 } else { zoom };
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        // whole-pixel shift, no zoom: copy rows
        if (z - 1.0).abs() < 1e-6 && (dx - dx.round()).abs() < 1e-4 && (dy - dy.round()).abs() < 1e-4 {
            let (sx, sy) = (dx.round() as isize, dy.round() as isize);
            for y in 0..h {
                let src_y = (y as isize - sy).clamp(0, h as isize - 1) as usize;
                for x in 0..w {
                    let src_x = (x as isize - sx).clamp(0, w as isize - 1) as usize;
                    let (a, b) = ((y * w + x) * 3, (src_y * w + src_x) * 3);
                    out.px[a..a + 3].copy_from_slice(&self.px[b..b + 3]);
                }
            }
            return out;
        }
        let step = 1.0 / z;
        for y in 0..h {
            let sy = (y as f32 + 0.5 - cy) / z - dy + cy - 0.5;
            let y0f = sy.floor();
            let fy = sy - y0f;
            let ya = (y0f as isize).clamp(0, h as isize - 1) as usize * w * 3;
            let yb = ((y0f as isize) + 1).clamp(0, h as isize - 1) as usize * w * 3;
            let mut sx = (0.5 - cx) / z - dx + cx - 0.5;
            let row = y * w * 3;
            for x in 0..w {
                let x0f = sx.floor();
                let fx = sx - x0f;
                let xa = (x0f as isize).clamp(0, w as isize - 1) as usize * 3;
                let xb = ((x0f as isize) + 1).clamp(0, w as isize - 1) as usize * 3;
                let (i00, i10) = (ya + xa, ya + xb);
                let (i01, i11) = (yb + xa, yb + xb);
                let o = row + x * 3;
                for c in 0..3 {
                    let top = self.px[i00 + c] as f32 * (1.0 - fx) + self.px[i10 + c] as f32 * fx;
                    let bot = self.px[i01 + c] as f32 * (1.0 - fx) + self.px[i11 + c] as f32 * fx;
                    out.px[o + c] = (top * (1.0 - fy) + bot * fy + 0.5) as u8;
                }
                sx += step;
            }
        }
        out
    }
}

/// The size frames are decoded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    /// The size measurement passes should use: about 960 px wide, keeping
    /// the source's shape, both dimensions even.
    pub fn analysis(src_w: u32, src_h: u32) -> Size {
        let w = 960.min(src_w.max(2));
        let h = ((w as f32 * src_h as f32 / src_w.max(1) as f32).round() as u32).max(2);
        Size { w: w + w % 2, h: h + h % 2 }
    }
}

/// What to read, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Pipe {
    pub input: PathBuf,
    /// Seconds into the file, and how long. `None` for the whole file.
    pub range: Option<(f64, f64)>,
    pub size: Size,
    pub fps: f64,
}

fn spawn_decoder(p: &Pipe, gray: bool) -> std::io::Result<Child> {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error", "-nostdin"]);
    if let Some((at, _)) = p.range { c.args(["-ss", &format!("{at}")]); }
    c.arg("-i").arg(&p.input);
    if let Some((_, len)) = p.range { c.args(["-t", &format!("{len}")]); }
    c.args(["-vf", &format!("scale={}:{}:flags=bicubic,fps={}", p.size.w, p.size.h, p.fps),
            "-f", "rawvideo", "-pix_fmt", if gray { "gray" } else { "rgb24" }, "-"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).spawn()
}

fn spawn_encoder(p: &Pipe, out: &Path, crf: u32) -> std::io::Result<Child> {
    Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-nostdin", "-f", "rawvideo", "-pix_fmt", "rgb24",
               "-s", &format!("{}x{}", p.size.w, p.size.h), "-r", &format!("{}", p.fps), "-i", "-",
               "-c:v", "libx264", "-preset", "veryfast", "-crf", &crf.to_string(), "-pix_fmt", "yuv420p"])
        .arg(out)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
}

fn finish(child: &mut Child, what: &str) -> Result<(), String> {
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() { let _ = e.read_to_string(&mut err); }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{what} exited {s}: {}", err.trim())),
        Err(e) => Err(format!("{what} failed: {e}")),
    }
}

impl Pipe {
    /// Every frame, in order, as grey — the cheap path for measurement
    /// passes ([`crate::stabilize::measure`], scene detection).
    pub fn gray(&self) -> Result<Vec<Gray>, String> {
        let mut dec = spawn_decoder(self, true).map_err(|e| format!("ffmpeg: {e}"))?;
        let mut stdout = dec.stdout.take().expect("piped");
        let n = (self.size.w * self.size.h) as usize;
        let mut buf = vec![0u8; n];
        let mut out = Vec::new();
        loop {
            match stdout.read_exact(&mut buf) {
                Ok(()) => out.push(Gray { w: self.size.w as usize, h: self.size.h as usize, px: buf.clone() }),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("decode: {e}")),
            }
        }
        finish(&mut dec, "decoder")?;
        Ok(out)
    }

    /// Decode, hand each frame to `f`, encode what it returns. One frame is
    /// in memory at a time.
    pub fn map(&self, out: &Path, crf: u32, mut f: impl FnMut(usize, Frame) -> Frame) -> Result<usize, String> {
        self.map_window(out, crf, 0, |i, w| f(i, w[w.len() / 2].clone()))
    }

    /// Like [`Pipe::map`], but `f` sees a window of `2·radius + 1` frames
    /// centred on the current one (clamped at the ends), which is what a
    /// temporal filter — a clean plate built from neighbouring frames —
    /// needs. Memory is `2·radius + 1` frames, no more.
    pub fn map_window(&self, out: &Path, crf: u32, radius: usize,
                      mut f: impl FnMut(usize, &[Frame]) -> Frame) -> Result<usize, String> {
        let mut dec = spawn_decoder(self, false).map_err(|e| format!("ffmpeg decode: {e}"))?;
        let mut enc = spawn_encoder(self, out, crf).map_err(|e| format!("ffmpeg encode: {e}"))?;
        let mut stdout = dec.stdout.take().expect("piped");
        let mut stdin = enc.stdin.take().expect("piped");
        let n = (self.size.w * self.size.h * 3) as usize;
        let mut buf_px = vec![0u8; n];
        // Frame i can only be written once frame i+radius has been read, so
        // the buffer holds the last 2·radius+1 frames and `next` trails the
        // read head by `radius`. At EOF the remaining frames are written
        // against a shortened window — they must not be dropped, or a
        // windowed pass would return a clip shorter than it was given.
        let mut buf: std::collections::VecDeque<Frame> = std::collections::VecDeque::new();
        let mut read_count = 0usize;   // frames read so far
        let mut next = 0usize;         // next frame index to write
        let mut written = 0usize;
        let mut write = |buf: &std::collections::VecDeque<Frame>, base: usize, i: usize,
                         f: &mut dyn FnMut(usize, &[Frame]) -> Frame,
                         stdin: &mut std::process::ChildStdin| -> Result<(), String> {
            let lo = i.saturating_sub(radius).max(base) - base;
            let hi = (i + radius + 1 - base).min(buf.len());
            let window: Vec<Frame> = buf.iter().skip(lo).take(hi - lo).cloned().collect();
            let got = f(i, &window);
            if got.w != self.size.w || got.h != self.size.h {
                return Err(format!("frame {i}: closure returned {}x{}, pipe is {}x{}",
                                   got.w, got.h, self.size.w, self.size.h));
            }
            stdin.write_all(&got.px).map_err(|e| format!("encode write: {e}"))
        };
        loop {
            match stdout.read_exact(&mut buf_px) {
                Ok(()) => {
                    buf.push_back(Frame { w: self.size.w, h: self.size.h, px: buf_px.clone() });
                    read_count += 1;
                    if buf.len() > 2 * radius + 1 { buf.pop_front(); }
                    // everything whose forward half has arrived
                    while next + radius < read_count {
                        write(&buf, read_count - buf.len(), next, &mut f, &mut stdin)?;
                        next += 1;
                        written += 1;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("decode read: {e}")),
            }
        }
        while next < read_count {
            write(&buf, read_count - buf.len(), next, &mut f, &mut stdin)?;
            next += 1;
            written += 1;
        }
        drop(stdin);
        finish(&mut dec, "decoder")?;
        finish(&mut enc, "encoder")?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ffmpeg_or_skip, shoot};

    fn pipe(path: &Path, w: u32, h: u32, fps: f64) -> Pipe {
        Pipe { input: path.into(), range: None, size: Size { w, h }, fps }
    }

    #[test]
    fn analysis_size_keeps_the_shape_and_stays_even() {
        assert_eq!(Size::analysis(3840, 2160), Size { w: 960, h: 540 });
        assert_eq!(Size::analysis(1920, 800), Size { w: 960, h: 400 });
        let s = Size::analysis(640, 480);
        assert_eq!((s.w, s.h), (640, 480), "never upscales past the source");
        assert_eq!(Size::analysis(3840, 1607).h % 2, 0, "even for the encoder");
    }

    /// The straightforward version this file's fast path must agree with.
    fn warp_reference(f: &Frame, dx: f32, dy: f32, zoom: f32) -> Frame {
        let mut out = Frame::black(f.w, f.h);
        let (cx, cy) = (f.w as f32 / 2.0, f.h as f32 / 2.0);
        let z = if zoom.abs() < 1e-6 { 1.0 } else { zoom };
        for y in 0..f.h {
            for x in 0..f.w {
                let sx = (x as f32 + 0.5 - cx) / z - dx + cx - 0.5;
                let sy = (y as f32 + 0.5 - cy) / z - dy + cy - 0.5;
                let (x0, y0) = (sx.floor(), sy.floor());
                let (fx, fy) = (sx - x0, sy - y0);
                let cl = |v: f32, hi: u32| v.clamp(0.0, (hi - 1) as f32) as u32;
                let (xa, ya) = (cl(x0, f.w), cl(y0, f.h));
                let (xb, yb) = (cl(x0 + 1.0, f.w), cl(y0 + 1.0, f.h));
                let (p00, p10, p01, p11) = (f.at(xa, ya), f.at(xb, ya), f.at(xa, yb), f.at(xb, yb));
                let mut v = [0u8; 3];
                for c in 0..3 {
                    let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
                    let bot = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
                    v[c] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
                }
                out.set(x, y, v);
            }
        }
        out
    }

    #[test]
    fn the_fast_warp_agrees_with_the_straightforward_one() {
        let mut f = Frame::black(40, 24);
        for y in 0..24 { for x in 0..40 {
            f.set(x, y, [(x * 6) as u8, (y * 10) as u8, ((x + y) * 4) as u8]);
        }}
        for &(dx, dy, z) in &[(3.0f32, -2.0f32, 1.0f32), (1.5, 0.25, 1.0), (0.0, 0.0, 1.2),
                              (-4.25, 3.75, 1.08), (0.0, 0.0, 1.0)] {
            let (fast, slow) = (f.warp(dx, dy, z), warp_reference(&f, dx, dy, z));
            let worst = fast.px.iter().zip(&slow.px).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(worst <= 1, "({dx}, {dy}, {z}): off by {worst}");
        }
    }

    #[test]
    fn warp_moves_the_picture_by_the_pixels_it_is_told_to() {
        let mut f = Frame::black(16, 16);
        f.set(4, 6, [255, 0, 0]);
        let moved = f.warp(3.0, -2.0, 1.0);
        assert_eq!(moved.at(7, 4), [255, 0, 0], "the dot moved by (+3, -2)");
        assert_eq!(moved.at(4, 6), [0, 0, 0]);
        // a half-pixel shift splits the dot between neighbours, it does not vanish
        let half = f.warp(0.5, 0.0, 1.0);
        assert!(half.at(4, 6)[0] > 100 && half.at(5, 6)[0] > 100, "{:?} {:?}", half.at(4, 6), half.at(5, 6));
        // zoom about the centre keeps the centre still
        let mut c = Frame::black(16, 16);
        c.set(8, 8, [0, 255, 0]);
        // >120, not >200: at zoom 2 the dot's source lands ¾ of a pixel off
        // the sample point, so bilinear splits it — it stays put, dimmer.
        assert!(c.warp(0.0, 0.0, 2.0).at(8, 8)[1] > 120, "{:?}", c.warp(0.0, 0.0, 2.0).at(8, 8));
    }

    #[test]
    fn frames_go_through_ffmpeg_and_come_back_changed() {
        if !ffmpeg_or_skip("frames_go_through_ffmpeg_and_come_back_changed") { return; }
        let s = shoot(&[1.0], 4.0);
        let take = &s.takes[0];
        let p = pipe(&take.video, 64, 36, 12.0);
        let out = s.dir.join(format!("piped-{}.mp4", std::process::id()));
        // swap red and blue on every frame
        let n = p.map(&out, 18, |_, mut f| {
            for px in f.px.chunks_mut(3) { px.swap(0, 2); }
            f
        }).expect("pipe runs");
        assert!(n >= 45 && n <= 49, "4 s at 12 fps: {n} frames");
        let got = crate::frame_rgb(&out, studio_core::Ticks::from_seconds(2.0), 64, 36).unwrap();
        let i = (18 * 64 + 32) * 3;
        assert!((got[i] as i32 - take.rgb[2] as i32).abs() <= 8
                && (got[i + 2] as i32 - take.rgb[0] as i32).abs() <= 8,
                "channels swapped: {:?} from source {:?}", &got[i..i + 3], take.rgb);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn a_window_sees_its_neighbours_and_no_frame_is_lost() {
        if !ffmpeg_or_skip("a_window_sees_its_neighbours_and_no_frame_is_lost") { return; }
        let s = shoot(&[1.0], 4.0);
        let p = pipe(&s.takes[0].video, 64, 36, 12.0);
        let out = s.dir.join(format!("win-{}.mp4", std::process::id()));
        let mut widths = Vec::new();
        let n = p.map_window(&out, 18, 2, |i, w| {
            widths.push((i, w.len()));
            w[w.len() / 2].clone()
        }).expect("pipe runs");
        assert_eq!(n, widths.len());
        assert!(widths.iter().all(|(_, l)| *l <= 5), "window never exceeds 2·radius+1: {widths:?}");
        assert!(widths.iter().filter(|(_, l)| *l == 5).count() > n - 5, "most frames get the full window");
        assert_eq!(widths.iter().map(|(i, _)| *i).collect::<Vec<_>>(), (0..n).collect::<Vec<_>>(),
                   "indices are dense and in order");
        // Against the INPUT count, not against n: comparing the output with
        // the number of frames the closure happened to be called for let a
        // broken tail (which drops the last `radius` frames) pass.
        let went_in = p.gray().unwrap().len();
        assert_eq!(n, went_in, "a windowed pass must not shorten the clip");
        let decoded = pipe(&out, 64, 36, 12.0).gray().unwrap().len();
        assert!(decoded.abs_diff(went_in) <= 1, "{decoded} frames out for {went_in} in");
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn a_shaky_shot_comes_out_steadier_than_it_went_in() {
        if !ffmpeg_or_skip("a_shaky_shot_comes_out_steadier_than_it_went_in") { return; }
        use crate::stabilize::{measure, solve, Budget, Flavor};
        let s = shoot(&[1.0], 4.0);
        // A shaky clip: the flat take, warped frame by frame.
        let shake: Vec<(f32, f32)> = (0..48).map(|i| {
            let t = i as f32;
            ((t * 0.9).sin() * 6.0 + t * 0.3, (t * 1.3).cos() * 4.0)
        }).collect();
        let p = pipe(&s.takes[0].video, 128, 72, 12.0);
        let shaky = s.dir.join(format!("shaky-{}.mp4", std::process::id()));
        // the flat colour of the fixture has nothing to track, so give the
        // frames a fixed pattern first, then shake that
        p.map(&shaky, 12, |i, mut f| {
            // Smooth, low-frequency texture. A hashed per-pixel pattern was
            // tried first: it aliases under bicubic scaling and the motion
            // search could not find it at all, so "stabilized" came out
            // exactly as shaky and the test measured nothing.
            for y in 0..f.h { for x in 0..f.w {
                let (fx, fy) = (x as f32, y as f32);
                let a = 128.0 + 90.0 * (fx / 9.0).sin() * (fy / 7.0).cos();
                let b = 128.0 + 90.0 * (fx / 13.0 + fy / 11.0).sin();
                f.set(x, y, [a as u8, b as u8, ((a + b) / 2.0) as u8]);
            }}
            let k = shake[i.min(shake.len() - 1)];
            f.warp(k.0, k.1, 1.0)
        }).expect("shaky clip");

        let q = pipe(&shaky, 128, 72, 12.0);
        let path = measure(&q.gray().expect("decodes"), 16);
        let sol = solve(&path, &Flavor::Smooth { sigma: 8.0 }, Budget { max_zoom: 1.3 }, 128, 72);
        let fixed = s.dir.join(format!("fixed-{}.mp4", std::process::id()));
        q.map(&fixed, 12, |i, f| {
            let c = sol.corrections[i.min(sol.corrections.len() - 1)];
            f.warp(c.dx, c.dy, sol.zoom)
        }).expect("stabilized clip");

        let jitter = |frames: &[Gray]| -> f32 {
            let p = measure(frames, 16);
            p.windows(3).map(|w| (w[2].x - 2.0 * w[1].x + w[0].x).abs() + (w[2].y - 2.0 * w[1].y + w[0].y).abs())
                .sum::<f32>() / (p.len() - 2) as f32
        };
        let before = jitter(&q.gray().unwrap());
        let after = jitter(&pipe(&fixed, 128, 72, 12.0).gray().unwrap());
        assert!(after < before * 0.5, "shake should be halved at least: {after} vs {before}");
        for f in [&shaky, &fixed] { let _ = std::fs::remove_file(f); }
    }
}
