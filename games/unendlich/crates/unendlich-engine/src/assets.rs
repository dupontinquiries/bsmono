//! Where `assets/` is at RUN time, not at build time.
//!
//! # The failure this exists to end
//!
//! A single run on an M2 Ultra reported three assets missing that are all
//! committed to the repository and present in any clone:
//!
//! ```text
//! Biome registry unavailable: path does not exist: assets/biomes
//! GPU unavailable: Shader file missing: assets/shaders/chunk_generation.comp.spv
//! Failed to create indirect draw set: Shader missing:
//!     /home/.../games/unendlich/crates/unendlich-engine/../../assets/shaders/chunk_cull.comp.spv
//! ```
//!
//! Two different resolution strategies, both wrong in the same run, and you can
//! tell which is which from the message:
//!
//! * `assets/biomes` is a **relative** path, tried against the CWD. It works
//!   when you `cd games/unendlich` first and fails when a launcher does not —
//!   and gitmind, which runs a pinned binary from its own store, does not.
//! * The `.spv` path is **absolute and compiled in**, via
//!   `concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/...")`. That is not a
//!   path the program looked up; it is a string baked into the binary at build
//!   time. It resolves only on the machine that built it, with the source tree
//!   still where it was, which makes any binary that is copied, pinned or
//!   shipped fail on a file that is right there in the repo.
//!
//! Thirteen files in this crate build paths the second way. The point of a
//! resolver is that they stop each having an opinion.
//!
//! # The order, and why it is this order
//!
//! 1. `UNENDLICH_ASSETS`, so a packager or a bisect can say it outright and
//!    nothing has to guess.
//! 2. The current directory and its ancestors — what a developer running
//!    `cargo run` from anywhere inside the checkout expects.
//! 3. The EXECUTABLE's directory and its ancestors — what a pinned, copied or
//!    installed binary needs, and the case that was missing. A deployed game
//!    sits beside its assets far more often than it sits under them.
//! 4. The compile-time manifest directory, last. It is right on the machine
//!    that built the binary and meaningless anywhere else, so it is a fallback
//!    rather than the answer.
//!
//! Every step looks for a directory that actually contains `assets/`, not for
//! any directory: a partial match is how a resolver settles on a plausible
//! wrong root and reports a confusing miss two subdirectories later.

use std::path::{Path, PathBuf};

/// Directory names that must exist under a candidate for it to be the root.
///
/// `shaders` specifically, rather than just `assets`: an `assets/` that a
/// packager created empty would otherwise win over the real one further up.
const MARKERS: [&str; 2] = ["assets", "assets/shaders"];

/// Is `dir` the root of a tree that has our assets in it?
///
/// Pure over the filesystem check it is handed, so the ordering rules below are
/// testable without a real tree.
pub fn is_asset_root(dir: &Path, exists: &dyn Fn(&Path) -> bool) -> bool {
    MARKERS.iter().all(|m| exists(&dir.join(m)))
}

/// The ordered candidates, before any of them is tested.
///
/// Separated from the filesystem so the ORDER — the part that decides which of
/// several valid roots wins — can be asserted directly.
pub fn candidate_roots(
    env_override: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(p) = env_override {
        out.push(p.to_path_buf());
    }
    for base in [cwd, exe_dir].into_iter().flatten() {
        out.push(base.to_path_buf());
        // Ancestors, so a binary in `target/release` finds the checkout two
        // levels up and a developer in `crates/unendlich-engine/src` finds it
        // three. Bounded rather than walking to `/`: past a handful of levels
        // a match is coincidence, and matching some unrelated `assets/` on the
        // way to the filesystem root is worse than reporting a miss.
        for anc in base.ancestors().skip(1).take(6) {
            out.push(anc.to_path_buf());
        }
    }
    out.push(manifest_dir.to_path_buf());
    for anc in manifest_dir.ancestors().skip(1).take(3) {
        out.push(anc.to_path_buf());
    }
    // Dedup while keeping the first occurrence: the cwd and the exe dir are
    // frequently the same, and a duplicate turns one failed probe into two.
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// The asset root for this process, found once.
pub fn root() -> Option<PathBuf> {
    use std::sync::OnceLock;
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let env = std::env::var_os("UNENDLICH_ASSETS").map(PathBuf::from);
        let cwd = std::env::current_dir().ok();
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let exists = |p: &Path| p.is_dir();
        candidate_roots(env.as_deref(), cwd.as_deref(), exe_dir.as_deref(), manifest)
            .into_iter()
            .find(|c| is_asset_root(c, &exists))
    })
    .clone()
}

