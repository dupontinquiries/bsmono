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

/// Write `<config_dir>/<file>`, creating the directory first. Atomic: see [`write_atomic`].
pub fn save(app: &str, file: &str, contents: &str) -> std::io::Result<()> {
    let d = ensure_config_dir(app)?;
    write_atomic(&d.join(file), contents)
}

/// Replace `path` with `contents` so a reader sees the old file or the new one, never a
/// torn one.
///
/// `std::fs::write` truncates and then writes. A process killed in between -- which on a
/// phone is how apps END, not an accident -- leaves an empty or half-written config, and
/// every loader here treats unreadable keys as defaults: the user's settings silently
/// reset. So: write a sibling temp file, flush it to disk, rename it over the target
/// (atomic within a directory on POSIX), then sync the directory so the rename itself
/// survives a power cut.
pub fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        // Best effort: not every filesystem lets a directory be opened for sync.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
    // Only strip comments that aren't inside quotes. A backslash inside quotes escapes the
    // next character, so `"say \"hi\" #1"` stays one value: without that, an escaped quote
    // flipped the state and the `#` after it cut the value off. Escapes are NOT decoded
    // here -- a caller that writes them decodes them.
    let mut in_q = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_q => escaped = true,
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
    fn a_quoted_value_keeps_hash_semicolon_and_escaped_quotes() {
        let ini = Ini::parse("name = \"Ada #1; \\\"the\\\" one\"  # real comment\nx = 1");
        assert_eq!(ini.top("name"), Some("Ada #1; \\\"the\\\" one"), "escapes are kept for the caller to decode");
        assert_eq!(ini.top("x"), Some("1"));
        // Unquoted, # still starts a comment -- unchanged for every existing file.
        assert_eq!(Ini::parse("y = a # b").top("y"), Some("a"));
    }

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("config-kit-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.conf");
        write_atomic(&p, "a = 1\n").unwrap();
        write_atomic(&p, "a = 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a = 2\n");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp.")).collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
        // A failed write (target is a directory) leaves the temp file cleaned up too.
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(write_atomic(&sub, "x").is_err());
        let leftovers = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp.")).count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_returns_default_when_absent() {
        // an app namespace that certainly has no file under a bogus config home
        with_env("XDG_CONFIG_HOME", Some("/nonexistent-xyz"), || {
            assert_eq!(load_or("no-such-app", "c.toml", "fallback"), "fallback");
        });
    }
}
