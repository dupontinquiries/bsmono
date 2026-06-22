// src/kde_tools.rs
#![allow(dead_code)]
//
// Detect which audio/system tools the controller needs and surface
// install hints for any that are missing. The user runs Fairphone FP5
// + KDE Plasma on Linux; some of our features (frequency sanitizer,
// adaptive cancellation, BT auto-connect, ping playback) shell out to
// command-line utilities. If those aren't installed the feature
// silently degrades, which is bad UX — better to surface "this is
// missing, here's how to get it" in a settings page.
//
// This module is pure detection + scripting. It doesn't run anything
// elevated — it just reports binary presence and returns text suitable
// for displaying in the UI as install instructions.

use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum InstallStatus {
    /// Binary found in PATH and runnable.
    Present,
    /// Binary not found at all.
    Missing,
    /// Binary present but our `--version`/`-h` probe failed unexpectedly.
    Broken { detail: String },
}

#[derive(Debug, Clone)]
pub struct Tool {
    /// Binary name as invoked (e.g. "paplay").
    pub bin: String,
    /// Human-friendly description of why the controller needs it.
    pub purpose: &'static str,
    /// What features stop working when it's absent.
    pub used_by: &'static str,
    /// Package name on Debian/Ubuntu (most relevant for FP5/KDE).
    pub apt_package: Option<&'static str>,
    /// Source repo URL (best-effort) for build-from-source.
    pub source_url: Option<&'static str>,
    /// Optional shell command that builds + installs from source.
    /// Multiline string; the UI shows it in a code block so the user
    /// can review before running. Always uses sudo for the install step.
    pub source_install_script: Option<&'static str>,
    /// Optional command (or set of commands) checking that the tool is
    /// functional beyond just being on PATH. None means "presence is
    /// enough".
    pub functional_check: Option<&'static str>,
}

