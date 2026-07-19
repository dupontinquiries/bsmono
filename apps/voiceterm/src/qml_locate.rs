// src/qml_locate.rs
// Where is qml/main.qml? The old search only probed CWD- and EXE-relative paths, which breaks
// the "binary installed to ~/.local/bin, launched from some other directory" case (e.g. the
// gitmind-installed `voiceterm@import-…` run from inside apps/gitmind of a checkout). This
// resolver is pure candidate-generation (unit-tested, no deps) + a thin fs probe.
//
// Candidate order (first hit wins):
//   1. $VOICETERM_QML_DIR/main.qml           — explicit override for launchers/packagers
//   2. CWD-relative qml/, ./qml/, ../qml/    — the original behavior, preserved verbatim
//   3. repo walk: <ancestor>/apps/voiceterm/qml/main.qml for each CWD ancestor (6 levels) —
//      launching from ANYWHERE inside a monorepo checkout finds the sibling app dir
//   4. EXE-relative qml/, ../qml/, ../../qml/ — the original fallback, preserved
//   5. $XDG_DATA_HOME/voiceterm/qml, ~/.local/share/voiceterm/qml — the right install target
//      for a binary living in ~/.local/bin
//   6. env!("CARGO_MANIFEST_DIR")/qml        — dev builds launched from anywhere on the build
//      machine; last because it's the stalest kind of path (baked at compile time)

use std::path::{Path, PathBuf};

/// How many CWD ancestors the repo walk probes (CWD itself + 6 parents).
const REPO_WALK_DEPTH: usize = 6;

/// Pure candidate list, in probe order, deduplicated. All inputs injected for testability.
pub fn candidate_paths(
    cwd: Option<&Path>,
    exe: Option<&Path>,
    env_override: Option<&str>,
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
    manifest_dir: &str,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };

    // 1. explicit override
    if let Some(dir) = env_override {
        if !dir.is_empty() {
            push(Path::new(dir).join("main.qml"));
        }
    }

    // 2. CWD-relative (original behavior, verbatim strings)
    push(PathBuf::from("qml/main.qml"));
    push(PathBuf::from("./qml/main.qml"));
    push(PathBuf::from("../qml/main.qml"));

    // 3. repo walk: any ancestor that contains apps/voiceterm/qml
    if let Some(cwd) = cwd {
        let mut dir = Some(cwd);
        for _ in 0..=REPO_WALK_DEPTH {
            let Some(d) = dir else { break };
            push(d.join("apps/voiceterm/qml/main.qml"));
            dir = d.parent();
        }
    }

    // 4. EXE-relative (original fallback)
    if let Some(exe_dir) = exe.and_then(|e| e.parent()) {
        for rel in ["qml/main.qml", "../qml/main.qml", "../../qml/main.qml"] {
            push(exe_dir.join(rel));
        }
    }

    // 5. XDG data dir
    if let Some(x) = xdg_data_home {
        push(x.join("voiceterm/qml/main.qml"));
    }
    if let Some(h) = home {
        push(h.join(".local/share/voiceterm/qml/main.qml"));
    }

    // 6. compile-time source dir
    push(Path::new(manifest_dir).join("qml/main.qml"));

    out
}

/// Probe the candidates against the filesystem, logging each like the original search did.
/// Returns the first that exists.
pub fn locate() -> Option<String> {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let env_override = std::env::var("VOICETERM_QML_DIR").ok();
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cands = candidate_paths(
        cwd.as_deref(),
        exe.as_deref(),
        env_override.as_deref(),
        xdg.as_deref(),
        home.as_deref(),
        env!("CARGO_MANIFEST_DIR"),
    );
    eprintln!("Searching for QML in:");
    let mut found = None;
    for c in &cands {
        let exists = c.exists();
        eprintln!("  {} -> {}", c.display(), if exists { "FOUND" } else { "no" });
        if exists && found.is_none() {
            found = Some(c.to_string_lossy().to_string());
            // keep logging the rest? The original stopped logging exe-relative after a hit;
            // stop here too — the interesting information is the path that won.
            break;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_the_original_cwd_and_exe_candidates_in_order() {
        let cands = candidate_paths(
            None,
            Some(Path::new("/home/u/.local/bin/voiceterm")),
            None,
            None,
            None,
            "/src/voiceterm",
        );
        let s: Vec<String> = cands.iter().map(|p| p.display().to_string()).collect();
        // originals, in the original order
        let i0 = s.iter().position(|p| p == "qml/main.qml").unwrap();
        let i1 = s.iter().position(|p| p == "./qml/main.qml").unwrap();
        let i2 = s.iter().position(|p| p == "../qml/main.qml").unwrap();
        assert!(i0 < i1 && i1 < i2);
        let e0 = s.iter().position(|p| p == "/home/u/.local/bin/qml/main.qml").unwrap();
        let e1 = s.iter().position(|p| p == "/home/u/.local/bin/../qml/main.qml").unwrap();
        let e2 = s.iter().position(|p| p == "/home/u/.local/bin/../../qml/main.qml").unwrap();
        assert!(i2 < e0 && e0 < e1 && e1 < e2);
        // manifest dir is last
        assert_eq!(s.last().unwrap(), "/src/voiceterm/qml/main.qml");
    }

    #[test]
    fn repo_walk_finds_the_sibling_app_from_inside_another_app() {
        // the reported failure: CWD deep in the checkout at apps/gitmind
        let cwd = Path::new("/home/k/Downloads/tmp-claude/monorepo/20260718/merged/tmp/apps/gitmind");
        let cands = candidate_paths(Some(cwd), None, None, None, None, "/x");
        let want = Path::new("/home/k/Downloads/tmp-claude/monorepo/20260718/merged/tmp/apps/voiceterm/qml/main.qml");
        assert!(cands.iter().any(|p| p == want), "repo walk must reach the sibling app dir");
        // and the walk stays bounded
        assert!(!cands.iter().any(|p| p == Path::new("/apps/voiceterm/qml/main.qml")));
    }

    #[test]
    fn env_override_wins_and_empty_override_is_ignored() {
        let cands = candidate_paths(None, None, Some("/opt/vt/qml"), None, None, "/x");
        assert_eq!(cands[0], Path::new("/opt/vt/qml/main.qml"));
        let cands = candidate_paths(None, None, Some(""), None, None, "/x");
        assert_eq!(cands[0], Path::new("qml/main.qml"));
    }

    #[test]
    fn xdg_candidates_cover_the_local_bin_install_case() {
        let cands = candidate_paths(
            None,
            Some(Path::new("/home/k/.local/bin/voiceterm@import-20260718234147")),
            None,
            Some(Path::new("/home/k/.local/share")),
            Some(Path::new("/home/k")),
            "/x",
        );
        assert!(cands.iter().any(|p| p == Path::new("/home/k/.local/share/voiceterm/qml/main.qml")));
        // XDG_DATA_HOME and HOME-derived paths dedupe when they coincide
        let n = cands.iter().filter(|p| p.ends_with("share/voiceterm/qml/main.qml")).count();
        assert_eq!(n, 1);
    }

    #[test]
    fn dedup_keeps_first_occurrence_only() {
        let cwd = Path::new("/r");
        let cands = candidate_paths(Some(cwd), Some(Path::new("/r/apps/voiceterm/bin/vt")), None, None, None, "/r/apps/voiceterm");
        let mut seen = std::collections::HashSet::new();
        for c in &cands {
            assert!(seen.insert(c.clone()), "duplicate candidate: {}", c.display());
        }
    }
}
