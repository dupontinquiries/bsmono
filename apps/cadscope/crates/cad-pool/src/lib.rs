//! cad-pool — asset pools (plan §10).
//!
//! `cad-ref` points inward: it grounds a design decision in what already exists. This crate points
//! outward: it decides what a game actually ships. Same scanning layer, opposite direction.
//!
//! The governing discipline is inherited verbatim from `library_shrink_v8.py`, whose docstring
//! states it in capitals: **DRY RUN BY DEFAULT. Nothing is deleted or rewritten without --apply.**
//! Every operation here produces a `Plan` — a list of intended actions with their sizes and
//! reasons — and a plan is an ordinary value you can print, diff, or throw away. `apply` is a
//! separate call that takes a plan and does what it says, and it is the only function in the crate
//! that touches the filesystem destructively.
//!
//! The second inheritance is subtler and worth stating: the v2 scripts shrink a library *while
//! keeping every model re-processable*. Reclaiming space by discarding the source is easy and
//! wrong, because the next tier the game ships cannot be re-exported. So the actions here are
//! selection and deduplication, never destruction of an original.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cad_ref::{Facts, Index, Kind};
use serde::{Deserialize, Serialize};

pub mod manifest;
pub use manifest::{Budget, PoolManifest};

/// What a plan proposes doing to one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Copy into the pool: this asset is wanted and not already there.
    Include { bytes: u64, reason: String },
    /// Already present and identical; nothing to do. Listed rather than omitted, because "why is
    /// this not in the diff" is a question a plan should answer.
    AlreadyPresent { bytes: u64 },
    /// A byte-identical twin of another included asset. The pool keeps one and points at it.
    Duplicate { bytes: u64, of: String },
    /// Wanted, but the budget ran out. Listed with what it would have cost, so raising the budget
    /// is an informed decision rather than a guess.
    OverBudget { bytes: u64 },
    /// Excluded by the manifest's rules.
    Excluded { reason: String },
}

impl Action {
    pub fn bytes(&self) -> u64 {
        match self {
            Action::Include { bytes, .. }
            | Action::AlreadyPresent { bytes }
            | Action::Duplicate { bytes, .. }
            | Action::OverBudget { bytes } => *bytes,
            Action::Excluded { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedItem {
    pub name: String,
    pub path: PathBuf,
    pub action: Action,
}

/// A complete proposal. Nothing has happened yet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub pool: String,
    pub items: Vec<PlannedItem>,
}

impl Plan {
    pub fn included(&self) -> impl Iterator<Item = &PlannedItem> {
        self.items.iter().filter(|i| matches!(i.action, Action::Include { .. }))
    }
    /// Bytes the pool will occupy if applied: everything included or already there. Duplicates
    /// cost nothing because they are not copied twice.
    pub fn resident_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.action, Action::Include { .. } | Action::AlreadyPresent { .. }))
            .map(|i| i.action.bytes())
            .sum()
    }
    /// Bytes not copied because an identical file already is — the dedupe saving, which is the
    /// number that justifies the scan.
    pub fn reclaimed_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.action, Action::Duplicate { .. }))
            .map(|i| i.action.bytes())
            .sum()
    }
    pub fn count(&self, f: impl Fn(&Action) -> bool) -> usize {
        self.items.iter().filter(|i| f(&i.action)).count()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Identity for dedupe. Size plus the first and last kilobyte: enough that two distinct assets
/// colliding requires them to share a length *and* both ends, and cheap enough to run over
/// thousands of files. A full hash is the upgrade if that ever proves insufficient — recorded in
/// PARITY rather than assumed away.
fn fingerprint(path: &Path, size: u64) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 1024];
    let n = f.read(&mut head).ok()?;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    eat(&size.to_le_bytes());
    eat(&head[..n]);
    if size > 2048 {
        let mut tail = [0u8; 1024];
        if f.seek(SeekFrom::End(-1024)).is_ok() {
            if let Ok(n) = f.read(&mut tail) {
                eat(&tail[..n]);
            }
        }
    }
    Some(h)
}

