//! manifest.rs — `pool.toml`: what a game wants, and what it will spend.
//!
//! Kept deliberately declarative. A pool is a *statement of intent* that lives in the repository
//! next to the game, so the answer to "why does blackstar ship these assets" is a file somebody
//! wrote rather than a command somebody once ran.

use std::path::Path;

use cad_ref::{Entry, Facts, Kind};
use serde::{Deserialize, Serialize};

/// What a pool may spend. `None` means unlimited, which is a legitimate answer for a small game
/// and a dangerous default for a large one — so a manifest that sets no budget says so explicitly
/// in its own file rather than inheriting a number from here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    pub bytes: Option<u64>,
    /// Triangle ceiling for the pool as a whole, advisory: reported, not enforced, because a
    /// budget that silently drops a model the game needs is worse than one that warns.
    pub triangles: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolManifest {
    pub name: String,
    /// Which corpus kinds this pool draws from: "model", "texture", "niftyplot", "doc".
    /// Empty means all of them.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Glob-ish name patterns to leave out. `*` matches any run of characters; everything else is
    /// literal. Deliberately not a regex — a pool manifest is read far more often than written,
    /// and a regex in a config file is a puzzle for whoever reads it next.
    #[serde(default)]
    pub exclude_names: Vec<String>,
    /// Path fragments to leave out, matched anywhere in the path.
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    #[serde(default)]
    pub budget: Budget,
}

/// `*`-glob match, anchored at both ends.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !name.starts_with(part) {
                return false;
            }
            pos = part.len();
            continue;
        }
        match name[pos..].find(part) {
            Some(at) => pos += at + part.len(),
            None => return false,
        }
    }
    // A pattern not ending in `*` must reach the end of the name.
    parts.last().map(|l| l.is_empty() || name.ends_with(l)).unwrap_or(true)
}

impl PoolManifest {
    pub fn new(name: &str) -> PoolManifest {
        PoolManifest { name: name.into(), ..Default::default() }
    }

    pub fn wants_kind(&self, k: Kind) -> bool {
        if self.kinds.is_empty() {
            return true;
        }
        let tag = match k {
            Kind::Model => "model",
            Kind::Texture => "texture",
            Kind::Niftyplot => "niftyplot",
            Kind::Doc => "doc",
        };
        self.kinds.iter().any(|w| w.eq_ignore_ascii_case(tag))
    }

    /// Why this asset is excluded, if it is.
    pub fn excluded(&self, name: &str, path: &Path) -> Option<String> {
        for pat in &self.exclude_names {
            if glob_match(pat, name) {
                return Some(format!("name matches '{pat}'"));
            }
        }
        let p = path.to_string_lossy();
        for frag in &self.exclude_paths {
            if p.contains(frag.as_str()) {
                return Some(format!("path contains '{frag}'"));
            }
        }
        None
    }

    /// A short note recorded against every inclusion, so a plan explains itself.
    pub fn reason_for(&self, e: &Entry) -> String {
        match &e.facts {
            Facts::Model(m) if m.triangles > 0 => format!("model, {} tris", m.triangles),
            Facts::Model(_) => "model".into(),
            Facts::Texture(t) => format!("texture {}x{}", t.width, t.height),
            Facts::Nifty(n) => format!("{} diagram", n.kind),
            Facts::Doc(_) => "document".into(),
            Facts::Unparsed => "indexed by name".into(),
        }
    }

    /// Parse the `pool.toml` subset: `name`, `kinds`, `exclude_names`, `exclude_paths`, and a
    /// `[budget]` table. Same reasoning as `ref.toml` — a handful of keys does not justify a TOML
    /// dependency, and the shape is fixed by this parser rather than by a schema elsewhere.
    pub fn parse(text: &str) -> PoolManifest {
        let mut m = PoolManifest::default();
        let mut in_budget = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_budget = line.starts_with("[budget");
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let list = |v: &str| -> Vec<String> {
                v.trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let scalar = |v: &str| v.trim_matches('"').trim_matches('\'').to_string();
            if in_budget {
                match k {
                    "bytes" => m.budget.bytes = parse_size(&scalar(v)),
                    "triangles" => m.budget.triangles = scalar(v).parse().ok(),
                    _ => {}
                }
                continue;
            }
            match k {
                "name" => m.name = scalar(v),
                "kinds" => m.kinds = list(v),
                "exclude_names" => m.exclude_names = list(v),
                "exclude_paths" => m.exclude_paths = list(v),
                _ => {}
            }
        }
        m
    }
}

/// `15G`, `500M`, `1024` — the v2 scripts' `parse_size`, which is what anyone writing a budget
/// will already have in their fingers.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1024u64),
        'M' => (&s[..s.len() - 1], 1024 * 1024),
        'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'T' => (&s[..s.len() - 1], 1024u64.pow(4)),
        _ => (s, 1),
    };
    num.trim().parse::<f64>().ok().map(|v| (v * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_anchor_at_both_ends() {
        assert!(glob_match("prototype_*", "prototype_scrap"));
        assert!(!glob_match("prototype_*", "my_prototype_scrap"));
        assert!(glob_match("*_wip", "gear_wip"));
        assert!(glob_match("*scrap*", "prototype_scrap_v2"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
    }

    #[test]
    fn sizes_parse_the_way_the_v2_scripts_wrote_them() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("15G"), Some(15 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("1.5M"), Some(1024 * 1024 * 3 / 2));
        assert_eq!(parse_size("nonsense"), None);
    }

    #[test]
    fn a_manifest_parses_its_documented_shape() {
        let m = PoolManifest::parse(
            r#"
name = "blackstar"
kinds = ["model", "texture"]
exclude_names = ["prototype_*", "*_wip"]
exclude_paths = ["/scratch/"]

[budget]
bytes = "500M"
triangles = 2000000
"#,
        );
        assert_eq!(m.name, "blackstar");
        assert_eq!(m.kinds, vec!["model".to_string(), "texture".to_string()]);
        assert_eq!(m.exclude_names.len(), 2);
        assert_eq!(m.budget.bytes, Some(500 * 1024 * 1024));
        assert_eq!(m.budget.triangles, Some(2_000_000));
        assert!(m.wants_kind(Kind::Model));
        assert!(!m.wants_kind(Kind::Doc));
    }

    #[test]
    fn an_empty_manifest_wants_everything_and_forbids_nothing() {
        let m = PoolManifest::parse("");
        for k in [Kind::Model, Kind::Texture, Kind::Doc, Kind::Niftyplot] {
            assert!(m.wants_kind(k));
        }
        assert!(m.excluded("anything", Path::new("a/b.obj")).is_none());
        assert_eq!(m.budget.bytes, None, "no budget must be explicit, never inherited");
    }

    #[test]
    fn exclusion_reasons_name_the_rule_that_fired() {
        let mut m = PoolManifest::new("x");
        m.exclude_names.push("junk_*".into());
        m.exclude_paths.push("/tmp/".into());
        let by_name = m.excluded("junk_thing", Path::new("a/b.obj")).unwrap();
        assert!(by_name.contains("junk_*"), "{by_name}");
        let by_path = m.excluded("fine", Path::new("/tmp/x.obj")).unwrap();
        assert!(by_path.contains("/tmp/"), "{by_path}");
    }
}