/// The full list of tools the controller benefits from. Order is the
/// approximate display order — most-impactful first.
pub fn known_tools() -> Vec<Tool> {
    vec![
        Tool {
            bin: "paplay".to_string(),
            purpose: "PulseAudio raw-PCM playback for sanitizer + estimate cues",
            used_by: "Frequency sanitizer, pre-buffer cues, adaptive cancellation",
            apt_package: Some("pulseaudio-utils"),
            source_url: Some("https://gitlab.freedesktop.org/pulseaudio/pulseaudio"),
            source_install_script: None,
            functional_check: Some("paplay --version"),
        },
        Tool {
            bin: "parec".to_string(),
            purpose: "PulseAudio mic capture for predictive frequency-cancellation",
            used_by: "Adaptive cancellation, pattern learner",
            apt_package: Some("pulseaudio-utils"),
            source_url: Some("https://gitlab.freedesktop.org/pulseaudio/pulseaudio"),
            source_install_script: None,
            functional_check: Some("parec --version"),
        },
        Tool {
            bin: "aplay".to_string(),
            purpose: "ALSA fallback playback when PulseAudio is unavailable",
            used_by: "Frequency sanitizer (fallback path)",
            apt_package: Some("alsa-utils"),
            source_url: Some("https://github.com/alsa-project/alsa-utils"),
            source_install_script: Some(
                "# Build alsa-utils from source\n\
                git clone https://github.com/alsa-project/alsa-utils.git\n\
                cd alsa-utils\n\
                libtoolize --force --copy --automake\n\
                aclocal\n\
                autoheader\n\
                automake --foreign --copy --add-missing\n\
                autoconf\n\
                ./configure --prefix=/usr/local\n\
                make -j$(nproc)\n\
                sudo make install"
            ),
            functional_check: Some("aplay --version"),
        },
        Tool {
            bin: "bluetoothctl".to_string(),
            purpose: "Bluetooth device management for headphones + earbuds",
            used_by: "BT auto-connect, Beo Grace pairing",
            apt_package: Some("bluez"),
            source_url: Some("https://github.com/bluez/bluez"),
            source_install_script: Some(
                "# Build bluez from source\n\
                sudo apt-get install -y libdbus-1-dev libudev-dev libical-dev \\\n\
                    libreadline-dev libglib2.0-dev libell-dev\n\
                git clone https://github.com/bluez/bluez.git\n\
                cd bluez\n\
                ./bootstrap-configure --prefix=/usr/local\n\
                make -j$(nproc)\n\
                sudo make install"
            ),
            functional_check: Some("bluetoothctl --version"),
        },
        Tool {
            bin: "ffmpeg".to_string(),
            purpose: "Audio metadata + decode (BPM/key analysis, tag fallback)",
            used_by: "Library scanner (analysis feature)",
            apt_package: Some("ffmpeg"),
            source_url: Some("https://github.com/FFmpeg/FFmpeg"),
            source_install_script: Some(
                "# Build ffmpeg from source (minimal config)\n\
                git clone https://github.com/FFmpeg/FFmpeg.git\n\
                cd FFmpeg\n\
                ./configure --prefix=/usr/local --enable-shared --enable-libmp3lame\n\
                make -j$(nproc)\n\
                sudo make install\n\
                sudo ldconfig"
            ),
            functional_check: Some("ffmpeg -version"),
        },
        Tool {
            bin: "avahi-browse".to_string(),
            purpose: "mDNS browsing fallback for discovering B&O devices",
            used_by: "Device discovery (when built-in mDNS misses)",
            apt_package: Some("avahi-utils"),
            source_url: Some("https://github.com/lathiat/avahi"),
            source_install_script: None,
            functional_check: Some("avahi-browse --version"),
        },
        Tool {
            bin: "playerctl".to_string(),
            purpose: "MPRIS controller — reads what's playing in other apps",
            used_by: "Context awareness (currently-playing hint, anticipator bias)",
            apt_package: Some("playerctl"),
            source_url: Some("https://github.com/altdesktop/playerctl"),
            source_install_script: Some(
                "# Build playerctl from source\n\
                sudo apt-get install -y meson libglib2.0-dev libgirepository1.0-dev\n\
                git clone https://github.com/altdesktop/playerctl.git\n\
                cd playerctl\n\
                meson setup mesonbuild --prefix=/usr/local\n\
                meson compile -C mesonbuild\n\
                sudo meson install -C mesonbuild"
            ),
            functional_check: Some("playerctl --version"),
        },
    ]
}

/// Check a single tool's installation status.
pub fn check(tool: &Tool) -> InstallStatus {
    let on_path = Command::new("which")
        .arg(&tool.bin)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !on_path { return InstallStatus::Missing; }
    if let Some(check_cmd) = tool.functional_check {
        // Functional probe: just verify the command itself returns
        // something. Many tools exit non-zero on --version; we tolerate.
        // Split on whitespace; first token is the bin (which we just
        // verified is on PATH).
        let parts: Vec<&str> = check_cmd.split_whitespace().collect();
        if parts.len() <= 1 { return InstallStatus::Present; }
        match Command::new(parts[0]).args(&parts[1..]).output() {
            Ok(_) => InstallStatus::Present,
            Err(e) => InstallStatus::Broken { detail: e.to_string() },
        }
    } else {
        InstallStatus::Present
    }
}

/// Run the full audit. Returns a JSON-serializable structure the UI
/// can render: per-tool status + which install paths are available.
pub fn audit_all() -> Vec<(Tool, InstallStatus)> {
    known_tools().into_iter()
        .map(|t| { let s = check(&t); (t, s) })
        .collect()
}

