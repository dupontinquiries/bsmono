//! config-kit — XDG base directories + tiny config load/save, done once.
//!
//! Every app recomputes the same thing: "where's my config dir, make it if missing, read
//! the file, fall back to a default." This crate is that, XDG-correct and app-namespaced,
//! plus a minimal `[section] key = value` parser for the many configs that don't need the
//! full `toml` crate. Apps with rich schemas keep `toml`; this removes the plumbing around
//! it. std-only.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn xdg(var: &str, default_rel: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(default_rel),
    }
}

/// `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}
/// `$XDG_DATA_HOME` or `~/.local/share`.
pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}
/// `$XDG_CACHE_HOME` or `~/.cache`.
pub fn cache_home() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}
/// `$XDG_STATE_HOME` or `~/.local/state`.
pub fn state_home() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

/// The config directory for a named app: `<config_home>/<app>`.
pub fn config_dir(app: &str) -> PathBuf {
    config_home().join(app)
}
/// The data directory for a named app: `<data_home>/<app>`.
pub fn data_dir(app: &str) -> PathBuf {
    data_home().join(app)
}

/// Ensure the app's config dir exists and return it.
pub fn ensure_config_dir(app: &str) -> std::io::Result<PathBuf> {
    let d = config_dir(app);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Read `<config_dir>/<file>` as a string, or `None` if it doesn't exist.
pub fn load(app: &str, file: &str) -> Option<String> {
    std::fs::read_to_string(config_dir(app).join(file)).ok()
}

/// Read the config file, or return `default` if it's absent (the create-read-fallback
/// pattern in one call).
pub fn load_or(app: &str, file: &str, default: &str) -> String {
    load(app, file).unwrap_or_else(|| default.to_string())
}

/// Write `<config_dir>/<file>`, creating the directory first.
pub fn save(app: &str, file: &str, contents: &str) -> std::io::Result<()> {
    let d = ensure_config_dir(app)?;
    std::fs::write(d.join(file), contents)
}

/// A parsed minimal config: `[section]` headers and `key = value` lines. Values are
/// trimmed; `#` and `;` start comments; keys outside any section land in the `""` section.
/// This is intentionally small — a stand-in for the common flat config, not a TOML clone.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ini {
    /// section -> (key -> value), preserving nothing fancy; ordered for stable output.
    pub sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut section = String::new();
        ini.sections.entry(section.clone()).or_default();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = inner.trim().to_string();
                ini.sections.entry(section.clone()).or_default();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                ini.sections
                    .entry(section.clone())
                    .or_default()
                    .insert(k.trim().to_string(), unquote(v.trim()).to_string());
            }
        }
        ini
    }

    /// Look up `key` in `section` (use `""` for the top-level section).
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections.get(section).and_then(|m| m.get(key)).map(|s| s.as_str())
    }

    /// Convenience for a top-level key.
    pub fn top(&self, key: &str) -> Option<&str> {
        self.get("", key)
    }
}

fn strip_comment(line: &str) -> &str {
    // only strip comments that aren't inside quotes; simple heuristic good enough for flat configs
    let mut in_q = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_q = !in_q,
            '#' | ';' if !in_q => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env<T>(k: &str, v: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var(k).ok();
        match v {
            Some(x) => std::env::set_var(k, x),
            None => std::env::remove_var(k),
        }
        let out = f();
        match prev {
            Some(p) => std::env::set_var(k, p),
            None => std::env::remove_var(k),
        }
        out
    }

    #[test]
    fn xdg_override_then_home_fallback() {
        with_env("XDG_CONFIG_HOME", Some("/cfg"), || {
            assert_eq!(config_dir("voiceterm"), std::path::Path::new("/cfg/voiceterm"));
        });
        with_env("XDG_CONFIG_HOME", None, || {
            with_env("HOME", Some("/home/kesslerd"), || {
                assert_eq!(config_home(), std::path::Path::new("/home/kesslerd/.config"));
            });
        });
    }

    #[test]
    fn ini_sections_keys_comments_quotes() {
        let text = "\
name = gitmind   # trailing comment
[audio]
device = \"USB Mic\"
gain = 0.8
; a comment line
[voice]
lang = en";
        let ini = Ini::parse(text);
        assert_eq!(ini.top("name"), Some("gitmind"));
        assert_eq!(ini.get("audio", "device"), Some("USB Mic"));
        assert_eq!(ini.get("audio", "gain"), Some("0.8"));
        assert_eq!(ini.get("voice", "lang"), Some("en"));
        assert_eq!(ini.get("audio", "missing"), None);
    }

    #[test]
    fn load_or_returns_default_when_absent() {
        // an app namespace that certainly has no file under a bogus config home
        with_env("XDG_CONFIG_HOME", Some("/nonexistent-xyz"), || {
            assert_eq!(load_or("no-such-app", "c.toml", "fallback"), "fallback");
        });
    }
}
