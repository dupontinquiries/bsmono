//! focus — contrast-detection autofocus for a camera whose lens actuator the
//! pipeline never drives.
//!
//! # Why this exists
//!
//! Measured on a Fairphone 5 (qcom-camss, libcamera 0.7, software ISP): the
//! cameras expose only Saturation/Contrast/Gamma through libcamera — no
//! `AfMode`, no `LensPosition`. The voice-coil motor (`dw9719`) is a separate
//! V4L2 subdev with one control, `focus_absolute` 0..1023, and libcamera never
//! touches it. Sweeping it by hand moved the sharpness of the live image by
//! ~18% (JPEG size 357 KB at 128 vs 303 KB at 1023), so the hardware works and
//! nothing was asking it to. This module is the asking.
//!
//! Three parts, each testable without a camera:
//!
//! * [`sharpness`] — how crisp a region of a frame is.
//! * [`Search`] — a coarse sweep then a refine, driven by the caller one probe
//!   at a time. It REFUSES on a flat scene ([`Outcome::position`] is `None`)
//!   rather than move the lens to a coin flip.
//! * [`pair_actuators`] / [`sensor_for_camera_id`] — which subdev moves which
//!   camera. The media graph has no ancillary link for the motor on this
//!   kernel (`dw9719 … 0 links`), so the pairing is by I2C bus, and only when
//!   that is unambiguous.
//!
//! The caller owns timing: after [`set_focus`] the lens needs a settle (about
//! 150 ms) and the frames already in flight were captured at the OLD position,
//! so score a frame whose `seq` is at least two past the one current when the
//! move was issued.

use crate::RawFrame;
use std::process::Command;

/// Luma, integer BT.601-ish: `(77r + 150g + 29b) >> 8`.
fn luma(px: &[u8]) -> i64 { ((77 * px[0] as i64) + (150 * px[1] as i64) + (29 * px[2] as i64)) >> 8 }

/// How crisp the region `roi` (`[u0, v0, u1, v1]`, fractions of the frame) is:
/// mean squared 4-neighbour Laplacian of luma, divided by the squared mean
/// luma. The division is what keeps an exposure change from reading as a focus
/// change — a frame twice as bright has twice the gradients and the same
/// sharpness. `None` for a torn frame or a region too small to hold a pixel
/// with four neighbours.
pub fn sharpness(f: &RawFrame, roi: [f32; 4]) -> Option<f64> {
    if !f.pixels_plausible() { return None; }
    let (w, h) = (f.width as usize, f.height as usize);
    let px = |v: f32, n: usize| (v.clamp(0.0, 1.0) * n as f32).round() as usize;
    let (x0, y0) = (px(roi[0], w).max(1), px(roi[1], h).max(1));
    let (x1, y1) = (px(roi[2], w).min(w - 1), px(roi[3], h).min(h - 1));
    if x1 <= x0 || y1 <= y0 { return None; }
    let at = |x: usize, y: usize| luma(&f.rgba[(y * w + x) * 4..]);
    let (mut sum_sq, mut sum_l, mut n) = (0f64, 0f64, 0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let c = at(x, y);
            let lap = 4 * c - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1);
            sum_sq += (lap * lap) as f64;
            sum_l += c as f64;
            n += 1.0;
        }
    }
    let mean = sum_l / n;
    Some((sum_sq / n) / ((mean + 1.0) * (mean + 1.0)))
}

/// The ROI for a tap at `(u, v)` (fractions of the frame): a square of side
/// `size` (fraction of the SHORTER edge is not known here, so this is a
/// fraction of each axis) centred on the tap and slid back inside the frame.
pub fn roi_at(u: f32, v: f32, size: f32) -> [f32; 4] {
    let s = size.clamp(0.05, 1.0);
    let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let x0 = (u - s / 2.0).clamp(0.0, 1.0 - s);
    let y0 = (v - s / 2.0).clamp(0.0, 1.0 - s);
    [x0, y0, x0 + s, y0 + s]
}

