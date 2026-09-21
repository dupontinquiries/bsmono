//! capabilities: what a device HAS, surveyed once at enrolment, so builds
//! can use it.
//!
//! The monorepo's apps default to features for a capable desktop (audio,
//! GPU scenes, speech recognition, Bluetooth). A fleet is not that: a
//! Fairphone has ALSA cards, a modem and a GPU; a reMarkable Paper Pro has
//! 2 GB, Bluetooth, a pen, an e-ink panel and no sound card at all. The
//! defaults stay portable; this module decides per DEVICE which of an app's
//! default features its hardware can use, and says why it dropped the rest.
//!
//! * [`survey_cmd`]: one BusyBox-safe probe, `key=value` lines, any Linux
//!   device (tablet or phone). Run at enrolment; stored per host.
//! * [`Capabilities`]: the parsed survey.
//! * [`select_features`]: an app's default features, minus those whose
//!   hardware needs are unmet. Needs come from the app's own
//!   `[package.metadata.gitmind.needs]` table (`feature = ["audio-out"]`),
//!   else from the feature's NAME (audio, ble, gpu, whisper, camera, ...).

use std::collections::BTreeMap;

/// Every key the survey prints. The probe, the parser and the tests share it.
pub const KEYS: &[&str] = &[
    "arch", "cores", "mem_mb", "libc", "gpu_render", "gpu_vivante", "vulkan_icd", "gles",
    "alsa_cards", "bluetooth", "bluez", "audio_server", "cameras", "modem",
    "touch", "pen", "keyboard", "display", "python3",
];

pub fn survey_cmd() -> String {
    // Plain if-chains: `a && b || c` mis-associates in sh.
    r#"echo "arch=$(uname -m)"
echo "cores=$(nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo)"
echo "mem_mb=$(awk '/^MemTotal/ {print int($2/1024)}' /proc/meminfo)"
if ls /lib/ld-musl-*.so.1 >/dev/null 2>&1; then echo libc=musl; else echo libc=glibc; fi
if ls /dev/dri/renderD* >/dev/null 2>&1; then echo gpu_render=1; fi
if [ -e /dev/galcore ]; then echo gpu_vivante=1; fi
if ls /usr/share/vulkan/icd.d/*.json /etc/vulkan/icd.d/*.json >/dev/null 2>&1; then echo vulkan_icd=1; fi
for f in /usr/lib/libGLESv2.so* /usr/lib/*/libGLESv2.so* /usr/lib64/libGLESv2.so*; do if [ -e "$f" ]; then echo gles=1; break; fi; done
echo "alsa_cards=$(grep -c ']: ' /proc/asound/cards 2>/dev/null || echo 0)"
if ls -d /sys/class/bluetooth/hci* >/dev/null 2>&1; then echo bluetooth=1; fi
if command -v bluetoothctl >/dev/null 2>&1; then echo bluez=1; fi
if pidof pipewire >/dev/null 2>&1; then echo audio_server=pipewire
elif pidof pulseaudio >/dev/null 2>&1; then echo audio_server=pulse
elif pidof bluealsa >/dev/null 2>&1; then echo audio_server=bluealsa; fi
n=$(ls /dev/video* 2>/dev/null | wc -l); if [ "$n" -gt 0 ]; then echo "cameras=$n"; fi
if [ -e /dev/cdc-wdm0 ] || ls /dev/wwan* >/dev/null 2>&1 || command -v mmcli >/dev/null 2>&1; then echo modem=1; fi
if [ -r /proc/bus/input/devices ]; then
  names=$(grep '^N: Name=' /proc/bus/input/devices | tr 'A-Z' 'a-z')
  case "$names" in *touch*|*elan*|*goodix*|*himax*|*novatek*|*focal*) echo touch=1;; esac
  case "$names" in *pen*|*stylus*|*wacom*|*digitizer*|*marker*) echo pen=1;; esac
  case "$names" in *keyboard*|*keypad*) echo keyboard=1;; esac
