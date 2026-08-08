//! remoteworkspace.rs — a build result that can be attributed to the source that produced it.
//!
//! `deviceplan` says which machine may take a build check. Sending the work is the easy half; the
//! hard half is knowing what came back.
//!
//! ## The failure that matters
//!
//! **A remote that builds a stale copy reports a green for code you did not send.** Every other
//! problem here announces itself — a transfer fails loudly, a missing toolchain errors — but a
//! successful build of the wrong source looks exactly like success, and this session has spent
//! itself on results that looked like success.
//!
//! So a result carries the hash of the source that produced it, and a result whose hash does not
//! match what was sent is **not a failure, it is unattributable**, which is a different thing and
//! wants a different response.
//!
//! ## Why /tmp is right and also a trap
//!
//! Cheap, per-session, cleaned by the OS. And shared: two sessions targeting the same machine will
//! collide unless the path carries the content hash, which it does here for exactly that reason.

/// A source tree prepared for sending.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Payload {
    /// Content hash of everything sent. **The identity of the work**, not a checksum for transport —
    /// transport errors are loud and this is for the quiet failure.
    pub digest: String,
    pub bytes: u64,
    pub files: usize,
}

/// Where a payload lands on the remote.
///
/// Includes the digest, so two sessions building different trees on one machine cannot collide — and
/// so a stale directory from a previous run is a different path rather than a silent reuse.
pub fn workspace_path(digest: &str) -> String {
    format!("/tmp/logscope-ws-{}", &digest[..digest.len().min(16)])
}

/// What a remote reported.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RemoteResult {
    /// The digest the remote says it built. **Reported by the remote, not assumed by us.**
    pub built_digest: String,
    pub device: String,
    pub target_triple: String,
    pub succeeded: bool,
    /// Whether the remote executed anything, or only compiled.
    pub executed: bool,
}

/// How a result may be used.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Attribution {
    /// It built what we sent, and ran it.
    Verified { device: String, triple: String },
    /// It built what we sent and could not run it. **Weaker than it looks**: a cross-compile catches
    /// type errors and misses alignment, endianness and which syscalls exist.
    CompiledOnly { device: String, triple: String },
    /// The build failed, on the source we sent.
    Failed { device: String },
    /// **The dangerous case.** The remote built something else, so its verdict — pass or fail — is
    /// about code we do not have.
    Unattributable { sent: String, built: String },
}

impl Attribution {
    /// May this result be treated as evidence about the code we sent?
    pub fn is_evidence(&self) -> bool {
        !matches!(self, Attribution::Unattributable { .. })
    }

    pub fn explain(&self) -> String {
        match self {
            Attribution::Verified { device, triple } => format!(
                "built and RAN on {device} ({triple}) — the strongest result available, because the \
target executed it"),
            Attribution::CompiledOnly { device, triple } => format!(
                "compiled on {device} for {triple} but not run. Weaker than it looks: this catches \
type errors and misses alignment, endianness and which syscalls exist"),
            Attribution::Failed { device } => format!("failed on {device}, on the source we sent"),
            Attribution::Unattributable { sent, built } => format!(
                "the remote reports building {built} and we sent {sent}. Its verdict is about code we \
do not have — this is NOT a failure, it is an unattributable result, and treating it as either a \
pass or a fail would be inventing a fact about our tree"),
        }
    }
}

/// Attribute a result to the payload that was sent.
pub fn attribute(sent: &Payload, r: &RemoteResult) -> Attribution {
    // Checked FIRST, before success. A green from the wrong source is the failure this exists to
    // catch, and checking success first would let it through on the happy path — which is exactly
    // where nobody looks.
    if r.built_digest != sent.digest {
        return Attribution::Unattributable {
            sent: sent.digest.clone(), built: r.built_digest.clone() };
    }
    if !r.succeeded { return Attribution::Failed { device: r.device.clone() } }
    if r.executed {
        Attribution::Verified { device: r.device.clone(), triple: r.target_triple.clone() }
    } else {
        Attribution::CompiledOnly { device: r.device.clone(), triple: r.target_triple.clone() }
    }
}

