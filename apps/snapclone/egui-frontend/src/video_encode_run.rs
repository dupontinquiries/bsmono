//! video_encode_run — the thin layer that actually runs the encode.
//!
//! `av1_encode::command` builds the argv and `video_spill` produces the frames. Both were
//! deliberately pure — "the exact `Command::spawn` is a thin, separate layer", as
//! `mediagen`'s header puts it — and neither ran anything. This is that layer, and it is
//! kept as small as the promise implies: it takes an argv it does not construct and a file it
//! does not write.
//!
//! ## Off the UI thread, and streamed
//!
//! A snap is seconds of video and hundreds of megabytes of raw frames. Reading that into
//! memory to hand to ffmpeg would double the peak on a phone that already has the frames on
//! disk, so the spill is streamed to stdin in chunks. And the whole thing runs on its own
//! thread: an encode that blocks a frame drops the preview, and the user is usually still
//! looking at the camera when it starts.
//!
//! ## What is kept when it fails
//!
//! The spill. On success it is deleted — cache bytes with nothing left to say — and on
//! failure it stays, because the one thing worth having after a failed encode is the input
//! that produced it. Deleting it would leave a message and no way to reproduce.
//!
//! ffmpeg's stderr TAIL is kept too, not the whole thing: it prints a banner and a
//! configuration dump before it prints why it stopped, and a message that leads with the
//! build flags buries the reason.

use std::io::{Read, Write};

/// How an encode ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// The encoder exited cleanly and the output exists.
    Ok { out: std::path::PathBuf, bytes: u64 },
    /// It ran and refused. `why` is the tail of stderr.
    Failed { code: Option<i32>, why: String },
    /// It never started — usually a missing binary.
    NotRun(String),
}

/// Lines of stderr kept from a failure.
///
/// Twelve: enough for ffmpeg's actual complaint and the line or two of context around it,
/// short enough that the reason is not buried under the configuration dump it prints first.
pub const STDERR_TAIL_LINES: usize = 12;

/// How much of the spill to hand over at a time.
///
/// 4MB — about one 720p RGBA frame. Small enough that a stall is visible as a stall rather
/// than as a freeze, large enough that the syscall overhead is nothing next to the encode.
pub const CHUNK: usize = 4 * 1024 * 1024;

/// Run an encode: `argv` with `spill` streamed to its stdin.
///
/// Blocking — the caller decides which thread this happens on. Kept that way so it is
/// testable without a runtime, and so the thread the encode runs on is a decision made
/// where the UI is, not buried in here.
pub fn run_blocking(argv: &[String], spill: &std::path::Path, out: &std::path::Path) -> Done {
    let Some((bin, args)) = argv.split_first() else {
        return Done::NotRun("empty command".into());
    };
    let mut child = match std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        // The binary is missing or not executable. Named separately from a non-zero exit
        // because "install ffmpeg" and "ffmpeg refused your arguments" are different problems
        // and the fixes have nothing in common.
        Err(e) => return Done::NotRun(format!("{bin}: {e}")),
    };

    let mut stdin = child.stdin.take();
    let feed = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::open(spill)?;
        let mut buf = vec![0u8; CHUNK];
        let w = stdin.as_mut().expect("piped above");
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
        }
        Ok(())
    })();
    // Closed before waiting, always. ffmpeg reads rawvideo until EOF, so a stdin left open is
    // a child that never exits and a UI thread joining a thread that never finishes — the
    // deadlock this ordering exists to avoid.
    drop(stdin);

    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => return Done::NotRun(format!("wait failed: {e}")),
    };

    if let Err(e) = feed {
        // A broken pipe here usually means the child already died with its own complaint, and
        // that complaint is more useful than "broken pipe" — so stderr wins when there is any.
        let why = if err.trim().is_empty() { format!("feeding frames failed: {e}") } else { tail(&err) };
        return Done::Failed { code: status.code(), why };
    }
    if !status.success() {
        return Done::Failed { code: status.code(), why: tail(&err) };
    }
    let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        // Exit 0 and no output. Reporting success here would hand the user a zero-byte clip
        // and call it a snap.
        return Done::Failed {
            code: status.code(),
            why: format!("exited cleanly but wrote nothing to {}", out.display()),
        };
    }
    Done::Ok { out: out.to_path_buf(), bytes }
}

/// The last few lines of a stderr dump.
pub fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[from..].join("\n")
}

