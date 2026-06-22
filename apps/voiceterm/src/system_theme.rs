// src/system_theme.rs
//
// v0.1.423 (system light/dark watcher): detect the desktop's
// color-scheme preference so the app can optionally auto-follow
// it. Supports GNOME/GTK (gsettings) and KDE Plasma (kdeglobals).
//
// The detection RUNS a command / reads a file; the PARSING is
// split into pure functions so it's unit-testable without a live
// desktop. main.rs exposes a qt_method that returns the result;
// QML polls it and, when "follow system" is enabled, flips
// between a chosen dark and light theme.

#![allow(dead_code)]

/// The desktop's color-scheme preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    /// Couldn't determine (no recognized desktop / command
    /// missing). Callers should NOT change the theme on Unknown.
    Unknown,
    Light,
    Dark,
}

impl ColorScheme {
    /// Wire value for QML: -1 unknown, 0 light, 1 dark.
    pub fn to_i32(self) -> i32 {
        match self {
            ColorScheme::Unknown => -1,
            ColorScheme::Light => 0,
            ColorScheme::Dark => 1,
        }
    }
}

/// Parse the output of
/// `gsettings get org.gnome.desktop.interface color-scheme`.
/// Typical values (quoted): 'prefer-dark', 'prefer-light',
/// 'default'. GNOME treats 'default' as light.
pub fn parse_gsettings_color_scheme(raw: &str) -> ColorScheme {
    let v = raw.trim().trim_matches('\'').trim_matches('"').to_lowercase();
    if v.contains("dark") {
        ColorScheme::Dark
    } else if v.contains("light") || v == "default" {
        ColorScheme::Light
    } else {
        ColorScheme::Unknown
    }
}

/// Parse a KDE `kdeglobals` file's contents for the color scheme.
/// KDE doesn't expose a single "prefer-dark" flag; instead the
/// active scheme name lives under `[General] ColorScheme=...`
/// (e.g. "BreezeDark" / "BreezeLight"). We classify by the name
/// containing "dark"/"light". If the name is ambiguous, fall back
/// to the window background lightness under
/// `[Colors:Window] BackgroundNormal=R,G,B` (dark bg → Dark).
pub fn parse_kdeglobals(contents: &str) -> ColorScheme {
    // First: ColorScheme= name heuristic.
    if let Some(name) = ini_value(contents, "General", "ColorScheme") {
        let n = name.to_lowercase();
        if n.contains("dark") {
            return ColorScheme::Dark;
        }
        if n.contains("light") {
            return ColorScheme::Light;
        }
    }
    // Fallback: window background lightness.
    if let Some(bg) = ini_value(contents, "Colors:Window", "BackgroundNormal") {
        if let Some(lum) = rgb_luma(&bg) {
            return if lum < 0.5 { ColorScheme::Dark } else { ColorScheme::Light };
        }
    }
    ColorScheme::Unknown
}

/// Minimal INI lookup: find `key=value` under `[section]`.
/// Returns the trimmed value. Stops at the next `[...]` header.
fn ini_value(contents: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let want_header = format!("[{}]", section);
    for line in contents.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_section = l == want_header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Parse a KDE "R,G,B" color and return perceived luma in 0.0..=1.0.
fn rgb_luma(s: &str) -> Option<f32> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 3 {
        return None;
    }
    let r: f32 = parts[0].trim().parse().ok()?;
    let g: f32 = parts[1].trim().parse().ok()?;
    let b: f32 = parts[2].trim().parse().ok()?;
    // Rec. 601 luma, normalized to 0..1.
    Some((0.299 * r + 0.587 * g + 0.114 * b) / 255.0)
}

/// Detect the system color scheme by trying, in order:
///   1. GNOME/GTK via `gsettings`
///   2. KDE via `~/.config/kdeglobals`
/// Returns Unknown if neither yields a confident answer (callers
/// must not change the theme on Unknown).
pub fn detect() -> ColorScheme {
    // 1. GNOME / GTK.
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let cs = parse_gsettings_color_scheme(&raw);
            if cs != ColorScheme::Unknown {
                return cs;
            }
        }
    }
    // 2. KDE Plasma via kdeglobals.
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::Path::new(&home).join(".config/kdeglobals");
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let cs = parse_kdeglobals(&contents);
            if cs != ColorScheme::Unknown {
                return cs;
            }
        }
    }
    ColorScheme::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsettings_prefer_dark() {
        assert_eq!(parse_gsettings_color_scheme("'prefer-dark'"), ColorScheme::Dark);
        assert_eq!(parse_gsettings_color_scheme("prefer-dark\n"), ColorScheme::Dark);
    }

    #[test]
    fn gsettings_prefer_light_and_default() {
        assert_eq!(parse_gsettings_color_scheme("'prefer-light'"), ColorScheme::Light);
        // GNOME 'default' = light.
        assert_eq!(parse_gsettings_color_scheme("'default'"), ColorScheme::Light);
    }

    #[test]
    fn gsettings_garbage_is_unknown() {
        assert_eq!(parse_gsettings_color_scheme(""), ColorScheme::Unknown);
        assert_eq!(parse_gsettings_color_scheme("'wat'"), ColorScheme::Unknown);
    }

    #[test]
    fn kde_scheme_name_dark() {
        let ini = "[General]\nColorScheme=BreezeDark\nWidgetStyle=Breeze\n";
        assert_eq!(parse_kdeglobals(ini), ColorScheme::Dark);
    }

    #[test]
    fn kde_scheme_name_light() {
        let ini = "[General]\nColorScheme=BreezeLight\n";
        assert_eq!(parse_kdeglobals(ini), ColorScheme::Light);
    }

    #[test]
    fn kde_fallback_to_bg_luma() {
        // Ambiguous scheme name → use window bg. Dark bg.
        let ini = "[General]\nColorScheme=Custom\n[Colors:Window]\nBackgroundNormal=30,30,30\n";
        assert_eq!(parse_kdeglobals(ini), ColorScheme::Dark);
        // Light bg.
        let ini2 = "[General]\nColorScheme=Custom\n[Colors:Window]\nBackgroundNormal=240,240,240\n";
        assert_eq!(parse_kdeglobals(ini2), ColorScheme::Light);
    }

    #[test]
    fn kde_no_info_is_unknown() {
        assert_eq!(parse_kdeglobals("[Other]\nx=1\n"), ColorScheme::Unknown);
    }

    #[test]
    fn ini_value_respects_section() {
        let ini = "[A]\nk=1\n[B]\nk=2\n";
        assert_eq!(ini_value(ini, "A", "k"), Some("1".to_string()));
        assert_eq!(ini_value(ini, "B", "k"), Some("2".to_string()));
        assert_eq!(ini_value(ini, "C", "k"), None);
    }

    #[test]
    fn scheme_wire_values() {
        assert_eq!(ColorScheme::Unknown.to_i32(), -1);
        assert_eq!(ColorScheme::Light.to_i32(), 0);
        assert_eq!(ColorScheme::Dark.to_i32(), 1);
    }
}
