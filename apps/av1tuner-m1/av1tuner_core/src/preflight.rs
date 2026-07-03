//! # Preflight — catch problems before encoding
//!
//! Runs before any chunks are queued. Returns a list of checks with 
//! their pass/fail status. The UI shows everything at once so the user 
//! sees all problems before they hit "go" a second time.

use std::path::Path;
use crate::audio::AudioSettings;
use crate::encoder::{EncoderKind, EncoderSettings};

#[derive(Clone, Debug)]
pub struct PreflightCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    pub fn fatal(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }
    pub fn warnings(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Warn).count()
    }
}

/// Run all preflight checks for a single-video encode.
pub fn run(
    input: &Path,
    output: &Path,
    state_dir: &Path,
    settings: &EncoderSettings,
    audio: &AudioSettings,
) -> PreflightReport {
    let mut checks = Vec::new();
    checks.push(check_ffprobe());
    checks.push(check_ffmpeg());
    checks.push(check_encoder(settings.kind));
    checks.push(check_input_exists(input));
    checks.push(check_input_not_output(input, output));
    checks.push(check_output_writable(output));
    checks.push(check_state_dir(state_dir));
    checks.push(check_crf(settings.quantizer(), settings.kind));
    checks.push(check_preset(settings.preset, settings.kind));
    checks.push(check_pixel_format(&settings.pixel_format));
    if audio.needs_libopus() {
        checks.push(check_libopus());
    }
    if matches!(settings.kind, EncoderKind::Vvenc) {
        checks.push(check_mkvmerge());
    }
    PreflightReport { checks }
}

/// VVenC output muxing requires mkvmerge — ffmpeg can't synthesize 
/// timestamps for raw H.266 streams, but mkvmerge can. Surface the 
/// dependency at preflight so the user installs it before a long encode 
/// instead of failing at the very end.
fn check_mkvmerge() -> PreflightCheck {
    // VVenC output requires a VVC-aware muxer because ffmpeg's VVC mux
    // (both MKV and MP4 paths) is broken in current versions. We accept
    // either MP4Box (GPAC — Fraunhofer's recommended path) or mkvmerge
    // (mkvtoolnix). Pass if either is present, fail if neither.
    let mp4box = which::which("MP4Box").ok();
    let mkvmerge = which::which("mkvmerge").ok();
    match (mp4box, mkvmerge) {
        (Some(p), _) => PreflightCheck {
            name: "vvc muxer".to_string(),
            status: CheckStatus::Pass,
            message: format!("found MP4Box: {} (preferred)", p.display()),
        },
        (None, Some(p)) => PreflightCheck {
            name: "vvc muxer".to_string(),
            status: CheckStatus::Pass,
            message: format!("found mkvmerge: {}", p.display()),
        },
        (None, None) => PreflightCheck {
            name: "vvc muxer".to_string(),
            status: CheckStatus::Fail,
            message: "VVenC output needs a VVC-aware muxer; neither MP4Box                       (GPAC) nor mkvmerge (mkvtoolnix) is on PATH. Install                       one: `sudo dnf install gpac` (Fedora/Asahi, preferred),                       `sudo apt install gpac` (Debian), or the mkvtoolnix                       package alternatively. ffmpeg cannot mux raw VVC                       correctly in current versions.".to_string(),
        },
    }
}

/// Verify ffmpeg has libopus support compiled in. Runs once at 
/// preflight when audio mode is EncodeOpus. Without libopus, ffmpeg 
/// will error at concat time with a less-helpful message.
fn check_libopus() -> PreflightCheck {
    let ffmpeg = match which::which("ffmpeg") {
        Ok(p) => p,
        Err(_) => return PreflightCheck {
            name: "libopus".to_string(),
            status: CheckStatus::Fail,
            message: "ffmpeg not on PATH".to_string(),
        },
    };
    // `ffmpeg -hide_banner -encoders` lists all encoders; we grep for libopus.
    let output = std::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output();
    let stdout = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(e) => return PreflightCheck {
            name: "libopus".to_string(),
            status: CheckStatus::Fail,
            message: format!("couldn't run ffmpeg -encoders: {}", e),
        },
    };
    if stdout.contains("libopus") {
        PreflightCheck {
            name: "libopus".to_string(),
            status: CheckStatus::Pass,
            message: "ffmpeg has libopus support".to_string(),
        }
    } else {
        PreflightCheck {
            name: "libopus".to_string(),
            status: CheckStatus::Fail,
            message: "ffmpeg was built without libopus — install ffmpeg with opus support, or switch audio to Passthrough".to_string(),
        }
    }
}

