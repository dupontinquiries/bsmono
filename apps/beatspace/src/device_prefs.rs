//! Per-device preferences: the sidecar, beatspace's version of studio's
//! `<name>.workspace`.
//!
//! Some choices belong to the DEVICE, not the project or the model: what a
//! library-bound render costs in storage is the one here. A Fairphone and
//! a studio desktop editing the same checkpoint directory want different
//! answers, and the checkpoint's `config.json` follows the directory, so
//! it is the wrong place. This file lives in the user's config directory
//! instead — `$XDG_CONFIG_HOME/beatspace/device.prefs`, or
//! `~/.config/beatspace/device.prefs` — and is written as the same lenient
//! `key=value` text studio's sidecar uses: defaults underneath, known keys
//! on top, everything else ignored, no error case on purpose. Safe to
//! delete, never load-bearing.

use std::path::PathBuf;

use audio_kit::BounceFormat;

#[derive(Debug, Clone, PartialEq)]
pub struct DevicePrefs {
    /// What library-bound renders — project sources, saved beats, exports —
    /// are encoded as. See `audio_kit::bounce`.
    pub bounce_format: BounceFormat,
}

impl Default for DevicePrefs {
    fn default() -> Self {
        Self { bounce_format: BounceFormat::default() }
    }
}

impl DevicePrefs {
    pub fn encode(&self) -> String {
        format!(
            "# per-device preferences; safe to delete, never load-bearing\n\
             bounce_format={}\n",
            self.bounce_format.token()
        )
    }

    /// Parse leniently: there is no error case ON PURPOSE.
    pub fn decode(s: &str) -> Self {
        let mut p = Self::default();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "bounce_format" => p.bounce_format = BounceFormat::from_token(v),
                _ => {} // a newer build's key, not our business
            }
        }
        p
    }

    /// Where the sidecar lives on this device, if a config directory can
    /// be found at all.
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("beatspace").join("device.prefs"))
    }

    /// Load from the sidecar; a missing or unreadable file is the default.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::decode(&s))
            .unwrap_or_default()
    }

    /// Write the sidecar. Returns the path written.
    pub fn save(&self) -> std::io::Result<PathBuf> {
        let p = Self::path().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no config directory (HOME unset)"))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, self.encode())?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips_and_unknown_lines_are_ignored() {
        let p = DevicePrefs { bounce_format: BounceFormat::Flac };
        assert_eq!(DevicePrefs::decode(&p.encode()), p);
        let lenient = "# comment\n\nbounce_format = wav\nfuture_key=whatever\nnot a pair\n";
        assert_eq!(DevicePrefs::decode(lenient).bounce_format, BounceFormat::Wav);
        assert_eq!(DevicePrefs::decode("").bounce_format, BounceFormat::Opus192, "empty is the default");
        assert_eq!(DevicePrefs::decode("bounce_format=mp3").bounce_format, BounceFormat::Opus192, "unknown token is the default");
    }

    #[test]
    fn the_sidecar_lives_under_the_config_directory() {
        let p = DevicePrefs::path();
        if let Some(p) = p {
            assert!(p.ends_with("beatspace/device.prefs"), "{}", p.display());
        }
    }
}
