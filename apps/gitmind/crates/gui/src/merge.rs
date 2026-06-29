//! When an imported zip is itself a git repo (its own commits, possibly different
//! authors), we don't flatten it onto the target's first commit — we bring its history
//! in as a branch and explore how to integrate it. Every strategy is tried in an
//! isolated worktree inside a throwaway workspace, so the real repo is never touched
//! until you pick a winner. Results are ranked cleanest-first.
//!
//! Note: gitmind's object store is regenerated from git history, so integrating the git
//! side is sufficient — the .gitmind store catches up on demand.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Strategy {
    pub id: &'static str,
    pub label: &'static str,
    pub rebase: bool,
    pub x_option: Option<&'static str>, // conflict bias: "ours" (repo) | "theirs" (zip)
    pub allow_unrelated: bool,
}

/// The strategy matrix to try, depending on whether the two histories share an ancestor.
pub fn strategies(related: bool) -> Vec<Strategy> {
    if related {
        vec![
            Strategy { id: "merge",         label: "Merge (auto)",          rebase: false, x_option: None,           allow_unrelated: false },
            Strategy { id: "merge-theirs",  label: "Merge, prefer zip",     rebase: false, x_option: Some("theirs"), allow_unrelated: false },
            Strategy { id: "merge-ours",    label: "Merge, prefer repo",    rebase: false, x_option: Some("ours"),   allow_unrelated: false },
            Strategy { id: "rebase",        label: "Rebase zip onto repo",  rebase: true,  x_option: None,           allow_unrelated: false },
            Strategy { id: "rebase-theirs", label: "Rebase, prefer zip",    rebase: true,  x_option: Some("theirs"), allow_unrelated: false },
        ]
    } else {
        vec![
            Strategy { id: "merge-unrelated",        label: "Merge (unrelated histories)", rebase: false, x_option: None,           allow_unrelated: true },
            Strategy { id: "merge-unrelated-theirs", label: "Merge unrelated, prefer zip", rebase: false, x_option: Some("theirs"), allow_unrelated: true },
            Strategy { id: "merge-unrelated-ours",   label: "Merge unrelated, prefer repo",rebase: false, x_option: Some("ours"),   allow_unrelated: true },
            Strategy { id: "rebase",                 label: "Rebase zip onto repo",        rebase: true,  x_option: None,           allow_unrelated: false },
        ]
    }
}

#[derive(Debug, Clone)]
pub struct Trial {
    pub id: String,
    pub label: String,
    pub ok: bool,
    pub conflicts: usize,
    pub commits: usize,
    pub summary: String,
    pub branch: String, // gm-<id>, present when ok
    pub recommended: bool,
}

/// Rank trials cleanest-first (clean before conflicted, then fewest conflicts) and flag
/// one recommendation: a clean plain merge if available, else the first clean result,
/// else the least-conflicting. Pure.
pub fn rank(mut trials: Vec<Trial>) -> Vec<Trial> {
    trials.sort_by(|a, b| b.ok.cmp(&a.ok).then(a.conflicts.cmp(&b.conflicts)));
    let pick = trials.iter().position(|t| t.ok && t.id == "merge")
        .or_else(|| trials.iter().position(|t| t.ok))
        .or(if trials.is_empty() { None } else { Some(0) });
    if let Some(idx) = pick {
        for (i, t) in trials.iter_mut().enumerate() {
            t.recommended = i == idx;
        }
    }
    trials
}

#[derive(Debug, Clone)]
pub struct Exploration {
    pub workspace: PathBuf, // the throwaway clone holding the gm-<id> result branches
    pub target_branch: String,
    pub related: bool,
    pub trials: Vec<Trial>,
}