/// Plan a pool from an index. `present` names assets the pool already holds.
pub fn plan(idx: &Index, m: &PoolManifest, present: &[String]) -> Plan {
    let mut items = Vec::new();
    let mut seen: HashMap<u64, String> = HashMap::new();
    let mut used: u64 = 0;
    let budget = m.budget.bytes.unwrap_or(u64::MAX);

    // Deterministic order: a plan must be diffable between runs, so selection cannot depend on
    // directory iteration order.
    let mut candidates: Vec<&cad_ref::Entry> = idx
        .entries
        .iter()
        .filter(|e| m.wants_kind(e.kind))
        .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));

    for e in candidates {
        let name = e.name.clone();
        let path = e.path.clone();
        if let Some(reason) = m.excluded(&name, &path) {
            items.push(PlannedItem { name, path, action: Action::Excluded { reason } });
            continue;
        }
        let bytes = e.bytes;
        if present.iter().any(|p| p == &name) {
            used = used.saturating_add(bytes);
            items.push(PlannedItem { name, path, action: Action::AlreadyPresent { bytes } });
            continue;
        }
        if let Some(fp) = fingerprint(&path, bytes) {
            if let Some(first) = seen.get(&fp) {
                let of = first.clone();
                items.push(PlannedItem { name, path, action: Action::Duplicate { bytes, of } });
                continue;
            }
            seen.insert(fp, name.clone());
        }
        if used.saturating_add(bytes) > budget {
            items.push(PlannedItem { name, path, action: Action::OverBudget { bytes } });
            continue;
        }
        used = used.saturating_add(bytes);
        let reason = m.reason_for(e);
        items.push(PlannedItem { name, path, action: Action::Include { bytes, reason } });
    }

    Plan { pool: m.name.clone(), items }
}

/// What `apply` did, or would have done.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub copied: usize,
    pub bytes: u64,
    pub failed: Vec<(PathBuf, String)>,
}

/// Execute a plan: copy every `Include` into `dest`. This is the only destructive call in the
/// crate, it is separate from planning on purpose, and it still never deletes an original —
/// reclaiming space by discarding a source would break the v2 pipeline's re-processability
/// guarantee, which is the whole reason those scripts shrink rather than prune.
pub fn apply(p: &Plan, dest: &Path) -> std::io::Result<Applied> {
    std::fs::create_dir_all(dest)?;
    let mut out = Applied::default();
    for item in p.included() {
        let Some(file) = item.path.file_name() else { continue };
        match std::fs::copy(&item.path, dest.join(file)) {
            Ok(n) => {
                out.copied += 1;
                out.bytes += n;
            }
            Err(e) => out.failed.push((item.path.clone(), e.to_string())),
        }
    }
    Ok(out)
}

/// Human-readable byte count, matching the v2 scripts' `human()`.
pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n}B")
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

