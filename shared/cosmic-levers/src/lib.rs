//! cosmic-levers — COSMIC desktop settings as files, the way cosmic-config
//! stores them, so a touch settings app (tideshell-settings) can change them
//! without linking libcosmic.
//!
//! ## The layout (cosmic-config, libcosmic `cosmic-config/src/lib.rs`)
//!
//! One file per key, RON-encoded:
//!
//! ```text
//! user:    $XDG_CONFIG_HOME/cosmic/<id>/v<version>/<key>     (written here)
//! system:  <each of $XDG_DATA_DIRS>/cosmic/<id>/v<version>/<key>
//! ```
//!
//! A reader takes the user file if it exists, else the first system file,
//! else the program's compiled-in default. COSMIC's components watch these
//! files and apply changes live (seen on the FP5 for Shortcuts; cosmic-idle
//! uses `ConfigWatchSource` on its config at epoch-1.8.0).
//!
//! ## Levers
//!
//! A [`Lever`] is one setting: where it lives, its type, its default, and
//! the control a UI shows for it. Every entry in [`levers`] was checked
//! against COSMIC's source (the ids, versions, key names, types, units and
//! defaults are quoted in its doc). Nothing here guesses a key: a wrong key
//! is a file COSMIC never reads, and the UI would show a change that does
//! nothing.
//!
//! Values are bool and `Option<u32>`, whose RON forms (`true`, `Some(900000)`,
//! `None`) are handled exactly here; a type needing a real RON parser is a
//! reason to add one, not to approximate.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub id: &'static str,
    pub version: u64,
    pub key: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// `Option<u32>` in milliseconds; None = never.
    OptMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    OptMillis(Option<u32>),
}

impl Value {
    /// The RON text cosmic-config writes for this value.
    pub fn to_ron(self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::OptMillis(Some(n)) => format!("Some({n})"),
            Value::OptMillis(None) => "None".into(),
        }
    }

    /// Parse the RON text of a file as `kind`. Accepts the forms RON writes
    /// (including a bare number, which RON's `implicit_some` produces for an
    /// Option). None = not a value of this kind.
    pub fn parse(kind: Kind, text: &str) -> Option<Value> {
        let t = text.trim();
        match kind {
            Kind::Bool => match t {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            Kind::OptMillis => {
                if t == "None" {
                    return Some(Value::OptMillis(None));
                }
                let inner = t.strip_prefix("Some(").and_then(|x| x.strip_suffix(')')).map(str::trim).unwrap_or(t);
                inner.parse::<u32>().ok().map(|n| Value::OptMillis(Some(n)))
            }
        }
    }

    /// For a UI: "On", "Off", "5 min", "Never".
    pub fn describe(self) -> String {
        match self {
            Value::Bool(true) => "On".into(),
            Value::Bool(false) => "Off".into(),
            Value::OptMillis(None) => "Never".into(),
            Value::OptMillis(Some(ms)) => describe_ms(ms),
        }
    }
}

