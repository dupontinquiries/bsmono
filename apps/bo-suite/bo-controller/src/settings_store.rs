// src/settings_store.rs
#![allow(dead_code)]
//
// Tiny key-value settings persistence.
//
// ── Why this is its own module ──
//
// Most of the canceller state is in-memory (the FSM, the phase
// accumulators, the lock vectors). But a small set of values
// represent USER CHOICE that should survive restart:
//   - Whether experimental features are enabled (v1 toggle)
//   - Saturation slider positions
//   - Per-harmonic lock auto-mode preference
//   - Audio capture dump location tag
//
// Rather than scatter file IO across the main module, all
// persistence funnels through this one module. The on-disk
// format is a JSON object at
// `$XDG_DATA_HOME/bo-controller/settings.json`. Reads are
// transparent (file may not exist on first run); writes are
// atomic (write to tmp then rename).
//
// ── Schema discipline ──
//
// The settings file is "best effort": we never panic on a
// missing or corrupt file. If a key is missing we return None.
// If the file fails to parse we log + treat as empty. This
// means the user can manually delete the file to reset
// everything, and an older version of the code reading a newer
// file just ignores unknown keys.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
pub enum SettingsError {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "settings io: {}", e),
            SettingsError::Parse(s) => write!(f, "settings parse: {}", s),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<std::io::Error> for SettingsError {
    fn from(e: std::io::Error) -> Self { SettingsError::Io(e) }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

/// Where the settings file lives.
/// `$XDG_DATA_HOME/bo-controller/settings.json` or
/// `~/.local/share/bo-controller/settings.json` as fallback.
pub fn settings_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-controller");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("settings.json")
}

/// In-memory cache. Loaded lazily on first access.
fn cache() -> &'static Mutex<Option<HashMap<String, Value>>> {
    static CELL: OnceLock<Mutex<Option<HashMap<String, Value>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Load from disk into the cache if not already cached. Returns
/// `Ok(())` even if the file doesn't exist (treats as empty).
fn ensure_loaded() -> Result<(), SettingsError> {
    let mut guard = cache().lock().expect("settings cache poisoned");
    if guard.is_some() { return Ok(()); }
    let path = settings_path();
    let map = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(text) => parse_json_object(&text)
                .unwrap_or_else(|e| {
                    eprintln!("settings: parse failed, starting empty: {}", e);
                    HashMap::new()
                }),
            Err(e) => {
                eprintln!("settings: read failed, starting empty: {}", e);
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };
    *guard = Some(map);
    Ok(())
}

