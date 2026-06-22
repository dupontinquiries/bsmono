// gitmind-core/src/patch.rs
//
// Email/patch-series interop — the overlap with b4 and mailing-list
// workflows. b4 itself wraps `git format-patch` / `git send-email` and
// applies series with `git am` (its `shazam -H` does a 3-way merge). We
// don't reimplement b4; we add the same git plumbing it stands on, so a
// gitmind repo can both PRODUCE a series for the list and CONSUME an mbox
// pulled from lore/public-inbox — and then track it as a review session
// (review.rs) the way b4's new `review` TUI tracks per-series branches.
//
// Built here:
//   - format_series : range -> patch files (what you send)
//   - apply_series  : patch files / mbox -> applied commits, 3-way capable
//                     (matching b4 shazam -H's better apply success rate)
//   - parse_mbox    : pull subject/from/message-id out of an mbox so the
//                     UI can show a series before applying it
//
// Deferred (network / out of scope for core): fetching a thread from
// lore by message-id (that's a `lei`/public-inbox HTTP call), trailer
// collection, and PGP attestation — all designed to layer on top.

use crate::backend::GitCli;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct PatchMeta {
    pub subject: String,
    pub from: String,
    pub message_id: String,
}

/// `git format-patch <range> -o <outdir>` → the generated patch files,
/// sorted. `range` is anything git accepts, e.g. "main..HEAD".
pub fn format_series(repo: &GitCli, range: &str, outdir: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(outdir).map_err(|e| e.to_string())?;
    let outdir_s = outdir.to_string_lossy().to_string();
    repo.git(&["format-patch", "-o", &outdir_s, range])?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(outdir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "patch").unwrap_or(false))
        .collect();
    files.sort();
    Ok(files)
}

/// Apply patch files (or an mbox) with `git am`, optionally 3-way. Returns
/// the new HEAD on success; aborts the am on failure so the tree is left
/// clean rather than half-applied.
pub fn apply_series(repo: &GitCli, patches: &[PathBuf], three_way: bool) -> Result<String, String> {
    let mut args: Vec<String> = vec!["am".into()];
    if three_way {
        args.push("--3way".into());
    }
    for p in patches {
        args.push(p.to_string_lossy().to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match repo.git(&arg_refs) {
        Ok(_) => repo.git(&["rev-parse", "HEAD"]),
        Err(e) => {
            let _ = repo.git(&["am", "--abort"]);
            Err(format!("am failed (aborted): {e}"))
        }
    }
}

/// Lightweight mbox splitter: one entry per message, pulling the headers a
/// UI needs to preview a series. Not a full RFC 5322 parser — it reads
/// headers up to the first blank line, which is all we need here.
pub fn parse_mbox(data: &str) -> Vec<PatchMeta> {
    let mut out = Vec::new();
    // Messages begin at a line starting with "From " (mbox separator).
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in data.lines() {
        if line.starts_with("From ") && !cur.is_empty() {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }

    for b in blocks {
        let (mut subject, mut from, mut mid) = (String::new(), String::new(), String::new());
        for line in b.lines() {
            if line.is_empty() {
                break; // end of headers
            }
            let lower = line.to_ascii_lowercase();
            if let Some(v) = line.strip_prefix("Subject:").or_else(|| line.strip_prefix("subject:")) {
                subject = v.trim().to_string();
            } else if lower.starts_with("from:") {
                from = line[5..].trim().to_string();
            } else if lower.starts_with("message-id:") {
                mid = line[11..].trim().to_string();
            }
        }
        if !subject.is_empty() || !mid.is_empty() {
            out.push(PatchMeta { subject, from, message_id: mid });
        }
    }
    out
}
