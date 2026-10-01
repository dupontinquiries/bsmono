//! profile — what sits in front of a sensor, as a file.
//!
//! Two things make a phone camera's picture differ from the scene, and neither is in libcamera's
//! tuning: the lens/cover in front of the sensor, and the unit-to-unit spread of the sensor. On the
//! Fairphone 5 the three sensors' libcamera tuning files are byte-identical copies of one another
//! (`imx800.yaml`, `imx858.yaml`, `s5kjn1.yaml`) and the owner's cameras sit behind a plastic cover
//! that tints the picture green. Software that wants a neutral picture needs a PER-CAMERA, per-cover
//! correction it can load, replace and ship — and it must come from a MEASUREMENT.
//!
//! ## The rule this module keeps
//!
//! Nothing here invents a number. [`CameraProfile::identity`] is the only built-in: gains of exactly
//! 1, provenance [`Provenance::Default`]. A correction is made by [`gains_from_neutral_patch`] from
//! a photo of something grey, and a profile carrying it says `provenance = measured`. This is the
//! same rule `snapshot-cam`'s lens profiles follow (a made-up coefficient "corrects" a lens into a
//! different wrong), and for the same reason.
//!
//! ## Where profiles live
//!
//! INI files, `*.conf`, in two places: the SYSTEM directory ([`system_dir`], default
//! `/usr/share/camera-profiles`, what an installer or `gitmind-egui` puts there for every user and
//! every app) and the USER directory (`$XDG_CONFIG_HOME/camera-profiles`). A user file with the same
//! `name` replaces the system one. [`resolve`] picks the best match for a sensor.
//!
//! ## What a profile corrects
//!
//! `wb_gains` — linear R, G, B multipliers applied AFTER the pipeline's own white balance, to take
//! out what the pipeline cannot see: a cover that tints every scene by the same amount. (A cast that
//! changes with the light is the pipeline's AWB's job; a cast that does not is this.) It does not
//! touch exposure, so it cannot recover clipped highlights.

use crate::orient::Facing;
use crate::RawFrame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Nothing was measured: the identity correction.
    Default,
    /// Taken from a photo of a neutral target through this very camera and cover.
    Measured,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CameraProfile {
    pub name: String,
    /// The sensor model as the kernel names it (`imx858`), compared case-insensitively. `*` matches
    /// any sensor (a profile for "the camera with the cover on", whichever it is).
    pub sensor: String,
    pub facing: Option<Facing>,
    /// What is in front of the lens, free text: "plastic film", "Bark case", "none".
    pub cover: String,
    /// Linear R, G, B multipliers, each within [`GAIN_MIN`], [`GAIN_MAX`].
    pub wb_gains: [f32; 3],
    /// Clockwise degrees to turn the picture upright, overriding the sensor's own mounting.
    pub rotation: Option<u32>,
    pub note: String,
    pub provenance: Provenance,
    pub source: String,
}

/// A correction beyond this is not a cover, it is a broken measurement.
pub const GAIN_MIN: f32 = 0.25;
pub const GAIN_MAX: f32 = 4.0;

impl CameraProfile {
    pub fn identity(sensor: &str) -> CameraProfile {
        CameraProfile {
            name: format!("{sensor}-identity"), sensor: sensor.into(), facing: None, cover: String::new(),
            wb_gains: [1.0; 3], rotation: None, note: String::new(), provenance: Provenance::Default, source: "built-in".into(),
        }
    }

    pub fn is_identity(&self) -> bool { self.wb_gains == [1.0; 3] && self.rotation.is_none() }

    pub fn render(&self) -> String {
        let mut s = format!(
            "# camera profile (capture-kit)\n\
             # sensor: the kernel's name for it (imx858), or * for any\n\
             # wb_gains: linear R, G, B multipliers applied after the pipeline's own white balance\n\
             # provenance: measured | default   (measured = from a photo of a neutral target)\n\
             name = {}\nsensor = {}\nwb_gains = {:.4}, {:.4}, {:.4}\nprovenance = {}\n",
            self.name, self.sensor, self.wb_gains[0], self.wb_gains[1], self.wb_gains[2],
            match self.provenance { Provenance::Default => "default", Provenance::Measured => "measured" });
        if let Some(f) = self.facing { s += &format!("facing = {}\n", match f { Facing::Front => "front", Facing::Back => "back", Facing::External => "external" }); }
        if !self.cover.is_empty() { s += &format!("cover = {}\n", self.cover); }
        if let Some(r) = self.rotation { s += &format!("rotation = {r}\n"); }
        if !self.note.is_empty() { s += &format!("note = {}\n", self.note.replace('\n', " ")); }
        s
    }

