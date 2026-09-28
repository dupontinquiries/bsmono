//! lens — clip-on lens profiles and the black-border autocrop.
//!
//! A clip-on lens (Moment's, or anyone's) changes three things about the
//! picture, and a profile says how to undo each:
//!
//! * **shape** — an anamorphic squeezes the scene horizontally by a fixed
//!   factor (`desqueeze`); a wide or fisheye bends straight lines
//!   (`distortion`, Brown `k1, k2`).
//! * **light fall-off** — corners darker than the centre (`vignetting`,
//!   lensfun's `pa` model `v1, v2, v3`; negative is darkening, as lensfun
//!   writes it).
//! * **focus** — an add-on moves where the phone can focus: a macro fixes
//!   a short working distance (the phone's AF only fine-tunes inside it),
//!   a tele lands AF somewhere else than bare. `focus = auto | camera |
//!   0.0–1.0` is applied through the camera's own focus control when a lens
//!   is picked (a fraction of the driver's range, because ranges differ
//!   between cameras); `peaking = true` turns focus peaking on with it.
//! * **coverage** — the lens's own barrel or a fisheye's image circle
//!   leaves black at the edges or corners. `autocrop = auto` finds the lit
//!   region every few frames and crops to the largest rectangle inside it;
//!   `crop = …` is a fixed window, what "lock" saves.
//!
//! Profiles are small INI files in `$XDG_CONFIG_HOME/snapshot-cam/lenses/`,
//! one per lens, and the built-ins below are written out as starting points.
//! Honest about the built-ins: only the facts a lens design fixes are filled
//! in (Moment's anamorphics squeeze 1.33× and 1.55×; a fisheye is a circle
//! on a phone's sensor). Distortion and vignetting coefficients are per
//! lens-on-phone and are ZERO until someone calibrates them — a made-up
//! number would "correct" a lens into a different wrong.

use std::path::{Path, PathBuf};

/// Whether the black border is found and cropped away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoCrop {
    #[default]
    Off,
    /// Measured from the picture, re-measured as it changes.
    Auto,
}

/// What picking a lens does to focus.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FocusHint {
    /// Leave the camera's focus as it is.
    #[default]
    Camera,
    /// Turn autofocus on.
    Auto,
    /// Manual focus at this fraction of the driver's range (0 = its min).
    Manual(f32),
}

impl FocusHint {
    pub fn to_conf(self) -> String {
        match self { FocusHint::Camera => "camera".into(), FocusHint::Auto => "auto".into(), FocusHint::Manual(f) => format!("{f:.3}") }
    }
    pub fn from_conf(v: &str) -> Result<FocusHint, String> {
        match v.trim() {
            "" | "camera" => Ok(FocusHint::Camera),
            "auto" => Ok(FocusHint::Auto),
            n => n.parse::<f32>().ok().filter(|f| (0.0..=1.0).contains(f)).map(FocusHint::Manual)
                .ok_or_else(|| format!("`focus`: `{n}` is not camera, auto or 0–1")),
        }
    }
}

/// One lens.
#[derive(Debug, Clone, PartialEq)]
pub struct LensProfile {
    pub name: String,
    /// Horizontal stretch the file needs; 1.0 for anything not anamorphic.
    pub desqueeze: f32,
    pub distortion: [f32; 2],
    pub vignetting: [f32; 3],
    pub autocrop: AutoCrop,
    /// A fixed crop window, `[u0, v0, u1, v1]` of the corrected frame.
    pub crop: Option<[f32; 4]>,
    pub focus: FocusHint,
    /// Turn focus peaking on when this lens is picked.
    pub peaking: bool,
    /// One line for the panel: how to use the lens.
    pub note: String,
    /// Where it came from — a file path, or "built-in".
    pub source: String,
}

impl LensProfile {
    /// No lens: the phone's own camera.
    pub fn none() -> LensProfile {
        LensProfile { name: "no lens".into(), desqueeze: 1.0, distortion: [0.0; 2], vignetting: [0.0; 3],
                      autocrop: AutoCrop::Off, crop: None, focus: FocusHint::Camera, peaking: false,
                      note: String::new(), source: "built-in".into() }
    }

