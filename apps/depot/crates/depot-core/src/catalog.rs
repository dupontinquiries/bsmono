//! The catalog: what the store holds, by kind, and how each projection should
//! present it. One JSON file, loaded on start, rewritten atomically after each
//! mutation.
//!
//! Why one JSON file and not sqlite: the catalog for 50k RPMs is ~20 MB, loads
//! in well under a second, and stays greppable and diffable — which matters on
//! a box with no internet where "what changed since the last pack" is a
//! question a human asks with `diff`. Per-package metadata that is large
//! (repodata XML fragments) is NOT inlined; it lives in the object store and
//! the catalog holds its hash, so the catalog stays proportional to the number
//! of artifacts, not their file lists.

use crate::{write_atomic, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const CATALOG_FILE: &str = "catalog.json";
pub const CATALOG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Catalog {
    pub version: u32,
    pub updated: u64,
    /// crate name → versions (any order; views sort by semver-ish string).
    #[serde(default)]
    pub crates: BTreeMap<String, Vec<CrateEntry>>,
    /// repo id (e.g. "fedora-42-x86_64") → packages.
    #[serde(default)]
    pub rpm: BTreeMap<String, RpmRepo>,
    #[serde(default)]
    pub nix: Vec<NixEntry>,
    #[serde(default)]
    pub toolchains: Vec<ToolchainEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrateEntry {
    pub vers: String,
    /// sha256 of the .crate file = object hash = index `cksum`.
    pub hash: String,
    pub size: u64,
    /// The complete sparse-index line for this version (JSON, one object).
    pub index_line: serde_json::Value,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub added: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RpmRepo {
    #[serde(default)]
    pub packages: Vec<RpmEntry>,
    /// Object hash of the repo's GPG public key (served as RPM-GPG-KEY), if
    /// one was ingested. dnf verifies signatures with it; depot does not.
    #[serde(default)]
    pub gpg_key: Option<String>,
    /// Free text shown in the UI ("Fedora 42 updates, x86_64").
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpmEntry {
    pub name: String,
    pub epoch: u64,
    pub version: String,
    pub release: String,
    pub arch: String,
    /// The .rpm's filename inside Packages/ (createrepo_c-style href).
    pub filename: String,
    pub hash: String,
    pub size: u64,
    /// Object hashes of the three repodata fragments derived from the header.
    pub primary_xml: String,
    pub filelists_xml: String,
    pub other_xml: String,
    #[serde(default)]
    pub added: u64,
}

impl RpmEntry {
    pub fn nevra(&self) -> String {
        if self.epoch > 0 {
            format!("{}-{}:{}-{}.{}", self.name, self.epoch, self.version, self.release, self.arch)
        } else {
            format!("{}-{}-{}.{}", self.name, self.version, self.release, self.arch)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NixEntry {
    /// e.g. "/nix/store/abc…-hello-2.12"
    pub store_path: String,
    /// The 32-char base32 hash part of the store path; `<hash>.narinfo`.
    pub hash_part: String,
    pub narinfo: String,
    /// Object hash of the compressed NAR and its URL relative to the cache root.
    pub nar_hash: String,
    pub nar_url: String,
    pub nar_size: u64,
    #[serde(default)]
    pub added: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainEntry {
    pub version: String,
    pub triple: String,
    /// "rustc" | "cargo" | "rust-std" | "clippy" | …
    pub component: String,
    pub filename: String,
    pub hash: String,
    pub size: u64,
    #[serde(default)]
    pub added: u64,
}

impl Catalog {
    pub fn load(root: &Path) -> Result<Catalog> {
        let p = root.join(CATALOG_FILE);
        if !p.exists() {
            return Ok(Catalog { version: CATALOG_VERSION, ..Default::default() });
        }
        let s = std::fs::read(&p)?;
        let c: Catalog = serde_json::from_slice(&s)?;
        Ok(c)
    }

    pub fn save(&mut self, root: &Path) -> Result<()> {
        self.version = CATALOG_VERSION;
        self.updated = crate::now();
        let s = serde_json::to_vec_pretty(self)?;
        write_atomic(&root.join(CATALOG_FILE), &s)?;
        Ok(())
    }

    /// Insert or replace a crate version. Returns true if it was new.
    pub fn upsert_crate(&mut self, name: &str, e: CrateEntry) -> bool {
        let v = self.crates.entry(name.to_string()).or_default();
        if let Some(existing) = v.iter_mut().find(|x| x.vers == e.vers) {
            let was_new = existing.hash != e.hash;
            *existing = e;
            was_new
        } else {
            v.push(e);
            true
        }
    }

    pub fn upsert_rpm(&mut self, repo: &str, e: RpmEntry) -> bool {
        let r = self.rpm.entry(repo.to_string()).or_default();
        if let Some(existing) = r.packages.iter_mut().find(|x| x.hash == e.hash) {
            *existing = e;
            false
        } else {
            r.packages.push(e);
            true
        }
    }

    pub fn upsert_nix(&mut self, e: NixEntry) -> bool {
        if let Some(x) = self.nix.iter_mut().find(|x| x.hash_part == e.hash_part) {
            let n = x.nar_hash != e.nar_hash;
            *x = e;
            n
        } else {
            self.nix.push(e);
            true
        }
    }

    pub fn upsert_toolchain(&mut self, e: ToolchainEntry) -> bool {
        if let Some(x) = self.toolchains.iter_mut().find(|x| x.version == e.version && x.triple == e.triple && x.component == e.component) {
            let n = x.hash != e.hash;
            *x = e;
            n
        } else {
            self.toolchains.push(e);
            true
        }
    }

    /// Every object hash the catalog references (artifacts + derived
    /// fragments). Used by `pack` and by `gc`.
    pub fn referenced_hashes(&self) -> Vec<String> {
        let mut v = Vec::new();
        for versions in self.crates.values() {
            for c in versions {
                v.push(c.hash.clone());
            }
        }
        for repo in self.rpm.values() {
            if let Some(k) = &repo.gpg_key {
                v.push(k.clone());
            }
            for p in &repo.packages {
                v.push(p.hash.clone());
                v.push(p.primary_xml.clone());
                v.push(p.filelists_xml.clone());
                v.push(p.other_xml.clone());
            }
        }
        for n in &self.nix {
            v.push(n.narinfo.clone());
            v.push(n.nar_hash.clone());
        }
        for t in &self.toolchains {
            v.push(t.hash.clone());
        }
        v.sort();
        v.dedup();
        v
    }

    pub fn counts(&self) -> Counts {
        Counts {
            crates: self.crates.len(),
            crate_versions: self.crates.values().map(Vec::len).sum(),
            rpm_repos: self.rpm.len(),
            rpm_packages: self.rpm.values().map(|r| r.packages.len()).sum(),
            nix_paths: self.nix.len(),
            toolchain_files: self.toolchains.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Counts {
    pub crates: usize,
    pub crate_versions: usize,
    pub rpm_repos: usize,
    pub rpm_packages: usize,
    pub nix_paths: usize,
    pub toolchain_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmpdir;

    #[test]
    fn roundtrip_and_upsert_semantics() {
        let d = tmpdir("catalog");
        let mut c = Catalog::load(&d).unwrap();
        assert_eq!(c.version, CATALOG_VERSION);
        let e = CrateEntry { vers: "1.0.0".into(), hash: "a".repeat(64), size: 1, index_line: serde_json::json!({"name":"x"}), yanked: false, added: 0 };
        assert!(c.upsert_crate("x", e.clone()));
        assert!(!c.upsert_crate("x", e.clone()), "same version, same hash: not new");
        let mut e2 = e.clone();
        e2.hash = "b".repeat(64);
        assert!(c.upsert_crate("x", e2), "same version, different bytes: counts as new (re-published)");
        assert_eq!(c.crates["x"].len(), 1);
        c.save(&d).unwrap();
        let c2 = Catalog::load(&d).unwrap();
        assert_eq!(c2.crates["x"][0].hash, "b".repeat(64));
        assert_eq!(c2.counts(), Counts { crates: 1, crate_versions: 1, ..Default::default() });
        assert_eq!(c2.referenced_hashes(), vec!["b".repeat(64)]);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn nevra_formats_epoch_only_when_nonzero() {
        let mut e = RpmEntry { name: "hello".into(), epoch: 0, version: "2.0".into(), release: "1".into(), arch: "x86_64".into(), filename: String::new(), hash: String::new(), size: 0, primary_xml: String::new(), filelists_xml: String::new(), other_xml: String::new(), added: 0 };
        assert_eq!(e.nevra(), "hello-2.0-1.x86_64");
        e.epoch = 3;
        assert_eq!(e.nevra(), "hello-3:2.0-1.x86_64");
    }
}
