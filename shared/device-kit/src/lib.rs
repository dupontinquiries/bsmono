//! device-kit — audio/video device selection + format negotiation, done once.
//!
//! snapclone's camera-bridge, bo-suite's soundscape engine, voiceterm/krapper's mic
//! handling, logscope, beatspace — they all re-derive the same plumbing: list devices, pick
//! one by name with a fallback, negotiate a workable format, and (for virtual cameras) load
//! v4l2loopback. This crate is that, version-agnostic and testable; the actual PipeWire/v4l2
//! capture is on-device behind a feature. Reference: snapclone's setup_v4l2loopback.sh.

/// What a device is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    AudioInput,
    AudioOutput,
    VideoCapture,
    /// A v4l2loopback / virtual output sink.
    VideoOutput,
}

/// An enumerated device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub kind: DeviceKind,
    pub id: String,
    pub name: String,
    pub path: String,
}

impl Device {
    pub fn new(kind: DeviceKind, id: &str, name: &str, path: &str) -> Device {
        Device { kind, id: id.to_string(), name: name.to_string(), path: path.to_string() }
    }
}

/// Select a device of `kind`: if `prefer_name` is given, a case-insensitive name-substring
/// match wins; otherwise the first device of that kind. `None` if there are none.
pub fn pick_device<'a>(devices: &'a [Device], kind: DeviceKind, prefer_name: Option<&str>) -> Option<&'a Device> {
    let of_kind: Vec<&Device> = devices.iter().filter(|d| d.kind == kind).collect();
    if let Some(want) = prefer_name {
        let want = want.to_lowercase();
        if let Some(d) = of_kind.iter().find(|d| d.name.to_lowercase().contains(&want)) {
            return Some(d);
        }
    }
    of_kind.into_iter().next()
}

// ── video format negotiation ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fourcc: String,
}

impl VideoFormat {
    pub fn new(width: u32, height: u32, fps: u32, fourcc: &str) -> VideoFormat {
        VideoFormat { width, height, fps, fourcc: fourcc.to_string() }
    }
    fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }
}

/// Pick the supported video format closest to `desired`: prefer the same fourcc when
/// available, then the nearest resolution (by area), then the highest fps not exceeding the
/// desired fps (falling back to the closest fps if none are ≤).
pub fn pick_video_format(supported: &[VideoFormat], desired: &VideoFormat) -> Option<VideoFormat> {
    if supported.is_empty() {
        return None;
    }
    // restrict to matching fourcc if any match, else consider all
    let want_cc = desired.fourcc.to_lowercase();
    let pool: Vec<&VideoFormat> = {
        let matching: Vec<&VideoFormat> = supported.iter().filter(|f| f.fourcc.to_lowercase() == want_cc).collect();
        if matching.is_empty() {
            supported.iter().collect()
        } else {
            matching
        }
    };
    let target_area = desired.area();
    pool.into_iter()
        .min_by(|a, b| {
            let da = (a.area() - target_area).abs();
            let db = (b.area() - target_area).abs();
            da.cmp(&db).then_with(|| {
                // among equal-distance resolutions, prefer fps ≤ desired, highest such
                let sa = fps_score(a.fps, desired.fps);
                let sb = fps_score(b.fps, desired.fps);
                sb.cmp(&sa) // higher score first
            })
        })
        .cloned()
}

fn fps_score(fps: u32, desired: u32) -> i64 {
    if fps <= desired {
        // prefer close-to-desired but not over: score grows toward desired
        fps as i64
    } else {
        // over-desired is penalized below any ≤ option
        -(fps as i64 - desired as i64) - 1_000_000
    }
}

// ── audio format negotiation ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub fn new(sample_rate: u32, channels: u16) -> AudioFormat {
        AudioFormat { sample_rate, channels }
    }
}

/// Pick the supported audio format closest to `desired`: nearest sample rate, then channel
/// count nearest the desired.
pub fn pick_audio_format(supported: &[AudioFormat], desired: &AudioFormat) -> Option<AudioFormat> {
    supported.iter().copied().min_by_key(|f| {
        let dr = (f.sample_rate as i64 - desired.sample_rate as i64).abs();
        let dc = (f.channels as i64 - desired.channels as i64).abs();
        dr * 8 + dc // sample rate dominates
    })
}

// ── v4l2loopback (virtual camera) ─────────────────────────────────────────────

/// Config for a v4l2loopback virtual video device (snapclone's camera sink).
#[derive(Debug, Clone)]
pub struct LoopbackConfig {
    pub device_nr: u32,
    pub card_label: String,
    pub exclusive_caps: bool,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        LoopbackConfig { device_nr: 10, card_label: "gitmind virtual camera".into(), exclusive_caps: true }
    }
}

/// The `modprobe` command to create the loopback device (mirrors setup_v4l2loopback.sh).
pub fn v4l2loopback_setup_cmd(cfg: &LoopbackConfig) -> (String, Vec<String>) {
    (
        "modprobe".to_string(),
        vec![
            "v4l2loopback".to_string(),
            format!("video_nr={}", cfg.device_nr),
            format!("card_label={}", cfg.card_label),
            format!("exclusive_caps={}", if cfg.exclusive_caps { 1 } else { 0 }),
        ],
    )
}

/// The device path for a v4l2 node number.
pub fn video_path(nr: u32) -> String {
    format!("/dev/video{nr}")
}

// ── enumeration ──────────────────────────────────────────────────────────────

