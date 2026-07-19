//! Import / export of custom keyboard layouts.
//!
//! A [`Layout`] already derives serde `Serialize`/`Deserialize`, so the
//! on-disk format is just pretty-printed JSON. This module adds the
//! surrounding workflow the keyboard builder needs:
//!
//! - a stable user layouts directory (`~/.local/share/rusty-board/layouts`)
//! - export a layout to a JSON string (for copy/paste sharing) or a file
//! - import a layout from a JSON string or file, with validation so a
//!   malformed or hostile file can't crash the keyboard at load time
//! - list / save / delete named custom layouts
//!
//! The builder mixes key *kinds* freely (regular character keys,
//! cardinal-gesture cells, steno cells) within one [`Layout`]; nothing
//! here constrains that — a [`Key`]'s `action` and `gestures` decide its
//! behavior, and a [`Layout`]'s `style` decides the dispatch mode. A
//! "mixed" layout simply carries different key kinds per cell.

use super::Layout;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Directory where user-created/imported layouts live. Created on first
/// use. Falls back to a temp-dir-relative path if no data dir resolves
/// (headless / unusual environments) so callers always get *a* path.
pub fn user_layouts_dir() -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|b| b.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("rusty-board").join("layouts")
}

/// Ensure the user layouts directory exists, returning its path.
pub fn ensure_user_layouts_dir() -> Result<PathBuf> {
    let dir = user_layouts_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating layouts dir {}", dir.display()))?;
    Ok(dir)
}

/// Turn an arbitrary display string into a safe filename stem: lowercase,
/// alnum and `-`/`_` preserved, everything else collapsed to `-`. Empty
/// input yields "layout". Used to derive `<slug>.json` from a layout name.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if ch == '-' || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "layout".to_string()
    } else {
        trimmed
    }
}

/// Validate a layout enough that loading it won't panic or produce a
/// visibly-broken keyboard. Returns a human-readable error describing the
/// first problem found. Kept intentionally lenient: the builder may
/// produce unconventional layouts on purpose, so we only reject things
/// that are structurally broken, not merely unusual.
pub fn validate(layout: &Layout) -> Result<()> {
    if layout.name.trim().is_empty() {
        return Err(anyhow!("layout name is empty"));
    }
    if layout.display_name.trim().is_empty() {
        return Err(anyhow!("layout display_name is empty"));
    }
    if layout.rows.is_empty() {
        return Err(anyhow!("layout has no rows"));
    }
    if !(layout.grid_columns.is_finite() && layout.grid_columns > 0.0) {
        return Err(anyhow!(
            "grid_columns must be a positive finite number (got {})",
            layout.grid_columns
        ));
    }
    let total_keys: usize = layout.rows.iter().map(|r| r.keys.len()).sum();
    if total_keys == 0 {
        return Err(anyhow!("layout has rows but no keys"));
    }
    // Per-row: a row whose key widths sum to more than ~2x grid_columns is
    // almost certainly a mistake (keys would overflow massively). Allow
    // generous slack for intentional wide keys.
    for (i, row) in layout.rows.iter().enumerate() {
        let row_units: f32 = row.keys.iter().map(|k| k.width).sum();
        if row_units > layout.grid_columns * 2.0 + 0.01 {
            return Err(anyhow!(
                "row {} key widths sum to {:.1} units, more than 2x grid_columns ({:.1})",
                i + 1,
                row_units,
                layout.grid_columns
            ));
        }
    }
    Ok(())
}

/// Serialize a layout to a pretty JSON string for sharing (copy/paste,
/// messaging, pasting into the builder's import box).
pub fn export_to_string(layout: &Layout) -> Result<String> {
    serde_json::to_string_pretty(layout).context("serializing layout to JSON")
}

/// Parse a layout from a JSON string and validate it. Use this for the
/// import-from-clipboard path. The returned layout is guaranteed to pass
/// [`validate`].
pub fn import_from_string(json: &str) -> Result<Layout> {
    let layout: Layout =
        serde_json::from_str(json).context("parsing layout JSON")?;
    validate(&layout)?;
    Ok(layout)
}