/// Hard fail if input and output resolve to the same file. Catches the 
/// "user clicked through derive but already had it set" case before we 
/// truncate the source mid-encode.
fn check_input_not_output(input: &Path, output: &Path) -> PreflightCheck {
    // Canonicalize both so symlinks + relative paths get resolved. Input 
    // must exist (we check that elsewhere); output usually doesn't yet, 
    // so canonicalize() will fail. Fall back to comparing the parent 
    // dir + filename in that case.
    let same = match (input.canonicalize(), output.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => {
            // Output doesn't exist yet. Compare what we can — parent 
            // directory (canonicalized if possible) plus filename.
            let in_canon = input.canonicalize().ok();
            let out_parent = output.parent().and_then(|p| p.canonicalize().ok());
            match (in_canon, out_parent) {
                (Some(in_path), Some(out_dir)) => {
                    let out_full = out_dir.join(
                        output.file_name().unwrap_or_default()
                    );
                    in_path == out_full
                }
                _ => {
                    // Last-resort: literal path comparison after as_os_str.
                    input.as_os_str() == output.as_os_str()
                }
            }
        }
    };
    if same {
        PreflightCheck {
            name: "input/output safety".to_string(),
            status: CheckStatus::Fail,
            message: "input and output point at the same file — encoding would destroy your source".to_string(),
        }
    } else {
        PreflightCheck {
            name: "input/output safety".to_string(),
            status: CheckStatus::Pass,
            message: "input and output are distinct".to_string(),
        }
    }
}

fn check_ffprobe() -> PreflightCheck {
    match which::which("ffprobe") {
        Ok(p) => PreflightCheck {
            name: "ffprobe".to_string(),
            status: CheckStatus::Pass,
            message: format!("found: {}", p.display()),
        },
        Err(_) => PreflightCheck {
            name: "ffprobe".to_string(),
            status: CheckStatus::Fail,
            message: "ffprobe not on PATH (install ffmpeg)".to_string(),
        },
    }
}

fn check_ffmpeg() -> PreflightCheck {
    match which::which("ffmpeg") {
        Ok(p) => PreflightCheck {
            name: "ffmpeg".to_string(),
            status: CheckStatus::Pass,
            message: format!("found: {}", p.display()),
        },
        Err(_) => PreflightCheck {
            name: "ffmpeg".to_string(),
            status: CheckStatus::Fail,
            message: "ffmpeg not on PATH".to_string(),
        },
    }
}

fn check_encoder(kind: EncoderKind) -> PreflightCheck {
    let (name, bin, install_hint) = match kind {
        EncoderKind::SvtAv1 => (
            "SVT-AV1", "SvtAv1EncApp",
            "dnf install svt-av1 / brew install svt-av1 / pacman -S svt-av1",
        ),
        EncoderKind::Vvenc => (
            "VVenC", "vvencapp",
            "use the Install VVenC button, or build from github.com/fraunhoferhhi/vvenc",
        ),
    };
    // Check PATH first, then for VVenC the av1tuner-managed install dir.
    let on_path = which::which(bin);
    let managed = match kind {
        EncoderKind::Vvenc => {
            crate::vvenc_install::InstallPaths::default_paths()
                .filter(|p| p.is_installed())
                .map(|p| p.vvencapp_path)
        }
        _ => None,
    };
    match (on_path, managed) {
        (Ok(p), _) => PreflightCheck {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: format!("found: {}", p.display()),
        },
        (Err(_), Some(p)) => PreflightCheck {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: format!("found in av1tuner local install: {}", p.display()),
        },
        (Err(_), None) => PreflightCheck {
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: format!("{} not found ({})", bin, install_hint),
        },
    }
}

fn check_input_exists(input: &Path) -> PreflightCheck {
    if !input.exists() {
        return PreflightCheck {
            name: "input file".to_string(),
            status: CheckStatus::Fail,
            message: format!("does not exist: {}", input.display()),
        };
    }
    let meta = match std::fs::metadata(input) {
        Ok(m) => m,
        Err(e) => return PreflightCheck {
            name: "input file".to_string(),
            status: CheckStatus::Fail,
            message: format!("cannot stat: {}", e),
        },
    };
    if !meta.is_file() {
        return PreflightCheck {
            name: "input file".to_string(),
            status: CheckStatus::Fail,
            message: "not a regular file".to_string(),
        };
    }
    PreflightCheck {
        name: "input file".to_string(),
        status: CheckStatus::Pass,
        message: format!("{} ({} bytes)", input.display(), meta.len()),
    }
}

fn check_output_writable(output: &Path) -> PreflightCheck {
    let parent = output.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return PreflightCheck {
            name: "output dir".to_string(),
            status: CheckStatus::Fail,
            message: format!("parent directory missing: {}", parent.display()),
        };
    }
    // Probe writability with a sentinel file.
    let probe = parent.join(".av1tuner_write_probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            PreflightCheck {
                name: "output dir".to_string(),
                status: CheckStatus::Pass,
                message: format!("writable: {}", parent.display()),
            }
        }
        Err(e) => PreflightCheck {
            name: "output dir".to_string(),
            status: CheckStatus::Fail,
            message: format!("not writable: {}", e),
        },
    }
}

