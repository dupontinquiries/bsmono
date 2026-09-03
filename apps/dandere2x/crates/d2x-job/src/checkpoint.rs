//! Resume a segmented upscale instead of starting it again.
//!
//! # Why segments are the unit
//!
//! `run_segmented` already cuts a video into keyframe-aligned segments, gives
//! each one a worker, and writes each to `stem.partNNN.ext` before concatenating
//! them. A segment is therefore the only place the work is already durable — it
//! ends with a complete, independently decodable file on disk.
//!
//! Anything finer would need the encoder to be resumable mid-stream, which it
//! is not, and anything coarser is what we have now: an upscale that takes
//! hours and loses all of it to a laptop lid.
//!
//! # A part file existing is NOT evidence it is finished
//!
//! This is the whole difficulty. A process killed mid-segment leaves a
//! truncated `.part` behind that looks exactly like a complete one — same name,
//! plausible size, and ffmpeg will happily concatenate it and produce a video
//! that is subtly short. So completion is recorded HERE, by the coordinating
//! thread, after the worker has returned success, and a part is only reused
//! when the checkpoint says so AND the file is still there.
//!
//! The reverse case is cheap and safe: a part on disk that the checkpoint does
//! not vouch for is simply redone.
//!
//! # And a checkpoint has to know what it was for
//!
//! Resuming into a different job is worse than not resuming: the parts
//! concatenate, the output is the right length, and half of it was upscaled
//! with the other model. So the checkpoint fingerprints the inputs that change
//! the pixels — source path, size and mtime, the segmentation, the model, the
//! scale and the backend — and refuses itself when any of them moves. A refused
//! checkpoint costs a rerun; an accepted wrong one costs a silent corruption
//! nobody sees until they watch it.
//!
//! The format is `key=value` lines, same grammar as the rest of this tree's
//! sidecars, so a stuck run can be inspected and edited with an editor rather
//! than a tool.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a checkpoint remembers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checkpoint {
    /// Identity of the job this belongs to. Compared verbatim; see `matches`.
    pub fingerprint: String,
    /// Indices of segments whose part file is COMPLETE.
    pub done: BTreeSet<usize>,
    /// How many segments the run was cut into, so a resume can tell a
    /// checkpoint for a 4-way split from one for an 8-way split even when
    /// every other input is identical.
    pub segments: usize,
}

impl Checkpoint {
    pub fn parse(text: &str) -> Checkpoint {
        let mut c = Checkpoint::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "fingerprint" => c.fingerprint = v.trim().to_string(),
                "segments" => c.segments = v.trim().parse().unwrap_or(0),
                "done" => {
                    // A malformed index is SKIPPED, not fatal. The cost of
                    // ignoring one is redoing a segment; the cost of refusing
                    // the file is redoing all of them.
                    for part in v.split(',') {
                        if let Ok(i) = part.trim().parse::<usize>() {
                            c.done.insert(i);
                        }
                    }
                }
                _ => {}
            }
        }
        c
    }

    pub fn render(&self) -> String {
        let done: Vec<String> = self.done.iter().map(|i| i.to_string()).collect();
        format!(
            "# dandere2x resume point — delete this file to start over\n\
             fingerprint={}\nsegments={}\ndone={}\n",
            self.fingerprint,
            self.segments,
            done.join(","),
        )
    }

    /// Is this checkpoint usable for the run described by `fingerprint` and
    /// `segments`?
    ///
    /// Both must match. The segment count is checked separately from the
    /// fingerprint because it is the one input a user changes casually — `-j 8`
    /// instead of `-j 4` — and the resulting parts cover different time ranges
    /// under the same names.
    pub fn matches(&self, fingerprint: &str, segments: usize) -> bool {
        !self.fingerprint.is_empty()
            && self.fingerprint == fingerprint
            && self.segments == segments

    }

    /// Segments that can be skipped: recorded done AND their part still on
    /// disk.
    ///
    /// Takes the existence test so the rule is testable without files. Both
    /// conditions, because either alone is wrong: the checkpoint alone trusts
    /// a part the user deleted, and the file alone trusts a truncated one left
    /// by a kill.
    pub fn resumable(&self, parts: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> BTreeSet<usize> {
        self.done
            .iter()
            .copied()
            .filter(|i| parts.get(*i).is_some_and(|p| exists(p)))
            .collect()
    }
}

/// Where the checkpoint for `output` lives.
pub fn path_for(output: &Path) -> PathBuf {
    let stem = output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    output.with_file_name(format!("{stem}.d2xresume"))
}

