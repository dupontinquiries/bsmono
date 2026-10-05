//! The title menu's Battleground rows start the SEPARATE game `games/clawbound-battleground`
//! (the 5v5 arena for the thirteen ancestors), the way the bsgames hub starts it: `cargo run`
//! from the monorepo root with `--quick-match` and the chosen ancestor by name.
//!
//! Until c21 those rows -- "Quick Play (5v5)", the Play screen's one-ancestor pick and the
//! six-pick draft -- printed a line to stderr and reopened the menu, because this binary has no
//! renderer for that match and never will: the arena is its own crate with its own loop. The
//! honest route is to launch it. The argv is pure so it is tested; `spawn` is the I/O.

use std::path::{Path, PathBuf};

/// The arena's manifest, relative to the monorepo root; also the marker [`fleet_root_from`]
/// looks for.
pub const MANIFEST: &str = "games/clawbound-battleground/Cargo.toml";

/// `cargo run --release --manifest-path games/clawbound-battleground/Cargo.toml -- --quick-match
/// [--ancestor NAME]`. The arena resolves `--ancestor` by its roster name, case-insensitively,
/// and the names are the same as this crate's `ancestor::kit(a).name`
/// (`kit_names_are_the_arenas_roster_names` pins that against the arena's source).
pub fn argv(ancestor: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = ["cargo", "run", "--release", "--manifest-path", MANIFEST, "--", "--quick-match"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(a) = ancestor {
        v.push("--ancestor".into());
        v.push(a.to_string());
    }
    v
}

/// The monorepo root: the first ancestor of `start` holding [`MANIFEST`]. Same rule as
/// `bsgames::centre::fleet_root_from`, with the arena as the marker.
pub fn fleet_root_from(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|d| d.join(MANIFEST).is_file()).map(Path::to_path_buf)
}

/// [`fleet_root_from`] the cwd, then this executable (a `target/release/clawbound-rs` under
/// the root); `None` when neither is inside the monorepo.
pub fn fleet_root() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|d| fleet_root_from(&d))
        .or_else(|| std::env::current_exe().ok().and_then(|e| fleet_root_from(&e)))
}

/// Start the arena (fire-and-forget) as `ancestor`, or a random one. Returns the line for the
/// menu's status row either way: what was launched, or why nothing was.
pub fn spawn(ancestor: Option<&str>) -> Result<String, String> {
    let root = fleet_root().ok_or_else(|| format!("can't find the monorepo root ({MANIFEST}) above the cwd or this binary"))?;
    let argv = argv(ancestor);
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&root)
        .spawn()
        .map(|_| match ancestor {
            Some(a) => format!("Battleground launched as {a} (building first if needed)"),
            None => "Battleground launched -- a random ancestor (building first if needed)".to_string(),
        })
        .map_err(|e| format!("couldn't launch the Battleground: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_the_hubs_launch_with_the_game_flags_after_the_separator() {
        assert_eq!(
            argv(None),
            vec!["cargo", "run", "--release", "--manifest-path", MANIFEST, "--", "--quick-match"]
        );
        let a = argv(Some("Snowhush"));
        let sep = a.iter().position(|s| s == "--").unwrap();
        assert!(a[sep..].contains(&"--ancestor".to_string()), "the game's flags follow `--`: {a:?}");
        assert_eq!(a.last().map(String::as_str), Some("Snowhush"));
        assert!(a[..sep].iter().all(|s| !s.starts_with("--quick") && s != "--ancestor"), "cargo would reject a game flag of its own: {a:?}");
    }

    #[test]
    fn fleet_root_is_the_ancestor_holding_the_arena() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = here.join("../..").canonicalize().unwrap();
        assert_eq!(fleet_root_from(here).unwrap().canonicalize().unwrap(), root);
        assert_eq!(fleet_root_from(&root).unwrap().canonicalize().unwrap(), root);
        let tmp = std::env::temp_dir().join(format!("clawbound-root-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("games/x")).unwrap();
        assert_eq!(fleet_root_from(&tmp.join("games/x")), None, "no marker: None, not `/`");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Every ancestor this menu can pick is a name the arena's roster resolves (`--ancestor` is
    /// matched against `gods::ROSTER[..].name`). A scan of the arena's source, with a floor on
    /// its own match count so a moved file fails loudly rather than passing vacuously.
    #[test]
    fn kit_names_are_the_arenas_roster_names() {
        let gods = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clawbound-battleground/src/gods.rs");
        let src = std::fs::read_to_string(&gods).unwrap_or_else(|e| panic!("{}: {e}", gods.display()));
        let roster: Vec<String> = src
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix("id: ")?;
                let (_, rest) = rest.split_once("name: \"")?;
                rest.split('"').next().map(str::to_string)
            })
            .collect();
        assert!(roster.len() >= 13, "the roster scan found {} names in {}", roster.len(), gods.display());
        for a in crate::ancestor::ALL {
            let name = crate::ancestor::kit(a).name;
            assert!(
                roster.iter().any(|r| r.eq_ignore_ascii_case(name)),
                "{a:?} is `{name}` here and not in the arena's roster {roster:?}"
            );
        }
    }
}
