//! # Config — persist last-used settings across app restarts
//!
//! Stores a small JSON blob at the platform's config dir:
//! - Linux: `$XDG_CONFIG_HOME/av1tuner/settings.json` (or `~/.config/...`)
//! - macOS: `~/Library/Application Support/av1tuner/settings.json`
//! - Windows: `%APPDATA%\av1tuner\settings.json`
//!
//! Saved on every settings change. Loaded once at app startup. Failures 
//! (missing file, bad JSON, no write permission) are non-fatal — we 
//! just fall back to defaults and log a warning.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::audio::AudioSettings;
use crate::encoder::EncoderSettings;

/// What gets remembered across launches. Keep this minimal — the app's
/// state machine is the source of truth for everything dynamic; this 
/// is just "what was the user last working on."
/// Forward-compatibility: `#[serde(default)]` on the struct level means 
/// missing fields in saved JSON from older app versions fall back to 
/// the field type's Default. That way adding new fields doesn't break 
/// existing users' saved settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    pub last_settings: EncoderSettings,
    pub last_audio: AudioSettings,
    pub last_chunk_seconds: f64,
    /// Workers running in parallel. None = "use cpu_count / 2" default.
    pub concurrency_override: Option<u32>,
    /// Remembered between sessions so the user doesn't re-browse for 
    /// the same source directory. Path only, not file — too risky to 
    /// auto-fill an input video and accidentally start an encode of it.
    pub last_input_dir: Option<PathBuf>,
    pub last_output_dir: Option<PathBuf>,
    /// Auto-remove per-chunk intermediates after a successful encode.
    /// ON by default (tidy workspace). Turn OFF for big batches so a 
    /// crash leaves completed chunks on disk for fast resume.
    #[serde(default = "default_true")]
    pub auto_cleanup: bool,
    /// Use the content-signature cache to skip re-encoding unchanged 
    /// clips with identical settings. ON by default.
    #[serde(default = "default_true")]
    pub use_cache: bool,
    /// Cache signature thoroughness. Cheap (default) = size+mtime+samples; 
    /// Full = whole-file hash (slower, certain).
    #[serde(default)]
    pub cache_mode: crate::cache::SignatureMode,
}

/// serde default helper — fields added later default to true for 
/// existing configs that predate them.
fn default_true() -> bool { true }

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            last_settings: EncoderSettings::default(),
            last_audio: AudioSettings::default(),
            last_chunk_seconds: 10.0,
            concurrency_override: None,
            last_input_dir: None,
            last_output_dir: None,
            auto_cleanup: true,
            use_cache: true,
            cache_mode: crate::cache::SignatureMode::Cheap,
        }
    }
}

impl UserConfig {
    /// Path to the config file. Returns None on platforms where we 
    /// can't determine a config dir.
    pub fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("settings.json"))
    }
    
    /// Load from disk. Returns default + logs a notice on missing or 
    /// corrupt config; never returns an error.
    pub fn load() -> Self {
        let path = match Self::path() {
            Some(p) => p,
            None => {
                log::info!("no config dir available; using defaults");
                return Self::default();
            }
        };
        if !path.exists() {
            log::debug!("no saved config at {}; using defaults", path.display());
            return Self::default();
        }
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(cfg) => {
                    log::debug!("loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    log::warn!("config at {} is corrupt: {}; using defaults",
                        path.display(), e);
                    Self::default()
                }
            },
            Err(e) => {
                log::warn!("couldn't read config at {}: {}; using defaults",
                    path.display(), e);
                Self::default()
            }
        }
    }
    
    /// Save to disk. Errors are logged but not returned — the app keeps 
    /// running with in-memory settings even if write fails. Uses 
    /// tmp+rename for atomicity.
    pub fn save(&self) {
        let path = match Self::path() {
            Some(p) => p,
            None => return,
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("couldn't create config dir {}: {}", 
                    parent.display(), e);
                return;
            }
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = match serde_json::to_vec_pretty(self) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("serialize config: {}", e);
                return;
            }
        };
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            log::warn!("write config tmp {}: {}", tmp.display(), e);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            log::warn!("rename config to {}: {}", path.display(), e);
        }
    }
}

/// Platform-appropriate config dir for our app.
fn config_dir() -> Option<PathBuf> {
    // Use XDG_CONFIG_HOME on Linux/BSD, $HOME/.config as fallback.
    // On macOS, $HOME/Library/Application Support.
    // On Windows, %APPDATA%.
    // 
    // Avoid the `directories` crate dependency for this — small enough 
    // to do inline.
    
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(PathBuf::from(appdata).join("av1tuner"));
        }
    }
    
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("av1tuner")
            );
        }
    }
    
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("av1tuner"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join(".config").join("av1tuner"));
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serializes() {
        let c = UserConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("last_settings"));
        let back: UserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.last_chunk_seconds, 10.0);
    }

    #[test]
    fn missing_fields_fall_back_via_default() {
        // Old saved configs missing newer fields must still load — 
        // serde's `#[serde(default)]` on the struct does this.
        let partial = r#"{
            "last_settings": {
                "kind": "SvtAv1", "svt_crf": 30, "preset": 5,
                "pixel_format": "yuv420p10le", "tune": 0, "extra_args": []
            },
            "last_chunk_seconds": 8.0
        }"#;
        let cfg: UserConfig = serde_json::from_str(partial).expect("partial JSON should load");
        assert_eq!(cfg.last_settings.svt_crf, 30);
        assert_eq!(cfg.last_chunk_seconds, 8.0);
        assert!(cfg.concurrency_override.is_none());
        assert!(cfg.last_input_dir.is_none());
        // New fields default to true / Cheap for old configs.
        assert!(cfg.auto_cleanup);
        assert!(cfg.use_cache);
        assert_eq!(cfg.cache_mode, crate::cache::SignatureMode::Cheap);
    }
}