/// Identity of a run, for [`Checkpoint::matches`].
///
/// Everything here changes the OUTPUT PIXELS. Source identity is path plus
/// size plus mtime rather than a content hash: hashing a 40 GB source to decide
/// whether to skip four minutes of work is the wrong trade, and size+mtime
/// catches every accident short of a deliberate touch.
///
/// Deliberately NOT included: thread counts, memory budgets, queue depth. Those
/// change how fast the same pixels are produced, and refusing a resume because
/// someone passed a different `--jobs`-adjacent tuning flag would make the
/// feature useless on the machine it matters most on — the one you are still
/// tuning.
pub fn fingerprint(
    input: &Path,
    size: u64,
    mtime_secs: i64,
    model: &str,
    scale: u32,
    backend: &str,
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        input.display(),
        size,
        mtime_secs,
        model,
        scale,
        backend
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> String {
        fingerprint(Path::new("/v/in.mkv"), 1234, 999, "rrdb-x2", 2, "gpu")
    }

    fn parts(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("/v/out.part{i:03}.mkv"))).collect()
    }

    #[test]
    fn a_checkpoint_round_trips() {
        let mut c = Checkpoint { fingerprint: fp(), segments: 4, ..Default::default() };
        c.done.insert(0);
        c.done.insert(2);
        assert_eq!(Checkpoint::parse(&c.render()), c);
    }

    #[test]
    fn a_checkpoint_for_a_different_job_is_refused() {
        // Resuming into a different job is worse than not resuming: the parts
        // concatenate, the length is right, and half the film was upscaled with
        // the other model.
        let c = Checkpoint { fingerprint: fp(), segments: 4, done: [0, 1].into() };
        assert!(c.matches(&fp(), 4));
        let other_model = fingerprint(Path::new("/v/in.mkv"), 1234, 999, "srvgg", 2, "gpu");
        assert!(!c.matches(&other_model, 4), "a different model was accepted");
        let edited = fingerprint(Path::new("/v/in.mkv"), 1234, 1000, "rrdb-x2", 2, "gpu");
        assert!(!c.matches(&edited, 4), "a re-encoded source was accepted");
        let other_scale = fingerprint(Path::new("/v/in.mkv"), 1234, 999, "rrdb-x2", 4, "gpu");
        assert!(!c.matches(&other_scale, 4), "a different scale was accepted");
    }

    #[test]
    fn a_different_segment_count_is_refused_even_with_everything_else_equal() {
        // `-j 8` instead of `-j 4` produces parts covering different time
        // ranges under the same names — the one input a user changes casually.
        let c = Checkpoint { fingerprint: fp(), segments: 4, done: [0].into() };
        assert!(!c.matches(&fp(), 8));
    }

    #[test]
    fn an_empty_checkpoint_never_matches() {
        // A zero-length or garbage file must not read as "everything is done".
        let c = Checkpoint::parse("");
        assert!(!c.matches(&fp(), 4));
        assert!(!c.matches("", 0), "an empty fingerprint matched an empty fingerprint");
    }

    #[test]
    fn resuming_needs_the_record_and_the_file() {
        // Either alone is wrong. The record alone trusts a part the user
        // deleted; the file alone trusts a truncated one left by a kill, which
        // concatenates into a video that is subtly short.
        let c = Checkpoint { fingerprint: fp(), segments: 4, done: [0, 1, 2].into() };
        let p = parts(4);
        let only_first_two = |q: &Path| q.ends_with("out.part000.mkv") || q.ends_with("out.part001.mkv");
        assert_eq!(c.resumable(&p, &only_first_two), [0, 1].into());
        let none = |_: &Path| false;
        assert!(c.resumable(&p, &none).is_empty(), "a missing part was reused");
        // And a part on disk that the record does not vouch for is redone.
        let all = |_: &Path| true;
        assert!(!c.resumable(&p, &all).contains(&3), "an unrecorded part was trusted");
    }

    #[test]
    fn a_corrupt_line_costs_only_itself() {
        // A kill mid-write leaves a half-line. Refusing the file would redo
        // every segment; skipping the line redoes one.
        let c = Checkpoint::parse(
            "# comment\nfingerprint=abc\nsegments=3\ndone=0,notanumber,2\ngarbage\n",
        );
        assert_eq!(c.fingerprint, "abc");
        assert_eq!(c.segments, 3);
        assert_eq!(c.done, [0, 2].into());
    }

    #[test]
    fn the_sidecar_sits_beside_the_output() {
        assert_eq!(
            path_for(Path::new("/v/movie.mkv")),
            PathBuf::from("/v/movie.d2xresume")
        );
    }
}