    /// Does this profile change the picture at all?
    pub fn is_identity(&self) -> bool {
        (self.desqueeze - 1.0).abs() < 1e-4 && self.distortion == [0.0; 2] && self.vignetting == [0.0; 3]
            && self.autocrop == AutoCrop::Off && self.crop.is_none()
    }
    // (focus, peaking and note do not change the picture, so they do not
    // make a profile non-identity: picking the macro does not by itself
    // send the preview through the CPU path.)

    /// The INI text, as `parse` reads it back.
    pub fn render(&self) -> String {
        let mut s = format!(
            "# snapshot-cam lens profile\n\
             # desqueeze: anamorphic factor (1.0 = none)\n\
             # distortion: Brown k1, k2 (r in half-diagonals; + corrects barrel)\n\
             # vignetting: v1, v2, v3 (lensfun pa model: the lens multiplied brightness by 1 + v1 r² + v2 r⁴ + v3 r⁶; negative = darker corners)\n\
             # autocrop: off | auto   crop: u0, v0, u1, v1 (fractions of the frame)\n\
             # focus: camera | auto | 0.0-1.0 (fraction of the camera's focus range)   peaking: true | false\n\
             name = {}\ndesqueeze = {}\ndistortion = {}, {}\nvignetting = {}, {}, {}\nautocrop = {}\n",
            self.name, self.desqueeze, self.distortion[0], self.distortion[1],
            self.vignetting[0], self.vignetting[1], self.vignetting[2],
            match self.autocrop { AutoCrop::Off => "off", AutoCrop::Auto => "auto" });
        if let Some(c) = self.crop {
            s += &format!("crop = {:.4}, {:.4}, {:.4}, {:.4}\n", c[0], c[1], c[2], c[3]);
        }
        s += &format!("focus = {}\npeaking = {}\n", self.focus.to_conf(), self.peaking);
        if !self.note.is_empty() { s += &format!("note = {}\n", self.note.replace('\n', " ")); }
        s
    }

    /// Read a profile. Unknown keys are ignored (a later version's field is
    /// not an error); a malformed number is, with the key named.
    pub fn parse(text: &str, source: &str) -> Result<LensProfile, String> {
        let ini = config_kit::Ini::parse(text);
        let get = |k: &str| ini.top(k).or_else(|| ini.get("lens", k)).map(str::trim);
        let name = get("name").filter(|n| !n.is_empty()).ok_or("no `name`")?.to_string();
        fn nums<const N: usize>(key: &str, v: Option<&str>, default: [f32; N]) -> Result<[f32; N], String> {
            let Some(v) = v else { return Ok(default) };
            let parts: Vec<&str> = v.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
            if parts.len() > N { return Err(format!("`{key}` has {} numbers, wants at most {N}", parts.len())); }
            let mut out = [0.0; N];
            for (i, p) in parts.iter().enumerate() {
                let x: f32 = p.parse().map_err(|_| format!("`{key}`: `{p}` is not a number"))?;
                if !x.is_finite() { return Err(format!("`{key}`: `{p}` is not finite")); }
                out[i] = x;
            }
            Ok(out)
        }
        let [desqueeze] = nums("desqueeze", get("desqueeze"), [1.0])?;
        if !(0.5..=3.0).contains(&desqueeze) { return Err(format!("`desqueeze` {desqueeze} is outside 0.5–3")); }
        let distortion = nums("distortion", get("distortion"), [0.0; 2])?;
        let vignetting = nums("vignetting", get("vignetting"), [0.0; 3])?;
        let autocrop = match get("autocrop").unwrap_or("off") {
            "auto" | "on" | "true" => AutoCrop::Auto,
            "off" | "false" | "" => AutoCrop::Off,
            other => return Err(format!("`autocrop`: `{other}` is not off or auto")),
        };
        let crop = match get("crop") {
            None | Some("") => None,
            Some(v) => {
                let c = nums::<4>("crop", Some(v), [0.0; 4])?;
                if !(c[0] >= 0.0 && c[1] >= 0.0 && c[2] <= 1.0 && c[3] <= 1.0 && c[2] - c[0] > 0.05 && c[3] - c[1] > 0.05) {
                    return Err(format!("`crop` {c:?} is not a window inside the frame"));
                }
                Some(c)
            }
        };
        let focus = FocusHint::from_conf(get("focus").unwrap_or(""))?;
        let peaking = matches!(get("peaking"), Some("true" | "on" | "yes"));
        let note = get("note").unwrap_or("").to_string();
        Ok(LensProfile { name, desqueeze, distortion, vignetting, autocrop, crop, focus, peaking, note, source: source.into() })
    }

