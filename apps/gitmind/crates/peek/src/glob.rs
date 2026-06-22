//! Glob expansion: a pattern like `src/**/*.rs:5xx` expands to every matching
//! file with the `:range` suffix re-attached, so the parser sees one query per
//! file. Non-glob inputs pass through untouched.
use anyhow::Result;
use globwalk::GlobWalkerBuilder;

/// Split a query string into (glob-or-path, ":range-suffix"). The suffix starts
/// at the LAST colon (ranges never contain colons; Windows drive letters aren't
/// a concern in this repo-relative tool).
fn split_suffix(s: &str) -> (&str, &str) {
    match s.rfind(':') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

pub fn expand_globs(patterns: &[String]) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for p in patterns {
        let (pat, suffix) = split_suffix(p);
        if is_glob(pat) {
            let walker = GlobWalkerBuilder::from_patterns(".", &[pat])
                .build()?
                .filter_map(|e| e.ok());
            let mut hits = Vec::new();
            for entry in walker {
                if entry.file_type().is_file() {
                    let s = entry.path().to_string_lossy();
                    let s = s.strip_prefix("./").unwrap_or(&s).to_string();
                    hits.push(format!("{}{}", s, suffix));
                }
            }
            hits.sort();
            files.extend(hits);
        } else {
            files.push(p.clone());
        }
    }
    Ok(files)
}