/// Facts a pool report shows about what it selected — the outbound counterpart to `cad-ref`'s
/// inbound summary.
pub fn summarise(idx: &Index, p: &Plan) -> String {
    let by_name: HashMap<&str, &cad_ref::Entry> =
        idx.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut tris = 0u64;
    let mut textures = 0usize;
    for item in p.included() {
        match by_name.get(item.name.as_str()).map(|e| (&e.facts, e.kind)) {
            Some((Facts::Model(m), _)) => tris += m.triangles as u64,
            Some((_, Kind::Texture)) => textures += 1,
            _ => {}
        }
    }
    format!(
        "{} included ({}), {} duplicate ({} reclaimed), {} over budget, {} excluded; {tris} tris, {textures} textures",
        p.count(|a| matches!(a, Action::Include { .. })),
        human(p.resident_bytes()),
        p.count(|a| matches!(a, Action::Duplicate { .. })),
        human(p.reclaimed_bytes()),
        p.count(|a| matches!(a, Action::OverBudget { .. })),
        p.count(|a| matches!(a, Action::Excluded { .. })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_ref::{Entry, ModelFacts, TextureFacts, TextureRole};

    fn model(name: &str, bytes: u64, tris: u32) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::Model,
            path: PathBuf::from(format!("assets/{name}.obj")),
            bytes,
            facts: Facts::Model(ModelFacts { triangles: tris, ..Default::default() }),
        }
    }
    fn texture(name: &str, bytes: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::Texture,
            path: PathBuf::from(format!("assets/{name}.png")),
            bytes,
            facts: Facts::Texture(TextureFacts { width: 256, height: 256, role: TextureRole::Base }),
        }
    }

    fn idx() -> Index {
        Index::from_entries(vec![
            model("bracket", 1000, 500),
            model("gear", 2000, 900),
            model("prototype_scrap", 4000, 100),
            texture("plating", 500),
        ])
    }

    #[test]
    fn a_plan_changes_nothing_by_itself() {
        let m = PoolManifest::new("blackstar");
        let p = plan(&idx(), &m, &[]);
        assert_eq!(p.count(|a| matches!(a, Action::Include { .. })), 4);
        // The plan is a value. Nothing on disk was touched to produce it, and the paths it names
        // still exist only as strings.
        assert!(!PathBuf::from("assets/bracket.obj").exists());
    }

    #[test]
    fn a_budget_stops_at_the_line_and_says_what_it_cost() {
        let mut m = PoolManifest::new("tight");
        m.budget.bytes = Some(3000);
        let p = plan(&idx(), &m, &[]);
        assert!(p.resident_bytes() <= 3000, "{}", p.resident_bytes());
        let over = p.count(|a| matches!(a, Action::OverBudget { .. }));
        assert!(over > 0, "something must be reported as over budget");
        // Every over-budget item still reports its size, so raising the budget is informed.
        for i in &p.items {
            if let Action::OverBudget { bytes } = i.action {
                assert!(bytes > 0);
            }
        }
    }

    #[test]
    fn exclusions_are_reported_not_silently_dropped() {
        let mut m = PoolManifest::new("clean");
        m.exclude_names.push("prototype_*".into());
        let p = plan(&idx(), &m, &[]);
        let excluded: Vec<&PlannedItem> = p
            .items
            .iter()
            .filter(|i| matches!(i.action, Action::Excluded { .. }))
            .collect();
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].name, "prototype_scrap");
        // It still appears in the plan, so "why is this missing" has an answer.
        assert!(p.items.iter().any(|i| i.name == "prototype_scrap"));
    }

    #[test]
    fn kinds_can_be_restricted() {
        let mut m = PoolManifest::new("models-only");
        m.kinds = vec!["model".into()];
        let p = plan(&idx(), &m, &[]);
        assert!(!p.items.iter().any(|i| i.name == "plating"), "textures should not be considered");
        assert_eq!(p.items.len(), 3);
    }

    #[test]
    fn assets_already_present_are_not_recopied() {
        let m = PoolManifest::new("warm");
        let p = plan(&idx(), &m, &["bracket".into()]);
        let bracket = p.items.iter().find(|i| i.name == "bracket").unwrap();
        assert!(matches!(bracket.action, Action::AlreadyPresent { .. }));
        // But it still counts against the budget, because it still occupies the pool.
        assert!(p.resident_bytes() >= 1000);
    }

    #[test]
    fn planning_is_deterministic() {
        let m = PoolManifest::new("repeat");
        assert_eq!(plan(&idx(), &m, &[]), plan(&idx(), &m, &[]));
    }

    #[test]
    fn a_plan_round_trips_as_json() {
        let m = PoolManifest::new("json");
        let p = plan(&idx(), &m, &[]);
        let back: Plan = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn human_matches_the_v2_scripts() {
        assert_eq!(human(512), "512B");
        assert_eq!(human(1024), "1.0K");
        assert_eq!(human(1024 * 1024 * 3 / 2), "1.5M");
    }
}
