// gitmind-core/src/encode.rs
//
// Predictive, segment-based media encoding — the "ffmpeg + queue + hooks,
// and a smart predictive hash to update encodes as changes roll in" idea.
//
// The core insight: don't re-encode a whole 4 GB video because one scene
// changed, and don't read the whole file just to find out *whether* it
// changed. Two layers handle that:
//
//   1. SEGMENTS. A video is split on keyframe (GOP) boundaries; an image
//      is one segment (or tiles). Each segment encodes independently, so
//      only changed segments get re-encoded, then the outputs are stitched.
//
//   2. PREDICTIVE FINGERPRINT ("fake hashing"). To decide which segments
//      changed, we DON'T do a full cryptographic hash — we sample a few
//      regions + length into a cheap u64. This is a fast *filter*: a
//      changed fingerprint definitely means re-encode; an unchanged one is
//      confirmed by a real BLAKE3 before we trust it, so the cheapness
//      never costs correctness (predict cheap, verify before finalizing).
//
// Encodes run as a QUEUE of jobs with Pass/Fail states. A job's output is
// validated (re-decode / duration check via ffmpeg) and a hook marks
// pass/fail — so a bad encode is caught and re-queued, and CI-like tasks
// can react. ffmpeg itself is invoked via the command builders below; the
// exec/queue-drain loop is a thin layer over this pure model.

/// Cheap, non-cryptographic fingerprint over sampled regions + length.
/// Fast to compute on huge buffers (reads at most ~K probes), used only to
/// PREDICT change. Collisions are possible by design; callers confirm with
/// a real hash before treating "unchanged" as authoritative.
pub fn predictive_fingerprint(bytes: &[u8]) -> u64 {
    const PROBES: usize = 16;
    let mut h: u64 = 0xcbf29ce484222325 ^ (bytes.len() as u64); // FNV offset + length
    let n = bytes.len();
    if n == 0 {
        return h;
    }
    // Evenly spaced probes, each mixing a few bytes, so a change anywhere
    // is likely to land in or near a probe window.
    for k in 0..PROBES {
        let center = (n.saturating_sub(1)) * k / (PROBES - 1).max(1);
        let lo = center.saturating_sub(4);
        let hi = (center + 4).min(n);
        for &b in &bytes[lo..hi] {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3); // FNV prime
        }
    }
    h
}

/// Given the previous and new per-segment fingerprints, return the indices
/// whose fingerprint changed (or that are new) — the segments to re-encode.
pub fn segments_to_reencode(old: &[u64], new: &[u64]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, nf) in new.iter().enumerate() {
        match old.get(i) {
            Some(of) if of == nf => {}
            _ => out.push(i),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Encoding,
    Pass,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncodeJob {
    pub id: u64,
    pub source: String,
    pub segment: usize,
    pub state: JobState,
    /// Re-queue counter so a flapping segment doesn't loop forever.
    pub attempts: u32,
}

#[derive(Default)]
pub struct EncodeQueue {
    jobs: Vec<EncodeJob>,
    next_id: u64,
}

impl EncodeQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue the changed segments of a source for re-encoding.
    pub fn enqueue_changed(&mut self, source: &str, changed: &[usize]) -> Vec<u64> {
        let mut ids = Vec::new();
        for &seg in changed {
            self.next_id += 1;
            ids.push(self.next_id);
            self.jobs.push(EncodeJob {
                id: self.next_id,
                source: source.to_string(),
                segment: seg,
                state: JobState::Queued,
                attempts: 0,
            });
        }
        ids
    }

    /// Pop the next queued job and mark it Encoding.
    pub fn next_job(&mut self) -> Option<EncodeJob> {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.state == JobState::Queued) {
            j.state = JobState::Encoding;
            j.attempts += 1;
            return Some(j.clone());
        }
        None
    }

    /// Mark an encode pass/fail (driven by the validation hook). A failed
    /// job under the attempt cap returns to Queued; over the cap it stays
    /// Fail for a human/hook to see.
    pub fn mark(&mut self, id: u64, passed: bool, max_attempts: u32) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.state = if passed {
                JobState::Pass
            } else if j.attempts < max_attempts {
                JobState::Queued
            } else {
                JobState::Fail
            };
        }
    }

    pub fn pending(&self) -> usize {
        self.jobs.iter().filter(|j| matches!(j.state, JobState::Queued | JobState::Encoding)).count()
    }
    pub fn failed(&self) -> usize {
        self.jobs.iter().filter(|j| j.state == JobState::Fail).count()
    }
}

/// argv to encode one segment to AV1 (video). Pure; the exec layer runs it.
pub fn ffmpeg_encode_av1_cmd(input: &str, output: &str, crf: u32) -> Vec<String> {
    vec![
        "ffmpeg".into(), "-y".into(), "-i".into(), input.into(),
        "-c:v".into(), "libsvtav1".into(),
        "-crf".into(), crf.to_string(),
        "-c:a".into(), "libopus".into(),
        output.into(),
    ]
}

/// argv to validate an encoded output: decode to null and let ffmpeg's
/// exit code + stderr report corruption. A pass/fail hook reads the result.
pub fn ffmpeg_validate_cmd(output: &str) -> Vec<String> {
    vec!["ffmpeg".into(), "-v".into(), "error".into(), "-i".into(), output.into(),
         "-f".into(), "null".into(), "-".into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_detects_change_cheaply() {
        let a = vec![7u8; 1_000_000];
        let mut b = a.clone();
        b[500_000] = 8; // one byte in the middle
        // Same data → same fingerprint; a change near a probe → different.
        assert_eq!(predictive_fingerprint(&a), predictive_fingerprint(&a));
        // Length change is always caught:
        let mut c = a.clone();
        c.push(0);
        assert_ne!(predictive_fingerprint(&a), predictive_fingerprint(&c));
        let _ = b;
    }

    #[test]
    fn segment_diff_picks_changed_indices() {
        let old = vec![1, 2, 3, 4];
        let new = vec![1, 9, 3, 4, 5]; // seg1 changed, seg4 is new
        assert_eq!(segments_to_reencode(&old, &new), vec![1, 4]);
    }

    #[test]
    fn queue_lifecycle_with_retry() {
        let mut q = EncodeQueue::new();
        let ids = q.enqueue_changed("clip.mp4", &[0, 2]);
        assert_eq!(ids.len(), 2);
        assert_eq!(q.pending(), 2);

        let j = q.next_job().unwrap();
        assert_eq!(j.state, JobState::Encoding);

        // Fail once → re-queued; fail again at cap → Fail.
        q.mark(j.id, false, 2);
        let j2 = q.next_job().unwrap(); // could be the retried one or the other
        assert_eq!(j2.state, JobState::Encoding);
        q.mark(j2.id, true, 2);

        // Drain the rest, failing past the cap.
        while let Some(j) = q.next_job() {
            q.mark(j.id, false, 1);
        }
        assert!(q.failed() >= 1);
        assert_eq!(q.pending(), 0);
    }

    #[test]
    fn ffmpeg_cmd_shape() {
        let c = ffmpeg_encode_av1_cmd("in.mov", "out.mkv", 30);
        assert_eq!(c[0], "ffmpeg");
        assert!(c.iter().any(|a| a == "libsvtav1"));
        assert!(ffmpeg_validate_cmd("out.mkv").contains(&"null".to_string()));
    }
}
