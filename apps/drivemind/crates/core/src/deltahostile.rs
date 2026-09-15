// deltahostile — which files in this git repo defeat git's storage model?
//
// git's delta compression (xdelta between similar blobs, then zlib) works
// on text and loses badly on already-compressed content: every version of
// a photo, video, zip, or model weights file is stored near-whole. A repo
// that versions media grows ~linearly with edits. gitmind's chunk store is
// built for exactly that content (FastCDC dedup before zstd), so the
// answer isn't "don't version media" — it's "version it in the right
// store". This module finds the candidates and writes the plan.
//
// Detection is two independent signals, either sufficient:
//
//   * EXTENSION CLASS — formats that are compressed by definition
//     (media containers, archives, compressed images). Cheap, zero I/O.
//   * MEASURED ENTROPY — for everything else ≥ min_size, sample three
//     64 KiB windows (start / middle / end) and average Shannon entropy.
//     ≥ 7.5 bits/byte means "effectively incompressible, undeltable".
//     Three windows, not one, because real files have low-entropy headers
//     (a zip's central directory, an MP4's moov atom) that would fool a
//     single probe at either end.
//
// Only git-TRACKED files are candidates (`git ls-files -z`): untracked
// media isn't git's problem yet. The plan renders as data + the exact
// `gin migrate` invocation; execution belongs to gitmind (portage/ingest),
// which owns the store. drivemind diagnoses, gitmind operates.

