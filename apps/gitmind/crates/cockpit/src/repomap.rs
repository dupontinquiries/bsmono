//! repomap — the cockpit's view of the whole monorepo as buildable crates. Wraps `repo_map` so the
//! cockpit (and its GUI/CLI) can list every crate and build/test any of them with the *correct*
//! command — workspace member, standalone, or cmake — which is what lets gitmind manage this repo,
//! not just the apps it happens to scan.
use repo_map::{scan_repo, CrateEntry};
use std::path::Path;

/// The top-level directories that hold buildable crates.
///
/// `tools` and `system` were missing, which is why gitmind could not build,
/// test or install any of them — astfind, the parity audit's neighbours, the
/// peek tools and both system daemons were simply invisible to the cockpit
/// while the doc comment above claimed "every crate in the repo".
///
/// Kept as a named constant rather than inline so the claim and the list are in
/// one place; a test asserts they agree with what is actually on disk.
pub const CRATE_ROOTS: [&str; 5] = ["apps", "shared", "games", "tools", "system"];

/// Top-level directories deliberately NOT scanned, and why.
///
/// `vendor` holds copies of crates that also live under `shared` — the fleet's
/// netcode and snapcore, tracked in `maintenance/VENDORED_DRIFT.md`. Scanning it
/// would put the same crate in the build matrix twice under one name, so a
/// "build everything" run would compile a copy that no app links and report its
/// failures as the real crate's. Excluded by decision rather than by oversight,
/// which is the difference the test below checks.
pub const EXCLUDED_ROOTS: [&str; 1] = ["vendor"];

/// Every crate in the repo, build-aware.
pub fn indexed_crates(repo_root: &str) -> Vec<CrateEntry> {
    scan_repo(Path::new(repo_root), &CRATE_ROOTS)
}

/// A build/test matrix: each crate paired with the argv that builds and tests it. This is what a
/// "build everything / test everything" cockpit action iterates.
pub fn build_matrix(repo_root: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
    indexed_crates(repo_root)
        .into_iter()
        .map(|c| (c.name.clone(), c.build_argv(), c.test_argv()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Repo root relative to this crate dir (apps/gitmind/crates/cockpit → repo root).
    fn root() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors().nth(4).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn cockpit_can_index_itself_and_command_it_correctly() {
        let crates = indexed_crates(&root());
        assert!(crates.len() > 100, "indexes the whole tree, got {}", crates.len());
        let me = crates.iter().find(|c| c.name == "cockpit").expect("cockpit in the index");
        // gitmind can test itself with the right workspace-scoped command
        assert_eq!(me.test_argv(), vec!["cargo", "test", "-p", "cockpit", "--manifest-path", "apps/gitmind/Cargo.toml"]);
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;

    fn repo_root() -> std::path::PathBuf {
        // crates/cockpit → apps/gitmind → apps → repo
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    #[test]
    fn every_top_level_directory_holding_crates_is_scanned() {
        // The failure this catches is silent: a new top-level directory appears,
        // gitmind keeps working, and everything inside it is invisible while the
        // doc still says "every crate in the repo". That is how tools/ and
        // system/ went missing.
        let root = repo_root();
        let mut missed = Vec::new();
        for entry in std::fs::read_dir(&root).expect("repo root") {
            let e = entry.expect("entry");
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" { continue; }
            // Does it contain a Cargo.toml anywhere shallow?
            let has_crate = walkdir_shallow(&e.path(), 3);
            if has_crate
                && !CRATE_ROOTS.contains(&name.as_str())
                && !EXCLUDED_ROOTS.contains(&name.as_str())
            {
                missed.push(name);
            }
        }
        assert!(missed.is_empty(),
                "top-level dirs with crates neither scanned nor explicitly excluded: {missed:?}");
    }

    #[test]
    fn vendored_copies_are_excluded_rather_than_forgotten() {
        // Scanning them would put one crate in the build matrix twice, so a
        // "build everything" run compiles a copy no app links and reports its
        // failures as the real crate's.
        assert!(EXCLUDED_ROOTS.contains(&"vendor"));
        for r in EXCLUDED_ROOTS {
            assert!(!CRATE_ROOTS.contains(&r), "{r} is both scanned and excluded");
        }
    }

    fn walkdir_shallow(dir: &std::path::Path, depth: u32) -> bool {
        if depth == 0 { return false; }
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name().map(|n| n == "Cargo.toml").unwrap_or(false) { return true; }
            if p.is_dir() && walkdir_shallow(&p, depth - 1) { return true; }
        }
        false
    }

    #[test]
    fn the_tools_are_now_visible_to_the_cockpit() {
        let found = indexed_crates(repo_root().to_str().unwrap());
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"astfind"), "astfind missing from {} crates", names.len());
    }

    #[test]
    fn scanning_is_deduplicated_and_ordered() {
        // Two roots can reach one crate through a symlink or a nested
        // workspace; a duplicate in the build matrix builds it twice.
        let found = indexed_crates(repo_root().to_str().unwrap());
        let mut keys: Vec<(String, String)> = found.iter()
            .map(|c| (c.name.clone(), c.path.clone())).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate crate entries");
    }
}