fn check_state_dir(state_dir: &Path) -> PreflightCheck {
    if let Err(e) = std::fs::create_dir_all(state_dir) {
        return PreflightCheck {
            name: "state dir".to_string(),
            status: CheckStatus::Fail,
            message: format!("cannot create {}: {}", state_dir.display(), e),
        };
    }
    PreflightCheck {
        name: "state dir".to_string(),
        status: CheckStatus::Pass,
        message: format!("ready: {}", state_dir.display()),
    }
}

fn check_crf(crf: u32, kind: EncoderKind) -> PreflightCheck {
    let (max, typical_range) = match kind {
        EncoderKind::SvtAv1 => (63, (18, 50)),
        // VVenC QP also 0-63. Typical VVC QPs run higher than SVT-AV1 
        // CRFs for comparable quality — VVC quantization is on a 
        // different scale.
        EncoderKind::Vvenc => (63, (22, 50)),
    };
    let label = match kind {
        EncoderKind::SvtAv1 => "CRF",
        EncoderKind::Vvenc => "QP",
    };
    if crf > max {
        return PreflightCheck {
            name: label.to_string(),
            status: CheckStatus::Fail,
            message: format!("{} {} out of range (0-{})", label, crf, max),
        };
    }
    let warn = crf < typical_range.0 || crf > typical_range.1;
    let status = if warn { CheckStatus::Warn } else { CheckStatus::Pass };
    let msg = if warn {
        format!("{} {} is unusual (typical: {}-{})",
            label, crf, typical_range.0, typical_range.1)
    } else {
        format!("{} {} ok", label, crf)
    };
    PreflightCheck {
        name: label.to_string(),
        status, message: msg,
    }
}

fn check_preset(preset: u32, kind: EncoderKind) -> PreflightCheck {
    match kind {
        EncoderKind::SvtAv1 => {
            let max = 13;
            if preset > max {
                return PreflightCheck {
                    name: "preset".to_string(),
                    status: CheckStatus::Fail,
                    message: format!("SVT-AV1 preset {} out of range (0-{})", preset, max),
                };
            }
            PreflightCheck {
                name: "preset".to_string(),
                status: CheckStatus::Pass,
                message: format!("SVT-AV1 preset {} ok", preset),
            }
        }
        EncoderKind::Vvenc => {
            // VvencPreset is enum-validated at the type level — no 
            // numeric range to check. Just acknowledge it.
            PreflightCheck {
                name: "preset".to_string(),
                status: CheckStatus::Pass,
                message: "VVenC preset selected via UI (faster/fast/medium/slow/slower)".to_string(),
            }
        }
    }
}

fn check_pixel_format(fmt: &str) -> PreflightCheck {
    let known = ["yuv420p", "yuv420p10le", "yuv422p", "yuv422p10le", "yuv444p", "yuv444p10le"];
    if known.contains(&fmt) {
        PreflightCheck {
            name: "pixel format".to_string(),
            status: CheckStatus::Pass,
            message: format!("{} ok", fmt),
        }
    } else {
        PreflightCheck {
            name: "pixel format".to_string(),
            status: CheckStatus::Warn,
            message: format!("{} is unusual — encoder may reject it", fmt),
        }
    }
}

/// Bonus: rough estimate of required free space, assuming the encode 
/// produces roughly 0.05 * source bytes. Returns None if can't tell.
pub fn estimate_output_bytes(input: &Path) -> Option<u64> {
    let meta = std::fs::metadata(input).ok()?;
    Some((meta.len() as f64 * 0.05) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crf_out_of_range_fails() {
        let c = check_crf(99, EncoderKind::SvtAv1);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn crf_extreme_warns() {
        let c = check_crf(5, EncoderKind::SvtAv1);
        assert_eq!(c.status, CheckStatus::Warn);
    }

    #[test]
    fn crf_typical_passes() {
        let c = check_crf(28, EncoderKind::SvtAv1);
        assert_eq!(c.status, CheckStatus::Pass);
    }

    #[test]
    fn preset_out_of_range_fails() {
        let c = check_preset(14, EncoderKind::SvtAv1);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn pixel_format_unknown_warns() {
        let c = check_pixel_format("rgb24");
        assert_eq!(c.status, CheckStatus::Warn);
    }

    #[test]
    fn pixel_format_known_passes() {
        let c = check_pixel_format("yuv420p10le");
        assert_eq!(c.status, CheckStatus::Pass);
    }

    #[test]
    fn input_equals_output_fails() {
        // Literal path comparison path (file doesn't need to exist).
        let same = std::path::Path::new("/tmp/some_video.mkv");
        let c = check_input_not_output(same, same);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn distinct_paths_pass() {
        let a = std::path::Path::new("/tmp/in.mkv");
        let b = std::path::Path::new("/tmp/out.mkv");
        let c = check_input_not_output(a, b);
        assert_eq!(c.status, CheckStatus::Pass);
    }
}