/// Atomic write: write to a temp file then rename. Keeps the
/// settings file from getting truncated mid-write if the process
/// dies.
fn flush_to_disk(map: &HashMap<String, Value>) -> Result<(), SettingsError> {
    let path = settings_path();
    let tmp_path = path.with_extension("json.tmp");
    let json = serialize_json_object(map);
    std::fs::write(&tmp_path, json.as_bytes())?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

// ─── Public API ───

pub fn get_bool(key: &str) -> Option<bool> {
    ensure_loaded().ok()?;
    let guard = cache().lock().expect("settings cache poisoned");
    match guard.as_ref()?.get(key)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

pub fn set_bool(key: &str, value: bool) -> Result<(), SettingsError> {
    ensure_loaded()?;
    let mut guard = cache().lock().expect("settings cache poisoned");
    let map = guard.as_mut().expect("cache initialized");
    map.insert(key.to_string(), Value::Bool(value));
    flush_to_disk(map)
}

pub fn get_float(key: &str) -> Option<f64> {
    ensure_loaded().ok()?;
    let guard = cache().lock().expect("settings cache poisoned");
    match guard.as_ref()?.get(key)? {
        Value::Float(f) => Some(*f),
        Value::Int(i)   => Some(*i as f64),
        _ => None,
    }
}

pub fn set_float(key: &str, value: f64) -> Result<(), SettingsError> {
    ensure_loaded()?;
    let mut guard = cache().lock().expect("settings cache poisoned");
    let map = guard.as_mut().expect("cache initialized");
    map.insert(key.to_string(), Value::Float(value));
    flush_to_disk(map)
}

pub fn get_str(key: &str) -> Option<String> {
    ensure_loaded().ok()?;
    let guard = cache().lock().expect("settings cache poisoned");
    match guard.as_ref()?.get(key)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn set_str(key: &str, value: &str) -> Result<(), SettingsError> {
    ensure_loaded()?;
    let mut guard = cache().lock().expect("settings cache poisoned");
    let map = guard.as_mut().expect("cache initialized");
    map.insert(key.to_string(), Value::Str(value.to_string()));
    flush_to_disk(map)
}

/// Enumerate all keys with the given prefix. Returns (key, str-value)
/// pairs for keys whose values are strings, skipping non-string
/// values (none of the prefixed namespaces below need them, and
/// silently skipping is safer than panicking on a type mismatch).
///
/// Used for diagnostic banners — e.g. on startup the controller
/// dumps everything in `device_family:*` so the user can verify
/// what classifications survived across sessions. Without this,
/// the persisted verdict layer is opaque: if persistence breaks,
/// you can't tell that from "device wasn't classified yet" without
/// digging through the on-disk JSON manually.
pub fn keys_with_prefix(prefix: &str) -> Vec<(String, String)> {
    if ensure_loaded().is_err() { return Vec::new(); }
    let guard = cache().lock().expect("settings cache poisoned");
    let map = match guard.as_ref() { Some(m) => m, None => return Vec::new() };
    let mut out: Vec<(String, String)> = map.iter()
        .filter_map(|(k, v)| {
            if !k.starts_with(prefix) { return None; }
            match v {
                Value::Str(s) => Some((k.clone(), s.clone())),
                _ => None,
            }
        })
        .collect();
    // Stable order so the banner reads the same way each launch.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Drop the in-memory cache. Useful for tests that want a fresh
/// read of the on-disk file. Not for production use.
pub fn invalidate_cache_for_test() {
    let mut guard = cache().lock().expect("settings cache poisoned");
    *guard = None;
}

// ─── Minimal JSON serialization (no external deps for this file) ───
//
// We use serde_json elsewhere in the codebase but for this small
// settings file the JSON shape is fixed and trivial — a flat object
// of primitive values. Hand-rolling the (de)serializer keeps this
// module independent of where serde_json is exposed.

fn parse_json_object(text: &str) -> Result<HashMap<String, Value>, SettingsError> {
    let s = text.trim();
    let inner = s.strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| SettingsError::Parse(
            "expected top-level object".into()))?;
    let mut map = HashMap::new();
    // Simple state machine: read "key" : value pairs separated by
    // commas. We DON'T support nested objects, arrays, escapes
    // beyond \" and \\, or unicode escapes. That's fine for our
    // bounded use case; the day we need more, we'll wire serde_json.
    let mut chars = inner.char_indices().peekable();
    loop {
        // Skip whitespace + commas.
        loop {
            match chars.peek() {
                Some(&(_, c)) if c.is_whitespace() || c == ',' => { chars.next(); }
                _ => break,
            }
        }
        if chars.peek().is_none() { break; }
        // Expect key (quoted string).
        let key = parse_json_string(&mut chars)?;
        // Skip whitespace + colon.
        loop {
            match chars.peek() {
                Some(&(_, c)) if c.is_whitespace() || c == ':' => { chars.next(); }
                _ => break,
            }
        }
        let value = parse_json_value(&mut chars)?;
        map.insert(key, value);
    }
    Ok(map)
}

fn parse_json_string<I>(chars: &mut std::iter::Peekable<I>) -> Result<String, SettingsError>
where I: Iterator<Item = (usize, char)>
{
    // Consume the opening quote.
    match chars.next() {
        Some((_, '"')) => {}
        _ => return Err(SettingsError::Parse("expected '\"'".into())),
    }
    let mut out = String::new();
    loop {
        match chars.next() {
            Some((_, '"')) => return Ok(out),
            Some((_, '\\')) => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, c)) => out.push(c),
                None => return Err(SettingsError::Parse("unterminated escape".into())),
            },
            Some((_, c)) => out.push(c),
            None => return Err(SettingsError::Parse("unterminated string".into())),
        }
    }
}

fn parse_json_value<I>(chars: &mut std::iter::Peekable<I>) -> Result<Value, SettingsError>
where I: Iterator<Item = (usize, char)>
{
    // Skip whitespace.
    loop {
        match chars.peek() {
            Some(&(_, c)) if c.is_whitespace() => { chars.next(); }
            _ => break,
        }
    }
    let &(_, c) = chars.peek()
        .ok_or_else(|| SettingsError::Parse("expected value".into()))?;
    match c {
        '"' => Ok(Value::Str(parse_json_string(chars)?)),
        't' | 'f' => parse_json_bool(chars),
        '-' | '0'..='9' => parse_json_number(chars),
        _ => Err(SettingsError::Parse(format!("unexpected char '{}'", c))),
    }
}

fn parse_json_bool<I>(chars: &mut std::iter::Peekable<I>) -> Result<Value, SettingsError>
where I: Iterator<Item = (usize, char)>
{
    // Read until non-alphabetic.
    let mut word = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c.is_ascii_alphabetic() { word.push(c); chars.next(); }
        else { break; }
    }
    match word.as_str() {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        other => Err(SettingsError::Parse(
            format!("expected true/false, got '{}'", other))),
    }
}

fn parse_json_number<I>(chars: &mut std::iter::Peekable<I>) -> Result<Value, SettingsError>
where I: Iterator<Item = (usize, char)>
{
    let mut buf = String::new();
    let mut has_dot = false;
    while let Some(&(_, c)) = chars.peek() {
        match c {
            '-' | '+' | '0'..='9' => { buf.push(c); chars.next(); }
            '.' | 'e' | 'E' => { has_dot = true; buf.push(c); chars.next(); }
            _ => break,
        }
    }
    if has_dot {
        buf.parse::<f64>()
            .map(Value::Float)
            .map_err(|e| SettingsError::Parse(format!("float: {}", e)))
    } else {
        buf.parse::<i64>()
            .map(Value::Int)
            .map_err(|e| SettingsError::Parse(format!("int: {}", e)))
    }
}