    /// A file name for this profile: its name, reduced to something safe.
    pub fn file_name(&self) -> String {
        let slug: String = self.name.to_lowercase().chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
        format!("{}.conf", if slug.is_empty() { "lens".into() } else { slug })
    }
}

/// The built-ins: no lens, and Moment's line with what the lens design fixes.
pub fn builtins() -> Vec<LensProfile> {
    let b = |name: &str, desqueeze: f32, autocrop: AutoCrop| LensProfile {
        name: name.into(), desqueeze, autocrop, ..LensProfile::none()
    };
    vec![
        LensProfile::none(),
        b("Moment Anamorphic 1.33x", 1.33, AutoCrop::Off),
        b("Moment Anamorphic 1.55x", 1.55, AutoCrop::Off),
        b("Moment Wide 18mm", 1.0, AutoCrop::Auto),
        b("Moment Fisheye 14mm", 1.0, AutoCrop::Auto),
        b("Moment Tele 58mm", 1.0, AutoCrop::Auto),
        LensProfile {
            peaking: true,
            note: "focus by moving the phone: the add-on fixes a short working distance and AF only fine-tunes inside it".into(),
            ..b("Moment Macro 10x", 1.0, AutoCrop::Off)
        },
    ]
}

pub fn lens_dir() -> PathBuf { config_kit::config_dir(crate::store::APP).join("lenses") }

/// The built-ins, then every `*.conf` in `dir` — a file with a built-in's
/// name REPLACES it (that is how a calibration is loaded over a starting
/// point). Files that do not parse are reported, not skipped silently.
pub fn load(dir: &Path) -> (Vec<LensProfile>, Vec<String>) {
    let mut out = builtins();
    let mut errors = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "conf")).collect()).unwrap_or_default();
    files.sort();
    for p in files {
        let text = match std::fs::read_to_string(&p) { Ok(t) => t, Err(e) => { errors.push(format!("{}: {e}", p.display())); continue; } };
        match LensProfile::parse(&text, &p.display().to_string()) {
            Ok(prof) => match out.iter_mut().find(|x| x.name == prof.name) {
                Some(slot) => *slot = prof,
                None => out.push(prof),
            },
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    (out, errors)
}

/// Write a profile into `dir` under its file name. Returns the path.
pub fn save(dir: &Path, p: &LensProfile) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(p.file_name());
    config_kit::write_atomic(&path, &p.render())?;
    Ok(path)
}

// ── the black-border autocrop ────────────────────────────────────────────