/// Summarise several remotes' results.
///
/// **Unattributable results are counted separately and never as passes or failures**, because
/// folding them into either number would make a fleet look healthier or sicker than it is on
/// evidence that does not exist.
pub fn summarise(results: &[Attribution]) -> String {
    let verified = results.iter().filter(|a| matches!(a, Attribution::Verified { .. })).count();
    let compiled = results.iter().filter(|a| matches!(a, Attribution::CompiledOnly { .. })).count();
    let failed = results.iter().filter(|a| matches!(a, Attribution::Failed { .. })).count();
    let bad = results.iter().filter(|a| !a.is_evidence()).count();
    let mut s = format!("{verified} verified, {compiled} compiled-only, {failed} failed");
    if bad > 0 {
        s.push_str(&format!(", {bad} UNATTRIBUTABLE — those remotes built something other than what \
was sent, so they are neither passes nor failures and are excluded from both counts"));
    }
    if verified == 0 && compiled > 0 {
        s.push_str(". Nothing was RUN, so nothing here is evidence about runtime behaviour on the \
target");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(d: &str) -> Payload { Payload { digest: d.into(), bytes: 4096, files: 12 } }
    fn res(d: &str, ok: bool, exec: bool) -> RemoteResult {
        RemoteResult { built_digest: d.into(), device: "pi".into(),
                       target_triple: "aarch64-unknown-linux-gnu".into(),
                       succeeded: ok, executed: exec }
    }

    #[test]
    fn a_STALE_remote_is_unattributable_rather_than_a_pass() {
        // The failure that matters: every other problem announces itself, and a successful build of
        // the wrong source looks exactly like success.
        let a = attribute(&payload("abc123"), &res("old999", true, true));
        assert!(!a.is_evidence());
        assert!(a.explain().contains("about code we do not have"));
    }

    #[test]
    fn the_DIGEST_is_checked_before_SUCCESS() {
        // Checking success first would let a stale green through on the happy path, which is exactly
        // where nobody looks.
        let stale_pass = attribute(&payload("abc"), &res("xyz", true, true));
        let stale_fail = attribute(&payload("abc"), &res("xyz", false, false));
        assert!(!stale_pass.is_evidence());
        assert!(!stale_fail.is_evidence(), "a stale FAILURE is equally uninformative");
    }

    #[test]
    fn COMPILED_ONLY_is_marked_weaker_than_it_looks() {
        // A cross-compile catches type errors and misses alignment, endianness and syscalls. Calling
        // it a pass is how a green build becomes a claim about runtime.
        let a = attribute(&payload("abc"), &res("abc", true, false));
        assert!(a.is_evidence());
        assert!(a.explain().contains("misses alignment, endianness"));
    }

    #[test]
    fn a_REAL_failure_is_attributed_to_the_device_that_saw_it() {
        assert_eq!(attribute(&payload("abc"), &res("abc", false, false)),
                   Attribution::Failed { device: "pi".into() });
    }

    #[test]
    fn UNATTRIBUTABLE_results_are_excluded_from_BOTH_counts() {
        // Folding them into either number would make the fleet look healthier or sicker than it is,
        // on evidence that does not exist.
        let s = summarise(&[
            attribute(&payload("abc"), &res("abc", true, true)),
            attribute(&payload("abc"), &res("stale", true, true)),
        ]);
        assert!(s.contains("1 verified"));
        assert!(s.contains("1 UNATTRIBUTABLE"));
        assert!(s.contains("neither passes nor failures"));
    }

    #[test]
    fn a_fleet_that_only_COMPILED_says_nothing_ran() {
        // Otherwise "3 compiled-only" reads as three passes.
        let s = summarise(&[attribute(&payload("a"), &res("a", true, false))]);
        assert!(s.contains("nothing here is evidence about runtime behaviour"));
    }

    #[test]
    fn the_workspace_PATH_carries_the_digest_so_sessions_cannot_collide() {
        // /tmp is shared: two sessions targeting one machine would otherwise overwrite each other,
        // and a stale directory would be silently reused.
        assert_ne!(workspace_path("aaaa1111bbbb2222"), workspace_path("cccc3333dddd4444"));
        assert!(workspace_path("aaaa1111bbbb2222").starts_with("/tmp/logscope-ws-"));
    }

    #[test]
    fn a_SHORT_digest_does_not_panic_the_path_builder() {
        // Truncation on a hash shorter than the slice would panic, and a crash while preparing work
        // is a worse failure than any it prevents.
        assert!(!workspace_path("ab").is_empty());
    }
}