fn git(cwd: &Path, path_env: &str, args: &[&str]) -> (bool, String, String) {
    // identity for the commits merge/rebase create, independent of global config
    let mut full: Vec<&str> = vec!["-c", "user.name=gitmind", "-c", "user.email=gitmind@local"];
    full.extend_from_slice(args);
    match Command::new("git").args(&full).current_dir(cwd).env("PATH", path_env).output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

/// Explore integration strategies in a throwaway clone of `target` with the `zip_repo`'s
/// history fetched in. The real `target` is never modified. Successful strategies leave
/// a `gm-<id>` branch in the workspace for `apply` to pull from.
pub fn run_exploration(target: &Path, zip_repo: &Path, ws: &Path, path_env: &str) -> Result<Exploration, String> {
    std::fs::create_dir_all(ws).map_err(|e| format!("workspace: {e}"))?;
    // local clone of the target (fast; hardlinked objects)
    let (ok, _, err) = git(ws, path_env, &["clone", "--quiet", "--local", &target.display().to_string(), "repo"]);
    if !ok {
        return Err(format!("couldn't copy the repo: {err}"));
    }
    let repo = ws.join("repo");
    let (_, target_branch, _) = git(&repo, path_env, &["rev-parse", "--abbrev-ref", "HEAD"]);
    // fetch the zip repo's history under a private ref
    let (_, zbr, _) = git(zip_repo, path_env, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let zref = if zbr.is_empty() || zbr == "HEAD" { "HEAD".to_string() } else { zbr };
    let (okf, _, errf) = git(&repo, path_env, &["fetch", "--quiet", &zip_repo.display().to_string(), &format!("{zref}:gmzip")]);
    if !okf {
        return Err(format!("couldn't read the zip's git history: {errf}"));
    }
    let (related, _, _) = git(&repo, path_env, &["merge-base", "HEAD", "gmzip"]);
    let zip_commits: usize = {
        let (_, c, _) = git(&repo, path_env, &["rev-list", "--count", &format!("{target_branch}..gmzip")]);
        c.parse().unwrap_or(0)
    };

    let mut trials = Vec::new();
    for s in strategies(related) {
        let wt = ws.join(format!("wt-{}", s.id));
        let _ = std::fs::remove_dir_all(&wt);
        let start = if s.rebase { "gmzip" } else { target_branch.as_str() };
        let (okw, _, _) = git(&repo, path_env, &["worktree", "add", "--quiet", "--detach", &wt.display().to_string(), start]);
        if !okw {
            continue;
        }
        let mut args: Vec<String> = Vec::new();
        if s.rebase {
            args.push("rebase".into());
            if let Some(x) = s.x_option { args.push("-X".into()); args.push(x.into()); }
            args.push(target_branch.clone());
        } else {
            args.push("merge".into());
            args.push("--no-edit".into());
            if s.allow_unrelated { args.push("--allow-unrelated-histories".into()); }
            if let Some(x) = s.x_option { args.push("-X".into()); args.push(x.into()); }
            args.push("gmzip".into());
        }
        let argref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (okrun, _, _) = git(&wt, path_env, &argref);
        let conflicts = {
            let (_, u, _) = git(&wt, path_env, &["diff", "--name-only", "--diff-filter=U"]);
            if u.is_empty() { 0 } else { u.lines().count() }
        };
        let ok = okrun && conflicts == 0;
        let mut branch = String::new();
        let summary;
        if ok {
            branch = format!("gm-{}", s.id);
            let _ = git(&wt, path_env, &["branch", "-f", &branch]);
            summary = if s.rebase {
                format!("clean · {zip_commits} commit(s) replayed onto {target_branch}")
            } else {
                format!("clean · merges {zip_commits} commit(s)")
            };
        } else {
            let _ = git(&wt, path_env, if s.rebase { &["rebase", "--abort"][..] } else { &["merge", "--abort"][..] });
            summary = format!("{conflicts} conflicting file(s)");
        }
        trials.push(Trial { id: s.id.into(), label: s.label.into(), ok, conflicts, commits: zip_commits, summary, branch, recommended: false });
        let _ = git(&repo, path_env, &["worktree", "remove", "--force", &wt.display().to_string()]);
    }

    Ok(Exploration { workspace: repo, target_branch, related, trials: rank(trials) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(repo: &Path, args: &[&str]) {
        let mut full: Vec<&str> = vec!["-c", "user.name=T", "-c", "user.email=t@t"];
        full.extend_from_slice(args);
        let o = Command::new("git").args(&full).current_dir(repo).output().unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
    }

    #[test]
    fn strategy_matrix_and_ranking() {
        assert!(strategies(true).iter().any(|s| s.id == "merge"));
        assert!(strategies(false).iter().all(|s| s.allow_unrelated || s.rebase), "unrelated set uses allow-unrelated (or rebase)");
        let ranked = rank(vec![
            Trial { id: "merge-theirs".into(), label: "".into(), ok: true, conflicts: 0, commits: 1, summary: "".into(), branch: "x".into(), recommended: false },
            Trial { id: "merge".into(), label: "".into(), ok: true, conflicts: 0, commits: 1, summary: "".into(), branch: "y".into(), recommended: false },
            Trial { id: "rebase".into(), label: "".into(), ok: false, conflicts: 3, commits: 1, summary: "".into(), branch: "".into(), recommended: false },
        ]);
        assert!(ranked[0].ok && ranked.last().unwrap().conflicts == 3, "clean first, conflicts last");
        assert!(ranked.iter().find(|t| t.id == "merge").unwrap().recommended, "plain clean merge recommended");
    }

    #[test]
    fn explores_real_divergent_repos() {
        let base = std::env::temp_dir().join(format!("gm-merge-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let target = base.join("target");
        let zip = base.join("zip");
        fs::create_dir_all(&target).unwrap();
        // shared base
        run(&target, &["init", "-q"]);
        fs::write(target.join("a.txt"), "base\n").unwrap();
        fs::write(target.join("b.txt"), "shared\n").unwrap();
        run(&target, &["add", "-A"]);
        run(&target, &["commit", "-q", "-m", "base"]);
        // zip = a clone that diverges, authored by someone else, editing a DIFFERENT file
        run(&base, &["clone", "-q", &target.display().to_string(), &zip.display().to_string()]);
        fs::write(zip.join("c.txt"), "from zip\n").unwrap();
        run(&zip, &["add", "-A"]);
        run(&zip, &["commit", "-q", "-m", "zip feature", "--author=Bob <bob@z>"]);
        // target moves on, editing yet another file (no overlap → clean merge)
        fs::write(target.join("a.txt"), "base\nmore\n").unwrap();
        run(&target, &["commit", "-qam", "target work", "--author=Alice <alice@t>"]);

        let ws = base.join("ws");
        let path = std::env::var("PATH").unwrap_or_default();
        let ex = run_exploration(&target, &zip, &ws, &path).expect("exploration runs");
        assert!(ex.related, "they share an ancestor");
        let merge = ex.trials.iter().find(|t| t.id == "merge").expect("merge tried");
        assert!(merge.ok && merge.conflicts == 0, "non-overlapping edits merge cleanly: {}", merge.summary);
        assert!(merge.commits >= 1, "zip's commit is counted");
        assert!(ex.trials.iter().any(|t| t.recommended), "something is recommended");
        // a result branch was left in the workspace for apply to pull
        let (found, _, _) = git(&ex.workspace, &path, &["rev-parse", "--verify", "gm-merge"]);
        assert!(found, "gm-merge branch persisted in the workspace");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn conflicting_edit_needs_a_bias() {
        let base = std::env::temp_dir().join(format!("gm-merge-conf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let target = base.join("target");
        let zip = base.join("zip");
        fs::create_dir_all(&target).unwrap();
        run(&target, &["init", "-q"]);
        fs::write(target.join("f.txt"), "one\n").unwrap();
        run(&target, &["add", "-A"]);
        run(&target, &["commit", "-q", "-m", "base"]);
        run(&base, &["clone", "-q", &target.display().to_string(), &zip.display().to_string()]);
        // both edit the SAME line differently → plain merge conflicts
        fs::write(zip.join("f.txt"), "zip-version\n").unwrap();
        run(&zip, &["commit", "-qam", "zip edit", "--author=Bob <bob@z>"]);
        fs::write(target.join("f.txt"), "target-version\n").unwrap();
        run(&target, &["commit", "-qam", "target edit", "--author=Alice <alice@t>"]);

        let ws = base.join("ws");
        let path = std::env::var("PATH").unwrap_or_default();
        let ex = run_exploration(&target, &zip, &ws, &path).expect("exploration runs");
        let plain = ex.trials.iter().find(|t| t.id == "merge").unwrap();
        assert!(!plain.ok && plain.conflicts >= 1, "same-line edit conflicts on auto merge");
        let theirs = ex.trials.iter().find(|t| t.id == "merge-theirs").unwrap();
        assert!(theirs.ok, "prefer-zip resolves it cleanly");
        let _ = fs::remove_dir_all(&base);
    }
}