/// The lit region of a picture with a lens's black border, as the largest
/// rectangle inside it: `[u0, v0, u1, v1]`, or `None` when there is no
/// border worth cropping.
///
/// On a small image (the shell renders ~160 px wide for this). The border
/// is found as DARK REGION CONNECTED TO THE FRAME EDGE — a flood fill from
/// the edges through pixels darker than a threshold relative to the
/// centre's brightness. A dark object in the middle of the scene is not
/// border; a dark object touching the edge can be mistaken for it, which is
/// why the shell smooths the result over time and offers a lock.
///
/// `aspect`: `Some(w/h)` for the largest rectangle of that shape (the
/// photo keeps its proportions), `None` for the largest by area.
pub fn detect_content(rgba: &[u8], w: u32, h: u32, aspect: Option<f32>) -> Option<[f32; 4]> {
    let (w, h) = (w as usize, h as usize);
    if w < 8 || h < 8 || rgba.len() < w * h * 4 { return None; }
    let luma: Vec<u8> = rgba.chunks_exact(4).take(w * h)
        .map(|p| ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8).collect();
    // Threshold from the centre half's median: a lens shadow is near black
    // in absolute terms and far below whatever the scene is.
    let mut centre: Vec<u8> = Vec::new();
    for y in h / 4..h * 3 / 4 { for x in w / 4..w * 3 / 4 { centre.push(luma[y * w + x]); } }
    centre.sort_unstable();
    let median = centre[centre.len() / 2] as f32;
    let thr = (median * 0.2).clamp(10.0, 40.0) as u8;
    let dark = |i: usize| luma[i] < thr;
    // Flood from the edges.
    let mut shadow = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w { stack.push(x); stack.push((h - 1) * w + x); }
    for y in 0..h { stack.push(y * w); stack.push(y * w + w - 1); }
    while let Some(i) = stack.pop() {
        if shadow[i] || !dark(i) { continue; }
        shadow[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 { stack.push(i - 1); }
        if x + 1 < w { stack.push(i + 1); }
        if y > 0 { stack.push(i - w); }
        if y + 1 < h { stack.push(i + w); }
    }
    let shadow_n = shadow.iter().filter(|s| **s).count();
    if shadow_n * 200 < w * h { return None; }
    if shadow_n * 10 > w * h * 9 { return None; } // all dark: a lens cap, not a border
    // Centroid of the lit region: a fisheye circle is not always centred.
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for (i, s) in shadow.iter().enumerate() { if !s { sx += (i % w) as f64; sy += (i / w) as f64; n += 1.0; } }
    let (cx, cy) = ((sx / n) as f32 + 0.5, (sy / n) as f32 + 0.5);
    // Summed-area table of the shadow, for an O(1) "is this rect clean".
    let mut sat = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += shadow[y * w + x] as u32;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let clean = |x0: usize, y0: usize, x1: usize, y1: usize| {
        let s = sat[y1 * (w + 1) + x1] + sat[y0 * (w + 1) + x0] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0];
        s == 0
    };
    // A rect of half-size (hw, hh) about the centroid, shifted inside the frame.
    let place = |hw: f32, hh: f32| -> Option<(usize, usize, usize, usize)> {
        let x0 = (cx - hw).max(0.0).min(w as f32 - 2.0 * hw);
        let y0 = (cy - hh).max(0.0).min(h as f32 - 2.0 * hh);
        let (x0, y0) = (x0.ceil().max(0.0) as usize, y0.ceil().max(0.0) as usize);
        let (x1, y1) = ((x0 as f32 + 2.0 * hw).floor() as usize, (y0 as f32 + 2.0 * hh).floor() as usize);
        (x1 <= w && y1 <= h && x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    };
    let fits = |hw: f32, hh: f32| place(hw, hh).is_some_and(|(a, b, c, d)| clean(a, b, c, d));
    // Largest scale s at which a rect of the given half-extents fits.
    let largest = |ew: f32, eh: f32| -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 { let m = (lo + hi) * 0.5; if fits(ew * m, eh * m) { lo = m } else { hi = m } }
        lo
    };
    let best = match aspect {
        Some(a) => {
            // Half-extents of the biggest rect of aspect `a` the frame holds.
            let (ew, eh) = if (w as f32 / h as f32) > a { (h as f32 * a * 0.5, h as f32 * 0.5) } else { (w as f32 * 0.5, w as f32 / a * 0.5) };
            let s = largest(ew, eh);
            (ew * s, eh * s)
        }
        None => {
            let mut best = (0.0, 0.0);
            for k in 1..=40 {
                let hh = h as f32 * 0.5 * k as f32 / 40.0;
                let (mut lo, mut hi) = (0.0f32, w as f32 * 0.5);
                for _ in 0..20 { let m = (lo + hi) * 0.5; if fits(m, hh) { lo = m } else { hi = m } }
                if lo * hh > best.0 * best.1 { best = (lo, hh); }
            }
            best
        }
    };
    // Inset: a real image circle has a soft, dim rim (the edge blur and
    // the lens's own fall-off) that sits above the threshold but reads as
    // dark corners in the photo. Two percent in from what was measured.
    let best = (best.0 * (1.0 - INSET), best.1 * (1.0 - INSET));
    let (x0, y0, x1, y1) = place(best.0, best.1)?;
    if (x1 - x0) * (y1 - y0) * 100 < w * h * 5 { return None; }
    if (x1 - x0) * (y1 - y0) * 100 > w * h * 97 { return None; }
    Some([x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32])
}

/// How far inside the measured edge the crop sits.
pub const INSET: f32 = 0.02;