/// What a finished search decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// Where to leave the lens, or `None` when the scene gave no usable peak
    /// (flat: a blank wall, a lens cap, the dark). The caller keeps the
    /// previous position — a refusal, not a guess.
    pub position: Option<i64>,
    /// Best score seen.
    pub peak: f64,
    /// Best score over the mean coarse score. ~1.0 is flat; a real subject
    /// pulls well past [`MIN_CONTRAST`].
    pub contrast: f64,
    /// How many probes it took.
    pub probes: usize,
}

/// A peak must beat the coarse-sweep mean by this factor to be believed.
pub const MIN_CONTRAST: f64 = 1.25;
/// Evenly spaced positions in the coarse sweep.
pub const COARSE: usize = 9;

/// One question for the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Move the lens here, settle, score a fresh frame, call [`Search::feed`].
    Probe(i64),
    Done(Outcome),
}

/// The search. Drive it: `loop { match s.step() { Probe(p) => { move(p); s.feed(score) } Done(o) => break } }`.
#[derive(Debug, Clone)]
pub struct Search {
    min: i64,
    step: i64,
    coarse: Vec<i64>,
    scores: Vec<f64>,
    // refine state
    best: Option<(i64, f64)>,
    d: i64,
    queue: Vec<i64>,
    refine_scores: Vec<(i64, f64)>,
    probes: usize,
    pending: Option<i64>,
}

impl Search {
    /// `min..=max` is the actuator's range and `step` its granularity.
    pub fn new(min: i64, max: i64, step: i64) -> Search {
        let step = step.max(1);
        let span = (max - min).max(0);
        let snap = |p: i64| (p - min) / step * step + min;
        let mut coarse: Vec<i64> = (0..COARSE as i64).map(|i| snap(min + span * i / (COARSE as i64 - 1))).collect();
        coarse.dedup();
        Search { min, step, coarse, scores: vec![], best: None, d: 0, queue: vec![], refine_scores: vec![], probes: 0, pending: None }
    }

    fn snap(&self, p: i64, max: i64) -> i64 { ((p.clamp(self.min, max) - self.min) / self.step) * self.step + self.min }

    fn max(&self) -> i64 { *self.coarse.last().unwrap_or(&self.min) }

    /// The next move, or the verdict.
    pub fn step(&mut self) -> Step {
        if let Some(p) = self.pending { return Step::Probe(p); }
        // Coarse sweep still running?
        if self.scores.len() < self.coarse.len() {
            let p = self.coarse[self.scores.len()];
            self.pending = Some(p);
            return Step::Probe(p);
        }
        // Coarse finished: pick the best, set the refine span to half a coarse gap.
        if self.best.is_none() {
            let (bi, bs) = self.scores.iter().enumerate()
                .fold((0usize, f64::MIN), |a, (i, &s)| if s > a.1 { (i, s) } else { a });
            self.best = Some((self.coarse[bi], bs));
            let gap = if self.coarse.len() > 1 { self.coarse[1] - self.coarse[0] } else { self.step };
            self.d = (gap / 2).max(self.step);
        }
        let mean = self.scores.iter().sum::<f64>() / self.scores.len() as f64;
        let contrast = if mean > 0.0 { self.best.unwrap().1 / mean } else { 0.0 };
        if contrast < MIN_CONTRAST {
            return Step::Done(Outcome { position: None, peak: self.best.unwrap().1, contrast, probes: self.probes });
        }
        // Refine: probe best±d; a better one becomes best; otherwise halve d.
        loop {
            if let Some(p) = self.queue.pop() { self.pending = Some(p); return Step::Probe(p); }
            if !self.refine_scores.is_empty() {
                let (bp, bs) = self.best.unwrap();
                let top = self.refine_scores.iter().cloned().fold((bp, bs), |a, c| if c.1 > a.1 { c } else { a });
                let improved = top.0 != bp;
                self.best = Some(top);
                self.refine_scores.clear();
                if !improved { self.d /= 2; }
            }
            if self.d < self.step {
                let (p, s) = self.best.unwrap();
                return Step::Done(Outcome { position: Some(p), peak: s, contrast, probes: self.probes });
            }
            let (bp, _) = self.best.unwrap();
            let max = self.max();
            let (lo, hi) = (self.snap(bp - self.d, max), self.snap(bp + self.d, max));
            let mut q = vec![];
            for p in [hi, lo] { if p != bp && !q.contains(&p) { q.push(p); } }
            if q.is_empty() { self.d /= 2; continue; }
            self.queue = q;
        }
    }