/// The command to list v4l2 devices.
pub fn list_video_cmd() -> (String, Vec<String>) {
    ("v4l2-ctl".to_string(), vec!["--list-devices".to_string()])
}

/// Parse `v4l2-ctl --list-devices` output into devices. A non-indented line is a device
/// name (with a bus/platform id in parens); the indented lines under it are its /dev/video
/// nodes. A loopback device (name/id mentions "v4l2loopback"/"Dummy") maps to VideoOutput.
pub fn parse_v4l2_list(output: &str) -> Vec<Device> {
    let mut out = Vec::new();
    let mut cur_name: Option<String> = None;
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with('\t') || line.starts_with("    ");
        if !indented {
            cur_name = Some(line.trim().trim_end_matches(':').to_string());
        } else if let Some(name) = &cur_name {
            let path = line.trim().to_string();
            if path.starts_with("/dev/video") {
                let lname = name.to_lowercase();
                let kind = if lname.contains("v4l2loopback") || lname.contains("dummy") {
                    DeviceKind::VideoOutput
                } else {
                    DeviceKind::VideoCapture
                };
                // one Device per named node; id = the path's node number
                let id = path.trim_start_matches("/dev/video").to_string();
                out.push(Device::new(kind, &id, name, &path));
                // only take the first node of each device to avoid dup capture nodes
                cur_name = None;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<Device> {
        vec![
            Device::new(DeviceKind::VideoCapture, "0", "USB Camera", "/dev/video0"),
            Device::new(DeviceKind::VideoOutput, "10", "gitmind virtual camera", "/dev/video10"),
            Device::new(DeviceKind::AudioInput, "hw:1", "USB Mic", "hw:1,0"),
            Device::new(DeviceKind::AudioInput, "hw:0", "Built-in", "hw:0,0"),
        ]
    }

    #[test]
    fn pick_prefers_name_then_falls_back() {
        let d = devices();
        assert_eq!(pick_device(&d, DeviceKind::AudioInput, Some("usb")).unwrap().name, "USB Mic");
        // no name match → first of kind
        assert_eq!(pick_device(&d, DeviceKind::AudioInput, Some("nope")).unwrap().name, "USB Mic");
        assert_eq!(pick_device(&d, DeviceKind::VideoCapture, None).unwrap().path, "/dev/video0");
        assert!(pick_device(&d, DeviceKind::VideoOutput, None).is_some());
    }

    #[test]
    fn video_format_picks_nearest_resolution_and_fourcc() {
        let supported = vec![
            VideoFormat::new(640, 480, 30, "YUYV"),
            VideoFormat::new(1280, 720, 30, "MJPG"),
            VideoFormat::new(1920, 1080, 30, "MJPG"),
        ];
        // want 720p MJPG → exact
        let got = pick_video_format(&supported, &VideoFormat::new(1280, 720, 30, "MJPG")).unwrap();
        assert_eq!((got.width, got.height, got.fourcc.as_str()), (1280, 720, "MJPG"));
        // want 1000x560 MJPG → nearest by area among MJPG is 720p
        let got = pick_video_format(&supported, &VideoFormat::new(1000, 560, 30, "MJPG")).unwrap();
        assert_eq!((got.width, got.height), (1280, 720));
    }

    #[test]
    fn video_format_prefers_fps_not_over_desired() {
        let supported = vec![
            VideoFormat::new(1280, 720, 60, "MJPG"),
            VideoFormat::new(1280, 720, 30, "MJPG"),
        ];
        let got = pick_video_format(&supported, &VideoFormat::new(1280, 720, 30, "MJPG")).unwrap();
        assert_eq!(got.fps, 30, "don't exceed the requested fps when a ≤ option exists");
    }

    #[test]
    fn audio_format_nearest_rate() {
        let supported = vec![AudioFormat::new(44100, 2), AudioFormat::new(48000, 1), AudioFormat::new(16000, 1)];
        let got = pick_audio_format(&supported, &AudioFormat::new(16000, 1)).unwrap();
        assert_eq!(got.sample_rate, 16000);
        let got = pick_audio_format(&supported, &AudioFormat::new(45000, 2)).unwrap();
        assert_eq!(got.sample_rate, 44100);
    }

    #[test]
    fn v4l2loopback_command_matches_reference() {
        let (prog, args) = v4l2loopback_setup_cmd(&LoopbackConfig { device_nr: 10, card_label: "gitmind cam".into(), exclusive_caps: true });
        assert_eq!(prog, "modprobe");
        assert_eq!(args[0], "v4l2loopback");
        assert!(args.contains(&"video_nr=10".to_string()));
        assert!(args.contains(&"card_label=gitmind cam".to_string()));
        assert!(args.contains(&"exclusive_caps=1".to_string()));
        assert_eq!(video_path(10), "/dev/video10");
    }

    #[test]
    fn parse_v4l2_list_devices() {
        let out = "USB Camera (usb-0000:00:14.0-1):\n\t/dev/video0\n\t/dev/video1\n\nDummy video device (0x0000) (platform:v4l2loopback-000):\n\t/dev/video10\n";
        let devs = parse_v4l2_list(out);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].kind, DeviceKind::VideoCapture);
        assert_eq!(devs[0].path, "/dev/video0");
        assert_eq!(devs[1].kind, DeviceKind::VideoOutput); // the loopback
        assert_eq!(devs[1].path, "/dev/video10");
    }
}