fi
if [ -S /tmp/qtfb.sock ]; then echo display=qtfb
elif ls /run/user/*/wayland-* >/dev/null 2>&1; then echo display=wayland
elif [ -e /dev/fb0 ]; then echo display=fbdev; fi
if command -v python3 >/dev/null 2>&1; then echo python3=1; fi
"#.to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub arch: String,
    pub cores: u32,
    pub mem_mb: u32,
    pub libc: String,
    pub gpu: bool,
    pub alsa_cards: u32,
    pub bluetooth: bool,
    pub audio_server: Option<String>,
    pub cameras: u32,
    pub modem: bool,
    pub touch: bool,
    pub pen: bool,
    pub keyboard: bool,
    pub display: Option<String>,
    pub python3: bool,
    /// Everything the probe said, for display and for keys added later.
    pub raw: BTreeMap<String, String>,
}

pub fn parse(text: &str) -> Capabilities {
    let raw: BTreeMap<String, String> = text
        .lines()
        .filter_map(|l| l.trim().split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| KEYS.contains(&k.as_str()))
        .collect();
    let on = |k: &str| raw.get(k).is_some_and(|v| v == "1");
    let num = |k: &str| raw.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    Capabilities {
        arch: raw.get("arch").cloned().unwrap_or_default(),
        cores: num("cores"),
        mem_mb: num("mem_mb"),
        libc: raw.get("libc").cloned().unwrap_or_default(),
        gpu: on("gpu_render") || on("gpu_vivante") || on("vulkan_icd") || on("gles"),
        alsa_cards: num("alsa_cards"),
        bluetooth: on("bluetooth") || on("bluez"),
        audio_server: raw.get("audio_server").cloned(),
        cameras: num("cameras"),
        modem: on("modem"),
        touch: on("touch"),
        pen: on("pen"),
        keyboard: on("keyboard"),
        display: raw.get("display").cloned(),
        python3: on("python3"),
        raw,
    }
}

impl Capabilities {
    pub fn audio_out(&self) -> bool {
        self.alsa_cards > 0 || self.audio_server.is_some()
    }

    /// One line per fact, for the Tablets page and `gitmind eink survey`.
    pub fn summary(&self) -> Vec<String> {
        let yn = |b: bool| if b { "yes" } else { "no" };
        vec![
            format!("{} · {} cores · {} MB · {}", self.arch, self.cores, self.mem_mb, self.libc),
            format!("display {} · touch {} · pen {} · keyboard {}", self.display.as_deref().unwrap_or("-"), yn(self.touch), yn(self.pen), yn(self.keyboard)),
            format!("GPU {} · camera(s) {} · modem {}", yn(self.gpu), self.cameras, yn(self.modem)),
            format!("audio: {} ALSA card(s), server {} · Bluetooth {}", self.alsa_cards, self.audio_server.as_deref().unwrap_or("none"), yn(self.bluetooth)),
        ]
    }
}

/// A hardware need a feature can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    AudioOut,
    AudioIn,
    Bluetooth,
    Gpu,
    Camera,
    Modem,
    /// Memory for large models (speech recognition), in MB.
    Memory(u32),
}

impl Need {
    fn parse(s: &str) -> Option<Need> {
        Some(match s.trim() {
            "audio-out" => Need::AudioOut,
            "audio-in" | "mic" => Need::AudioIn,
            "bluetooth" => Need::Bluetooth,
            "gpu" => Need::Gpu,
            "camera" => Need::Camera,
            "modem" => Need::Modem,
            m if m.starts_with("memory:") => Need::Memory(m[7..].parse().ok()?),
            _ => return None,
        })
    }

    pub fn met(&self, c: &Capabilities) -> bool {
        match self {
            Need::AudioOut => c.audio_out(),
            // A capture device: an ALSA card, or an audio server (a headset mic).
            Need::AudioIn => c.alsa_cards > 0 || c.audio_server.is_some(),
            Need::Bluetooth => c.bluetooth,
            Need::Gpu => c.gpu,
            Need::Camera => c.cameras > 0,
            Need::Modem => c.modem,
            Need::Memory(mb) => c.mem_mb >= *mb,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Need::AudioOut => "no audio output (no ALSA card, no audio server)".into(),
            Need::AudioIn => "no audio input".into(),
            Need::Bluetooth => "no Bluetooth".into(),
            Need::Gpu => "no usable GPU".into(),
            Need::Camera => "no camera".into(),
            Need::Modem => "no modem".into(),
            Need::Memory(mb) => format!("needs {mb} MB of memory"),
        }
    }
}

/// Needs inferred from a feature's name, when the app declares none.
pub fn needs_by_name(feature: &str) -> Vec<Need> {
    let f = feature.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| f.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == *x));
    let mut n = Vec::new();
    if has(&["whisper", "stt", "asr"]) {
        n.extend([Need::AudioIn, Need::Memory(3000)]);
    } else if has(&["mic", "voice", "record", "capture"]) {
        n.push(Need::AudioIn);
    }
    if has(&["audio", "sound", "music", "playback", "rodio", "cpal", "midi", "alsa", "opus", "speaker"]) {
        n.push(Need::AudioOut);
    }
    if has(&["ble", "bluetooth", "bt", "btleplug", "bluez"]) {
        n.push(Need::Bluetooth);
    }
    if has(&["gpu", "wgpu", "vulkan", "glow", "gles"]) || f.contains("gpu-scene") || f.contains("scene3d") {
        n.push(Need::Gpu);
    }
    if has(&["camera", "webcam", "v4l", "v4l2"]) {
        n.push(Need::Camera);
    }
    if has(&["modem", "sms", "telephony"]) {
        n.push(Need::Modem);
    }
    n
}

/// The result of choosing features for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Build with these (and `--no-default-features` when `no_default`).
    pub features: Vec<String>,
    pub no_default: bool,
    /// (feature, why it was dropped)
    pub dropped: Vec<(String, String)>,
}

fn list_items(v: &str) -> Vec<String> {
    v.trim().trim_start_matches('[').trim_end_matches(']').split(',')
        .map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect()
}

/// Choose `cargo_toml`'s default features that `caps` can use. `keep`:
/// features that must stay whatever (the one that turns on the UI/eframe).
/// With nothing to drop the build is left alone (plain default features).
pub fn select_features(cargo_toml: &str, caps: &Capabilities, keep: &[String]) -> Selection {
    let mut section = String::new();
    let mut defaults: Vec<String> = Vec::new();
    let mut declared: BTreeMap<String, Vec<Need>> = BTreeMap::new();
    for raw in cargo_toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim().trim_matches('"');
        match section.as_str() {
            "[features]" if k == "default" => defaults = list_items(v),
            "[package.metadata.gitmind.needs]" => {
                declared.insert(k.to_string(), list_items(v).iter().filter_map(|n| Need::parse(n)).collect());
            }
            _ => {}
        }
    }
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    for f in &defaults {
        if keep.contains(f) {
            kept.push(f.clone());
            continue;
        }
        let needs = declared.get(f).cloned().unwrap_or_else(|| needs_by_name(f));
        match needs.iter().find(|n| !n.met(caps)) {
            Some(n) => dropped.push((f.clone(), n.describe())),
            None => kept.push(f.clone()),
        }
    }
    for k in keep {
        if !kept.contains(k) {
            kept.push(k.clone());
        }
    }
    if dropped.is_empty() {
        return Selection { features: keep.to_vec(), no_default: false, dropped };
    }
    Selection { features: kept, no_default: true, dropped }
}

/// Where a host's survey is kept (the raw probe output).
pub fn store_path(host: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let safe: String = host.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
    base.join("gitmind/capabilities").join(format!("{safe}.survey"))
}

pub fn save(host: &str, survey: &str) -> std::io::Result<()> {
    let p = store_path(host);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, survey)
}

pub fn load(host: &str) -> Option<Capabilities> {
    std::fs::read_to_string(store_path(host)).ok().map(|s| parse(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a reMarkable Paper Pro reports (shape from the real session:
    /// 4 cores in /proc/cpuinfo on i.MX8MM, 1.9 GB, BlueZ, no sound card).
    const RMPP: &str = "arch=aarch64\ncores=4\nmem_mb=1900\nlibc=glibc\nalsa_cards=0\nbluetooth=1\nbluez=1\ntouch=1\npen=1\ndisplay=qtfb\npython3=1\n";
    /// A Fairphone 5 on postmarketOS.
    const FP5: &str = "arch=aarch64\ncores=8\nmem_mb=7600\nlibc=musl\ngpu_render=1\nalsa_cards=1\nbluetooth=1\naudio_server=pipewire\ncameras=3\nmodem=1\ntouch=1\ndisplay=wayland\npython3=1\n";

    const STUDIO: &str = "[package]\nname = \"studio-app\"\n[features]\ndefault = [\"app\", \"audio\", \"midi\", \"picker\"]\napp = [\"dep:eframe\"]\naudio = []\nmidi = []\npicker = []\n";
    const APERTURE: &str = "[package]\nname = \"aperture-egui\"\n[features]\ndefault = [\"app\", \"tree-sitter\", \"engine\"]\napp = []\nengine = []\n[package.metadata.gitmind.needs]\nengine = [\"audio-in\", \"memory:3000\"]\n";

    #[test]
    fn the_same_app_builds_differently_for_a_tablet_and_a_phone() {
        let (rm, fp) = (parse(RMPP), parse(FP5));
        let keep = vec!["app".to_string()];
        let s = select_features(STUDIO, &rm, &keep);
        assert!(s.no_default);
        assert_eq!(s.features, vec!["app", "picker"]);
        assert_eq!(s.dropped.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), vec!["audio", "midi"]);
        assert!(s.dropped[0].1.contains("no audio output"));
        let s = select_features(STUDIO, &fp, &keep);
        assert!(!s.no_default && s.dropped.is_empty(), "the phone keeps its defaults: {s:?}");
    }

    #[test]
    fn declared_needs_beat_names_and_memory_counts() {
        let keep = vec!["app".to_string()];
        let s = select_features(APERTURE, &parse(RMPP), &keep);
        assert_eq!(s.dropped, vec![("engine".to_string(), "no audio input".to_string())]);
        let s = select_features(APERTURE, &parse(FP5), &keep);
        assert!(s.dropped.is_empty());
        // a whisper feature on a small device fails on memory even with a mic
        let mut small = parse(FP5);
        small.mem_mb = 2000;
        assert_eq!(needs_by_name("whisper"), vec![Need::AudioIn, Need::Memory(3000)]);
        assert!(!Need::Memory(3000).met(&small));
    }

    #[test]
    fn the_real_aperture_manifest_drops_voice_and_engine_on_the_tablet() {
        let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../aperture/crates/aperture-egui/Cargo.toml")).unwrap();
        let keep = vec!["app".to_string()];
        let s = select_features(&toml, &parse(RMPP), &keep);
        let dropped: Vec<&str> = s.dropped.iter().map(|d| d.0.as_str()).collect();
        assert!(dropped.contains(&"voice") && dropped.contains(&"engine"), "{s:?}");
        assert!(s.features.contains(&"tree-sitter".to_string()));
        assert!(select_features(&toml, &parse(FP5), &keep).dropped.is_empty(), "the phone keeps everything");
    }

    #[test]
    fn names_map_to_needs_by_whole_word() {
        assert_eq!(needs_by_name("ble"), vec![Need::Bluetooth]);
        assert_eq!(needs_by_name("gpu-scene"), vec![Need::Gpu]);
        assert!(needs_by_name("tree-sitter").is_empty());
        assert!(needs_by_name("table").is_empty(), "'ble' inside a word is not Bluetooth");
    }

    #[test]
    fn once_bluealsa_runs_the_tablet_has_audio() {
        let mut rm = parse(RMPP);
        assert!(!rm.audio_out());
        rm = parse(&format!("{RMPP}audio_server=bluealsa\n"));
        assert!(rm.audio_out());
    }

    #[test]
    fn the_survey_runs_here_and_every_key_it_prints_is_known() {
        let out = std::process::Command::new("sh").arg("-c").arg(survey_cmd()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let k = line.split('=').next().unwrap();
            assert!(KEYS.contains(&k), "survey printed an unlisted key {k:?}");
        }
        let c = parse(&text);
        assert!(c.cores > 0 && c.mem_mb > 0 && !c.arch.is_empty(), "{c:?}");
        assert_eq!(c.summary().len(), 4);
    }

    #[test]
    fn surveys_are_stored_per_host() {
        let dir = std::env::temp_dir().join(format!("caps_{}", std::process::id()));
        std::env::set_var("XDG_STATE_HOME", &dir);
        save("10.11.99.1", RMPP).unwrap();
        assert_eq!(load("10.11.99.1").unwrap().pen, true);
        assert!(load("other").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
