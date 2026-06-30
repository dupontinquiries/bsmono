//! Project identity, for recognizing the same project across multiple workspaces and for
//! keying a single shared build directory. A git repo's *root commit(s)* are identical in
//! every clone of the same project — even after branches diverge — so they form a stable
//! identity. Repos with no git history fall back to a gitmind id, then to a path hash.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn git_lines(repo: &Path, path_env: &str, args: &[&str]) -> Vec<String> {
    Command::new("git").args(args).current_dir(repo).env("PATH", path_env).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// All root (parentless) commits of a repo, sorted+deduped. Shared by every clone of the
/// same project, so two copies always return the same set.
pub fn root_commits(repo: &Path, path_env: &str) -> Vec<String> {
    let mut v = git_lines(repo, path_env, &["rev-list", "--max-parents=0", "HEAD"]);
    v.sort();
    v.dedup();
    v
}

/// Read a gitmind project id from .gitmind/config.toml if one is recorded (`id = "..."`).
pub fn gitmind_project_id(repo: &Path) -> Option<String> {
    let toml = std::fs::read_to_string(repo.join(".gitmind/config.toml")).ok()?;
    for line in toml.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("id") {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix('=') {
                let v = v.trim().trim_matches('"').trim();
                if !v.is_empty() { return Some(v.to_string()); }
            }
        }
    }
    None
}

/// Stable 64-bit FNV-1a as hex (no external hashing dep), for the path fallback.
fn fnv_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn short(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect()
}

/// A short, stable key identifying a project across copies. Root-commit identity first
/// (shared by all clones), then a gitmind id, then a hash of the canonical path.
pub fn project_key(repo: &Path, path_env: &str) -> String {
    if let Some(root) = root_commits(repo, path_env).first() {
        return short(root);
    }
    if let Some(id) = gitmind_project_id(repo) {
        return short(&id);
    }
    let canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    short(&fnv_hex(&canon.display().to_string()))
}

/// A human-browsable build-dir name: `<crate-name>-<key>`, so copies of one project share
/// a directory while different projects stay separate.
pub fn build_slug(repo: &Path, path_env: &str) -> String {
    let toml = std::fs::read_to_string(repo.join("Cargo.toml")).unwrap_or_default();
    let name = crate::buildflags::crate_name(&toml)
        .or_else(|| repo.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    format!("{}-{}", crate::places::sanitize_name(&name), project_key(repo, path_env))
}

/// A set of copies of one project across the filesystem.
#[derive(Debug, Clone, PartialEq)]
pub struct DupGroup {
    pub key: String,
    pub name: String,
    pub paths: Vec<String>,
}

/// Group `(path, key, name)` entries by project key, returning only the keys that occur in
/// more than one place (the duplicates). Paths sorted; groups ordered by key. Pure.
pub fn group_duplicates(entries: Vec<(String, String, String)>) -> Vec<DupGroup> {
    let mut map: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for (path, key, name) in entries {
        let e = map.entry(key).or_insert_with(|| (name.clone(), Vec::new()));
        if e.0.is_empty() { e.0 = name; }
        e.1.push(path);
    }
    map.into_iter()
        .filter(|(_, (_, paths))| paths.len() > 1)
        .map(|(key, (name, mut paths))| { paths.sort(); paths.dedup(); DupGroup { key, name, paths } })
        .filter(|g| g.paths.len() > 1)
        .collect()
}

/// How one copy of a project relates to another, by git ancestry.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    pub relation: String, // "same" | "ahead" (a newer) | "behind" (a older) | "diverged" | "unrelated"
    pub a_ahead: usize,   // commits in a not in b
    pub b_ahead: usize,   // commits in b not in a
}

fn git_ok(cwd: &Path, path_env: &str, args: &[&str]) -> bool {
    Command::new("git").args(args).current_dir(cwd).env("PATH", path_env)
        .output().map(|o| o.status.success()).unwrap_or(false)
}

fn git1(cwd: &Path, path_env: &str, args: &[&str]) -> String {
    git_lines(cwd, path_env, args).into_iter().next().unwrap_or_default()
}