    /// Unknown keys are ignored (a later version's field is not an error); a malformed or
    /// implausible value is, with the key named.
    pub fn parse(text: &str, source: &str) -> Result<CameraProfile, String> {
        let ini = config_kit::Ini::parse(text);
        let get = |k: &str| ini.top(k).or_else(|| ini.get("camera", k)).map(str::trim);
        let name = get("name").filter(|n| !n.is_empty()).ok_or("no `name`")?.to_string();
        let sensor = get("sensor").filter(|n| !n.is_empty()).ok_or("no `sensor` (use * for any)")?.to_string();
        let wb_gains = match get("wb_gains") {
            None | Some("") => [1.0; 3],
            Some(v) => {
                let parts: Vec<&str> = v.split(',').map(str::trim).collect();
                if parts.len() != 3 { return Err(format!("`wb_gains` has {} numbers, wants 3 (R, G, B)", parts.len())); }
                let mut g = [0.0f32; 3];
                for (i, p) in parts.iter().enumerate() {
                    let x: f32 = p.parse().map_err(|_| format!("`wb_gains`: `{p}` is not a number"))?;
                    if !x.is_finite() || !(GAIN_MIN..=GAIN_MAX).contains(&x) {
                        return Err(format!("`wb_gains`: {x} is outside {GAIN_MIN}-{GAIN_MAX} — that is a broken measurement, not a cover"));
                    }
                    g[i] = x;
                }
                g
            }
        };
        let facing = match get("facing").map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("any") => None,
            Some("front") => Some(Facing::Front),
            Some("back") | Some("rear") => Some(Facing::Back),
            Some("external") => Some(Facing::External),
            Some(o) => return Err(format!("`facing`: `{o}` is not front, back or external")),
        };
        let rotation = match get("rotation") {
            None | Some("") => None,
            Some(v) => match v.parse::<u32>() {
                Ok(d @ (0 | 90 | 180 | 270)) => Some(d),
                _ => return Err(format!("`rotation`: `{v}` is not 0, 90, 180 or 270")),
            },
        };
        let provenance = match get("provenance").unwrap_or("default") {
            "measured" => Provenance::Measured,
            "default" | "" => Provenance::Default,
            o => return Err(format!("`provenance`: `{o}` is not measured or default")),
        };
        if provenance == Provenance::Default && wb_gains != [1.0; 3] {
            return Err("gains other than 1,1,1 must say `provenance = measured` — a correction nobody measured is a guess".into());
        }
        Ok(CameraProfile {
            name, sensor, facing, cover: get("cover").unwrap_or("").to_string(), wb_gains, rotation,
            note: get("note").unwrap_or("").to_string(), provenance, source: source.into(),
        })
    }

    pub fn file_name(&self) -> String {
        let safe: String = self.name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        format!("{safe}.conf")
    }
}

/// The directory a system install (or gitmind) puts shared profiles in. `CAMERA_PROFILE_SYSTEM_DIR`
/// overrides it (tests, and a phone that keeps them elsewhere).
pub fn system_dir() -> PathBuf {
    std::env::var_os("CAMERA_PROFILE_SYSTEM_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/camera-profiles"))
}

/// The per-user directory.
pub fn user_dir() -> PathBuf { config_kit::config_dir("camera-profiles") }

/// Every `*.conf` in `dir`, plus a report of the ones that would not parse (never skipped silently).
pub fn load_dir(dir: &Path) -> (Vec<CameraProfile>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "conf")).collect()).unwrap_or_default();
    files.sort();
    for p in files {
        match std::fs::read_to_string(&p) {
            Err(e) => errors.push(format!("{}: {e}", p.display())),
            Ok(t) => match CameraProfile::parse(&t, &p.display().to_string()) {
                Ok(prof) => out.push(prof),
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            },
        }
    }
    (out, errors)
}

