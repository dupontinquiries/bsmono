//! pro — the DSLR-style settings as data: lens, crop, exposure
//! compensation, white balance, the overlays, and what they add up to for
//! `develop::render`.
//!
//! Headless, so "which window does the shutter render, at what size, with
//! which gains" is answered and tested without a window. The shell owns the
//! widgets and the timing (when autocrop re-measures); this owns the
//! arithmetic.

use crate::develop::{self, Params, WhiteBalance};
use crate::hwctl::{self, Role};
use crate::lens::{self, AutoCrop, FocusHint, LensProfile};

/// Which aids are drawn over the viewfinder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overlays {
    pub histogram: bool,
    pub zebra: bool,
    pub peaking: bool,
    pub grid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pro {
    pub lenses: Vec<LensProfile>,
    /// Index into `lenses`.
    pub lens: usize,
    /// The autocrop's current (settled) measurement.
    pub measured_crop: Option<[f32; 4]>,
    /// Exposure compensation, stops, in thirds.
    pub ev: f32,
    pub wb: WhiteBalance,
    /// Grey-world gains, re-measured while `wb` is Auto.
    pub auto_gains: [f32; 3],
    pub overlays: Overlays,
    /// Profiles that did not load, for the panel to show.
    pub load_errors: Vec<String>,
    /// The camera cover's measured tint and haze, and whether to undo them.
    pub cover: crate::cover::CoverProfile,
    pub cover_on: bool,
}

impl Default for Pro {
    fn default() -> Self {
        Pro { lenses: lens::builtins(), lens: 0, measured_crop: None, ev: 0.0, wb: WhiteBalance::AsShot,
              auto_gains: [1.0; 3], overlays: Overlays::default(), load_errors: vec![],
              cover: crate::cover::CoverProfile::none(), cover_on: false }
    }
}

/// EV range and step: ±3 stops in thirds, a camera's dial.
pub const EV_RANGE: f32 = 3.0;
pub const EV_STEP: f32 = 1.0 / 3.0;
/// The size autocrop and auto-WB measure at.
pub const MEASURE_W: u32 = 160;

/// `inner` (fractions of `outer`'s picture) in frame fractions.
pub fn compose(outer: [f32; 4], inner: [f32; 4]) -> [f32; 4] {
    let (w, h) = (outer[2] - outer[0], outer[3] - outer[1]);
    [outer[0] + inner[0] * w, outer[1] + inner[1] * h, outer[0] + inner[2] * w, outer[1] + inner[3] * h]
}

impl Pro {
    /// Built-ins plus the profiles in `dir`.
    pub fn with_lenses_from(dir: &std::path::Path) -> Pro {
        let (lenses, load_errors) = lens::load(dir);
        Pro { lenses, load_errors, ..Pro::default() }
    }

    pub fn profile(&self) -> &LensProfile { &self.lenses[self.lens.min(self.lenses.len() - 1)] }
    pub fn profile_mut(&mut self) -> &mut LensProfile { let i = self.lens.min(self.lenses.len() - 1); &mut self.lenses[i] }

    /// Pick a lens. Its `peaking = true` turns the aid on (never off: an
    /// aid the user chose stays).
    pub fn select_lens(&mut self, i: usize) {
        if i < self.lenses.len() && i != self.lens {
            self.lens = i;
            self.measured_crop = None;
            if self.profile().peaking { self.overlays.peaking = true; }
        }
    }

    /// What the lens's focus hint asks of the camera's controls: autofocus
    /// on, or manual focus at the fraction of this driver's range. Empty
    /// when the hint is "camera" or the camera has no such control.
    pub fn lens_focus(&self, ctl: &hwctl::Controls) -> Vec<(String, i64)> {
        match self.profile().focus {
            FocusHint::Camera => vec![],
            FocusHint::Auto => ctl.get(Role::AutoFocus).map(|_| ctl.plan_set(Role::AutoFocus, 1)).unwrap_or_default(),
            FocusHint::Manual(f) => ctl.get(Role::Focus)
                .map(|c| ctl.plan_set(Role::Focus, c.min + ((c.max - c.min) as f32 * f).round() as i64)).unwrap_or_default(),
        }
    }

    /// Record the camera's current focus into the profile, for "save": the
    /// manual position as a fraction when autofocus is off, `auto` when on.
    /// Returns whether anything was recorded.
    pub fn capture_focus(&mut self, ctl: &hwctl::Controls) -> bool {
        let af_on = ctl.get(Role::AutoFocus).map(|a| a.value != 0);
        let hint = match (af_on, ctl.get(Role::Focus)) {
            (Some(true), _) => FocusHint::Auto,
            (_, Some(c)) if c.max > c.min => FocusHint::Manual(((c.value - c.min) as f32 / (c.max - c.min) as f32).clamp(0.0, 1.0)),
            _ => return false,
        };
        self.profile_mut().focus = hint;
        true
    }