pub fn describe_ms(ms: u32) -> String {
    let s = ms / 1000;
    match s {
        _ if ms % 1000 != 0 || s < 60 => format!("{:.0} s", ms as f64 / 1000.0),
        _ if s % 3600 == 0 => format!("{} h", s / 3600),
        _ if s % 60 == 0 => format!("{} min", s / 60),
        _ => format!("{} min {} s", s / 60, s % 60),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The user's file; what a write changes.
    User(PathBuf),
    /// A system default file (read-only).
    System(PathBuf),
    /// No file: the program's compiled-in default.
    Default,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub value: Value,
    pub source: Source,
    /// The file text the value came from, for showing what is on disk.
    pub raw: Option<String>,
    /// A file that exists but does not hold a value of this kind. The value
    /// shown then comes from the next source; COSMIC may treat the bad file
    /// differently (an error and its own default), so the UI says so.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Toggle,
    /// (label, value) choices, in order.
    Choices(&'static [(&'static str, Option<u32>)]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lever {
    pub section: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub entry: Entry,
    pub kind: Kind,
    /// The owning program's compiled-in default (from its source).
    pub default: Value,
    pub control: Control,
}

const MIN: u32 = 60 * 1000;

/// Screen-off and suspend choices, in ms. The defaults (15 min, 30 min) are
/// among them so the current value always shows as selected.
pub const IDLE_CHOICES: &[(&str, Option<u32>)] = &[
    ("30 s", Some(30 * 1000)),
    ("1 min", Some(MIN)),
    ("2 min", Some(2 * MIN)),
    ("5 min", Some(5 * MIN)),
    ("10 min", Some(10 * MIN)),
    ("15 min", Some(15 * MIN)),
    ("30 min", Some(30 * MIN)),
    ("1 h", Some(60 * MIN)),
    ("Never", None),
];

/// The settings this crate knows, each checked against COSMIC's source.
///
/// - `com.system76.CosmicTheme.Mode` v1 (libcosmic
///   `cosmic-theme/src/model/mode.rs`, `#[version = 1]`): `is_dark: bool`
///   default true, `auto_switch: bool` default false.
/// - `com.system76.CosmicIdle` v1 (cosmic-idle epoch-1.8.0, `src/main.rs`
///   `Config::new("com.system76.CosmicIdle", 1)`; `cosmic-idle-config`):
///   `screen_off_time`, `suspend_on_battery_time`, `suspend_on_ac_time`,
///   each `Option<u32>` in ms, defaults 15 min, 15 min, 30 min. cosmic-idle
///   watches this config (`ConfigWatchSource`), so changes apply live.
pub fn levers() -> Vec<Lever> {
    let mode = |key| Entry { id: "com.system76.CosmicTheme.Mode", version: 1, key };
    let idle = |key| Entry { id: "com.system76.CosmicIdle", version: 1, key };
    vec![
        Lever { section: "Appearance", label: "Dark mode", help: "COSMIC's light or dark theme.", entry: mode("is_dark"), kind: Kind::Bool, default: Value::Bool(true), control: Control::Toggle },
        Lever { section: "Appearance", label: "Switch light/dark automatically", help: "Follow the time of day instead of the dark-mode switch.", entry: mode("auto_switch"), kind: Kind::Bool, default: Value::Bool(false), control: Control::Toggle },
        Lever { section: "Screen and power", label: "Turn the screen off after", help: "Idle time before cosmic-idle turns the display off.", entry: idle("screen_off_time"), kind: Kind::OptMillis, default: Value::OptMillis(Some(15 * MIN)), control: Control::Choices(IDLE_CHOICES) },
        Lever { section: "Screen and power", label: "Suspend on battery after", help: "Idle time before suspending while on battery.", entry: idle("suspend_on_battery_time"), kind: Kind::OptMillis, default: Value::OptMillis(Some(15 * MIN)), control: Control::Choices(IDLE_CHOICES) },
        Lever { section: "Screen and power", label: "Suspend when charging after", help: "Idle time before suspending while on external power.", entry: idle("suspend_on_ac_time"), kind: Kind::OptMillis, default: Value::OptMillis(Some(30 * MIN)), control: Control::Choices(IDLE_CHOICES) },
    ]
}

/// Where cosmic-config's files are.
#[derive(Debug, Clone)]
pub struct Store {
    pub config_home: PathBuf,
    pub data_dirs: Vec<PathBuf>,
}

impl Store {
    /// `$XDG_CONFIG_HOME` (else `~/.config`) and `$XDG_DATA_DIRS` (else
    /// `/usr/local/share:/usr/share`), the XDG defaults cosmic-config's xdg
    /// crate uses.
    pub fn from_env() -> Option<Store> {
        let config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(':').filter(|p| p.starts_with('/')).map(PathBuf::from).collect())
            .unwrap_or_else(|| vec![PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]);
        Some(Store { config_home, data_dirs })
    }

    fn rel(e: &Entry) -> PathBuf {
        Path::new("cosmic").join(e.id).join(format!("v{}", e.version)).join(e.key)
    }

    pub fn user_path(&self, e: &Entry) -> PathBuf {
        self.config_home.join(Self::rel(e))
    }

    pub fn system_path(&self, e: &Entry) -> Option<PathBuf> {
        self.data_dirs.iter().map(|d| d.join(Self::rel(e))).find(|p| p.is_file())
    }

    /// The value as COSMIC would see it, and where it came from.
    pub fn read(&self, l: &Lever) -> Reading {
        let mut problem = None;
        let user = self.user_path(&l.entry);
        let candidates: Vec<(PathBuf, bool)> = std::iter::once((user, true)).chain(self.system_path(&l.entry).map(|p| (p, false))).collect();
        for (path, is_user) in candidates {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            match Value::parse(l.kind, &text) {
                Some(v) => {
                    let source = if is_user { Source::User(path) } else { Source::System(path) };
                    return Reading { value: v, source, raw: Some(text), problem };
                }
                None => {
                    problem.get_or_insert_with(|| format!("{} holds {:?}, not a {:?}; showing the next source (COSMIC may use its own default instead)", path.display(), text.trim(), l.kind));
                }
            }
        }
        Reading { value: l.default, source: Source::Default, raw: None, problem }
    }

    /// Write the user file (atomically: COSMIC watches it and must never
    /// read half a value). Refuses a value of the wrong kind.
    pub fn write(&self, l: &Lever, v: Value) -> Result<PathBuf, String> {
        if Value::parse(l.kind, &v.to_ron()).is_none() {
            return Err(format!("{:?} is not a {:?}", v, l.kind));
        }
        let path = self.user_path(&l.entry);
        let dir = path.parent().ok_or("no parent directory")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!(".{}.cosmic-levers-tmp", l.entry.key));
        std::fs::write(&tmp, v.to_ron()).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// Remove the user file, so the system default (or the program's) applies
    /// again. Returns whether there was one.
    pub fn reset(&self, l: &Lever) -> Result<bool, String> {
        let p = self.user_path(&l.entry);
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let base = std::env::temp_dir().join(format!("cosmic-levers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Store { config_home: base.join("home"), data_dirs: vec![base.join("local-share"), base.join("share")] }
    }
    fn lever(key: &str) -> Lever {
        levers().into_iter().find(|l| l.entry.key == key).unwrap()
    }

    #[test]
    fn values_round_trip_through_the_ron_cosmic_writes() {
        for v in [Value::Bool(true), Value::Bool(false), Value::OptMillis(Some(900000)), Value::OptMillis(None)] {
            let k = if matches!(v, Value::Bool(_)) { Kind::Bool } else { Kind::OptMillis };
            assert_eq!(Value::parse(k, &v.to_ron()), Some(v));
        }
        assert_eq!(Value::parse(Kind::OptMillis, " Some( 60000 )\n"), Some(Value::OptMillis(Some(60000))));
        assert_eq!(Value::parse(Kind::OptMillis, "300000"), Some(Value::OptMillis(Some(300000))), "implicit_some");
        assert_eq!(Value::parse(Kind::Bool, "True"), None, "RON booleans are lowercase");
        assert_eq!(Value::parse(Kind::OptMillis, "Some(-1)"), None);
        assert_eq!(Value::OptMillis(Some(15 * MIN)).describe(), "15 min");
        assert_eq!(Value::OptMillis(Some(3_600_000)).describe(), "1 h");
        assert_eq!(Value::OptMillis(Some(30_000)).describe(), "30 s");
        assert_eq!(Value::OptMillis(Some(90_000)).describe(), "1 min 30 s");
        assert_eq!(Value::OptMillis(None).describe(), "Never");
    }

    #[test]
    fn a_read_takes_user_then_system_then_default_and_says_which() {
        let s = store("read");
        let l = lever("screen_off_time");
        assert_eq!(s.read(&l).source, Source::Default);
        assert_eq!(s.read(&l).value, Value::OptMillis(Some(15 * MIN)));
        // A system default in the SECOND data dir.
        let sys = s.data_dirs[1].join("cosmic/com.system76.CosmicIdle/v1/screen_off_time");
        std::fs::create_dir_all(sys.parent().unwrap()).unwrap();
        std::fs::write(&sys, "Some(120000)").unwrap();
        let r = s.read(&l);
        assert_eq!((r.value, r.source.clone()), (Value::OptMillis(Some(120000)), Source::System(sys.clone())));
        // The user's file wins; its path is the one cosmic-config uses.
        let written = s.write(&l, Value::OptMillis(Some(5 * MIN))).unwrap();
        assert_eq!(written, s.config_home.join("cosmic/com.system76.CosmicIdle/v1/screen_off_time"));
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "Some(300000)");
        let r = s.read(&l);
        assert_eq!((r.value, r.source), (Value::OptMillis(Some(5 * MIN)), Source::User(written.clone())));
        // Never = None.
        s.write(&l, Value::OptMillis(None)).unwrap();
        assert_eq!(s.read(&l).value, Value::OptMillis(None));
        // A broken user file: reported, and the next source shown.
        std::fs::write(&written, "fifteen minutes").unwrap();
        let r = s.read(&l);
        assert_eq!(r.value, Value::OptMillis(Some(120000)));
        assert!(r.problem.as_deref().unwrap().contains("fifteen minutes"), "{:?}", r.problem);
        // Reset removes the user file only.
        assert!(s.reset(&l).unwrap());
        assert!(!s.reset(&l).unwrap());
        assert!(sys.exists());
        assert_eq!(s.read(&l).source, Source::System(sys));
        // No temp files left behind.
        let dir = written.parent().unwrap();
        assert!(std::fs::read_dir(dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("tmp")));
        let _ = std::fs::remove_dir_all(s.config_home.parent().unwrap());
    }

    #[test]
    fn a_wrong_kind_is_refused_and_every_lever_is_consistent() {
        let s = store("kinds");
        assert!(s.write(&lever("is_dark"), Value::OptMillis(Some(1))).is_err());
        assert!(!s.user_path(&lever("is_dark").entry).exists(), "nothing written on refusal");
        for l in levers() {
            assert_eq!(Value::parse(l.kind, &l.default.to_ron()), Some(l.default), "{} default matches its kind", l.label);
            if let Control::Choices(cs) = l.control {
                assert_eq!(l.kind, Kind::OptMillis);
                let Value::OptMillis(d) = l.default else { unreachable!() };
                assert!(cs.iter().any(|(_, v)| *v == d), "{}: the default is one of the choices", l.label);
            }
            assert!(!l.entry.key.contains('/') && l.entry.id.starts_with("com.system76."));
        }
        let _ = std::fs::remove_dir_all(s.config_home.parent().unwrap());
    }
}
