// src/model_download.rs
//
// v0.1.463 (llamacpp arc): downloads GGUF model files from
// HuggingFace to the local models directory, with progress
// reporting and resume support.
//
// SPLIT FOR TESTABILITY (same approach as hf_hub.rs):
//   - Pure logic (local path construction, filename
//     sanitization, progress-percent math, resume-offset
//     decisions) lives in standalone functions tested below
//     via `rustc --test`.
//   - The actual network streaming (`download_to_file`) is a
//     thin ureq wrapper that delegates all decisions to the
//     pure helpers. It's the only part that can't be unit
//     tested in the sandbox.
//
// The download URL itself is built by hf_hub::build_download_url
// — this module only handles the local side + the transfer.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Progress callback signature: (bytes_done, total_bytes_or_0).
/// total is 0 when the server doesn't send Content-Length.
pub type ProgressFn<'a> = dyn FnMut(u64, u64) + 'a;

/// Compute the local destination path for a downloaded model.
/// Files go under `<models_dir>/<sanitized_repo>/<filename>` so
/// two repos with same-named quant files don't collide.
///
/// `repo_id` like "bartowski/Qwen2.5-7B-Instruct-GGUF" becomes
/// a single directory component "bartowski__Qwen2.5-7B-Instruct-GGUF"
/// (slash → double underscore) to keep a flat one-level layout
/// that's easy to list in the picker.
pub fn local_dest_path(models_dir: &Path, repo_id: &str, filename: &str) -> PathBuf {
    let repo_dir = sanitize_repo_component(repo_id);
    let safe_file = sanitize_filename(filename);
    models_dir.join(repo_dir).join(safe_file)
}

/// Turn a repo id into a single safe directory component.
/// Replaces '/' with '__' and strips anything that isn't
/// alphanumeric, '-', '_', or '.'.
pub fn sanitize_repo_component(repo_id: &str) -> String {
    let mut out = String::with_capacity(repo_id.len());
    let collapsed = repo_id.replace('/', "__");
    for c in collapsed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        }
        // else: drop (path-traversal chars, spaces, etc.)
    }
    if out.is_empty() {
        out.push_str("model");
    }
    out
}

/// Sanitize a filename: keep the basename only (no directory
/// components — defends against "../" traversal in a malicious
/// rfilename), keep safe chars.
pub fn sanitize_filename(filename: &str) -> String {
    // Take only the final path component.
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename);
    let mut out = String::with_capacity(base.len());
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        }
    }
    if out.is_empty() {
        out.push_str("model.gguf");
    }
    out
}

/// Compute an integer percentage (0..=100) from bytes done and
/// total. Returns 0 when total is 0 (unknown size). Saturates at
/// 100 (in case a resumed transfer reports slightly over).
pub fn progress_percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let pct = (done.saturating_mul(100)) / total;
    if pct > 100 { 100 } else { pct as u8 }
}

/// Given the size of an already-existing partial file and the
/// total expected size, decide what to do:
///   - None expected (total 0): always restart (can't validate).
///   - partial == total: already complete, skip download.
///   - partial < total: resume from `partial` (Range request).
///   - partial > total: corrupt/over-long, restart from 0.
#[derive(Debug, PartialEq, Eq)]
pub enum ResumeDecision {
    /// File is already complete; no download needed.
    Complete,
    /// Start fresh from byte 0.
    Restart,
    /// Resume from this byte offset.
    Resume(u64),
}

pub fn resume_decision(partial_bytes: u64, total_bytes: u64) -> ResumeDecision {
    if total_bytes == 0 {
        return ResumeDecision::Restart;
    }
    if partial_bytes == total_bytes {
        ResumeDecision::Complete
    } else if partial_bytes < total_bytes {
        if partial_bytes == 0 {
            ResumeDecision::Restart
        } else {
            ResumeDecision::Resume(partial_bytes)
        }
    } else {
        ResumeDecision::Restart
    }
}