    /// The score for the position `step` last asked for.
    pub fn feed(&mut self, score: f64) {
        let Some(p) = self.pending.take() else { return };
        self.probes += 1;
        if self.scores.len() < self.coarse.len() { self.scores.push(score); }
        else { self.refine_scores.push((p, score)); }
    }
}

/// One sensor and the lens motor on its I2C bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub sensor_name: String,
    pub sensor_path: String,
    pub actuator_name: String,
    pub actuator_path: String,
}

/// Lens-motor chips seen in mainline `drivers/media/i2c`.
const ACTUATORS: &[&str] = &["dw9714", "dw9718", "dw9719", "dw9768", "dw9807", "ak7375", "ad5820", "lc898", "bu64", "vm149", "gt9", "wv511"];

fn is_actuator(chip: &str) -> bool { ACTUATORS.iter().any(|p| chip.starts_with(p)) }

/// Split a V4L2 subdev name `"imx858 17-0029"` into `(chip, bus, addr)`.
fn parse_subdev_name(name: &str) -> Option<(&str, &str, &str)> {
    let (chip, loc) = name.trim().split_once(' ')?;
    let (bus, addr) = loc.trim().split_once('-')?;
    (!bus.is_empty() && !addr.is_empty() && bus.chars().all(|c| c.is_ascii_digit())).then_some((chip, bus, addr))
}

/// Pair each lens motor with the sensor on its I2C bus. `subdevs` is
/// `(path, name)` as the kernel lists them (`/sys/class/video4linux/v4l-subdevN/name`).
/// A motor pairs only when EXACTLY ONE sensor shares its bus: two sensors on
/// a bus is a guess, and a wrong guess drives the wrong camera's lens.
pub fn pair_actuators(subdevs: &[(String, String)]) -> Vec<Pair> {
    let parsed: Vec<_> = subdevs.iter().filter_map(|(p, n)| parse_subdev_name(n).map(|t| (p, n, t))).collect();
    let mut out = Vec::new();
    for (ap, an, (achip, abus, _)) in parsed.iter().filter(|(_, _, (c, _, _))| is_actuator(c)) {
        let _ = achip;
        let sensors: Vec<_> = parsed.iter().filter(|(_, _, (c, b, _))| !is_actuator(c) && b == abus).collect();
        if let [(sp, sn, _)] = sensors.as_slice() {
            out.push(Pair { sensor_name: (*sn).clone(), sensor_path: (*sp).clone(), actuator_name: (*an).clone(), actuator_path: (*ap).clone() });
        }
    }
    out
}

/// The sensor behind a libcamera camera id such as
/// `/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29`: the subdev whose I2C
/// address is that `camera@` unit address. `None` unless exactly one matches.
pub fn sensor_for_camera_id<'a>(id: &str, subdevs: &'a [(String, String)]) -> Option<&'a (String, String)> {
    let unit = id.rsplit('/').next()?.strip_prefix("camera@")?;
    let want = u32::from_str_radix(unit, 16).ok()?;
    let hits: Vec<_> = subdevs.iter().filter(|(_, n)| {
        parse_subdev_name(n).is_some_and(|(c, _, a)| !is_actuator(c) && u32::from_str_radix(a, 16).ok() == Some(want))
    }).collect();
    if let [one] = hits.as_slice() { Some(one) } else { None }
}

/// The actuator's range from `v4l2-ctl -l` output: `(min, max, step)` of
/// `focus_absolute`.
pub fn parse_focus_range(list_ctrls: &str) -> Option<(i64, i64, i64)> {
    let line = list_ctrls.lines().find(|l| l.trim_start().starts_with("focus_absolute"))?;
    let field = |k: &str| line.split_whitespace().find_map(|t| t.strip_prefix(k)?.strip_prefix('=')?.parse::<i64>().ok());
    Some((field("min")?, field("max")?, field("step").unwrap_or(1).max(1)))
}