/// Move `current` toward `measured`: a step, not a jump, so one bad frame
/// (a hand over a corner) nudges the crop rather than yanking it; changes
/// under `DEADBAND` are ignored so the crop does not breathe.
pub fn settle(current: Option<[f32; 4]>, measured: Option<[f32; 4]>) -> Option<[f32; 4]> {
    const DEADBAND: f32 = 0.01;
    const STEP: f32 = 0.5;
    match (current, measured) {
        (None, m) => m,
        (Some(_), None) => None,
        (Some(c), Some(m)) => {
            if c.iter().zip(&m).all(|(a, b)| (a - b).abs() < DEADBAND) { return Some(c); }
            Some([0, 1, 2, 3].map(|i| c[i] + (m[i] - c[i]) * STEP))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lit disc (a fisheye's image circle) on black, `w × h`.
    fn circle(w: u32, h: u32, r: f32, cx: f32, cy: f32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let c = if d < r { 150 + ((x * 7 + y * 3) % 60) as u8 } else { 4 };
            v.extend_from_slice(&[c, c, c, 255]);
        } }
        v
    }

    #[test]
    fn a_fisheye_circle_crops_to_a_rectangle_inside_it() {
        let (w, h) = (160, 90);
        let img = circle(w, h, 40.0, 80.0, 45.0);
        let win = detect_content(&img, w, h, Some(16.0 / 9.0)).expect("a border to crop");
        // Every corner of the window is inside the circle.
        for (u, v) in [(win[0], win[1]), (win[2], win[1]), (win[0], win[3]), (win[2], win[3])] {
            let (x, y) = (u * w as f32, v * h as f32);
            assert!(((x - 80.0).powi(2) + (y - 45.0).powi(2)).sqrt() <= 41.5, "corner ({x}, {y}) outside the circle");
        }
        // Keeps the aspect, and is not timid: a 16:9 rect in r=40 has a
        // half-diagonal of 40, so its width is about 70 of 160.
        let (cw, ch) = ((win[2] - win[0]) * w as f32, (win[3] - win[1]) * h as f32);
        assert!(((cw / ch) - 16.0 / 9.0).abs() < 0.15, "{cw}×{ch}");
        assert!(cw > 60.0, "too small: {cw}");
    }

    #[test]
    fn an_off_centre_circle_is_followed_and_area_mode_is_bigger() {
        let (w, h) = (160, 90);
        let img = circle(w, h, 38.0, 95.0, 45.0);
        let a = detect_content(&img, w, h, Some(16.0 / 9.0)).unwrap();
        let centre = (a[0] + a[2]) * 0.5 * w as f32;
        assert!((centre - 95.0).abs() < 4.0, "followed the circle: {centre}");
        let b = detect_content(&img, w, h, None).unwrap();
        let area = |x: [f32; 4]| (x[2] - x[0]) * (x[3] - x[1]);
        assert!(area(b) >= area(a) * 0.99, "free aspect is at least as big");
    }

    #[test]
    fn dark_corners_from_a_tele_lens_are_trimmed() {
        // Vignetted corners: a big circle that clips the frame's edges.
        let (w, h) = (160, 90);
        let img = circle(w, h, 82.0, 80.0, 45.0);
        let win = detect_content(&img, w, h, Some(16.0 / 9.0)).expect("corners to crop");
        let area = (win[2] - win[0]) * (win[3] - win[1]);
        assert!(area > 0.6 && area < 0.97, "a trim, not a crop: {area}");
    }

    #[test]
    fn a_clean_frame_a_dark_object_in_the_middle_and_a_lens_cap_are_not_cropped() {
        let (w, h) = (64u32, 36u32);
        let lit: Vec<u8> = (0..w * h).flat_map(|i| { let c = 120 + (i % 50) as u8; [c, c, c, 255] }).collect();
        assert_eq!(detect_content(&lit, w, h, None), None);
        let mut hole = lit.clone();
        for y in 12..24 { for x in 24..40 { let i = ((y * w + x) * 4) as usize; hole[i..i + 3].copy_from_slice(&[0, 0, 0]); } }
        assert_eq!(detect_content(&hole, w, h, None), None, "a black object in the scene is not border");
        let cap = vec![3u8; (w * h * 4) as usize];
        assert_eq!(detect_content(&cap, w, h, None), None);
    }

    #[test]
    fn the_crop_settles_rather_than_jumping_and_ignores_jitter() {
        let a = Some([0.2, 0.1, 0.8, 0.9]);
        assert_eq!(settle(None, a), a);
        let jitter = Some([0.205, 0.1, 0.8, 0.9]);
        assert_eq!(settle(a, jitter), a, "under the deadband");
        let moved = settle(a, Some([0.4, 0.1, 0.8, 0.9])).unwrap();
        assert!((moved[0] - 0.3).abs() < 1e-5, "half way: {moved:?}");
    }

    #[test]
    fn profiles_round_trip_and_a_file_replaces_a_builtin() {
        let d = tempfile::tempdir().unwrap();
        let mut p = builtins().into_iter().find(|p| p.name == "Moment Wide 18mm").unwrap();
        p.distortion = [0.08, -0.01];
        p.vignetting = [-0.3, 0.1, 0.0];
        p.crop = Some([0.05, 0.02, 0.95, 0.98]);
        let path = save(d.path(), &p).unwrap();
        assert!(path.ends_with("moment-wide-18mm.conf"), "{}", path.display());
        let back = LensProfile::parse(&std::fs::read_to_string(&path).unwrap(), "x").unwrap();
        assert_eq!((back.distortion, back.vignetting, back.crop, back.autocrop), (p.distortion, p.vignetting, p.crop, p.autocrop));
        // A new lens, and one broken file.
        std::fs::write(d.path().join("mine.conf"), "name = My Old Nikkor\ndesqueeze = 1.0\nvignetting = 0.4\n").unwrap();
        std::fs::write(d.path().join("broken.conf"), "name = Broken\ndistortion = 0.1, lots\n").unwrap();
        let (all, errs) = load(d.path());
        assert_eq!(all.iter().filter(|x| x.name == "Moment Wide 18mm").count(), 1, "replaced, not duplicated");
        assert_eq!(all.iter().find(|x| x.name == "Moment Wide 18mm").unwrap().distortion, [0.08, -0.01]);
        assert!(all.iter().any(|x| x.name == "My Old Nikkor" && x.vignetting == [0.4, 0.0, 0.0]));
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("broken.conf") && errs[0].contains("lots"), "{errs:?}");
    }

    #[test]
    fn focus_hints_round_trip_and_bad_ones_are_refused() {
        let mut p = LensProfile { name: "Tele".into(), ..LensProfile::none() };
        for f in [FocusHint::Camera, FocusHint::Auto, FocusHint::Manual(0.42)] {
            p.focus = f;
            p.peaking = true;
            let back = LensProfile::parse(&p.render(), "x").unwrap();
            assert_eq!((back.focus, back.peaking), (f, true));
        }
        assert!(LensProfile::parse("name = a\nfocus = 3", "x").unwrap_err().contains("focus"));
        let macro_ = builtins().into_iter().find(|p| p.name.contains("Macro")).unwrap();
        assert!(macro_.peaking && !macro_.note.is_empty());
        assert_eq!(macro_.focus, FocusHint::Camera, "no made-up focus value");
        assert!(macro_.is_identity(), "focus and peaking do not change the picture");
    }

    #[test]
    fn bad_profiles_are_refused_with_the_reason() {
        assert!(LensProfile::parse("desqueeze = 1.3", "x").unwrap_err().contains("name"));
        assert!(LensProfile::parse("name = a\ndesqueeze = 9", "x").unwrap_err().contains("desqueeze"));
        assert!(LensProfile::parse("name = a\ncrop = 0.9, 0, 0.1, 1", "x").unwrap_err().contains("crop"));
        assert!(LensProfile::parse("name = a\nautocrop = sometimes", "x").unwrap_err().contains("autocrop"));
        assert!(LensProfile::parse("name = a\nfuture_field = 3", "x").is_ok(), "an unknown key is not an error");
    }

    #[test]
    fn the_builtins_carry_only_what_the_lens_design_fixes() {
        for p in builtins() {
            assert_eq!(p.distortion, [0.0; 2], "{}: uncalibrated coefficients must be zero", p.name);
            assert_eq!(p.vignetting, [0.0; 3], "{}", p.name);
        }
        let ana = builtins().into_iter().find(|p| p.name.contains("1.33")).unwrap();
        assert_eq!(ana.desqueeze, 1.33);
        assert!(builtins()[0].is_identity());
    }
}