/// Serialize an audit result to JSON for QML consumption.
pub fn audit_to_json(results: &[(Tool, InstallStatus)]) -> String {
    let entries: Vec<serde_json::Value> = results.iter().map(|(t, s)| {
        let (status_label, status_detail) = match s {
            InstallStatus::Present => ("present", String::new()),
            InstallStatus::Missing => ("missing", String::new()),
            InstallStatus::Broken { detail } => ("broken", detail.clone()),
        };
        serde_json::json!({
            "bin": t.bin,
            "purpose": t.purpose,
            "used_by": t.used_by,
            "apt_package": t.apt_package,
            "source_url": t.source_url,
            "source_script": t.source_install_script,
            "status": status_label,
            "status_detail": status_detail,
        })
    }).collect();
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
}

/// Build the user-runnable apt install command for missing tools. The
/// caller should NOT auto-run this — it requires sudo and should be
/// presented for review.
pub fn apt_install_command(results: &[(Tool, InstallStatus)]) -> Option<String> {
    let packages: Vec<&str> = results.iter()
        .filter(|(_, s)| matches!(s, InstallStatus::Missing | InstallStatus::Broken { .. }))
        .filter_map(|(t, _)| t.apt_package)
        .collect();
    if packages.is_empty() { return None; }
    let mut deduped = packages;
    deduped.sort(); deduped.dedup();
    Some(format!("sudo apt-get update && sudo apt-get install -y {}",
                 deduped.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tools_nonempty() {
        let t = known_tools();
        assert!(!t.is_empty(), "must declare at least one tool");
        assert!(t.iter().any(|x| x.bin == "paplay"));
        assert!(t.iter().any(|x| x.bin == "bluetoothctl"));
    }

    #[test]
    fn each_tool_has_purpose_and_used_by() {
        for t in known_tools() {
            assert!(!t.purpose.is_empty(), "{} missing purpose", t.bin);
            assert!(!t.used_by.is_empty(), "{} missing used_by", t.bin);
        }
    }

    #[test]
    fn apt_install_command_aggregates_missing() {
        let results = vec![
            (Tool {
                bin: "x".into(),
                purpose: "_", used_by: "_",
                apt_package: Some("pkg-a"),
                source_url: None, source_install_script: None,
                functional_check: None,
            }, InstallStatus::Missing),
            (Tool {
                bin: "y".into(),
                purpose: "_", used_by: "_",
                apt_package: Some("pkg-b"),
                source_url: None, source_install_script: None,
                functional_check: None,
            }, InstallStatus::Missing),
            (Tool {
                bin: "z".into(),
                purpose: "_", used_by: "_",
                apt_package: Some("pkg-c"),
                source_url: None, source_install_script: None,
                functional_check: None,
            }, InstallStatus::Present),
        ];
        let cmd = apt_install_command(&results).unwrap();
        assert!(cmd.contains("pkg-a"));
        assert!(cmd.contains("pkg-b"));
        assert!(!cmd.contains("pkg-c"), "present packages should be excluded");
        assert!(cmd.starts_with("sudo apt-get update"));
    }

    #[test]
    fn apt_install_command_none_when_all_present() {
        let results: Vec<(Tool, InstallStatus)> = known_tools()
            .into_iter()
            .map(|t| (t, InstallStatus::Present))
            .collect();
        assert!(apt_install_command(&results).is_none());
    }

    #[test]
    fn audit_json_includes_every_tool() {
        let results = audit_all();
        let json = audit_to_json(&results);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(arr.len(), known_tools().len());
        for entry in arr {
            assert!(entry.get("bin").is_some());
            assert!(entry.get("status").is_some());
        }
    }

    #[test]
    fn duplicate_packages_deduplicated() {
        // paplay + parec both come from pulseaudio-utils; the install
        // command should mention pulseaudio-utils once, not twice.
        let results: Vec<(Tool, InstallStatus)> = known_tools()
            .into_iter()
            .map(|t| (t, InstallStatus::Missing))
            .collect();
        let cmd = apt_install_command(&results).unwrap();
        let count = cmd.matches("pulseaudio-utils").count();
        assert_eq!(count, 1, "pulseaudio-utils should appear exactly once");
    }
}