/// System profiles, then user profiles; a user profile with the same `name` replaces the system one.
pub fn load_all(system: &Path, user: &Path) -> (Vec<CameraProfile>, Vec<String>) {
    let (mut all, mut errors) = load_dir(system);
    let (mine, e2) = load_dir(user);
    errors.extend(e2);
    for p in mine {
        match all.iter_mut().find(|x| x.name == p.name) { Some(slot) => *slot = p, None => all.push(p) }
    }
    (all, errors)
}

/// The best profile for `sensor` (the kernel's name, e.g. `imx858`; a trailing I2C address such as
/// `imx858 17-0029` is ignored) facing `facing`. Most specific wins: sensor+facing, sensor, any+facing,
/// any. Among equals the LAST in `profiles` wins, so load system first and user second.
pub fn resolve<'a>(profiles: &'a [CameraProfile], sensor: &str, facing: Option<Facing>) -> Option<&'a CameraProfile> {
    let sensor = sensor.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    let mut best: Option<(u8, &CameraProfile)> = None;
    for p in profiles {
        let exact = p.sensor.eq_ignore_ascii_case(&sensor);
        if !exact && p.sensor != "*" { continue; }
        // A profile that names a facing must match it; one that names none matches any.
        let facing_match = match (p.facing, facing) { (None, _) => false, (Some(a), Some(b)) => a == b, (Some(_), None) => continue };
        if p.facing.is_some() && !facing_match { continue; }
        let score = (exact as u8) * 2 + (p.facing.is_some() as u8);
        if best.map_or(true, |(s, _)| score >= s) { best = Some((score, p)); }
    }
    best.map(|(_, p)| p)
}

impl RawFrame {
    /// The frame with `gains` (linear R, G, B) applied: Q8.8 fixed point, saturating at 255, alpha
    /// untouched. All-ones returns the frame itself (the same `Arc`, no copy); a torn frame comes back
    /// unchanged. `captured_ms` and `seq` are kept — it is the same moment.
    pub fn balanced(&self, gains: [f32; 3]) -> RawFrame {
        if gains == [1.0; 3] || !self.pixels_plausible() { return self.clone(); }
        let q = gains.map(|g| (g.clamp(GAIN_MIN, GAIN_MAX) * 256.0).round() as u32);
        let mut out = self.rgba.to_vec();
        for px in out.chunks_exact_mut(4) {
            for c in 0..3 { px[c] = ((px[c] as u32 * q[c] + 128) >> 8).min(255) as u8; }
        }
        RawFrame { width: self.width, height: self.height, captured_ms: self.captured_ms, seq: self.seq, rgba: Arc::from(out) }
    }
}

/// Pixels this bright or brighter are clipped and say nothing about colour; pixels this dark are noise.
const CLIPPED: u8 = 250;
const NOISE_FLOOR: u8 = 12;