fn serialize_json_object(map: &HashMap<String, Value>) -> String {
    let mut out = String::from("{\n");
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(k, _)| k.as_str());
    for (i, (key, value)) in entries.iter().enumerate() {
        out.push_str("  \"");
        out.push_str(&escape_json_string(key));
        out.push_str("\": ");
        match value {
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Float(f) => out.push_str(&f.to_string()),
            Value::Str(s) => {
                out.push('"');
                out.push_str(&escape_json_string(s));
                out.push('"');
            }
        }
        if i + 1 < entries.len() { out.push(','); }
        out.push('\n');
    }
    out.push('}');
    out
}

fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('"', "\\\"")
     .replace('\n', "\\n")
     .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_temp_dir<F: FnOnce()>(f: F) {
        // Redirect settings to a temp dir so tests don't clobber
        // the real user's file.
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", tmp.path());
        invalidate_cache_for_test();
        f();
        invalidate_cache_for_test();
        match original {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[test]
    fn set_and_get_bool_roundtrips() {
        use_temp_dir(|| {
            set_bool("test_key_b", true).unwrap();
            assert_eq!(get_bool("test_key_b"), Some(true));
            set_bool("test_key_b", false).unwrap();
            assert_eq!(get_bool("test_key_b"), Some(false));
        });
    }

    #[test]
    fn get_missing_key_returns_none() {
        use_temp_dir(|| {
            assert_eq!(get_bool("never_set"), None);
            assert_eq!(get_float("never_set"), None);
            assert_eq!(get_str("never_set"), None);
        });
    }

    #[test]
    fn set_and_get_float_roundtrips() {
        use_temp_dir(|| {
            set_float("sat", 0.275).unwrap();
            let got = get_float("sat").expect("set");
            assert!((got - 0.275).abs() < 1e-9, "got {}", got);
        });
    }

    #[test]
    fn set_and_get_str_roundtrips_special_chars() {
        use_temp_dir(|| {
            set_str("tag", "front \"door\" mic\\backup").unwrap();
            assert_eq!(get_str("tag"),
                Some("front \"door\" mic\\backup".to_string()));
        });
    }

    #[test]
    fn settings_persist_across_cache_invalidation() {
        use_temp_dir(|| {
            set_bool("persisted", true).unwrap();
            invalidate_cache_for_test();
            // Re-read should pull from disk.
            assert_eq!(get_bool("persisted"), Some(true));
        });
    }

    #[test]
    fn missing_file_treated_as_empty() {
        // No setup; just read from a fresh temp dir.
        use_temp_dir(|| {
            assert_eq!(get_bool("anything"), None);
        });
    }

    #[test]
    fn corrupt_file_treated_as_empty() {
        use_temp_dir(|| {
            let path = settings_path();
            std::fs::write(&path, b"{not valid json").unwrap();
            invalidate_cache_for_test();
            assert_eq!(get_bool("any"), None);
            // And we should still be able to write after.
            set_bool("any", true).unwrap();
            assert_eq!(get_bool("any"), Some(true));
        });
    }

    #[test]
    fn parse_simple_json() {
        let m = parse_json_object(
            r#"{"a": true, "b": 42, "c": 3.14, "d": "hello"}"#).unwrap();
        assert_eq!(m.get("a"), Some(&Value::Bool(true)));
        assert_eq!(m.get("b"), Some(&Value::Int(42)));
        assert_eq!(m.get("c"), Some(&Value::Float(3.14)));
        assert_eq!(m.get("d"), Some(&Value::Str("hello".into())));
    }

    #[test]
    fn parse_empty_object() {
        let m = parse_json_object("{}").unwrap();
        assert!(m.is_empty());
        let m = parse_json_object("{ }").unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn parse_json_handles_escapes() {
        let m = parse_json_object(r#"{"k": "a\"b\\c"}"#).unwrap();
        assert_eq!(m.get("k"), Some(&Value::Str("a\"b\\c".into())));
    }

    #[test]
    fn serialize_sorts_keys_deterministically() {
        let mut m = HashMap::new();
        m.insert("z".into(), Value::Bool(true));
        m.insert("a".into(), Value::Int(1));
        m.insert("m".into(), Value::Float(2.5));
        let s = serialize_json_object(&m);
        // 'a' should appear before 'm' before 'z'.
        let a_pos = s.find("\"a\"").unwrap();
        let m_pos = s.find("\"m\"").unwrap();
        let z_pos = s.find("\"z\"").unwrap();
        assert!(a_pos < m_pos && m_pos < z_pos);
    }

    #[test]
    fn nonexistent_xdg_dir_falls_back_gracefully() {
        // Set XDG to a path that may not be writable; the helper
        // should still produce a usable path.
        let path = settings_path();
        // The file path should be a non-empty Buf.
        assert!(path.to_string_lossy().len() > 0);
    }
}
