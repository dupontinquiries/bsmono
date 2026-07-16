//! Suites — named bundles of apps installed together. Each app carries sane build/run
//! defaults (target, features, launch flags); installing a suite builds every app for the
//! device it's run on and drops a .desktop launcher for each, so they appear in the Plasma
//! app grid on both mobile and desktop. Stored as JSON under ~/.config/gitmind/suites/.

use std::path::PathBuf;
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct SuiteApp {
    pub name: String,
    pub path: String,
    pub features: Vec<String>,
    pub no_default: bool,
    pub target: String,    // "" = build native for whatever device installs it
    pub run_flags: String,
    /// Override the binary actually built/resolved, when it doesn't match `name` — e.g.
    /// unendlich's real game binary is `unendlich-fullgame` (a workspace-root app with several
    /// benchmark/probe bins alongside it; `--bin unendlich` matches none of them). `None` (the
    /// common case) uses `name` itself, same as before this field existed.
    pub bin_name: Option<String>,
}

impl SuiteApp {
    /// A sane-default entry: native build, default features, no flags.
    pub fn simple(name: &str, path: &str) -> SuiteApp {
        SuiteApp { name: name.into(), path: path.into(), features: vec![], no_default: false, target: String::new(), run_flags: String::new(), bin_name: None }
    }
    /// The actual binary/build identifier — `bin_name` when set, else `name`. What
    /// `install_suite_app_logged` hands to `ops::build_locally_logged` as the app id.
    pub fn build_app_id(&self) -> &str {
        self.bin_name.as_deref().unwrap_or(&self.name)
    }
    pub fn to_value(&self) -> Value {
        json!({
            "name": self.name, "path": self.path, "features": self.features,
            "no_default": self.no_default, "target": self.target, "run_flags": self.run_flags,
            "bin_name": self.bin_name,
        })
    }
    fn from_value(v: &Value) -> Option<SuiteApp> {
        Some(SuiteApp {
            name: v.get("name")?.as_str()?.to_string(),
            path: v.get("path")?.as_str()?.to_string(),
            features: v.get("features").and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            no_default: v.get("no_default").and_then(|b| b.as_bool()).unwrap_or(false),
            target: v.get("target").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            run_flags: v.get("run_flags").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            bin_name: v.get("bin_name").and_then(|s| s.as_str()).map(String::from),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suite {
    pub name: String,
    pub description: String,
    pub apps: Vec<SuiteApp>,
}

impl Suite {
    pub fn to_value(&self) -> Value {
        json!({
            "name": self.name, "description": self.description,
            "apps": self.apps.iter().map(|a| a.to_value()).collect::<Vec<_>>(),
        })
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_value()).unwrap_or_default()
    }
    pub fn parse(text: &str) -> Option<Suite> {
        let v: Value = serde_json::from_str(text).ok()?;
        Some(Suite {
            name: v.get("name")?.as_str()?.to_string(),
            description: v.get("description").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            apps: v.get("apps").and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(SuiteApp::from_value).collect()).unwrap_or_default(),
        })
    }
}

fn suites_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(format!("{h}/.config/gitmind/suites")))
}

pub fn suite_path(name: &str) -> Option<PathBuf> {
    let safe = crate::places::sanitize_name(name);
    suites_dir().map(|d| d.join(format!("{safe}.json")))
}

/// All saved suites, sorted by name.
pub fn list_suites() -> Vec<Suite> {
    let Some(dir) = suites_dir() else { return vec![] };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    if let Some(s) = Suite::parse(&t) { out.push(s); }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn load_suite(name: &str) -> Option<Suite> {
    let p = suite_path(name)?;
    Suite::parse(&std::fs::read_to_string(p).ok()?)
}

pub fn save_suite(s: &Suite) -> Result<(), String> {
    let p = suite_path(&s.name).ok_or("no HOME set")?;
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    std::fs::write(&p, s.to_json()).map_err(|e| e.to_string())
}

pub fn delete_suite(name: &str) -> Result<(), String> {
    let p = suite_path(name).ok_or("no HOME set")?;
    std::fs::remove_file(p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_app_id_falls_back_to_name_when_bin_name_is_unset() {
        let a = SuiteApp::simple("voiceterm", "apps/voiceterm");
        assert_eq!(a.build_app_id(), "voiceterm");
    }

    #[test]
    fn build_app_id_uses_the_override_when_set() {
        // the exact bug report: unendlich's real game binary is `unendlich-fullgame`, not
        // `unendlich` — `--bin unendlich` matches none of the workspace's actual bin targets.
        let mut a = SuiteApp::simple("unendlich", "unendlich");
        a.bin_name = Some("unendlich-fullgame".into());
        assert_eq!(a.build_app_id(), "unendlich-fullgame");
    }

    #[test]
    fn round_trips_through_json() {
        let s = Suite {
            name: "all".into(),
            description: "everything".into(),
            apps: vec![
                SuiteApp::simple("gitmind-gui", "/home/u/Code/gitmind/crates/gui"),
                SuiteApp { name: "logscope".into(), path: "/home/u/Code/logscope".into(), features: vec!["gpu".into()], no_default: true, target: "aarch64-unknown-linux-musl".into(), run_flags: "--fullscreen".into(), bin_name: None },
            ],
        };
        let parsed = Suite::parse(&s.to_json()).expect("parse");
        assert_eq!(parsed, s, "suite survives a JSON round-trip");
        assert_eq!(parsed.apps[0].features.len(), 0);
        assert!(parsed.apps[1].no_default && parsed.apps[1].run_flags == "--fullscreen");
    }
}