/// Write a layout to an explicit path as pretty JSON.
pub fn export_to_file(layout: &Layout, path: &Path) -> Result<()> {
    let json = export_to_string(layout)?;
    std::fs::write(path, json)
        .with_context(|| format!("writing layout to {}", path.display()))?;
    Ok(())
}

/// Save a layout into the user layouts directory as `<slug>.json`,
/// where the slug is derived from `layout.name`. Returns the path
/// written. Validates first so we never persist a broken layout.
pub fn save_user_layout(layout: &Layout) -> Result<PathBuf> {
    validate(layout)?;
    let dir = ensure_user_layouts_dir()?;
    let path = dir.join(format!("{}.json", slugify(&layout.name)));
    export_to_file(layout, &path)?;
    Ok(path)
}

/// A discovered user layout: its file path plus the parsed name/display
/// name. Parsing failures are reported via `error` so the UI can show a
/// row for a corrupt file rather than silently hiding it.
#[derive(Debug, Clone)]
pub struct UserLayoutEntry {
    pub path: PathBuf,
    pub name: String,
    pub display_name: String,
    pub error: Option<String>,
}

/// List the user layouts directory. Each `.json` file becomes an entry;
/// files that fail to parse get an entry with `error` set so the builder
/// can surface "this layout file is broken" instead of dropping it.
pub fn list_user_layouts() -> Vec<UserLayoutEntry> {
    let dir = user_layouts_dir();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match Layout::load_from_file(&path) {
            Ok(l) => out.push(UserLayoutEntry {
                path: path.clone(),
                name: l.name,
                display_name: l.display_name,
                error: None,
            }),
            Err(e) => out.push(UserLayoutEntry {
                name: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string(),
                display_name: "(unparseable)".to_string(),
                error: Some(e.to_string()),
                path,
            }),
        }
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    out
}

/// Delete a user layout file by path. Refuses paths outside the user
/// layouts dir as a guard against an import box being abused to delete
/// arbitrary files.
pub fn delete_user_layout(path: &Path) -> Result<()> {
    let dir = user_layouts_dir();
    let canon_dir = dir.canonicalize().unwrap_or(dir.clone());
    let canon_path = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !canon_path.starts_with(&canon_dir) {
        return Err(anyhow!(
            "refusing to delete {} — outside the user layouts directory",
            path.display()
        ));
    }
    std::fs::remove_file(&canon_path)
        .with_context(|| format!("deleting {}", canon_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basics() {
        assert_eq!(slugify("My Cool Layout"), "my-cool-layout");
        assert_eq!(slugify("  spaced  out  "), "spaced-out");
        assert_eq!(slugify("weird!!!chars@@@"), "weird-chars");
        assert_eq!(slugify("keep_under-score"), "keep_under-score");
        assert_eq!(slugify(""), "layout");
        assert_eq!(slugify("!!!"), "layout");
        assert_eq!(slugify("CAPS"), "caps");
    }

    #[test]
    fn export_import_roundtrip() {
        let original = Layout::default_qwerty();
        let json = export_to_string(&original).expect("export");
        let back = import_from_string(&json).expect("import");
        assert_eq!(original.name, back.name);
        assert_eq!(original.rows.len(), back.rows.len());
        // Spot-check a key survives.
        assert_eq!(
            original.rows[0].keys.len(),
            back.rows[0].keys.len()
        );
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut l = Layout::default_qwerty();
        l.name = "  ".into();
        assert!(validate(&l).is_err());
    }

    #[test]
    fn validate_rejects_no_rows() {
        let mut l = Layout::default_qwerty();
        l.rows.clear();
        assert!(validate(&l).is_err());
    }

    #[test]
    fn validate_rejects_bad_grid_columns() {
        let mut l = Layout::default_qwerty();
        l.grid_columns = 0.0;
        assert!(validate(&l).is_err());
        l.grid_columns = f32::NAN;
        assert!(validate(&l).is_err());
    }

    #[test]
    fn validate_accepts_default_layouts() {
        assert!(validate(&Layout::default_qwerty()).is_ok());
    }

    #[test]
    fn import_rejects_malformed_json() {
        assert!(import_from_string("{ not valid json").is_err());
        // Valid JSON but not a layout.
        assert!(import_from_string("{\"foo\": 1}").is_err());
    }
}
