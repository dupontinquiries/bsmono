// gitmind-core/src/encode_run.rs
//
// The executor for `encode`. Third module in this crate with a careful
// plan and no runner — after `remote` (a backup trait with no target)
// and `reroll` (a plan with no ssh adapter).
//
// `encode` builds the ffmpeg argv, tracks segments, decides what needs
// re-encoding, and manages a retry queue. It never spawns a process.
// `ffmpeg` appears nowhere else in the tree, so `MEDIA.md`'s "ffmpeg +
// queue + hooks" was queue and hooks.
//
// ## What this adds and what it deliberately does not
//
// It runs `ffmpeg_encode_av1_cmd` and `ffmpeg_validate_cmd`, and feeds
// the pass/fail back into `EncodeQueue::mark` so the retry logic that
// already exists gets exercised. It does NOT decide policy: which
// segments, which CRF, how many attempts — all of that is `encode`'s and
// stays there.
//
// ## Why validate is a separate call
//
// `encode` already ships `ffmpeg_validate_cmd` with the note that a
// pass/fail hook reads the result, so the two-step shape is the module's
// own design. It matters: ffmpeg can exit 0 having written a file that
// does not decode, and a queue that marked those as passed would drop
// them from the retry list — a corrupt encode is then indistinguishable
// from a good one until someone plays it.

use crate::encode::{EncodeQueue, ffmpeg_encode_av1_cmd, ffmpeg_validate_cmd};

/// What running one job produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Encoded and the output decodes.
    Ok,
    /// ffmpeg failed, or wrote something that will not decode. The
    /// message is ffmpeg's own stderr tail — it is the only place the
    /// actual reason appears, and a generic "encode failed" cannot be
    /// acted on.
    Failed(String),
}

/// Run one argv, returning the tail of stderr on failure.
///
/// The TAIL rather than the whole thing: ffmpeg writes banners and
/// per-frame progress to stderr, and the error is at the end. Carrying
/// megabytes of progress lines into a queue's failure record would make
/// the record useless and the log unreadable.
fn run(argv: &[String]) -> Result<(), String> {
    let (cmd, args) = argv.split_first().ok_or("empty command")?;
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        // A missing ffmpeg is the most likely first failure on a fresh
        // machine, and it deserves to say so rather than surfacing as an
        // encode error about the file.
        .map_err(|e| format!("{cmd}: {e} (is ffmpeg installed?)"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(8).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(tail.join("\n"))
}

/// Encode one file, then verify it decodes.
///
/// Both steps, always: see the module note on why a separate validate
/// matters.
pub fn encode_and_validate(input: &str, output: &str, crf: u32) -> Outcome {
    if let Err(e) = run(&ffmpeg_encode_av1_cmd(input, output, crf)) {
        return Outcome::Failed(e);
    }
    match run(&ffmpeg_validate_cmd(output)) {
        Ok(()) => Outcome::Ok,
        Err(e) => Outcome::Failed(format!("output does not decode: {e}")),
    }
}

/// Drain a queue, encoding each job.
///
/// `out_path` maps a job to its destination — supplied by the caller
/// because naming outputs is a repository concern this module has no
/// business deciding.
///
/// Returns (passed, failed). `mark` is called for BOTH outcomes, which
/// is what drives the retry logic already in `EncodeQueue`: a failure
/// that never reaches `mark` is a job that silently disappears from the
/// queue instead of being retried.
pub fn drain(
    q: &mut EncodeQueue,
    crf: u32,
    max_attempts: u32,
    mut out_path: impl FnMut(&crate::encode::EncodeJob) -> (String, String),
    mut on_line: impl FnMut(&str),
) -> (usize, usize) {
    let (mut passed, mut failed) = (0usize, 0usize);
    // `next_job` does NOT pop — it finds a Queued job and flips it to
    // Encoding. So termination depends on `mark` moving that job out of
    // Queued, which it does for both outcomes: passed leaves it done,
    // failed either re-queues it (attempts remaining) or fails it.
    //
    // That is why `mark` is called on BOTH branches below and not only
    // on success. Skipping it on failure would leave the job in
    // Encoding forever — not an infinite loop, but a job that silently
    // vanishes from the queue instead of being retried, which is worse
    // because the retry logic exists and would look like it ran.
    while let Some(job) = q.next_job() {
        let (input, output) = out_path(&job);
        on_line(&format!("encoding {input} → {output}"));
        match encode_and_validate(&input, &output, crf) {
            Outcome::Ok => {
                q.mark(job.id, true, max_attempts);
                passed += 1;
                on_line(&format!("✓ {output}"));
            }
            Outcome::Failed(e) => {
                q.mark(job.id, false, max_attempts);
                failed += 1;
                on_line(&format!("✗ {output} — {e}"));
            }
        }
    }
    (passed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_binary_says_it_is_missing() {
        // The most likely first failure on a fresh machine. It must not
        // surface as an encode error about the file, which would send a
        // person looking at their video instead of their PATH.
        let e = run(&["definitely-not-a-real-binary-xyz".to_string()]).unwrap_err();
        assert!(e.contains("ffmpeg is installed") || e.contains("is ffmpeg installed"), "{e}");
    }

    #[test]
    fn an_empty_command_is_an_error_not_a_panic() {
        // `split_first` on an empty slice returns None; indexing would
        // have panicked, and this runs inside a queue drain where a
        // panic loses every remaining job.
        assert!(run(&[]).is_err());
    }

    #[test]
    fn a_failing_command_carries_the_tail_of_its_stderr() {
        // Ffmpeg writes banners and per-frame progress to stderr and
        // puts the error at the END. Carrying the whole thing would make
        // a queue's failure record useless; carrying the head would
        // carry the banner and not the reason.
        //
        // `sh` stands in for ffmpeg here: the point is the tail
        // extraction, not the tool.
        let argv: Vec<String> = ["sh", "-c", "for i in 1 2 3 4 5 6 7 8 9 10; do \
             echo line$i >&2; done; echo THE_REAL_ERROR >&2; exit 1"]
            .iter().map(|s| s.to_string()).collect();
        let e = run(&argv).unwrap_err();
        assert!(e.contains("THE_REAL_ERROR"), "lost the tail: {e}");
        assert!(!e.contains("line1\n"), "kept the whole log: {e}");
    }

    #[test]
    fn a_successful_command_is_ok() {
        let argv: Vec<String> = ["sh", "-c", "exit 0"].iter().map(|s| s.to_string()).collect();
        assert!(run(&argv).is_ok());
    }
}
