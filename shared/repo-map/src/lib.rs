//! repo-map — the monorepo, as a machine-usable index. `gitmind` (to build/deploy/catalog) and
//! `logscope` (to navigate/build/test under its autonomous loop) both need one accurate answer to
//! "what crates exist and how do I correctly build+test each?" — a question this tree makes
//! non-trivial: some crates are workspace members (`-p name`), some are standalone workspaces built
//! from their own dir (the games, gpu-gemm), and some are C++/CMake (blackstar). This crate is that
//! answer: a pure command model + a filesystem scan.

use std::path::Path;

/// How a crate is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrateKind {
    /// A member of a cargo workspace rooted at `workspace_root` — built with `-p name` against that
    /// workspace's manifest.
    WorkspaceMember { workspace_root: String },
    /// Its own workspace/crate — built from its own directory (games, gpu-gemm-style excludes).
    Standalone,
    /// A C++/CMake project.
    Cmake,
}

/// One crate in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateEntry {
    pub name: String,
    /// Path to the crate dir, relative to the repo root.
    pub path: String,
    pub kind: CrateKind,
}

impl CrateEntry {
    /// argv to build this crate — unambiguous from any working directory (uses `--manifest-path`).
    pub fn build_argv(&self) -> Vec<String> {
        self.cargo_argv("build")
    }
    /// argv to test this crate.
    pub fn test_argv(&self) -> Vec<String> {
        self.cargo_argv("test")
    }

    fn cargo_argv(&self, verb: &str) -> Vec<String> {
        match &self.kind {
            CrateKind::WorkspaceMember { workspace_root } => vec![
                "cargo".into(), verb.into(), "-p".into(), self.name.clone(),
                "--manifest-path".into(), join(workspace_root, "Cargo.toml"),
            ],
            CrateKind::Standalone => vec![
                "cargo".into(), verb.into(),
                "--manifest-path".into(), join(&self.path, "Cargo.toml"),
            ],
            CrateKind::Cmake => match verb {
                "test" => vec!["ctest".into(), "--test-dir".into(), join(&self.path, "build")],
                _ => vec!["cmake".into(), "--build".into(), join(&self.path, "build")],
            },
        }
    }
}

fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{}/{}", a.trim_end_matches('/'), b)
    }
}

/// Does a Cargo.toml declare its own workspace (→ a build root, i.e. standalone here)?
pub fn declares_workspace(cargo_toml: &str) -> bool {
    cargo_toml.lines().any(|l| l.trim_start().starts_with("[workspace]"))
}

/// Extract the `name = "..."` from a `[package]` Cargo.toml (first match).
pub fn package_name(cargo_toml: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t.starts_with("[package]");
            continue;
        }
        if in_pkg {
            if let Some(rest) = t.strip_prefix("name") {
                if let Some(q0) = rest.find('"') {
                    if let Some(q1) = rest[q0 + 1..].find('"') {
                        return Some(rest[q0 + 1..q0 + 1 + q1].to_string());
                    }
                }
            }
        }
    }
    None
}

/// Scan `root` for crates under the given top-level dirs (e.g. `["apps", "shared", "games"]`).
/// A crate dir has a `Cargo.toml` with a `[package]`, or a `CMakeLists.txt`. Kind is resolved by
/// walking up to the nearest ancestor whose `Cargo.toml` declares `[workspace]`: if that ancestor
/// is the crate itself (or none is found) it's Standalone; otherwise a WorkspaceMember of it.
pub fn scan_repo(root: &Path, tops: &[&str]) -> Vec<CrateEntry> {
    let mut out = Vec::new();
    for top in tops {
        walk(&root.join(top), root, 0, &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name && a.path == b.path);
    out
}

fn walk(dir: &Path, root: &Path, depth: u32, out: &mut Vec<CrateEntry>) {
    if depth > 5 {
        return;
    }
    let cargo = dir.join("Cargo.toml");
    let cmake = dir.join("CMakeLists.txt");
    let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().replace('\\', "/");
    if cargo.exists() {
        if let Ok(txt) = std::fs::read_to_string(&cargo) {
            if let Some(name) = package_name(&txt) {
                let kind = resolve_kind(dir, root);
                out.push(CrateEntry { name, path: rel.clone(), kind });
            }
        }
    } else if cmake.exists() {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(CrateEntry { name, path: rel, kind: CrateKind::Cmake });
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            let n = ent.file_name().to_string_lossy().into_owned();
            if p.is_dir() && !n.starts_with('.') && n != "target" && n != "node_modules" && n != "third_party" {
                walk(&p, root, depth + 1, out);
            }
        }
    }
}

/// Nearest ancestor (inclusive) whose Cargo.toml declares `[workspace]`. If it's the crate dir
/// itself or nothing is found → Standalone; else WorkspaceMember of that ancestor.
fn resolve_kind(crate_dir: &Path, root: &Path) -> CrateKind {
    let mut cur = crate_dir.parent();
    while let Some(anc) = cur {
        if anc.starts_with(root) || anc == root {
            let ct = anc.join("Cargo.toml");
            if let Ok(txt) = std::fs::read_to_string(&ct) {
                if declares_workspace(&txt) {
                    let wr = anc.strip_prefix(root).unwrap_or(anc).to_string_lossy().replace('\\', "/");
                    return CrateKind::WorkspaceMember { workspace_root: wr };
                }
            }
        }
        if anc == root {
            break;
        }
        cur = anc.parent();
    }
    CrateKind::Standalone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_name_and_workspace_parse() {
        assert_eq!(package_name("[package]\nname = \"voice-kit\"\nversion=\"0.1\""), Some("voice-kit".into()));
        assert_eq!(package_name("[dependencies]\nname = \"nope\""), None);
        assert!(declares_workspace("[workspace]\nmembers = []"));
        assert!(!declares_workspace("[package]\nname=\"x\""));
    }

    #[test]
    fn member_build_command_targets_the_workspace() {
        let e = CrateEntry {
            name: "cockpit".into(),
            path: "apps/gitmind/crates/cockpit".into(),
            kind: CrateKind::WorkspaceMember { workspace_root: "apps/gitmind".into() },
        };
        assert_eq!(e.test_argv(), vec!["cargo", "test", "-p", "cockpit", "--manifest-path", "apps/gitmind/Cargo.toml"]);
    }

    #[test]
    fn standalone_and_cmake_commands() {
        let s = CrateEntry { name: "bsgames".into(), path: "games/bsgames".into(), kind: CrateKind::Standalone };
        assert_eq!(s.build_argv(), vec!["cargo", "build", "--manifest-path", "games/bsgames/Cargo.toml"]);
        let c = CrateEntry { name: "blackstar".into(), path: "games/blackstar".into(), kind: CrateKind::Cmake };
        assert_eq!(c.build_argv(), vec!["cmake", "--build", "games/blackstar/build"]);
        assert_eq!(c.test_argv(), vec!["ctest", "--test-dir", "games/blackstar/build"]);
    }
}
