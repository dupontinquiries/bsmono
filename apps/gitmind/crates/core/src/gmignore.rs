// gitmind-core/src/gmignore.rs
//
// Smart fanning: a `.gmignore` file at the repo root restricts which paths
// the watcher (and ingest) considers. It's gitignore-flavored — comments,
// blank lines, globs, anchored and directory patterns, and `!` negation
// with last-match-wins — but deliberately a pragmatic subset, not the full
// gitignore spec. The win: on a big tree you fan out the watch only over
// the paths that matter (skip target/, node_modules/, build artifacts,
// secrets), which keeps the realtime time-machine cheap and focused.

use std::path::Path;

struct Rule {
    /// Pattern with leading '/'/trailing '/' stripped; globs intact.
    pat: String,
    negate: bool,
    dir_only: bool,
    anchored: bool,
    has_slash: bool,
}

#[derive(Default)]
pub struct GmIgnore {
    rules: Vec<Rule>,
}

impl GmIgnore {
    /// Load `<repo_root>/.gmignore`. Absent file → matches nothing.
    pub fn load(repo_root: &Path) -> GmIgnore {
        match std::fs::read_to_string(repo_root.join(".gmignore")) {
            Ok(s) => GmIgnore::from_str(&s),
            Err(_) => GmIgnore::default(),
        }
    }

    /// Load and merge `.gitignore` then `.gmignore` (gmignore rules win on
    /// conflict via last-match). Used by native commits so their file set
    /// matches what git would track. v1 reads only the repo-root files —
    /// nested `.gitignore` files aren't yet honored.
    pub fn load_all(repo_root: &Path) -> GmIgnore {
        let mut text = String::new();
        for f in [".gitignore", ".gmignore"] {
            if let Ok(s) = std::fs::read_to_string(repo_root.join(f)) {
                text.push_str(&s);
                text.push('\n');
            }
        }
        GmIgnore::from_str(&text)
    }

    pub fn from_str(text: &str) -> GmIgnore {
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = raw.trim_end();
            if line.is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let mut s = line;
            let negate = s.starts_with('!');
            if negate {
                s = &s[1..];
            }
            let dir_only = s.ends_with('/');
            let s = s.trim_end_matches('/');
            let anchored = s.starts_with('/');
            let s = s.trim_start_matches('/');
            if s.is_empty() {
                continue;
            }
            rules.push(Rule {
                pat: s.to_string(),
                negate,
                dir_only,
                anchored,
                has_slash: s.contains('/'),
            });
        }
        GmIgnore { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Should `rel` (a repo-relative path with '/' separators) be ignored?
    /// Last matching rule wins, so a later `!pattern` can re-include.
    pub fn ignored(&self, rel: &str) -> bool {
        let rel = rel.trim_start_matches('/');
        let components: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
        let basename = components.last().copied().unwrap_or("");
        let mut decision = false;
        for r in &self.rules {
            if rule_matches(r, rel, &components, basename) {
                decision = !r.negate;
            }
        }
        decision
    }
}

fn rule_matches(r: &Rule, rel: &str, components: &[&str], basename: &str) -> bool {
    if r.anchored || r.has_slash {
        // Match against the full path from the root. For dir_only, also match
        // anything *under* the directory.
        if glob_match(&r.pat, rel) {
            return true;
        }
        if r.dir_only {
            let prefix = format!("{}/", r.pat);
            if rel.starts_with(&prefix) || glob_prefix(&r.pat, rel) {
                return true;
            }
        }
        false
    } else if r.dir_only {
        // A bare directory name → match that component anywhere in the path
        // (the dir and everything inside it).
        components.iter().any(|c| glob_match(&r.pat, c))
    } else {
        // Bare name/glob → match the basename, or any component (gitignore
        // matches non-anchored names at any depth).
        glob_match(&r.pat, basename) || components.iter().any(|c| glob_match(&r.pat, c))
    }
}

/// True if `pat` (with `*` and `?`) matches a path prefix component-wise,
/// used so `a/b` (dir) ignores `a/b/c.rs`.
fn glob_prefix(pat: &str, rel: &str) -> bool {
    let pc: Vec<&str> = pat.split('/').collect();
    let rc: Vec<&str> = rel.split('/').collect();
    if rc.len() <= pc.len() {
        return false;
    }
    pc.iter().zip(rc.iter()).all(|(p, r)| glob_match(p, r))
}

/// Minimal glob: `*` matches any run of non-'/' chars, `?` any single char.
/// No `**`/character classes — enough for typical ignore patterns.
fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => {
                // Try consuming zero or more chars (not crossing '/').
                if m(&p[1..], t) {
                    return true;
                }
                if let Some(&c) = t.first() {
                    if c != '/' {
                        return m(p, &t[1..]);
                    }
                }
                false
            }
            Some('?') => match t.first() {
                Some(&c) if c != '/' => m(&p[1..], &t[1..]),
                _ => false,
            },
            Some(&pc) => match t.first() {
                Some(&tc) if tc == pc => m(&p[1..], &t[1..]),
                _ => false,
            },
        }
    }
    m(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_patterns() {
        let gi = GmIgnore::from_str("# comment\ntarget/\n*.log\n/build\nsecret.txt\nnode_modules/\n");
        assert!(gi.ignored("target/debug/app"));
        assert!(gi.ignored("crates/x/target/foo"));
        assert!(gi.ignored("logs/run.log"));
        assert!(gi.ignored("build")); // anchored top-level
        assert!(gi.ignored("a/secret.txt"));
        assert!(gi.ignored("node_modules/pkg/index.js"));
        assert!(!gi.ignored("src/main.rs"));
        assert!(!gi.ignored("deep/build/keep")); // /build is anchored to root
    }

    #[test]
    fn negation_last_match_wins() {
        let gi = GmIgnore::from_str("*.log\n!keep.log\n");
        assert!(gi.ignored("debug.log"));
        assert!(!gi.ignored("keep.log")); // re-included by negation
    }

    #[test]
    fn anchored_path_pattern() {
        let gi = GmIgnore::from_str("/dist/bundle.js\n");
        assert!(gi.ignored("dist/bundle.js"));
        assert!(!gi.ignored("src/dist/bundle.js"));
    }

    #[test]
    fn empty_ignore_matches_nothing() {
        let gi = GmIgnore::from_str("\n#only comments\n");
        assert!(gi.is_empty());
        assert!(!gi.ignored("anything/at/all.rs"));
    }
}
