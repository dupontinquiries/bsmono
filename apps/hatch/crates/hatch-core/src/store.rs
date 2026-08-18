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
    /// Master switch. Off sets every duration in `anim` to zero rather than
    /// bypassing the board — see the loader.
    pub animations: bool,
    /// Tile motion, the plasmoid's Appearance group.
    pub anim: crate::anim::BoardConfig,
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
            animations: true,
            anim: crate::anim::BoardConfig::default(),
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
        // Animation tunables. BoardConfig's own comment has said "tunables the
        // settings page exposes; defaults follow the plasmoid schema" since it
        // was written, and no settings page ever reached it — the plasmoid's
        // main.xml carried tileSlideStyle, restackDuration and restackStagger
        // and only QML could read them.
        if let Some(v) = ini.top("animations") { c.animations = v != "false" && v != "0"; }
        if let Some(v) = ini.top("slide_style") {
            // By NAME, not the plasmoid's integer. main.xml stores
            // tileSlideStyle=1 and only the QML knew which pattern that was;
            // an ini a person edits by hand should not need that table.
            if let Some(p) = crate::grid::SlidePattern::from_name(v) { c.anim.pattern = p; }
        }
        if let Some(v) = ini.top("enter_ms").and_then(|v| v.parse().ok()) { c.anim.enter_ms = v; }
        if let Some(v) = ini.top("stagger_ms").and_then(|v| v.parse().ok()) { c.anim.stagger_ms = v; }
        if let Some(v) = ini.top("restack_ms").and_then(|v| v.parse().ok()) { c.anim.restack_ms = v; }
        if let Some(v) = ini.top("restack_stagger_ms").and_then(|v| v.parse().ok()) {
            c.anim.restack_stagger_ms = v;
        }
        if let Some(v) = ini.top("exit_ms").and_then(|v| v.parse().ok()) { c.anim.exit_ms = v; }
        // Animations off means DURATIONS OF ZERO, not a separate code path.
        // The board still ticks, tiles still enter and leave and get removed
        // on the tick that finishes their fade — they just arrive instantly.
        // A second path that skips the board would drift from the animated one
        // and only be exercised by whoever turned animations off.
        if !c.animations {
            c.anim.enter_ms = 0.0;
            c.anim.stagger_ms = 0.0;
            c.anim.restack_ms = 0.0;
            c.anim.restack_stagger_ms = 0.0;
            c.anim.exit_ms = 0.0;
        }
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
            "columns={}\nicon_px={}\nopacity={}\nicon_theme={}\nauto_hide={}\nterminal_wrap={}\nsystem_actions={}\n\
             animations={}\nslide_style={}\nenter_ms={}\nstagger_ms={}\nrestack_ms={}\n\
             restack_stagger_ms={}\nexit_ms={}\n",
            self.columns,
            self.icon_px,
            self.opacity,
            self.icon_theme,
            self.auto_hide,
            self.terminal_wrap.join(" "),
            self.system_actions.join(","),
            self.animations,
            self.anim.pattern.name(),
            self.anim.enter_ms,
            self.anim.stagger_ms,
            self.anim.restack_ms,
            self.anim.restack_stagger_ms,
            self.anim.exit_ms,
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

    // Store tests run against a private XDG_CONFIG_HOME so they cannot touch
    // a real user's launcher state; the guard also serialises them against
    // the other modules that redirect the same variable.
    #[test]
    fn config_and_favorites_and_recents_round_trip() {
        let _guard = crate::testenv::scoped_config_home("store");

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
    }
}
#[cfg(test)]
mod anim_config_tests {
    use super::*;
    use crate::grid::SlidePattern;

    #[test]
    fn the_slide_pattern_round_trips_by_name() {
        // config.ini is hand-editable, so the text form is the contract. The
        // plasmoid's integer discriminants stay for its own wire format.
        for p in [SlidePattern::None, SlidePattern::FromBelow, SlidePattern::FromEdges,
                  SlidePattern::Cascade, SlidePattern::Ripple, SlidePattern::Scatter,
                  SlidePattern::Wave] {
            assert_eq!(SlidePattern::from_name(p.name()), Some(p), "{}", p.name());
        }
    }

    #[test]
    fn an_unknown_pattern_name_keeps_the_default() {
        // A typo must not silently select an arbitrary animation.
        assert_eq!(SlidePattern::from_name("from-bellow"), None);
        assert_eq!(SlidePattern::from_name(""), None);
        // Case and underscore spellings are accepted, since both are natural
        // to type.
        assert_eq!(SlidePattern::from_name("From_Below"), Some(SlidePattern::FromBelow));
    }

    #[test]
    fn animations_off_means_zero_durations_not_a_second_code_path() {
        // The board still ticks, tiles still enter, restack and get removed on
        // the tick that finishes their fade — they just arrive instantly. A
        // bypass would drift from the animated path and only be exercised by
        // whoever turned animations off.
        let ini = "animations=false\n";
        let parsed = config_kit::Ini::parse(ini);
        let mut c = Config::default();
        if let Some(v) = parsed.top("animations") { c.animations = v != "false" && v != "0"; }
        assert!(!c.animations);
        // Mirror the loader's clamp.
        if !c.animations {
            c.anim.enter_ms = 0.0;
            c.anim.stagger_ms = 0.0;
            c.anim.restack_ms = 0.0;
            c.anim.restack_stagger_ms = 0.0;
            c.anim.exit_ms = 0.0;
        }
        let mut b = crate::anim::Board::new(c.anim);
        let g = crate::grid::GridSpec::fit(800.0, 600.0, 96.0, 0.0);
        b.retarget(&["a", "b"], &g, 0.0);
        b.tick(0.0);
        // With zero durations the FIRST tick must land every tile at rest. An
        // "instant" animation that still needs a frame to settle is the bug
        // this shape avoids, and is_settled is the board's own word for it.
        assert_eq!(b.len(), 2);
        assert!(b.is_settled(), "zero durations must settle on the first tick");
    }
}
