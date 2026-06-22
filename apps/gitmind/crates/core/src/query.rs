// gitmind-core/src/query.rs
//
// Search history by author and/or language. Thin, honest wrapper over
// `git log`: author maps to `--author`, language maps to a pathspec over
// that language's file extensions, so "what did Alice change in Rust"
// becomes one log query. Returns structured hits the UI/CLI can render.

use crate::backend::GitCli;

#[derive(Clone, Debug, PartialEq)]
pub struct CommitHit {
    pub oid: String,
    pub author: String,
    pub date_unix: i64,
    pub summary: String,
}

/// Map a language name (or a raw extension) to the file extensions that
/// identify it. Unknown input is treated as a literal extension, so
/// `lang="rs"` and `lang="rust"` both work.
pub fn lang_to_exts(lang: &str) -> Vec<String> {
    let l = lang.trim().trim_start_matches('.').to_ascii_lowercase();
    let set: &[&str] = match l.as_str() {
        "rust" => &["rs"],
        "python" | "py" => &["py", "pyi"],
        "javascript" | "js" => &["js", "mjs", "cjs", "jsx"],
        "typescript" | "ts" => &["ts", "tsx"],
        "go" | "golang" => &["go"],
        "c" => &["c", "h"],
        "cpp" | "c++" => &["cpp", "cc", "cxx", "hpp", "hh"],
        "java" => &["java"],
        "ruby" | "rb" => &["rb"],
        "markdown" | "md" => &["md"],
        "shell" | "sh" | "bash" => &["sh", "bash"],
        // Fall back to treating the token itself as the extension.
        other => return vec![other.to_string()],
    };
    set.iter().map(|s| s.to_string()).collect()
}

/// Search the current branch's history. `author` is a substring git
/// matches against name/email; `lang` restricts to commits touching files
/// of that language. Either or both may be None.
pub fn search(
    repo: &GitCli,
    author: Option<&str>,
    lang: Option<&str>,
    limit: usize,
) -> Result<Vec<CommitHit>, String> {
    // Field-separated, one commit per line. \x1f between fields.
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("--max-count={limit}"),
        "--format=%H%x1f%an%x1f%at%x1f%s".into(),
    ];
    if let Some(a) = author {
        args.push(format!("--author={a}"));
    }
    // Pathspecs that select the language go after `--`.
    let mut pathspecs: Vec<String> = Vec::new();
    if let Some(lg) = lang {
        for ext in lang_to_exts(lg) {
            // glob magic so it matches at any depth, including the root.
            pathspecs.push(format!(":(glob)**/*.{ext}"));
        }
    }
    if !pathspecs.is_empty() {
        args.push("--".into());
        args.extend(pathspecs);
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = repo.git(&arg_refs)?;

    let mut hits = Vec::new();
    for line in out.lines() {
        let mut f = line.split('\u{1f}');
        let oid = f.next().unwrap_or("");
        let author = f.next().unwrap_or("");
        let at = f.next().unwrap_or("0");
        let summary = f.next().unwrap_or("");
        if oid.is_empty() {
            continue;
        }
        hits.push(CommitHit {
            oid: oid.chars().take(12).collect(),
            author: author.to_string(),
            date_unix: at.parse().unwrap_or(0),
            summary: summary.to_string(),
        });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_mapping() {
        assert_eq!(lang_to_exts("rust"), vec!["rs"]);
        assert_eq!(lang_to_exts(".RS"), vec!["rs"]);
        assert!(lang_to_exts("python").contains(&"py".to_string()));
        // Unknown → treated as a literal extension.
        assert_eq!(lang_to_exts("zig"), vec!["zig"]);
    }
}