/// Read the range off a device.
pub fn read_range(prog: &str, device: &str) -> Result<(i64, i64, i64), String> {
    let out = Command::new(prog).args(["-d", device, "-l"]).output().map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() { return Err(format!("{prog} -d {device} -l: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    parse_focus_range(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| format!("{device}: no focus_absolute control"))
}

/// Move the lens.
pub fn set_focus(prog: &str, device: &str, pos: i64) -> Result<(), String> {
    let out = Command::new(prog).args(["-d", device, "--set-ctrl", &format!("focus_absolute={pos}")])
        .output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("{prog} -d {device} focus_absolute={pos}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The subdev list as the kernel reports it: `(/dev/v4l-subdevN, name)`.
pub fn list_subdevs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/video4linux") else { return out };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with("v4l-subdev") { continue; }
        if let Ok(name) = std::fs::read_to_string(e.path().join("name")) {
            out.push((format!("/dev/{n}"), name.trim().to_string()));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> RawFrame {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h { for x in 0..w { let l = f(x, y); v.extend_from_slice(&[l, l, l, 255]); } }
        RawFrame { width: w, height: h, captured_ms: 0, seq: 0, rgba: Arc::from(v) }
    }

    /// Drive a search against a synthetic scene whose sharpness is `score(pos)`.
    fn run(min: i64, max: i64, step: i64, score: impl Fn(i64) -> f64) -> Outcome {
        let mut s = Search::new(min, max, step);
        for _ in 0..200 {
            match s.step() { Step::Probe(p) => s.feed(score(p)), Step::Done(o) => return o }
        }
        panic!("search did not terminate");
    }

    fn peak_at(c: f64, width: f64) -> impl Fn(i64) -> f64 {
        move |p| 1.0 + 20.0 * (-((p as f64 - c) / width).powi(2)).exp()
    }

    #[test]
    fn sharp_beats_blurred_and_exposure_does_not_move_it() {
        let sharp = frame(64, 64, |x, y| if (x / 4 + y / 4) % 2 == 0 { 40 } else { 200 });
        let soft = frame(64, 64, |x, y| (120.0 + 80.0 * (((x + y) as f32) / 20.0).sin().abs() * 0.1) as u8);
        let whole = [0.0, 0.0, 1.0, 1.0];
        let (a, b) = (sharpness(&sharp, whole).unwrap(), sharpness(&soft, whole).unwrap());
        assert!(a > 20.0 * b, "checker {a} vs smooth {b}");
        // The same scene at half brightness: gradients halve, mean halves, score holds.
        let dim = frame(64, 64, |x, y| if (x / 4 + y / 4) % 2 == 0 { 20 } else { 100 });
        let d = sharpness(&dim, whole).unwrap();
        assert!((d / a - 1.0).abs() < 0.15, "exposure moved the score: {a} vs {d}");
    }

    #[test]
    fn roi_is_honoured_and_bad_input_is_refused() {
        // Detail only in the left half.
        let f = frame(64, 64, |x, y| if x < 32 && (x / 2 + y / 2) % 2 == 0 { 30 } else { 150 });
        let left = sharpness(&f, [0.0, 0.0, 0.5, 1.0]).unwrap();
        let right = sharpness(&f, [0.6, 0.0, 1.0, 1.0]).unwrap();
        assert!(left > 50.0 * right.max(1e-9), "left {left} right {right}");
        assert!(sharpness(&f, [0.5, 0.5, 0.5, 0.5]).is_none(), "empty region");
        let torn = RawFrame { rgba: Arc::from(vec![0u8; 10]), ..f.clone() };
        assert!(sharpness(&torn, [0.0, 0.0, 1.0, 1.0]).is_none(), "torn frame");
    }

    #[test]
    fn roi_at_stays_inside_the_frame() {
        for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5), (0.02, 0.98)] {
            let r = roi_at(u, v, 0.2);
            assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 1.0 + 1e-6 && r[3] <= 1.0 + 1e-6, "{r:?}");
            assert!(((r[2] - r[0]) - 0.2).abs() < 1e-5, "size kept at the edge: {r:?}");
        }
        let c = roi_at(0.5, 0.5, 0.2);
        assert!(((c[0] + c[2]) / 2.0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn finds_a_peak_anywhere_in_the_range_with_few_probes() {
        let mut worst_probes = 0;
        for c in [0, 60, 128, 300, 511, 700, 900, 1023] {
            let o = run(0, 1023, 1, peak_at(c as f64, 70.0));
            let got = o.position.unwrap_or_else(|| panic!("refused a clear peak at {c}: {o:?}"));
            assert!((got - c).abs() <= 8, "peak {c}: found {got}");
            worst_probes = worst_probes.max(o.probes);
        }
        assert!(worst_probes <= 40, "{worst_probes} probes — a phone focuses in about a second");
    }

    #[test]
    fn a_flat_scene_is_refused_not_guessed() {
        let o = run(0, 1023, 1, |_| 3.0);
        assert_eq!(o.position, None, "{o:?}");
        let noisy = run(0, 1023, 1, |p| 3.0 + ((p * 7919) % 13) as f64 * 0.01);
        assert_eq!(noisy.position, None, "noise is not a peak: {noisy:?}");
    }

    #[test]
    fn respects_the_actuator_step_and_range() {
        let o = run(10, 250, 5, peak_at(133.0, 40.0));
        let p = o.position.unwrap();
        assert!((10..=250).contains(&p) && (p - 10) % 5 == 0, "off-grid or out of range: {p}");
        assert!((p - 133).abs() <= 10);
        // A one-position actuator has nothing to search.
        let fixed = run(0, 0, 1, |_| 5.0);
        assert!(fixed.position.is_none() || fixed.position == Some(0));
    }

    #[test]
    fn a_monotone_scene_lands_on_the_end() {
        // Subject nearer than the lens can focus: sharpness rises to the limit.
        let o = run(0, 1023, 1, |p| 1.0 + p as f64 / 40.0);
        assert!(o.position.unwrap() >= 1010, "{o:?}");
    }

    fn fp5() -> Vec<(String, String)> {
        // Names read off a Fairphone 5 (/sys/class/video4linux/v4l-subdev*/name).
        [("/dev/v4l-subdev27", "s5kjn1 19-0010"), ("/dev/v4l-subdev28", "imx800 16-001a"),
         ("/dev/v4l-subdev29", "imx858 17-0029"), ("/dev/v4l-subdev30", "dw9719 17-000e"),
         ("/dev/v4l-subdev3", "msm_csid0")]
            .iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn the_fp5_motor_pairs_with_the_imx858_only() {
        let pairs = pair_actuators(&fp5());
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        assert_eq!(pairs[0].sensor_name, "imx858 17-0029");
        assert_eq!(pairs[0].actuator_path, "/dev/v4l-subdev30");
    }

    #[test]
    fn two_sensors_on_one_bus_is_no_pairing() {
        let mut d = fp5();
        d.push(("/dev/v4l-subdev31".into(), "ov5640 17-003c".into()));
        assert!(pair_actuators(&d).is_empty(), "ambiguous bus must not guess");
    }

    #[test]
    fn camera_ids_resolve_to_their_sensor() {
        let d = fp5();
        let id = |s: &str| sensor_for_camera_id(s, &d).map(|(_, n)| n.as_str());
        assert_eq!(id("/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29"), Some("imx858 17-0029"));
        assert_eq!(id("/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a"), Some("imx800 16-001a"));
        assert_eq!(id("/base/soc@0/cci@ac4b000/i2c-bus@1/camera@10"), Some("s5kjn1 19-0010"));
        assert_eq!(id("/base/soc@0/cci@ac4b000/i2c-bus@1/camera@0e"), None, "the motor's address is not a camera");
        assert_eq!(id("not-a-camera-id"), None);
    }

    #[test]
    fn the_range_parses_from_v4l2_ctl() {
        let t = "Camera Controls\n\n                 focus_absolute 0x009a090a (int)    : min=0 max=1023 step=1 default=0 value=0 flags=has-min-max\n";
        assert_eq!(parse_focus_range(t), Some((0, 1023, 1)));
        assert_eq!(parse_focus_range("exposure 0x1 (int) : min=1 max=2 step=1"), None);
    }
}
