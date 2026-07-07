//! repomap — the cockpit's view of the whole monorepo as buildable crates. Wraps `repo_map` so the
//! cockpit (and its GUI/CLI) can list every crate and build/test any of them with the *correct*
//! command — workspace member, standalone, or cmake — which is what lets gitmind manage this repo,
//! not just the apps it happens to scan.
use repo_map::{scan_repo, CrateEntry};
use std::path::Path;

/// Every crate in the repo (apps + shared + games), build-aware.
pub fn indexed_crates(repo_root: &str) -> Vec<CrateEntry> {
    scan_repo(Path::new(repo_root), &["apps", "shared", "games"])
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