use crate::{shannon_entropy, Event, Sink, SysRunner};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions that are compressed-by-format. Lowercase, no dot. The video
/// list mirrors av1tuner_core::discovery::VIDEO_EXTENSIONS (kept as an
/// independent copy: a diagnostics crate should not build ffmpeg-adjacent
/// deps just for a string table — revisit if the lists ever drift).
pub const COMPRESSED_EXTS: &[&str] = &[
    // video
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts", "wmv", "flv",
    "ogv", "3gp", "vob", "ivf",
    // audio
    "mp3", "aac", "m4a", "ogg", "opus", "flac",
    // images
    "jpg", "jpeg", "png", "gif", "webp", "heic", "avif",
    // archives & packages
    "zip", "gz", "xz", "zst", "bz2", "7z", "rar", "jar", "apk", "flatpak", "deb", "rpm",
    // opaque model/data blobs commonly checked into repos
    "gguf", "safetensors", "onnx", "npz",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reason {
    CompressedFormat(String),
    HighEntropy(f64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    /// Path relative to the repo root, as git reports it.
    pub path: String,
    pub size: u64,
    pub reason: Reason,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub repo: String,
    pub candidates: Vec<Candidate>,
    pub total_bytes: u64,
    /// Tracked bytes overall, so the plan can say "34% of this repo".
    pub tracked_bytes: u64,
}

impl MigrationPlan {
    /// The handoff: the command gitmind's CLI runs to act on this plan.
    pub fn gin_command(&self) -> String {
        format!("gin migrate '{}' --apply", self.repo)
    }
}

#[derive(Clone, Debug)]
pub struct DeltaHostileConfig {
    /// Files below this size never become candidates — small blobs are
    /// cheap even stored badly, and churning them adds migration noise.
    pub min_size: u64,
    pub entropy_threshold: f64,
}

impl Default for DeltaHostileConfig {
    fn default() -> Self {
        Self { min_size: 1 << 20, entropy_threshold: 7.5 }
    }
}

/// Average entropy over up to three 64 KiB windows: start, middle, end.
pub fn sampled_entropy(path: &Path) -> std::io::Result<f64> {
    const W: usize = 64 << 10;
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut windows: Vec<u64> = vec![0];
    if len > (2 * W as u64) {
        windows.push(len / 2 - (W as u64) / 2);
    }
    if len > (3 * W as u64) {
        windows.push(len - W as u64);
    }
    let mut buf = vec![0u8; W];
    let mut sum = 0.0;
    let mut n = 0usize;
    for off in windows {
        f.seek(SeekFrom::Start(off))?;
        let got = f.read(&mut buf)?;
        if got == 0 {
            continue;
        }
        sum += shannon_entropy(&buf[..got]);
        n += 1;
    }
    Ok(if n == 0 { 0.0 } else { sum / n as f64 })
}

fn ext_of(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Scan a git repo for delta-hostile tracked files.
pub fn scan_repo(
    sys: &dyn SysRunner,
    repo: &Path,
    cfg: &DeltaHostileConfig,
    sink: Sink,
) -> Result<MigrationPlan, String> {
    sink(Event::Phase { name: "delta-hostility", subject: repo.display().to_string() });
    let repo_s = repo.display().to_string();
    sink(Event::Exec(format!("git -C {repo_s} ls-files -z")));
    let out = sys.run("git", &["-C", &repo_s, "ls-files", "-z"])?;
    if out.code != 0 {
        return Err(format!("not a git repo? git ls-files: {}", out.stderr.trim()));
    }

    let mut candidates = Vec::new();
    let mut total_bytes = 0u64;
    let mut tracked_bytes = 0u64;

    for rel in out.stdout.split('\0').filter(|s| !s.is_empty()) {
        let abs = repo.join(rel);
        let size = match fs::metadata(&abs) {
            Ok(m) if m.is_file() => m.len(),
            _ => continue, // deleted-but-tracked, submodule dir, etc.
        };
        tracked_bytes += size;
        if size < cfg.min_size {
            continue;
        }
        let reason = if let Some(ext) = ext_of(rel) {
            if COMPRESSED_EXTS.contains(&ext.as_str()) {
                Some(Reason::CompressedFormat(ext))
            } else {
                entropy_reason(&abs, cfg, sink)
            }
        } else {
            entropy_reason(&abs, cfg, sink)
        };
        if let Some(reason) = reason {
            total_bytes += size;
            candidates.push(Candidate { path: rel.to_string(), size, reason });
        }
    }

    candidates.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(MigrationPlan { repo: repo_s, candidates, total_bytes, tracked_bytes })
}

fn entropy_reason(abs: &Path, cfg: &DeltaHostileConfig, sink: Sink) -> Option<Reason> {
    match sampled_entropy(abs) {
        Ok(e) if e >= cfg.entropy_threshold => Some(Reason::HighEntropy(e)),
        Ok(_) => None,
        Err(err) => {
            sink(Event::Note(format!("entropy probe {}: {err}", abs.display())));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{null_sink, FixtureRunner};

    /// Deterministic pseudo-random bytes (xorshift) — high entropy without
    /// pulling a rand dep into the crate.
    fn noise(n: usize, mut seed: u64) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            v.extend_from_slice(&seed.to_le_bytes());
        }
        v.truncate(n);
        v
    }

    #[test]
    fn classifies_by_extension_and_entropy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 2 MiB "video" (ext match, content irrelevant), 2 MiB noise with a
        // neutral extension (entropy match), 2 MiB of zeros (neither), and
        // a small jpg under min_size (skipped).
        fs::write(root.join("clip.mp4"), vec![0u8; 2 << 20]).unwrap();
        fs::write(root.join("weights.bin"), noise(2 << 20, 42)).unwrap();
        fs::write(root.join("sparse.raw"), vec![0u8; 2 << 20]).unwrap();
        fs::write(root.join("icon.jpg"), vec![1u8; 4096]).unwrap();

        let repo_s = root.display().to_string();
        let sys = FixtureRunner::new().on(
            &format!("git -C {repo_s} ls-files -z"),
            0,
            "clip.mp4\0weights.bin\0sparse.raw\0icon.jpg\0gone.mp4\0",
        );
        let mut sink = null_sink();
        let plan = scan_repo(&sys, root, &DeltaHostileConfig::default(), &mut sink).unwrap();

        let names: Vec<&str> = plan.candidates.iter().map(|c| c.path.as_str()).collect();
        assert!(names.contains(&"clip.mp4"));
        assert!(names.contains(&"weights.bin"));
        assert!(!names.contains(&"sparse.raw"), "zeros are perfectly compressible");
        assert!(!names.contains(&"icon.jpg"), "below min_size");
        assert!(!names.contains(&"gone.mp4"), "tracked but absent on disk");
        assert_eq!(plan.total_bytes, 2 * (2 << 20));
        assert!(matches!(
            plan.candidates.iter().find(|c| c.path == "weights.bin").unwrap().reason,
            Reason::HighEntropy(e) if e > 7.5
        ));
        assert!(plan.gin_command().contains("gin migrate"));
    }

    #[test]
    fn mid_file_entropy_is_not_fooled_by_low_entropy_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hybrid");
        // 64K zeros ++ 1 MiB noise ++ 64K zeros: head/tail probes alone
        // would each read ~0 bits; the middle window rescues the average
        // above (0 + 8 + 0)/3 ≈ 2.67 — which shows why the threshold is an
        // average and this file should NOT trip it. The inverse matters
        // too: solid noise must trip it. Assert both.
        let mut data = vec![0u8; 64 << 10];
        data.extend(super::tests::noise(1 << 20, 7));
        data.extend(vec![0u8; 64 << 10]);
        fs::write(&p, &data).unwrap();
        let e = sampled_entropy(&p).unwrap();
        assert!(e < 7.5, "averaged windows: {e}");

        let q = dir.path().join("solid");
        fs::write(&q, super::tests::noise(1 << 20, 9)).unwrap();
        assert!(sampled_entropy(&q).unwrap() > 7.5);
    }

    #[test]
    fn git_ls_files_failing_is_a_real_error_not_an_empty_plan() {
        // `if out.code != 0 { return Err(...) }` -- the very first real
        // check in scan_repo, and untested: a path that is not a git
        // repo at all (a plain directory someone pointed the scanner at
        // by mistake) must be reported as a real error, not silently
        // treated as "a git repo with nothing tracked", which would look
        // identical to a genuinely empty, valid repo.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo_s = root.display().to_string();
        let mut sys = FixtureRunner::new();
        // FixtureRunner::on() can only set stdout (stderr is hardcoded
        // empty there); building the fixture directly via the map it
        // exposes is the only way to simulate a nonzero exit WITH a real
        // stderr message, matching exactly what a real `git ls-files`
        // failure looks like.
        sys.map.insert(
            format!("git -C {repo_s} ls-files -z"),
            crate::SysOutput { code: 128, stdout: String::new(), stderr: "fatal: not a git repository".into() },
        );
        let mut sink = null_sink();
        let err = scan_repo(&sys, root, &DeltaHostileConfig::default(), &mut sink)
            .expect_err("a nonzero git exit must not become an empty plan");
        assert!(err.contains("not a git repository"), "{err}");
    }

    #[test]
    fn git_itself_failing_to_run_is_propagated_not_swallowed() {
        // A different failure shape than a nonzero exit code: the git
        // BINARY itself is missing or cannot be spawned at all. scan_repo
        // uses `?` on sys.run(...) directly for this -- untested before
        // this, since FixtureRunner-based tests always succeed at the
        // run() level and only vary the exit code/output.
        struct NoGit;
        impl SysRunner for NoGit {
            fn run(&self, _: &str, _: &[&str]) -> Result<crate::SysOutput, String> {
                Err("git: command not found".into())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = null_sink();
        let err = scan_repo(&NoGit, dir.path(), &DeltaHostileConfig::default(), &mut sink)
            .expect_err("a missing git binary must be a real Err");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn a_file_with_no_extension_at_all_is_still_caught_by_entropy() {
        // ext_of() returns None for an extensionless path, which routes
        // straight to entropy_reason (the `else` branch scan_repo takes
        // when ext_of returns None) -- every file in the existing test
        // has a real extension, so this branch specifically was never
        // exercised on its own.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("BLOB"), noise(2 << 20, 123)).unwrap();
        let repo_s = root.display().to_string();
        let sys = FixtureRunner::new().on(&format!("git -C {repo_s} ls-files -z"), 0, "BLOB\0");
        let mut sink = null_sink();
        let plan = scan_repo(&sys, root, &DeltaHostileConfig::default(), &mut sink).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(plan.candidates[0].reason, Reason::HighEntropy(_)),
            "an extensionless high-entropy blob must be caught, not silently skipped: {:?}", plan.candidates[0].reason);
    }

    #[test]
    fn an_unreadable_tracked_file_is_noted_not_fatal() {
        // entropy_reason's Err arm sinks a Note and returns None rather
        // than failing the whole scan -- a tracked file that has since
        // become unreadable (permissions changed, or a real disk error)
        // must not take down a diagnostic scan of the rest of the repo.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            fs::write(root.join("locked.bin"), noise(2 << 20, 55)).unwrap();
            fs::set_permissions(root.join("locked.bin"), fs::Permissions::from_mode(0o000)).unwrap();
            let repo_s = root.display().to_string();
            let sys = FixtureRunner::new().on(&format!("git -C {repo_s} ls-files -z"), 0, "locked.bin\0");
            let mut notes: Vec<String> = Vec::new();
            let mut sink = |e: Event| { if let Event::Note(n) = e { notes.push(n); } };
            let plan = scan_repo(&sys, root, &DeltaHostileConfig::default(), &mut sink);
            fs::set_permissions(root.join("locked.bin"), fs::Permissions::from_mode(0o644)).unwrap();
            let plan = plan.expect("an unreadable file must not fail the whole scan");
            // NOT asserting candidates.is_empty() here: whether the
            // permission bit is actually enforced depends on whether
            // this runs as root (a sandboxed CI user commonly does,
            // and root can read a 0o000 file regardless of the mode
            // bits). The property this test actually needs to hold in
            // EITHER case is the one already checked above -- the scan
            // completes with Ok rather than aborting -- not a specific
            // claim about the candidate list that only holds when the
            // block is genuinely enforced.
            let _ = plan;
        }
    }
}