/// The gains that would turn a photo of a NEUTRAL patch (a grey card, a white wall, `roi` in
/// `[u0, v0, u1, v1]` fractions of the frame) grey: `[G/R, 1, G/B]`.
///
/// Clipped and near-black pixels are left out of the means — a patch with specular highlights in it
/// would otherwise pull every channel toward 255 and read as "no cast". `None` when too few usable
/// pixels remain, the patch is too dark to trust, or the answer is outside [`GAIN_MIN`], [`GAIN_MAX`].
pub fn gains_from_neutral_patch(f: &RawFrame, roi: [f32; 4]) -> Option<[f32; 3]> {
    if !f.pixels_plausible() { return None; }
    let (w, h) = (f.width as usize, f.height as usize);
    let px = |v: f32, n: usize| (v.clamp(0.0, 1.0) * n as f32).round() as usize;
    let (x0, y0, x1, y1) = (px(roi[0], w), px(roi[1], h), px(roi[2], w).min(w), px(roi[3], h).min(h));
    if x1 <= x0 || y1 <= y0 { return None; }
    let (mut s, mut n) = ([0u64; 3], 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &f.rgba[(y * w + x) * 4..(y * w + x) * 4 + 3];
            if p.iter().any(|&c| c >= CLIPPED) || p.iter().all(|&c| c <= NOISE_FLOOR) { continue; }
            for c in 0..3 { s[c] += p[c] as u64; }
            n += 1;
        }
    }
    if n < 100 { return None; }
    let m = s.map(|v| v as f32 / n as f32);
    if m[1] < 24.0 { return None; }
    let g = [m[1] / m[0].max(1.0), 1.0, m[1] / m[2].max(1.0)];
    g.iter().all(|x| (GAIN_MIN..=GAIN_MAX).contains(x)).then_some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RawFrame {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h { v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]); }
        RawFrame { width: w, height: h, captured_ms: 5, seq: 9, rgba: Arc::from(v) }
    }
    fn measured(sensor: &str, facing: Option<Facing>, gains: [f32; 3]) -> CameraProfile {
        CameraProfile { name: format!("{sensor}-{facing:?}-{gains:?}"), sensor: sensor.into(), facing, cover: "plastic film".into(),
            wb_gains: gains, rotation: None, note: "n".into(), provenance: Provenance::Measured, source: "t".into() }
    }
    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("capture-kit-profile-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_profile_survives_render_and_parse() {
        let mut p = measured("imx858", Some(Facing::Back), [1.12, 1.0, 1.07]);
        p.rotation = Some(270);
        let back = CameraProfile::parse(&p.render(), "t").unwrap();
        assert_eq!(back.sensor, "imx858");
        assert_eq!(back.facing, Some(Facing::Back));
        assert_eq!(back.rotation, Some(270));
        assert_eq!(back.provenance, Provenance::Measured);
        for (a, b) in back.wb_gains.iter().zip(p.wb_gains) { assert!((a - b).abs() < 1e-3); }
    }

    #[test]
    fn bad_profiles_are_refused_with_the_key_named() {
        let bad = |t: &str| CameraProfile::parse(t, "t").unwrap_err();
        assert!(bad("sensor = x").contains("name"));
        assert!(bad("name = a").contains("sensor"));
        assert!(bad("name = a\nsensor = x\nwb_gains = 1, 1").contains("wants 3"));
        assert!(bad("name = a\nsensor = x\nprovenance = measured\nwb_gains = 1, 1, 9").contains("outside"));
        assert!(bad("name = a\nsensor = x\nprovenance = measured\nwb_gains = 1, nan, 1").contains("wb_gains"));
        assert!(bad("name = a\nsensor = x\nrotation = 45").contains("rotation"));
        assert!(bad("name = a\nsensor = x\nfacing = sideways").contains("facing"));
        assert!(CameraProfile::parse("name = a\nsensor = x\nfuture_key = 1", "t").is_ok(), "unknown keys are not errors");
    }

    #[test]
    fn a_correction_nobody_measured_is_refused() {
        // The module's one rule, enforced at the door: gains != 1 need `provenance = measured`.
        let e = CameraProfile::parse("name = a\nsensor = x\nwb_gains = 1.2, 1.0, 1.1", "t").unwrap_err();
        assert!(e.contains("measured"), "{e}");
        assert!(CameraProfile::parse("name = a\nsensor = x\nprovenance = measured\nwb_gains = 1.2, 1.0, 1.1", "t").is_ok());
        assert!(CameraProfile::identity("imx858").is_identity());
    }

    #[test]
    fn the_most_specific_profile_wins_and_the_later_of_equals() {
        let any = measured("*", None, [1.1, 1.0, 1.0]);
        let any_back = measured("*", Some(Facing::Back), [1.2, 1.0, 1.0]);
        let sensor = measured("imx858", None, [1.3, 1.0, 1.0]);
        let sensor_back = measured("imx858", Some(Facing::Back), [1.4, 1.0, 1.0]);
        let all = [any.clone(), any_back.clone(), sensor.clone(), sensor_back.clone()];
        let g = |sensor: &str, f| resolve(&all, sensor, f).map(|p| p.wb_gains[0]);
        assert_eq!(g("imx858", Some(Facing::Back)), Some(1.4), "sensor + facing");
        assert_eq!(g("IMX858 17-0029", Some(Facing::Back)), Some(1.4), "case and the I2C suffix are ignored");
        assert_eq!(g("imx858", Some(Facing::Front)), Some(1.3), "a back-only profile must not match a front camera");
        assert_eq!(g("s5kjn1", Some(Facing::Back)), Some(1.2), "any sensor, facing matches");
        assert_eq!(g("s5kjn1", Some(Facing::Front)), Some(1.1), "any sensor, any facing");
        assert_eq!(resolve(&[sensor_back.clone()], "imx858", None), None, "a profile that requires a facing needs one");
        assert_eq!(resolve(&[], "imx858", None), None);
        let twin = measured("imx858", None, [1.9, 1.0, 1.0]);
        assert_eq!(resolve(&[sensor.clone(), twin], "imx858", None).map(|p| p.wb_gains[0]), Some(1.9), "later wins a tie");
    }

    #[test]
    fn a_user_profile_replaces_the_system_one_of_the_same_name_and_bad_files_are_reported() {
        let (sys, usr) = (tmp("sys"), tmp("usr"));
        let mut a = measured("imx858", None, [1.1, 1.0, 1.0]);
        a.name = "cover".into();
        std::fs::write(sys.join("cover.conf"), a.render()).unwrap();
        let mut b = a.clone();
        b.wb_gains = [1.25, 1.0, 1.0];
        std::fs::write(usr.join("cover.conf"), b.render()).unwrap();
        std::fs::write(usr.join("broken.conf"), "name = broken\n").unwrap();
        std::fs::write(usr.join("notes.txt"), "ignored").unwrap();
        let (all, errors) = load_all(&sys, &usr);
        assert_eq!(all.len(), 1, "same name: replaced, not duplicated");
        assert!((all[0].wb_gains[0] - 1.25).abs() < 1e-3, "the user's wins");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("broken.conf") && errors[0].contains("sensor"));
        assert!(load_all(&tmp("nonexistent-a"), &tmp("nonexistent-b")).0.is_empty(), "missing dirs are empty, not an error");
    }

    #[test]
    fn balanced_removes_a_measured_cast_and_changes_nothing_else() {
        let cast = solid(40, 40, [100, 130, 110]); // a green cast
        let g = gains_from_neutral_patch(&cast, [0.0, 0.0, 1.0, 1.0]).expect("measurable");
        let fixed = cast.balanced(g);
        for c in 0..3 { assert!((fixed.rgba[c] as i32 - 130).abs() <= 1, "channel {c}: {}", fixed.rgba[c]); }
        assert_eq!(fixed.rgba[3], 255, "alpha untouched");
        assert_eq!((fixed.captured_ms, fixed.seq), (5, 9));
        // all-ones is the frame itself, not a copy
        assert!(Arc::ptr_eq(&cast.rgba, &cast.balanced([1.0; 3]).rgba));
        // saturates instead of wrapping
        let hot = solid(4, 4, [200, 200, 200]).balanced([2.0, 1.0, 1.0]);
        assert_eq!(&hot.rgba[..3], &[255, 200, 200]);
        let torn = RawFrame { rgba: Arc::from(vec![1u8; 7]), ..cast.clone() };
        assert_eq!(torn.balanced([2.0, 1.0, 1.0]), torn);
    }

    #[test]
    fn the_neutral_patch_measurement_is_not_fooled_by_highlights_or_darkness() {
        // A 50x50 green-cast patch with a third of it specular-clipped white: clipped pixels must be
        // LEFT OUT, or the white drags every channel up and the patch reads as nearly neutral.
        let mut f = solid(50, 50, [100, 130, 110]);
        let mut v = f.rgba.to_vec();
        for i in 0..(50 * 50 / 3) { v[i * 4..i * 4 + 3].copy_from_slice(&[255, 255, 255]); }
        f.rgba = Arc::from(v);
        let g = gains_from_neutral_patch(&f, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert!((g[0] - 1.3).abs() < 0.02 && (g[2] - 130.0 / 110.0).abs() < 0.02, "{g:?}");
        assert_eq!(g[1], 1.0);
        assert_eq!(gains_from_neutral_patch(&solid(50, 50, [10, 14, 11]), [0.0, 0.0, 1.0, 1.0]), None, "too dark to trust");
        assert_eq!(gains_from_neutral_patch(&solid(8, 8, [100, 130, 110]), [0.0, 0.0, 1.0, 1.0]), None, "too few pixels");
        assert_eq!(gains_from_neutral_patch(&solid(50, 50, [100, 130, 110]), [0.5, 0.5, 0.5, 0.5]), None, "empty region");
        assert_eq!(gains_from_neutral_patch(&solid(50, 50, [20, 130, 110]), [0.0, 0.0, 1.0, 1.0]), None, "a 6.5x correction is a broken measurement");
        let neutral = gains_from_neutral_patch(&solid(50, 50, [128, 128, 128]), [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(neutral, [1.0, 1.0, 1.0], "a grey patch needs no correction");
    }
}