/// An asset path, e.g. `assets/shaders/chunk_cull.comp.spv`.
///
/// `None` when no root was found OR the file is not under the root that was —
/// the caller gets one answer to check rather than a path that may or may not
/// be there, which is how a missing file became a panic in the first place.
pub fn find(rel: &str) -> Option<PathBuf> {
    let p = root()?.join(rel);
    p.is_file().then_some(p)
}

/// The same, for a shader by bare name (`chunk_cull.comp.spv`).
pub fn shader(name: &str) -> Option<PathBuf> {
    find(&format!("assets/shaders/{name}"))
}

/// What to tell the user when an asset is missing.
///
/// The root that WAS found, or the fact that none was, plus the override to
/// set. "File not found" on its own sends people looking for the file, which
/// in every instance of this bug so far has been present the whole time.
pub fn miss_report(rel: &str) -> String {
    match root() {
        Some(r) => format!(
            "{rel} not found under asset root {} — the root resolved but the file is not in it",
            r.display()
        ),
        None => format!(
            "{rel} not found: no asset root at or above the working directory ({}) \
             or the executable ({}). Set UNENDLICH_ASSETS to the directory that \
             contains `assets/`.",
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".into()),
            std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".into()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn the_override_wins_over_everything() {
        // A packager saying where the assets are must not be second-guessed by
        // a cwd that happens to look like a checkout.
        let c = candidate_roots(
            Some(&p("/opt/unendlich")),
            Some(&p("/home/u/checkout")),
            Some(&p("/usr/bin")),
            &p("/build/games/unendlich"),
        );
        assert_eq!(c[0], p("/opt/unendlich"));
    }

    #[test]
    fn the_executables_directory_is_searched_and_the_manifest_is_last() {
        // The case the old code could not express: a binary copied away from
        // its build tree, sitting beside its assets. And the compile-time
        // manifest path last, because it is right only on the build machine.
        let c = candidate_roots(None, Some(&p("/tmp")), Some(&p("/opt/game/bin")), &p("/build/games/unendlich"));
        let pos = |s: &str| c.iter().position(|x| x == &p(s));
        assert!(pos("/opt/game/bin").is_some(), "the exe dir is not searched at all");
        assert!(pos("/opt/game").is_some(), "the exe dir's parent is not searched");
        assert!(
            pos("/opt/game").unwrap() < pos("/build/games/unendlich").unwrap(),
            "the compile-time manifest path outranks the executable's own tree"
        );
        assert_eq!(c.last().map(|x| x.as_path()), Some(Path::new("/")),
                   "the manifest ancestors should trail the list");
    }

    #[test]
    fn the_cwd_outranks_the_executable() {
        // A developer running from inside the checkout means the checkout,
        // even when an installed copy exists elsewhere.
        let c = candidate_roots(None, Some(&p("/home/u/checkout")), Some(&p("/usr/bin")), &p("/build"));
        let pos = |s: &str| c.iter().position(|x| x == &p(s)).unwrap();
        assert!(pos("/home/u/checkout") < pos("/usr/bin"));
    }

    #[test]
    fn the_walk_is_bounded_and_does_not_reach_for_every_ancestor() {
        // Matching some unrelated `assets/` on the way to `/` is worse than
        // reporting a miss, so the climb stops. Both directions: it must climb
        // FAR ENOUGH for target/release, and not forever.
        let c = candidate_roots(None, Some(&p("/a/b/c/d/e/f/g/h/i/j")), None, &p("/build"));
        assert!(c.contains(&p("/a/b/c/d/e/f/g/h")), "did not climb far enough for target/release");
        assert!(!c.contains(&p("/a/b")), "climbed further than a match could mean anything");
    }

    #[test]
    fn duplicate_candidates_are_probed_once() {
        // cwd and exe dir are the same whenever someone runs ./game from its
        // own directory, which is most of the time.
        let c = candidate_roots(None, Some(&p("/x/y")), Some(&p("/x/y")), &p("/x/y"));
        let n = c.iter().filter(|q| *q == &p("/x/y")).count();
        assert_eq!(n, 1, "the same directory is probed {n} times");
    }

    #[test]
    fn a_root_needs_the_shaders_directory_not_just_assets() {
        // An empty `assets/` created by a packager must not win over the real
        // tree further up — a plausible wrong root reports its miss two
        // directories later, where it makes no sense.
        let only_assets = |q: &Path| q.ends_with("assets");
        assert!(!is_asset_root(Path::new("/fake"), &only_assets));
        let full = |q: &Path| q.ends_with("assets") || q.ends_with("assets/shaders");
        assert!(is_asset_root(Path::new("/real"), &full));
    }
}