/// Compare two copies of the same project by history, in a throwaway bare clone of `a`
/// with `b`'s tip fetched in, so neither real repo is touched. "ahead" means `a` is newer
/// (contains b's history plus more); "behind" the reverse; "diverged" means each has
/// commits the other lacks (a merge is needed to roll them together).
pub fn relate(a: &Path, b: &Path, path_env: &str) -> Relation {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("gm-rel-{}-{}", std::process::id(), nanos));
    let _ = std::fs::remove_dir_all(&tmp);
    let unknown = Relation { relation: "unrelated".into(), a_ahead: 0, b_ahead: 0 };
    if !git_ok(Path::new("/"), path_env, &["clone", "--bare", "--quiet", "--local", &a.display().to_string(), &tmp.display().to_string()]) {
        return unknown;
    }
    let a_head = git1(&tmp, path_env, &["rev-parse", "HEAD"]);
    let fetched = git_ok(&tmp, path_env, &["fetch", "--no-tags", "--quiet", &b.display().to_string(), "HEAD:refs/gmother"]);
    let b_head = if fetched { git1(&tmp, path_env, &["rev-parse", "refs/gmother"]) } else { String::new() };
    let result = if a_head.is_empty() || b_head.is_empty() {
        unknown
    } else if a_head == b_head {
        Relation { relation: "same".into(), a_ahead: 0, b_ahead: 0 }
    } else {
        let a_ahead: usize = git1(&tmp, path_env, &["rev-list", "--count", &format!("refs/gmother..{a_head}")]).parse().unwrap_or(0);
        let b_ahead: usize = git1(&tmp, path_env, &["rev-list", "--count", &format!("{a_head}..refs/gmother")]).parse().unwrap_or(0);
        let b_anc = git_ok(&tmp, path_env, &["merge-base", "--is-ancestor", &b_head, &a_head]);
        let a_anc = git_ok(&tmp, path_env, &["merge-base", "--is-ancestor", &a_head, &b_head]);
        let has_base = git_ok(&tmp, path_env, &["merge-base", &a_head, &b_head]);
        let relation = if b_anc { "ahead" } else if a_anc { "behind" } else if has_base { "diverged" } else { "unrelated" };
        Relation { relation: relation.into(), a_ahead, b_ahead }
    };
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Pick the canonical copy among duplicates: one under a known repos base if possible,
/// else the shallowest path, else the first. Pure.
pub fn canonical<'a>(paths: &'a [String], bases: &[String]) -> &'a str {
    paths.iter()
        .find(|p| bases.iter().any(|b| !b.is_empty() && p.starts_with(b)))
        .or_else(|| paths.iter().min_by_key(|p| (p.matches('/').count(), p.len())))
        .map(|s| s.as_str())
        .unwrap_or("")
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
    fn clones_of_one_project_share_a_key_even_after_diverging() {
        let base = std::env::temp_dir().join(format!("gm-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let a = base.join("Code/proj");
        fs::create_dir_all(&a).unwrap();
        run(&a, &["init", "-q"]);
        fs::write(a.join("Cargo.toml"), "[package]\nname = \"proj\"\nversion = \"0.1.0\"\n").unwrap();
        run(&a, &["add", "-A"]);
        run(&a, &["commit", "-q", "-m", "first"]);
        // a second workspace = a clone, then both diverge
        let b = base.join("scratch/proj-copy");
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        run(&base, &["clone", "-q", &a.display().to_string(), &b.display().to_string()]);
        fs::write(a.join("x.txt"), "a\n").unwrap(); run(&a, &["add", "-A"]); run(&a, &["commit", "-qm", "a work"]);
        fs::write(b.join("y.txt"), "b\n").unwrap(); run(&b, &["add", "-A"]); run(&b, &["commit", "-qm", "b work"]);

        let path = std::env::var("PATH").unwrap_or_default();
        let ka = project_key(&a, &path);
        let kb = project_key(&b, &path);
        assert_eq!(ka, kb, "same project → same key despite divergent branches");
        assert!(!ka.is_empty());
        // an unrelated project gets a different key
        let c = base.join("Code/other");
        fs::create_dir_all(&c).unwrap();
        run(&c, &["init", "-q"]);
        fs::write(c.join("f"), "z").unwrap(); run(&c, &["add", "-A"]); run(&c, &["commit", "-qm", "unrelated"]);
        assert_ne!(project_key(&c, &path), ka, "different project → different key");

        // build slug carries the crate name
        assert!(build_slug(&a, &path).starts_with("proj-"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grouping_and_canonical_are_pure() {
        let entries = vec![
            ("/home/u/Code/proj".to_string(), "abc123".to_string(), "proj".to_string()),
            ("/tmp/scratch/proj".to_string(), "abc123".to_string(), "proj".to_string()),
            ("/home/u/Code/solo".to_string(), "def456".to_string(), "solo".to_string()),
        ];
        let groups = group_duplicates(entries);
        assert_eq!(groups.len(), 1, "only the duplicated key forms a group");
        assert_eq!(groups[0].paths.len(), 2);
        assert_eq!(groups[0].name, "proj");
        let bases = vec!["/home/u/Code".to_string()];
        assert_eq!(canonical(&groups[0].paths, &bases), "/home/u/Code/proj", "prefer a copy under a repos base");
        // no base match → shallowest path (fewer components): /tmp/scratch/proj
        assert_eq!(canonical(&groups[0].paths, &[]), "/tmp/scratch/proj");
    }

    #[test]
    fn relate_classifies_newer_older_and_diverged() {
        let base = std::env::temp_dir().join(format!("gm-rel-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let old = base.join("old");
        let new = base.join("new");
        fs::create_dir_all(&old).unwrap();
        run(&old, &["init", "-q"]);
        fs::write(old.join("f"), "1\n").unwrap();
        run(&old, &["add", "-A"]); run(&old, &["commit", "-qm", "base"]);
        // new = clone of old, then old stays put, new gets ahead
        run(&base, &["clone", "-q", &old.display().to_string(), &new.display().to_string()]);
        fs::write(new.join("g"), "2\n").unwrap();
        run(&new, &["add", "-A"]); run(&new, &["commit", "-qm", "more"]);

        let path = std::env::var("PATH").unwrap_or_default();
        // old vs new: old is behind (new is newer)
        let r = relate(&old, &new, &path);
        assert_eq!(r.relation, "behind", "old copy is behind the newer one");
        assert_eq!(r.b_ahead, 1, "new has one extra commit");
        // new vs old: new is ahead
        assert_eq!(relate(&new, &old, &path).relation, "ahead");
        // identical copy → same
        let twin = base.join("twin");
        run(&base, &["clone", "-q", &new.display().to_string(), &twin.display().to_string()]);
        assert_eq!(relate(&new, &twin, &path).relation, "same");
        // both commit different things → diverged (each has a unique commit)
        fs::write(new.join("n2"), "n\n").unwrap();
        run(&new, &["add", "-A"]); run(&new, &["commit", "-qm", "new work"]);
        fs::write(twin.join("h"), "3\n").unwrap();
        run(&twin, &["add", "-A"]); run(&twin, &["commit", "-qm", "twin work"]);
        assert_eq!(relate(&new, &twin, &path).relation, "diverged", "both have unique commits");
        let _ = fs::remove_dir_all(&base);
    }
}
