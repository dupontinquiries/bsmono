//! repo — logscope's build-aware view of the monorepo (shared with gitmind via `repo_map`). Lets
//! logscope's sandbox + autonomous loop resolve *any* crate to the correct build/test argv, so it
//! can work on this repository — pick a crate, build it, run its tests, read the verdict — instead
//! of only the workspace it happens to sit in.
use repo_map::{scan_repo, CrateEntry};
use std::path::Path;

/// Every crate in the repo, build-aware.
pub fn crates(repo_root: &str) -> Vec<CrateEntry> {
    scan_repo(Path::new(repo_root), &["apps", "shared", "games"])
}

/// Resolve a crate by name to the argv that tests it (what the sandbox runs).
pub fn test_argv_for(repo_root: &str, name: &str) -> Option<Vec<String>> {
    crates(repo_root).into_iter().find(|c| c.name == name).map(|c| c.test_argv())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors().nth(4).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn logscope_resolves_any_crate_to_a_test_command() {
        // its own crate (a workspace member) and a standalone game
        let mine = test_argv_for(&root(), "logscope-egui").expect("finds logscope-egui");
        assert!(mine.contains(&"-p".to_string()) && mine.contains(&"logscope-egui".to_string()));
        let game = test_argv_for(&root(), "bsgames").expect("finds bsgames");
        assert_eq!(game, vec!["cargo", "test", "--manifest-path", "games/bsgames/Cargo.toml"]);
    }
}
