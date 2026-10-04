//! Runtime knobs, all from the environment so they can be changed on the
//! device (or in an AppLoad manifest, or by gitmind at launch) without a
//! rebuild. Every value in effect is printed at startup.

use crate::policy::Profile;
use crate::touch::Transform;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// `EINK_PROFILE`; `None` = pick from the panel (colour → fast-color).
    pub profile: Option<Profile>,
    /// `EINK_BACKEND` = auto | qtfb | blight | mem.
    pub backend: String,
    /// `EINK_SCALE`: egui pixels-per-point. The panels are 226–264 dpi, so 2.
    pub scale: f32,
    /// `EINK_FEATHER` = 0|1 overrides the profile's anti-aliasing default.
    pub feather: Option<bool>,
    /// `EINK_TRACE=1`: print every refresh decision.
    pub trace: bool,
    /// `EINK_TOUCH`: calibration flags for raw evdev input.
    pub touch: Transform,
    /// `EINK_PEN`: the same flags for the pen digitizer, whose axes differ
    /// from the touchscreen's on these devices.
    pub pen: Transform,
    /// `EINK_FAST_WAVEFORM`: what "fast" means on the device (ufast|fast|animate).
    pub fast_waveform: Option<String>,
    /// `EINK_SETTLE_MS`: quiet time before the clean-up refresh.
    pub settle: Duration,
    /// `EINK_MIN_FRAME_MS`: floor between frames an app's animation can force.
    pub min_frame: Duration,
    /// `EINK_THREADS`: raster workers.
    pub threads: usize,
    /// mem backend: `EINK_MEM_SIZE=WxH`, `EINK_DUMP=dir`.
    /// `EINK_MOTION_HZ`: refresh cap for moving regions drawn mono (default 6).
    pub motion_hz: f32,
    /// `EINK_MOTION_COLOR=1`: keep colour while moving (then `EINK_MOTION_COLOR_HZ`, default 2).
    pub motion_color: bool,
    pub motion_color_hz: f32,
    /// `EINK_DITHER=0` disables ordered dithering of mono motion.
    pub dither: bool,
    pub mem_size: (u32, u32),
    pub dump: Option<String>,
    /// `EINK_INPUT`: where touch and pen come from under qtfb. `qtfb` (default): the display server's forwarded input,
    /// which on the Paper Pro arrives at ~7 reports/s although the digitizer reports at ~60 Hz. `evdev`: read the
    /// FINGER directly (non-exclusive) and drop the forwarded finger touch; the pen still comes from the display
    /// server. `evdev-pen-compare`: as `evdev`,
    /// and the direct pen reader only LOGS (to check its mapping). `evdev-all`: finger and pen both direct. `compare`: forwarded input drives the app,
    /// the direct readers only LOG what they see, to check their coordinates first.
    pub input: String,
    /// `EINK_FULL_EVERY_PAGES`: full refresh every N page turns (0 = never).
    pub full_every_pages: u32,
    /// `EINK_THEME=egui` keeps egui's own theme instead of the e-ink preset.
    pub eink_theme: bool,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let profile = match env("EINK_PROFILE") {
            Some(s) => Some(Profile::parse(&s).ok_or_else(|| format!("EINK_PROFILE={s:?}: want fast-color|fast-mono|quality"))?),
            None => None,
        };
        let touch = Transform::parse(&env("EINK_TOUCH").unwrap_or_default())?;
        let pen = Transform::parse(&env("EINK_PEN").unwrap_or_default()).map_err(|e| e.replace("EINK_TOUCH", "EINK_PEN"))?;
        let num = |k: &str, d: u64| env(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let mem_size = env("EINK_MEM_SIZE")
            .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
            .unwrap_or((702, 936));
        Ok(Config {
            profile,
            backend: env("EINK_BACKEND").unwrap_or_else(|| "auto".into()),
            scale: env("EINK_SCALE").and_then(|v| v.parse().ok()).unwrap_or(2.0),
            feather: env("EINK_FEATHER").map(|v| v == "1"),
            trace: env("EINK_TRACE").as_deref() == Some("1"),
            touch,
            pen,
            fast_waveform: env("EINK_FAST_WAVEFORM"),
            settle: Duration::from_millis(num("EINK_SETTLE_MS", 450)),
            min_frame: Duration::from_millis(num("EINK_MIN_FRAME_MS", 120)),
            threads: num("EINK_THREADS", std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(2)) as usize,
            motion_hz: env("EINK_MOTION_HZ").and_then(|v| v.parse().ok()).unwrap_or(6.0),
            motion_color: env("EINK_MOTION_COLOR").as_deref() == Some("1"),
            motion_color_hz: env("EINK_MOTION_COLOR_HZ").and_then(|v| v.parse().ok()).unwrap_or(2.0),
            dither: env("EINK_DITHER").as_deref() != Some("0"),
            mem_size,
            dump: env("EINK_DUMP"),
            input: env("EINK_INPUT").unwrap_or_else(|| "qtfb".into()),
            eink_theme: env("EINK_THEME").as_deref() != Some("egui"),
            full_every_pages: num("EINK_FULL_EVERY_PAGES", 6) as u32,
        })
    }
}