/// Run it, then clean up the spill according to how it went.
///
/// Success deletes the spill; failure keeps it. After a failed encode the input that produced
/// it is the one thing worth having, and deleting it leaves a message and no way to reproduce.
pub fn run_and_tidy(argv: &[String], spill: &std::path::Path, out: &std::path::Path) -> Done {
    let done = run_blocking(argv, spill, out);
    if matches!(done, Done::Ok { .. }) {
        let _ = std::fs::remove_file(spill);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sc-enc-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// The frames reach the child's stdin, all of them, streamed rather than slurped.
    #[test]
    fn the_spill_is_piped_to_the_child_in_full() {
        let dir = scratch("pipe");
        let spill = dir.join("in.rgba");
        let out = dir.join("out.bin");
        // Three chunks' worth would be slow; a few hundred KB proves the loop without it.
        let data = vec![7u8; 300_000];
        std::fs::write(&spill, &data).unwrap();

        // `sh -c 'cat > out'` stands in for ffmpeg: it reads stdin to EOF and writes a file,
        // which is the contract that matters here.
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("cat > {}", out.display()),
        ];
        let done = run_blocking(&argv, &spill, &out);
        assert!(matches!(done, Done::Ok { .. }), "{done:?}");
        assert_eq!(std::fs::read(&out).unwrap(), data, "the frames did not arrive intact");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Install ffmpeg" and "ffmpeg refused your arguments" are different problems whose fixes
    /// have nothing in common, so they are different variants.
    #[test]
    fn a_missing_binary_is_not_reported_as_a_failed_encode() {
        let dir = scratch("missing");
        let spill = dir.join("in.rgba");
        std::fs::write(&spill, b"x").unwrap();
        let done = run_blocking(
            &["definitely-not-a-real-encoder-xyz".to_string()],
            &spill,
            &dir.join("out.bin"),
        );
        match done {
            Done::NotRun(msg) => assert!(msg.contains("definitely-not"), "{msg}"),
            other => panic!("expected NotRun, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg prints a banner and a configuration dump before it prints why it stopped. A
    /// message that leads with the build flags buries the reason.
    #[test]
    fn a_failure_reports_the_end_of_stderr_not_the_beginning() {
        let dir = scratch("stderr");
        let spill = dir.join("in.rgba");
        std::fs::write(&spill, b"x").unwrap();
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "cat >/dev/null; for i in $(seq 1 40); do echo banner line $i >&2; done; \
             echo 'Invalid pixel format' >&2; exit 1"
                .to_string(),
        ];
        let done = run_blocking(&argv, &spill, &dir.join("out.bin"));
        match done {
            Done::Failed { code, why } => {
                assert_eq!(code, Some(1));
                assert!(why.contains("Invalid pixel format"), "the reason was cut: {why}");
                assert!(!why.contains("banner line 1"), "the whole dump came through: {why}");
                assert!(why.lines().count() <= STDERR_TAIL_LINES);
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exit 0 and no output. Reporting success would hand the user a zero-byte clip and call
    /// it a snap.
    #[test]
    fn a_clean_exit_that_wrote_nothing_is_a_failure() {
        let dir = scratch("empty");
        let spill = dir.join("in.rgba");
        std::fs::write(&spill, b"x").unwrap();
        let argv = vec!["sh".to_string(), "-c".to_string(), "cat >/dev/null".to_string()];
        let done = run_blocking(&argv, &spill, &dir.join("never-written.bin"));
        match done {
            Done::Failed { why, .. } => assert!(why.contains("wrote nothing"), "{why}"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// After a failed encode the input that produced it is the one thing worth having.
    /// Deleting it leaves a message and no way to reproduce.
    #[test]
    fn the_spill_survives_a_failure_and_is_cleaned_up_after_a_success() {
        let dir = scratch("tidy");
        let out = dir.join("out.bin");

        let spill = dir.join("bad.rgba");
        std::fs::write(&spill, b"x").unwrap();
        let fail = vec!["sh".to_string(), "-c".to_string(), "cat >/dev/null; exit 3".to_string()];
        let _ = run_and_tidy(&fail, &spill, &out);
        assert!(spill.exists(), "a failed encode deleted its own input");

        let spill2 = dir.join("good.rgba");
        std::fs::write(&spill2, b"x").unwrap();
        let ok = vec!["sh".to_string(), "-c".to_string(), format!("cat > {}", out.display())];
        let done = run_and_tidy(&ok, &spill2, &out);
        assert!(matches!(done, Done::Ok { .. }), "{done:?}");
        assert!(!spill2.exists(), "a successful encode left its raw frames behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