    /// The crop before zoom: the profile's fixed window, else the autocrop's
    /// measurement, else the whole frame.
    pub fn base_window(&self) -> [f32; 4] {
        let p = self.profile();
        p.crop.or(if p.autocrop == AutoCrop::Auto { self.measured_crop } else { None }).unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    /// Is autocrop measuring right now (on, and not overridden by a lock)?
    pub fn measuring(&self) -> bool { self.profile().autocrop == AutoCrop::Auto && self.profile().crop.is_none() }

    /// Does the picture need `develop::render` at all?
    pub fn develops(&self) -> bool {
        !self.profile().is_identity() || self.ev.abs() > 1e-3 || self.wb != WhiteBalance::AsShot || self.cover_active()
    }

    /// Is a measured cover correction being applied?
    pub fn cover_active(&self) -> bool { self.cover_on && self.cover != crate::cover::CoverProfile::none() }
    pub fn flare(&self) -> [f32; 3] { if self.cover_active() { self.cover.flare } else { [0.0; 3] } }
    pub fn cover_gains(&self) -> [f32; 3] { if self.cover_active() { self.cover.gains } else { [1.0; 3] } }

    /// Does the preview need the CPU path — developing, or measuring for an
    /// overlay the GPU path cannot compute?
    pub fn needs_cpu(&self) -> bool {
        self.develops() || self.overlays.histogram || self.overlays.zebra || self.overlays.peaking
    }

    /// Cover tint, then white balance and exposure.
    pub fn gains(&self) -> [f32; 3] {
        let g = develop::gains(self.wb, self.auto_gains, self.ev);
        let c = self.cover_gains();
        [g[0] * c[0], g[1] * c[1], g[2] * c[2]]
    }

    /// The render for a `sw × sh` frame with the zoom window `zoom` (in
    /// fractions of the cropped picture): native size, or fitted inside
    /// `fit` for the preview.
    pub fn params(&self, zoom: [f32; 4], sw: u32, sh: u32, fit: Option<(u32, u32)>) -> Params {
        let p = self.profile();
        let window = compose(self.base_window(), zoom);
        let native = develop::native_size(window, sw, sh, p.desqueeze);
        let (out_w, out_h) = match fit { Some((mw, mh)) => develop::fit_size(native, mw, mh), None => native };
        Params { window, out_w, out_h, distortion: p.distortion, vignetting: p.vignetting, gains: self.gains(), flare: self.flare() }
    }

    /// What autocrop and auto-WB measure: the whole corrected frame, small,
    /// neutral colour, no desqueeze (the crop is found in frame fractions).
    pub fn measure_params(&self, sw: u32, sh: u32) -> Params {
        let p = self.profile();
        let (w, h) = develop::fit_size((sw, sh), MEASURE_W, MEASURE_W);
        Params { window: [0.0, 0.0, 1.0, 1.0], out_w: w, out_h: h, distortion: p.distortion, vignetting: p.vignetting, gains: self.cover_gains(), flare: self.flare() }
    }

    /// Fold one measurement in: the settled crop, and grey-world gains when
    /// WB is auto (measured inside the crop, so the black border does not
    /// drag the average).
    pub fn measured(&mut self, small: &[u8], w: u32, h: u32) {
        if self.measuring() {
            let m = lens::detect_content(small, w, h, Some(w as f32 / h as f32));
            self.measured_crop = lens::settle(self.measured_crop, m);
        }
        if self.wb == WhiteBalance::Auto {
            let b = self.base_window();
            let (x0, y0) = ((b[0] * w as f32) as usize, (b[1] * h as f32) as usize);
            let (x1, y1) = (((b[2] * w as f32) as usize).min(w as usize), ((b[3] * h as f32) as usize).min(h as usize));
            let mut inside = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
            for y in y0..y1 { inside.extend_from_slice(&small[(y * w as usize + x0) * 4..(y * w as usize + x1) * 4]); }
            let g = develop::grey_world(&inside);
            self.auto_gains = [0, 1, 2].map(|i| self.auto_gains[i] + (g[i] - self.auto_gains[i]) * 0.5);
        }
    }

    /// "Lock": the measured crop becomes the profile's fixed window.
    /// Returns whether there was one to lock.
    pub fn lock_crop(&mut self) -> bool {
        let Some(c) = self.measured_crop else { return false };
        self.profile_mut().crop = Some(c);
        true
    }

    pub fn unlock_crop(&mut self) { self.profile_mut().crop = None; }

    pub fn nudge_ev(&mut self, thirds: i32) {
        self.ev = ((self.ev / EV_STEP).round() + thirds as f32).clamp(-EV_RANGE / EV_STEP, EV_RANGE / EV_STEP) * EV_STEP;
        if self.ev.abs() < 1e-4 { self.ev = 0.0; }
    }

    /// "+⅔", "−1⅓", "0" — how a camera writes EV.
    pub fn ev_label(&self) -> String {
        let t = (self.ev / EV_STEP).round() as i32;
        if t == 0 { return "±0".into(); }
        let (whole, frac) = (t.abs() / 3, t.abs() % 3);
        let f = ["", "⅓", "⅔"][frac as usize];
        let sign = if t > 0 { "+" } else { "−" };
        if whole == 0 { format!("{sign}{f}") } else { format!("{sign}{whole}{f}") }
    }

    /// The line over the viewfinder: what is changing the picture.
    pub fn readout(&self, meter_ev: Option<f32>) -> String {
        let mut parts = vec![format!("EV {}", self.ev_label()), self.wb.label()];
        let p = self.profile();
        if p.name != LensProfile::none().name { parts.push(p.name.clone()); }
        if p.crop.is_some() { parts.push("crop locked".into()); }
        else if self.measuring() { parts.push(if self.measured_crop.is_some() { "autocrop".into() } else { "autocrop: no border".into() }); }
        match p.focus {
            FocusHint::Manual(f) => parts.push(format!("focus {:.0}%", f * 100.0)),
            FocusHint::Auto => parts.push("AF".into()),
            FocusHint::Camera => {}
        }
        if self.cover_active() { parts.push("cover".into()); }
        if let Some(m) = meter_ev { parts.push(format!("meter {:+.1}", m)); }
        parts.join(" · ")
    }

    /// Settings-file lines.
    pub fn conf(&self) -> String {
        let o = self.overlays;
        format!("lens = {}\nev = {:.4}\nwb = {}\nhistogram = {}\nzebra = {}\npeaking = {}\ngrid = {}\ncover = {}\n",
                self.profile().name, self.ev, self.wb.to_conf(), o.histogram, o.zebra, o.peaking, o.grid, self.cover_on)
    }

    /// Read what `conf` wrote; anything missing keeps its default.
    pub fn apply_conf(&mut self, get: impl Fn(&str) -> Option<String>) {
        if let Some(name) = get("lens") {
            if let Some(i) = self.lenses.iter().position(|p| p.name == name) { self.lens = i; }
        }
        if let Some(ev) = get("ev").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()) {
            self.ev = 0.0;
            self.nudge_ev((ev / EV_STEP).round() as i32);
        }
        if let Some(wb) = get("wb") { self.wb = WhiteBalance::from_conf(&wb); }
        let b = |k: &str| get(k).map(|v| v == "true").unwrap_or(false);
        self.overlays = Overlays { histogram: b("histogram"), zebra: b("zebra"), peaking: b("peaking"), grid: b("grid") };
        self.cover_on = b("cover");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pro_with(name: &str) -> Pro {
        let mut p = Pro::default();
        p.lens = p.lenses.iter().position(|l| l.name == name).unwrap();
        p
    }

    #[test]
    fn no_lens_and_neutral_settings_do_not_develop() {
        let p = Pro::default();
        assert!(!p.develops() && !p.needs_cpu());
        let mut p = Pro::default();
        p.overlays.grid = true;
        assert!(!p.needs_cpu(), "the grid is drawn, not computed");
        p.overlays.histogram = true;
        assert!(p.needs_cpu() && !p.develops());
    }

    #[test]
    fn the_anamorphic_file_is_desqueezed_at_native_height() {
        let p = pro_with("Moment Anamorphic 1.33x");
        assert!(p.develops());
        let full = p.params([0.0, 0.0, 1.0, 1.0], 1280, 720, None);
        assert_eq!((full.out_w, full.out_h), (1702, 720));
        let prev = p.params([0.0, 0.0, 1.0, 1.0], 1280, 720, Some((480, 480)));
        assert_eq!(prev.out_w, 480);
        assert!(((prev.out_w as f32 / prev.out_h as f32) - 1702.0 / 720.0).abs() < 0.02);
    }

    #[test]
    fn the_crop_and_the_zoom_compose() {
        let mut p = pro_with("Moment Fisheye 14mm");
        assert!(p.measuring());
        p.measured_crop = Some([0.25, 0.0, 0.75, 1.0]);
        // 2× zoom on the middle of the crop.
        let w = p.params([0.25, 0.25, 0.75, 0.75], 1000, 500, None).window;
        assert_eq!(w, [0.375, 0.25, 0.625, 0.75]);
        // A lock pins it; the measurement no longer matters.
        assert!(p.lock_crop());
        p.measured_crop = Some([0.0, 0.0, 1.0, 1.0]);
        assert!(!p.measuring());
        assert_eq!(p.base_window(), [0.25, 0.0, 0.75, 1.0]);
        p.unlock_crop();
        assert_eq!(p.base_window(), [0.0, 0.0, 1.0, 1.0]);
        // Another lens starts with no measurement.
        p.measured_crop = Some([0.1, 0.1, 0.9, 0.9]);
        p.select_lens(0);
        assert_eq!(p.base_window(), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn ev_moves_in_thirds_and_reads_like_a_camera() {
        let mut p = Pro::default();
        assert_eq!(p.ev_label(), "±0");
        p.nudge_ev(2);
        assert_eq!(p.ev_label(), "+⅔");
        p.nudge_ev(2);
        assert_eq!(p.ev_label(), "+1⅓");
        p.nudge_ev(-10);
        assert_eq!(p.ev_label(), "−2");
        p.nudge_ev(-100);
        assert!((p.ev + 3.0).abs() < 1e-4, "clamped at −3: {}", p.ev);
    }

    #[test]
    fn a_lens_brings_its_focus_and_save_records_the_current_one() {
        let ctl = hwctl::Controls { device: "/dev/video0".into(), ctrls: hwctl::parse(
            "focus_absolute 0x009a090a (int) : min=0 max=250 step=5 default=0 value=100\n\
             focus_automatic_continuous 0x009a090c (bool) : default=1 value=1\n") };
        let mut p = pro_with("Moment Tele 58mm");
        assert!(p.lens_focus(&ctl).is_empty(), "a hint of `camera` leaves focus alone");
        p.profile_mut().focus = FocusHint::Manual(0.8);
        assert_eq!(p.lens_focus(&ctl), vec![("focus_automatic_continuous".into(), 0), ("focus_absolute".into(), 200)]);
        p.profile_mut().focus = FocusHint::Auto;
        assert_eq!(p.lens_focus(&ctl), vec![("focus_automatic_continuous".into(), 1)]);
        // Save: AF on records `auto`; AF off records the position.
        assert!(p.capture_focus(&ctl));
        assert_eq!(p.profile().focus, FocusHint::Auto);
        let mut manual = ctl.clone();
        manual.note_applied(&[("focus_automatic_continuous".into(), 0)]);
        p.capture_focus(&manual);
        assert_eq!(p.profile().focus, FocusHint::Manual(0.4));
        assert!(p.readout(None).contains("focus 40%"));
        // Picking the macro turns peaking on.
        let mut q = Pro::default();
        q.select_lens(q.lenses.iter().position(|l| l.name.contains("Macro")).unwrap());
        assert!(q.overlays.peaking);
    }

    #[test]
    fn the_cover_is_applied_only_when_measured_and_on() {
        let mut p = Pro::default();
        p.cover_on = true;
        assert!(!p.develops(), "on, but nothing measured: nothing to do");
        p.cover = crate::cover::CoverProfile { gains: [1.0, 1.0, 1.3], flare: [0.02; 3] };
        assert!(p.develops());
        let prm = p.params([0.0, 0.0, 1.0, 1.0], 100, 50, None);
        assert_eq!((prm.gains[2], prm.flare), (1.3, [0.02; 3]));
        p.cover_on = false;
        assert_eq!(p.params([0.0, 0.0, 1.0, 1.0], 100, 50, None).flare, [0.0; 3]);
    }

    #[test]
    fn settings_round_trip() {
        let mut p = pro_with("Moment Tele 58mm");
        p.nudge_ev(-2);
        p.wb = WhiteBalance::Kelvin(3200);
        p.overlays = Overlays { histogram: true, zebra: true, peaking: false, grid: true };
        let text = p.conf();
        let ini = config_kit::Ini::parse(&text);
        let mut q = Pro::default();
        q.apply_conf(|k| ini.top(k).map(String::from));
        assert_eq!((q.profile().name.as_str(), q.ev_label(), q.wb, q.overlays), ("Moment Tele 58mm", p.ev_label(), p.wb, p.overlays));
    }

    #[test]
    fn a_fisheye_frame_measures_to_a_crop_and_the_render_is_inside_the_circle() {
        // End to end on pixels: a lit disc on black → measured → the shutter's
        // params cover only the lit part.
        let (w, h) = (640u32, 360u32);
        let mut img = Vec::new();
        for y in 0..h { for x in 0..w {
            let d = ((x as f32 - 320.0).powi(2) + (y as f32 - 180.0).powi(2)).sqrt();
            let c = if d < 170.0 { 160 } else { 2 };
            img.extend_from_slice(&[c, c, c, 255]);
        } }
        let mut p = pro_with("Moment Fisheye 14mm");
        for _ in 0..6 {
            let m = p.measure_params(w, h);
            let small = develop::render(&img, w, h, &m);
            p.measured(&small, m.out_w, m.out_h);
        }
        let win = p.base_window();
        assert!(win != [0.0, 0.0, 1.0, 1.0], "nothing measured");
        let shot = p.params([0.0, 0.0, 1.0, 1.0], w, h, None);
        let out = develop::render(&img, w, h, &shot);
        let dark = out.chunks_exact(4).filter(|px| px[0] < 50).count();
        assert!(dark * 100 < out.len() / 4, "the file still has border in it: {dark} dark pixels");
    }
}
//! pro — the DSLR-style settings as data: lens, crop, exposure
//! compensation, white balance, the overlays, and what they add up to for
//! `develop::render`.
//!
//! Headless, so "which window does the shutter render, at what size, with
//! which gains" is answered and tested without a window. The shell owns the
//! widgets and the timing (when autocrop re-measures); this owns the
//! arithmetic.

use crate::develop::{self, Params, WhiteBalance};
use crate::hwctl::{self, Role};
use crate::lens::{self, AutoCrop, FocusHint, LensProfile};

/// Which aids are drawn over the viewfinder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overlays {
    pub histogram: bool,
    pub zebra: bool,
    pub peaking: bool,
    pub grid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pro {
    pub lenses: Vec<LensProfile>,
    /// Index into `lenses`.
    pub lens: usize,
    /// The autocrop's current (settled) measurement.
    pub measured_crop: Option<[f32; 4]>,
    /// Exposure compensation, stops, in thirds.
    pub ev: f32,
    pub wb: WhiteBalance,
    /// Grey-world gains, re-measured while `wb` is Auto.
    pub auto_gains: [f32; 3],
    pub overlays: Overlays,
    /// Profiles that did not load, for the panel to show.
    pub load_errors: Vec<String>,
    /// The camera cover's measured tint and haze, and whether to undo them.
    pub cover: crate::cover::CoverProfile,
    pub cover_on: bool,
}

impl Default for Pro {
    fn default() -> Self {
        Pro { lenses: lens::builtins(), lens: 0, measured_crop: None, ev: 0.0, wb: WhiteBalance::AsShot,
              auto_gains: [1.0; 3], overlays: Overlays::default(), load_errors: vec![],
              cover: crate::cover::CoverProfile::none(), cover_on: false }
    }
}

/// EV range and step: ±3 stops in thirds, a camera's dial.
pub const EV_RANGE: f32 = 3.0;
pub const EV_STEP: f32 = 1.0 / 3.0;
/// The size autocrop and auto-WB measure at.
pub const MEASURE_W: u32 = 160;

/// `inner` (fractions of `outer`'s picture) in frame fractions.
pub fn compose(outer: [f32; 4], inner: [f32; 4]) -> [f32; 4] {
    let (w, h) = (outer[2] - outer[0], outer[3] - outer[1]);
    [outer[0] + inner[0] * w, outer[1] + inner[1] * h, outer[0] + inner[2] * w, outer[1] + inner[3] * h]
}

impl Pro {
    /// Built-ins plus the profiles in `dir`.
    pub fn with_lenses_from(dir: &std::path::Path) -> Pro {
        let (lenses, load_errors) = lens::load(dir);
        Pro { lenses, load_errors, ..Pro::default() }
    }

    pub fn profile(&self) -> &LensProfile { &self.lenses[self.lens.min(self.lenses.len() - 1)] }
    pub fn profile_mut(&mut self) -> &mut LensProfile { let i = self.lens.min(self.lenses.len() - 1); &mut self.lenses[i] }

    /// Pick a lens. Its `peaking = true` turns the aid on (never off: an
    /// aid the user chose stays).
    pub fn select_lens(&mut self, i: usize) {
        if i < self.lenses.len() && i != self.lens {
            self.lens = i;
            self.measured_crop = None;
            if self.profile().peaking { self.overlays.peaking = true; }
        }
    }

    /// What the lens's focus hint asks of the camera's controls: autofocus
    /// on, or manual focus at the fraction of this driver's range. Empty
    /// when the hint is "camera" or the camera has no such control.
    pub fn lens_focus(&self, ctl: &hwctl::Controls) -> Vec<(String, i64)> {
        match self.profile().focus {
            FocusHint::Camera => vec![],
            FocusHint::Auto => ctl.get(Role::AutoFocus).map(|_| ctl.plan_set(Role::AutoFocus, 1)).unwrap_or_default(),
            FocusHint::Manual(f) => ctl.get(Role::Focus)
                .map(|c| ctl.plan_set(Role::Focus, c.min + ((c.max - c.min) as f32 * f).round() as i64)).unwrap_or_default(),
        }
    }

    /// Record the camera's current focus into the profile, for "save": the
    /// manual position as a fraction when autofocus is off, `auto` when on.
    /// Returns whether anything was recorded.
    pub fn capture_focus(&mut self, ctl: &hwctl::Controls) -> bool {
        let af_on = ctl.get(Role::AutoFocus).map(|a| a.value != 0);
        let hint = match (af_on, ctl.get(Role::Focus)) {
            (Some(true), _) => FocusHint::Auto,
            (_, Some(c)) if c.max > c.min => FocusHint::Manual(((c.value - c.min) as f32 / (c.max - c.min) as f32).clamp(0.0, 1.0)),
            _ => return false,
        };
        self.profile_mut().focus = hint;
        true
    }

    /// The crop before zoom: the profile's fixed window, else the autocrop's
    /// measurement, else the whole frame.
    pub fn base_window(&self) -> [f32; 4] {
        let p = self.profile();
        p.crop.or(if p.autocrop == AutoCrop::Auto { self.measured_crop } else { None }).unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    /// Is autocrop measuring right now (on, and not overridden by a lock)?
    pub fn measuring(&self) -> bool { self.profile().autocrop == AutoCrop::Auto && self.profile().crop.is_none() }

    /// Does the picture need `develop::render` at all?
    pub fn develops(&self) -> bool {
        !self.profile().is_identity() || self.ev.abs() > 1e-3 || self.wb != WhiteBalance::AsShot || self.cover_active()
    }

    /// Is a measured cover correction being applied?
    pub fn cover_active(&self) -> bool { self.cover_on && self.cover != crate::cover::CoverProfile::none() }
    pub fn flare(&self) -> [f32; 3] { if self.cover_active() { self.cover.flare } else { [0.0; 3] } }
    pub fn cover_gains(&self) -> [f32; 3] { if self.cover_active() { self.cover.gains } else { [1.0; 3] } }

    /// Does the preview need the CPU path — developing, or measuring for an
    /// overlay the GPU path cannot compute?
    pub fn needs_cpu(&self) -> bool {
        self.develops() || self.overlays.histogram || self.overlays.zebra || self.overlays.peaking
    }

    /// Cover tint, then white balance and exposure.
    pub fn gains(&self) -> [f32; 3] {
        let g = develop::gains(self.wb, self.auto_gains, self.ev);
        let c = self.cover_gains();
        [g[0] * c[0], g[1] * c[1], g[2] * c[2]]
    }

    /// The render for a `sw × sh` frame with the zoom window `zoom` (in
    /// fractions of the cropped picture): native size, or fitted inside
    /// `fit` for the preview.
    pub fn params(&self, zoom: [f32; 4], sw: u32, sh: u32, fit: Option<(u32, u32)>) -> Params {
        let p = self.profile();
        let window = compose(self.base_window(), zoom);
        let native = develop::native_size(window, sw, sh, p.desqueeze);
        let (out_w, out_h) = match fit { Some((mw, mh)) => develop::fit_size(native, mw, mh), None => native };
        Params { window, out_w, out_h, distortion: p.distortion, vignetting: p.vignetting, gains: self.gains(), flare: self.flare() }
    }

    /// What autocrop and auto-WB measure: the whole corrected frame, small,
    /// neutral colour, no desqueeze (the crop is found in frame fractions).
    pub fn measure_params(&self, sw: u32, sh: u32) -> Params {
        let p = self.profile();
        let (w, h) = develop::fit_size((sw, sh), MEASURE_W, MEASURE_W);
        Params { window: [0.0, 0.0, 1.0, 1.0], out_w: w, out_h: h, distortion: p.distortion, vignetting: p.vignetting, gains: self.cover_gains(), flare: self.flare() }
    }

    /// Fold one measurement in: the settled crop, and grey-world gains when
    /// WB is auto (measured inside the crop, so the black border does not
    /// drag the average).
    pub fn measured(&mut self, small: &[u8], w: u32, h: u32) {
        if self.measuring() {
            let m = lens::detect_content(small, w, h, Some(w as f32 / h as f32));
            self.measured_crop = lens::settle(self.measured_crop, m);
        }
        if self.wb == WhiteBalance::Auto {
            let b = self.base_window();
            let (x0, y0) = ((b[0] * w as f32) as usize, (b[1] * h as f32) as usize);
            let (x1, y1) = (((b[2] * w as f32) as usize).min(w as usize), ((b[3] * h as f32) as usize).min(h as usize));
            let mut inside = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
            for y in y0..y1 { inside.extend_from_slice(&small[(y * w as usize + x0) * 4..(y * w as usize + x1) * 4]); }
            let g = develop::grey_world(&inside);
            self.auto_gains = [0, 1, 2].map(|i| self.auto_gains[i] + (g[i] - self.auto_gains[i]) * 0.5);
        }
    }

    /// "Lock": the measured crop becomes the profile's fixed window.
    /// Returns whether there was one to lock.
    pub fn lock_crop(&mut self) -> bool {
        let Some(c) = self.measured_crop else { return false };
        self.profile_mut().crop = Some(c);
        true
    }

    pub fn unlock_crop(&mut self) { self.profile_mut().crop = None; }

    pub fn nudge_ev(&mut self, thirds: i32) {
        self.ev = ((self.ev / EV_STEP).round() + thirds as f32).clamp(-EV_RANGE / EV_STEP, EV_RANGE / EV_STEP) * EV_STEP;
        if self.ev.abs() < 1e-4 { self.ev = 0.0; }
    }

    /// "+⅔", "−1⅓", "0" — how a camera writes EV.
    pub fn ev_label(&self) -> String {
        let t = (self.ev / EV_STEP).round() as i32;
        if t == 0 { return "±0".into(); }
        let (whole, frac) = (t.abs() / 3, t.abs() % 3);
        let f = ["", "⅓", "⅔"][frac as usize];
        let sign = if t > 0 { "+" } else { "−" };
        if whole == 0 { format!("{sign}{f}") } else { format!("{sign}{whole}{f}") }
    }

    /// The line over the viewfinder: what is changing the picture.
    pub fn readout(&self, meter_ev: Option<f32>) -> String {
        let mut parts = vec![format!("EV {}", self.ev_label()), self.wb.label()];
        let p = self.profile();
        if p.name != LensProfile::none().name { parts.push(p.name.clone()); }
        if p.crop.is_some() { parts.push("crop locked".into()); }
        else if self.measuring() { parts.push(if self.measured_crop.is_some() { "autocrop".into() } else { "autocrop: no border".into() }); }
        match p.focus {
            FocusHint::Manual(f) => parts.push(format!("focus {:.0}%", f * 100.0)),
            FocusHint::Auto => parts.push("AF".into()),
            FocusHint::Camera => {}
        }
        if self.cover_active() { parts.push("cover".into()); }
        if let Some(m) = meter_ev { parts.push(format!("meter {:+.1}", m)); }
        parts.join(" · ")
    }

    /// Settings-file lines.
    pub fn conf(&self) -> String {
        let o = self.overlays;
        format!("lens = {}\nev = {:.4}\nwb = {}\nhistogram = {}\nzebra = {}\npeaking = {}\ngrid = {}\ncover = {}\n",
                self.profile().name, self.ev, self.wb.to_conf(), o.histogram, o.zebra, o.peaking, o.grid, self.cover_on)
    }

    /// Read what `conf` wrote; anything missing keeps its default.
    pub fn apply_conf(&mut self, get: impl Fn(&str) -> Option<String>) {
        if let Some(name) = get("lens") {
            if let Some(i) = self.lenses.iter().position(|p| p.name == name) { self.lens = i; }
        }
        if let Some(ev) = get("ev").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()) {
            self.ev = 0.0;
            self.nudge_ev((ev / EV_STEP).round() as i32);
        }
        if let Some(wb) = get("wb") { self.wb = WhiteBalance::from_conf(&wb); }
        let b = |k: &str| get(k).map(|v| v == "true").unwrap_or(false);
        self.overlays = Overlays { histogram: b("histogram"), zebra: b("zebra"), peaking: b("peaking"), grid: b("grid") };
        self.cover_on = b("cover");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pro_with(name: &str) -> Pro {
        let mut p = Pro::default();
        p.lens = p.lenses.iter().position(|l| l.name == name).unwrap();
        p
    }

    #[test]
    fn no_lens_and_neutral_settings_do_not_develop() {
        let p = Pro::default();
        assert!(!p.develops() && !p.needs_cpu());
        let mut p = Pro::default();
        p.overlays.grid = true;
        assert!(!p.needs_cpu(), "the grid is drawn, not computed");
        p.overlays.histogram = true;
        assert!(p.needs_cpu() && !p.develops());
    }

    #[test]
    fn the_anamorphic_file_is_desqueezed_at_native_height() {
        let p = pro_with("Moment Anamorphic 1.33x");
        assert!(p.develops());
        let full = p.params([0.0, 0.0, 1.0, 1.0], 1280, 720, None);
        assert_eq!((full.out_w, full.out_h), (1702, 720));
        let prev = p.params([0.0, 0.0, 1.0, 1.0], 1280, 720, Some((480, 480)));
        assert_eq!(prev.out_w, 480);
        assert!(((prev.out_w as f32 / prev.out_h as f32) - 1702.0 / 720.0).abs() < 0.02);
    }

    #[test]
    fn the_crop_and_the_zoom_compose() {
        let mut p = pro_with("Moment Fisheye 14mm");
        assert!(p.measuring());
        p.measured_crop = Some([0.25, 0.0, 0.75, 1.0]);
        // 2× zoom on the middle of the crop.
        let w = p.params([0.25, 0.25, 0.75, 0.75], 1000, 500, None).window;
        assert_eq!(w, [0.375, 0.25, 0.625, 0.75]);
        // A lock pins it; the measurement no longer matters.
        assert!(p.lock_crop());
        p.measured_crop = Some([0.0, 0.0, 1.0, 1.0]);
        assert!(!p.measuring());
        assert_eq!(p.base_window(), [0.25, 0.0, 0.75, 1.0]);
        p.unlock_crop();
        assert_eq!(p.base_window(), [0.0, 0.0, 1.0, 1.0]);
        // Another lens starts with no measurement.
        p.measured_crop = Some([0.1, 0.1, 0.9, 0.9]);
        p.select_lens(0);
        assert_eq!(p.base_window(), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn ev_moves_in_thirds_and_reads_like_a_camera() {
        let mut p = Pro::default();
        assert_eq!(p.ev_label(), "±0");
        p.nudge_ev(2);
        assert_eq!(p.ev_label(), "+⅔");
        p.nudge_ev(2);
        assert_eq!(p.ev_label(), "+1⅓");
        p.nudge_ev(-10);
        assert_eq!(p.ev_label(), "−2");
        p.nudge_ev(-100);
        assert!((p.ev + 3.0).abs() < 1e-4, "clamped at −3: {}", p.ev);
    }

    #[test]
    fn a_lens_brings_its_focus_and_save_records_the_current_one() {
        let ctl = hwctl::Controls { device: "/dev/video0".into(), ctrls: hwctl::parse(
            "focus_absolute 0x009a090a (int) : min=0 max=250 step=5 default=0 value=100\n\
             focus_automatic_continuous 0x009a090c (bool) : default=1 value=1\n") };
        let mut p = pro_with("Moment Tele 58mm");
        assert!(p.lens_focus(&ctl).is_empty(), "a hint of `camera` leaves focus alone");
        p.profile_mut().focus = FocusHint::Manual(0.8);
        assert_eq!(p.lens_focus(&ctl), vec![("focus_automatic_continuous".into(), 0), ("focus_absolute".into(), 200)]);
        p.profile_mut().focus = FocusHint::Auto;
        assert_eq!(p.lens_focus(&ctl), vec![("focus_automatic_continuous".into(), 1)]);
        // Save: AF on records `auto`; AF off records the position.
        assert!(p.capture_focus(&ctl));
        assert_eq!(p.profile().focus, FocusHint::Auto);
        let mut manual = ctl.clone();
        manual.note_applied(&[("focus_automatic_continuous".into(), 0)]);
        p.capture_focus(&manual);
        assert_eq!(p.profile().focus, FocusHint::Manual(0.4));
        assert!(p.readout(None).contains("focus 40%"));
        // Picking the macro turns peaking on.
        let mut q = Pro::default();
        q.select_lens(q.lenses.iter().position(|l| l.name.contains("Macro")).unwrap());
        assert!(q.overlays.peaking);
    }

    #[test]
    fn the_cover_is_applied_only_when_measured_and_on() {
        let mut p = Pro::default();
        p.cover_on = true;
        assert!(!p.develops(), "on, but nothing measured: nothing to do");
        p.cover = crate::cover::CoverProfile { gains: [1.0, 1.0, 1.3], flare: [0.02; 3] };
        assert!(p.develops());
        let prm = p.params([0.0, 0.0, 1.0, 1.0], 100, 50, None);
        assert_eq!((prm.gains[2], prm.flare), (1.3, [0.02; 3]));
        p.cover_on = false;
        assert_eq!(p.params([0.0, 0.0, 1.0, 1.0], 100, 50, None).flare, [0.0; 3]);
    }

    #[test]
    fn settings_round_trip() {
        let mut p = pro_with("Moment Tele 58mm");
        p.nudge_ev(-2);
        p.wb = WhiteBalance::Kelvin(3200);
        p.overlays = Overlays { histogram: true, zebra: true, peaking: false, grid: true };
        let text = p.conf();
        let ini = config_kit::Ini::parse(&text);
        let mut q = Pro::default();
        q.apply_conf(|k| ini.top(k).map(String::from));
        assert_eq!((q.profile().name.as_str(), q.ev_label(), q.wb, q.overlays), ("Moment Tele 58mm", p.ev_label(), p.wb, p.overlays));
    }

    #[test]
    fn a_fisheye_frame_measures_to_a_crop_and_the_render_is_inside_the_circle() {
        // End to end on pixels: a lit disc on black → measured → the shutter's
        // params cover only the lit part.
        let (w, h) = (640u32, 360u32);
        let mut img = Vec::new();
        for y in 0..h { for x in 0..w {
            let d = ((x as f32 - 320.0).powi(2) + (y as f32 - 180.0).powi(2)).sqrt();
            let c = if d < 170.0 { 160 } else { 2 };
            img.extend_from_slice(&[c, c, c, 255]);
        } }
        let mut p = pro_with("Moment Fisheye 14mm");
        for _ in 0..6 {
            let m = p.measure_params(w, h);
            let small = develop::render(&img, w, h, &m);
            p.measured(&small, m.out_w, m.out_h);
        }
        let win = p.base_window();
        assert!(win != [0.0, 0.0, 1.0, 1.0], "nothing measured");
        let shot = p.params([0.0, 0.0, 1.0, 1.0], w, h, None);
        let out = develop::render(&img, w, h, &shot);
        let dark = out.chunks_exact(4).filter(|px| px[0] < 50).count();
        assert!(dark * 100 < out.len() / 4, "the file still has border in it: {dark} dark pixels");
    }
}