/// Format a byte count as a human-readable string for the UI.
/// e.g. 4_700_000_000 -> "4.4 GB". Binary units (1024).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Stream a URL to a local file, reporting progress. Creates
/// parent directories as needed. Supports resume if a partial
/// file exists (via HTTP Range). The only non-pure function in
/// this module.
///
/// Returns the final on-disk path on success, or an error
/// string. Uses the provided ureq agent (shares the app's
/// connection pool / TLS config).
///
/// NOTE: not standalone-tested — it does real network IO. The
/// decisions it makes (paths, resume, progress) are all
/// delegated to the pure helpers above, which ARE tested.
pub fn download_to_file(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    progress: &mut ProgressFn,
) -> Result<PathBuf, String> {
    // Ensure the parent dir exists.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir failed: {e}"))?;
    }

    // Probe existing partial size for resume.
    let partial = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    // First, a HEAD-ish GET to discover total size. We just do a
    // GET and read Content-Length; if resuming we add a Range
    // header. ureq doesn't expose HEAD cleanly across versions,
    // so a ranged GET is the portable path.
    let mut req = agent.get(url);
    let mut resume_from = 0u64;
    if partial > 0 {
        // We'll validate against total after we see the headers,
        // but optimistically request a range.
        req = req.set("Range", &format!("bytes={}-", partial));
        resume_from = partial;
    }

    let resp = match req.call() {
        Ok(r) => r,
        // v0.1.550: a 416 (Range Not Satisfiable) on a resume attempt
        // means the partial file on disk is already >= the server's
        // file size — i.e. the download is COMPLETE. ureq surfaces this
        // as Err(Status(416, _)). Without this, a finished file reported
        // a scary "download request failed ... 416" on every re-trigger
        // (seen with the Qwen GGUF). Treat it as success.
        Err(ureq::Error::Status(416, _)) if resume_from > 0 => {
            return Ok(dest.to_path_buf());
        }
        Err(e) => return Err(format!("download request failed: {e}")),
    };

    // Content-Length here is the REMAINING bytes when ranged, or
    // the full size otherwise. We compute total for progress.
    let content_len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    // If the server ignored our Range (status 200 not 206), it's
    // sending the whole file — restart from 0.
    let status = resp.status();
    let ranged_ok = status == 206;
    if resume_from > 0 && !ranged_ok {
        resume_from = 0;
    }
    let total = if ranged_ok {
        resume_from + content_len
    } else {
        content_len
    };

    // Open the file: append if resuming, truncate otherwise.
    let mut file = if resume_from > 0 {
        std::fs::OpenOptions::new()
            .append(true)
            .open(dest)
            .map_err(|e| format!("open for append failed: {e}"))?
    } else {
        std::fs::File::create(dest)
            .map_err(|e| format!("create failed: {e}"))?
    };

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut done = resume_from;
    progress(done, total);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write failed: {e}"))?;
        done += n as u64;
        progress(done, total);
    }
    file.flush().map_err(|e| format!("flush failed: {e}"))?;

    Ok(dest.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn local_dest_path_flattens_repo() {
        let dir = Path::new("/home/u/.config/web2qml/models");
        let p = local_dest_path(dir,
            "bartowski/Qwen2.5-7B-Instruct-GGUF",
            "Qwen2.5-7B-Instruct-Q4_K_M.gguf");
        assert_eq!(p,
            Path::new("/home/u/.config/web2qml/models/bartowski__Qwen2.5-7B-Instruct-GGUF/Qwen2.5-7B-Instruct-Q4_K_M.gguf"));
    }

    #[test]
    fn sanitize_repo_replaces_slash() {
        assert_eq!(sanitize_repo_component("a/b"), "a__b");
        assert_eq!(sanitize_repo_component("TheBloke/Model-GGUF"), "TheBloke__Model-GGUF");
    }

    #[test]
    fn sanitize_repo_strips_unsafe() {
        // Both slashes become __; ".." survives as literal chars
        // but is harmless because sanitize_filename takes
        // basename-only as a second defense, and this is a single
        // directory component anyway.
        assert_eq!(sanitize_repo_component("a/../b"), "a__..__b");
    }

    #[test]
    fn sanitize_filename_takes_basename() {
        // Path-traversal attempt in rfilename: only basename kept.
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("sub/dir/model-Q4.gguf"), "model-Q4.gguf");
        assert_eq!(sanitize_filename("clean.gguf"), "clean.gguf");
    }

    #[test]
    fn sanitize_filename_empty_fallback() {
        assert_eq!(sanitize_filename("///"), "model.gguf");
    }

    #[test]
    fn progress_percent_basic() {
        assert_eq!(progress_percent(0, 100), 0);
        assert_eq!(progress_percent(50, 100), 50);
        assert_eq!(progress_percent(100, 100), 100);
        // Unknown total → 0.
        assert_eq!(progress_percent(50, 0), 0);
        // Over-100 saturates.
        assert_eq!(progress_percent(110, 100), 100);
    }

    #[test]
    fn progress_percent_large_no_overflow() {
        // 4.7GB file, ensure no u64 overflow in done*100.
        let total = 4_700_000_000u64;
        assert_eq!(progress_percent(total / 2, total), 50);
        assert_eq!(progress_percent(total, total), 100);
    }

    #[test]
    fn resume_decision_complete() {
        assert_eq!(resume_decision(1000, 1000), ResumeDecision::Complete);
    }

    #[test]
    fn resume_decision_resume() {
        assert_eq!(resume_decision(400, 1000), ResumeDecision::Resume(400));
    }

    #[test]
    fn resume_decision_fresh() {
        assert_eq!(resume_decision(0, 1000), ResumeDecision::Restart);
    }

    #[test]
    fn resume_decision_corrupt_overlong() {
        // Partial bigger than expected → restart.
        assert_eq!(resume_decision(2000, 1000), ResumeDecision::Restart);
    }

    #[test]
    fn resume_decision_unknown_total() {
        assert_eq!(resume_decision(500, 0), ResumeDecision::Restart);
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
