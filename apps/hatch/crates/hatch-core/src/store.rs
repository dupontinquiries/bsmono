//! store — what hatch remembers, and where.
//!
//! Everything lives under `~/.config/hatch/` via config-kit, in formats a
//! phone user can fix over ssh with a text editor:
//!
//! * `favorites`  — one desktop-file id per line, in display order.
//! * `recents`    — `id<TAB>unix-seconds`, newest first, capped.
//! * `config.ini` — flat keys (see [`Config`]).

const APP: &str = "hatch";
const RECENTS_CAP: usize = 40;

/// Launcher configuration. Mirrors the plasmoid's knobs where they overlap
/// (icon size, opacity, grid columns) so the two shells stay visually in step.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 0 = auto from width.
    pub columns: u32,
    pub icon_px: u32,
    /// Window/background opacity 0..=1 (matches plasmoid `backgroundOpacity`).
    pub opacity: f32,
    pub icon_theme: String,
    /// Prefix for Terminal=true entries.
    pub terminal_wrap: Vec<String>,
    /// System-row action ids, in order. Known ids: lock, logout, reboot, poweroff.
    pub system_actions: Vec<String>,
    /// Hide the window when it loses focus (launcher behaviour).
    pub auto_hide: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            columns: 0,
            icon_px: 64,
            opacity: 0.4,
            icon_theme: default_icon_theme(),
            terminal_wrap: vec!["konsole".into(), "-e".into()],
            system_actions: vec!["lock".into(), "reboot".into(), "poweroff".into()],
            auto_hide: true,
        }
    }
}

/// The theme most likely installed: honour an explicit env hint, else breeze
/// (the Plasma Mobile default), and resolve() falls back to hicolor anyway.
fn default_icon_theme() -> String {
    std::env::var("HATCH_ICON_THEME").unwrap_or_else(|_| "breeze".into())
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config::default();
        let Some(text) = config_kit::load(APP, "config.ini") else { return c };
        let ini = config_kit::Ini::parse(&text);
        if let Some(v) = ini.top("columns").and_then(|v| v.parse().ok()) { c.columns = v; }
        if let Some(v) = ini.top("icon_px").and_then(|v| v.parse().ok()) { c.icon_px = v; }
        if let Some(v) = ini.top("opacity").and_then(|v| v.parse().ok()) { c.opacity = v; }
        if let Some(v) = ini.top("icon_theme") { c.icon_theme = v.to_string(); }
        if let Some(v) = ini.top("auto_hide") { c.auto_hide = v != "false" && v != "0"; }
        if let Some(v) = ini.top("terminal_wrap") {
            c.terminal_wrap = v.split_whitespace().map(str::to_string).collect();
        }
        if let Some(v) = ini.top("system_actions") {
            c.system_actions = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        c
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = format!(
            "columns={}\nicon_px={}\nopacity={}\nicon_theme={}\nauto_hide={}\nterminal_wrap={}\nsystem_actions={}\n",
            self.columns,
            self.icon_px,
            self.opacity,
            self.icon_theme,
            self.auto_hide,
            self.terminal_wrap.join(" "),
            self.system_actions.join(","),
        );
        config_kit::save(APP, "config.ini", &text)
    }
}

pub fn load_favorites() -> Vec<String> {
    config_kit::load(APP, "favorites")
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn save_favorites(ids: &[String]) -> std::io::Result<()> {
    config_kit::save(APP, "favorites", &(ids.join("\n") + "\n"))
}

/// Recents, newest first: `(id, unix_seconds)`.
pub fn load_recents() -> Vec<(String, u64)> {
    config_kit::load(APP, "recents")
        .map(|t| {
            t.lines()
                .filter_map(|l| {
                    let (id, ts) = l.split_once('\t')?;
                    Some((id.trim().to_string(), ts.trim().parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn record_recent(id: &str) -> std::io::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut rec = load_recents();
    rec.retain(|(i, _)| i != id);
    rec.insert(0, (id.to_string(), now));
    rec.truncate(RECENTS_CAP);
    let text: String = rec.iter().map(|(i, t)| format!("{i}\t{t}\n")).collect();
    config_kit::save(APP, "recents", &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Store tests run against a private XDG_CONFIG_HOME so they cannot touch a
    // real user's launcher state. Env is process-global, so they share one
    // serialised test to avoid racing other tests' config-kit reads.
    #[test]
    fn config_and_favorites_and_recents_round_trip() {
        let tmp = std::env::temp_dir().join(format!("hatch-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("XDG_CONFIG_HOME", &tmp);

        let mut c = Config::default();
        c.columns = 5;
        c.opacity = 0.7;
        c.system_actions = vec!["lock".into(), "poweroff".into()];
        c.save().unwrap();
        let back = Config::load();
        assert_eq!(back.columns, 5);
        assert!((back.opacity - 0.7).abs() < 1e-6);
        assert_eq!(back.system_actions, vec!["lock", "poweroff"]);

        save_favorites(&["a.desktop-id".into(), "b".into()]).unwrap();
        assert_eq!(load_favorites(), vec!["a.desktop-id", "b"]);

        record_recent("b").unwrap();
        record_recent("a").unwrap();
        record_recent("b").unwrap(); // relaunch moves to front, no duplicate
        let rec = load_recents();
        let ids: Vec<_> = rec.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"]);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
