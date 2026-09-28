//! settings_store — where the keyboard's settings live between runs.
//!
//! `settings_model` has always said every change is *persisted*
//! (`Change::persist`), and nothing wrote anything: the in-keyboard panel kept
//! choices in a `HashMap` that died with the process. This is the file.
//!
//! `$XDG_CONFIG_HOME/rusty-board/settings.conf` (or `~/.config/...`), one
//! `key = value` line per setting, keys exactly as `settings_model` names them
//! (`layout.letters = dvorak`). Plain lines rather than TOML so a person can
//! read it, and so the keyboard, the companion app (`rusty-board-settings`)
//! and a shell one-liner all agree on the format. Unknown keys are kept on
//! save (a newer build's settings survive an older build's edit); unknown
//! values are ignored on load (the setting falls back to its default).
//!
//! Also here: `layout.user`, the user layout (by slug) the keyboard should
//! show, or empty for the built-in one. It is not a `settings_model` choice
//! because its options are files on disk, not a static list.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The user layout to show (a slug from the user layouts directory), or empty.
pub const USER_LAYOUT_KEY: &str = "layout.user";

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rusty-board").join("settings.conf"))
}

/// Parse the file's text: every `key = value` line, comments skipped.
pub fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// Only the values the settings model accepts (plus `layout.user`): what the
/// keyboard should apply. A value the model does not list is dropped, so the
/// setting shows its default rather than something no control can reach.
pub fn valid_values(all: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in all {
        if k == USER_LAYOUT_KEY {
            out.insert(k.clone(), v.clone());
            continue;
        }
        let Some(s) = crate::settings_model::SETTINGS.iter().find(|s| s.key == k) else { continue };
        let ok = match s.kind {
            crate::settings_model::Kind::Choice => s.values.contains(&v.as_str()),
            _ => true,
        };
        if ok {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Render the whole map back to text, sorted, with a header.
pub fn render(values: &BTreeMap<String, String>) -> String {
    let mut s = String::from("# rusty-board settings: key = value (edited by the keyboard and rusty-board-settings)\n");
    for (k, v) in values {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

pub fn load() -> BTreeMap<String, String> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default()
}

/// Set one key and write the file, keeping everything else in it.
pub fn set(key: &str, value: &str) -> Result<PathBuf, String> {
    let p = path().ok_or("no XDG_CONFIG_HOME or HOME")?;
    let mut all = std::fs::read_to_string(&p).map(|t| parse(&t)).unwrap_or_default();
    all.insert(key.to_string(), value.to_string());
    write(&p, &all)?;
    Ok(p)
}

/// Replace the whole file (the companion's "reset" writes an empty map).
pub fn write(p: &std::path::Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(p, render(values)).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_round_trips_and_keeps_keys_it_does_not_know() {
        let t = "# c\nlayout.letters = dvorak\nfuture.key = 3\n\n";
        let m = parse(t);
        assert_eq!(m.get("layout.letters").map(String::as_str), Some("dvorak"));
        assert_eq!(parse(&render(&m)), m, "render then parse is lossless");
        assert!(m.contains_key("future.key"), "kept for the newer build that wrote it");
    }

    #[test]
    fn only_values_a_control_can_reach_are_applied() {
        let mut m = BTreeMap::new();
        m.insert("layout.letters".to_string(), "dvorak".to_string());
        m.insert("layout.geometry".to_string(), "hexagonal".to_string());
        m.insert("no.such.key".to_string(), "x".to_string());
        m.insert(USER_LAYOUT_KEY.to_string(), "my-board".to_string());
        let v = valid_values(&m);
        assert_eq!(v.get("layout.letters").map(String::as_str), Some("dvorak"));
        assert!(!v.contains_key("layout.geometry"), "not an option: default instead");
        assert!(!v.contains_key("no.such.key"));
        assert_eq!(v.get(USER_LAYOUT_KEY).map(String::as_str), Some("my-board"));
    }
}
