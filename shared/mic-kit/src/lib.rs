//! mic-kit — shared, secured microphone access with graceful realtime fallback.
//!
//! Microphones become a shared resource across apps (bo-suite's multi-mic, logscope voice
//! commands, voiceterm), but access is **gated by user profile** and streams are **encrypted**
//! so one profile can't listen through another's grant. Crucially, a mic is a *step-up, not a
//! guarantee*: the [`AudioInput`] contract is realtime-safe and **always yields a frame**
//! (silence at worst), so when a device drops, a profile stops matching, or a mic is busy, the
//! audio path degrades gracefully in realtime instead of stalling or erroring.
//!
//! Two halves: the **broker** (profiles → policy → encrypted grants) and the **fallback chain**
//! (never-blocking input/output that always produces something).

use std::collections::{HashMap, HashSet};

// ── identities + policy ──────────────────────────────────────────────────────

/// A device user profile (identity that mic access is gated on).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProfileId(pub String);

/// A microphone identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MicId(pub String);

/// Which profiles may use which mics. Default is deny — a mic must be explicitly allowed.
#[derive(Debug, Clone, Default)]
pub struct MicPolicy {
    allowed: HashMap<MicId, HashSet<ProfileId>>,
}

impl MicPolicy {
    pub fn new() -> MicPolicy {
        MicPolicy::default()
    }
    /// Permit `profile` to access `mic`.
    pub fn allow(&mut self, mic: MicId, profile: ProfileId) {
        self.allowed.entry(mic).or_default().insert(profile);
    }
    /// Does the policy permit this profile on this mic?
    pub fn permits(&self, profile: &ProfileId, mic: &MicId) -> bool {
        self.allowed.get(mic).is_some_and(|s| s.contains(profile))
    }
    /// Profiles allowed on a mic.
    pub fn profiles_for(&self, mic: &MicId) -> Vec<ProfileId> {
        self.allowed.get(mic).map(|s| s.iter().cloned().collect()).unwrap_or_default()
    }
}

/// Why a mic request was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// The profile isn't permitted on this mic.
    NotPermitted,
    /// The mic isn't present on the device right now.
    Unavailable,
}

/// A granted, profile-scoped, encrypted handle to a mic. Frames are sealed with a key derived
/// from the profile's secret + the mic, so only the matching profile can open them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicGrant {
    pub mic: MicId,
    pub profile: ProfileId,
    key: [u8; crypto_kit::KEY_LEN],
}

impl MicGrant {
    /// Encrypt one frame of samples for transport/storage.
    pub fn seal_frame(&self, samples: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        crypto_kit::seal(&self.key, &bytes)
    }
    /// Decrypt a frame. `None` if the ciphertext is corrupt or the key doesn't match.
    pub fn open_frame(&self, sealed: &[u8]) -> Option<Vec<f32>> {
        let bytes = crypto_kit::open(&self.key, sealed)?;
        Some(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
}

/// Grants and tracks shared mic access. Multiple profiles can share one mic (each with its own
/// encrypted grant); requests that don't match policy are denied.
pub struct MicBroker {
    policy: MicPolicy,
    present: HashSet<MicId>,
    active: Vec<(MicId, ProfileId)>,
}

impl MicBroker {
    pub fn new(policy: MicPolicy) -> MicBroker {
        MicBroker { policy, present: HashSet::new(), active: Vec::new() }
    }

    /// Update which mics are physically present (hotplug). Grants for now-absent mics should be
    /// treated as dead by their [`AudioInput`] (which will fall back).
    pub fn set_present(&mut self, mics: impl IntoIterator<Item = MicId>) {
        self.present = mics.into_iter().collect();
    }

    /// Request access to `mic` as `profile`, providing the profile's secret (used to derive the
    /// per-grant encryption key). Grants only when the policy permits and the mic is present.
    pub fn request(&mut self, profile: &ProfileId, mic: &MicId, profile_secret: &[u8]) -> Result<MicGrant, Denied> {
        if !self.policy.permits(profile, mic) {
            return Err(Denied::NotPermitted);
        }
        if !self.present.contains(mic) {
            return Err(Denied::Unavailable);
        }
        let key = *crypto_kit::derive_session_key(profile_secret, mic.0.as_bytes());
        self.active.push((mic.clone(), profile.clone()));
        Ok(MicGrant { mic: mic.clone(), profile: profile.clone(), key })
    }

    /// Release a grant (a profile stops using a mic).
    pub fn release(&mut self, grant: &MicGrant) {
        self.active.retain(|(m, p)| !(m == &grant.mic && p == &grant.profile));
    }

    /// Profiles currently sharing a mic.
    pub fn sharers(&self, mic: &MicId) -> Vec<ProfileId> {
        self.active.iter().filter(|(m, _)| m == mic).map(|(_, p)| p.clone()).collect()
    }
}

// ── graceful realtime fallback (input) ───────────────────────────────────────

/// The status of a realtime read: was it the live source, a fallback, or silence?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStatus {
    Live,
    Fallback,
    Silent,
}

/// Realtime audio input. **Contract:** `read` must never block or allocate — it fills `out`
/// (with silence if it has nothing) and returns promptly. This is what makes a mic a step-up
/// rather than a hard dependency.
pub trait AudioInput {
    /// Fill `out` with the next frame; return what was actually produced.
    fn read(&mut self, out: &mut [f32]) -> InputStatus;
    /// Is this source currently able to produce live audio?
    fn is_live(&self) -> bool;
}

/// The ultimate fallback: always silence, always available.
pub struct Silence;

impl AudioInput for Silence {
    fn read(&mut self, out: &mut [f32]) -> InputStatus {
        out.iter_mut().for_each(|s| *s = 0.0);
        InputStatus::Silent
    }
    fn is_live(&self) -> bool {
        false
    }
}

/// Try sources in priority order and always yield a frame: the first live source wins; if none
/// are live, fill silence. Never blocks, never errors — the graceful realtime fallback for mic
/// input. Source 0 producing → `Live`; a later source producing → `Fallback`; none → `Silent`.
pub struct FallbackChain {
    sources: Vec<Box<dyn AudioInput + Send>>,
}

impl FallbackChain {
    pub fn new(sources: Vec<Box<dyn AudioInput + Send>>) -> FallbackChain {
        FallbackChain { sources }
    }

    /// Number of sources in the chain.
    pub fn len(&self) -> usize {
        self.sources.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

impl AudioInput for FallbackChain {
    fn read(&mut self, out: &mut [f32]) -> InputStatus {
        for (i, src) in self.sources.iter_mut().enumerate() {
            if src.is_live() {
                let st = src.read(out);
                if st != InputStatus::Silent {
                    return if i == 0 { InputStatus::Live } else { InputStatus::Fallback };
                }
            }
        }
        out.iter_mut().for_each(|s| *s = 0.0);
        InputStatus::Silent
    }
    fn is_live(&self) -> bool {
        self.sources.iter().any(|s| s.is_live())
    }
}

// ── graceful realtime fallback (output) ──────────────────────────────────────

/// Realtime audio output. **Contract:** `write` must never block; if the sink is gone it drops
/// the frame and reports so, so playback degrades gracefully rather than stalling.
pub trait AudioOutput {
    /// Write a frame; return whether it was actually delivered to a live sink.
    fn write(&mut self, frame: &[f32]) -> bool;
    fn is_live(&self) -> bool;
}

/// Discards everything (the output fallback when no sink is available).
pub struct NullSink;

impl AudioOutput for NullSink {
    fn write(&mut self, _frame: &[f32]) -> bool {
        false
    }
    fn is_live(&self) -> bool {
        false
    }
}

/// Write to the first live sink; if none, drop the frame (report `false`). The output analog of
/// [`FallbackChain`].
pub struct OutputFallback {
    sinks: Vec<Box<dyn AudioOutput + Send>>,
}

impl OutputFallback {
    pub fn new(sinks: Vec<Box<dyn AudioOutput + Send>>) -> OutputFallback {
        OutputFallback { sinks }
    }
}

impl AudioOutput for OutputFallback {
    fn write(&mut self, frame: &[f32]) -> bool {
        for sink in &mut self.sinks {
            if sink.is_live() && sink.write(frame) {
                return true;
            }
        }
        false // dropped gracefully
    }
    fn is_live(&self) -> bool {
        self.sinks.iter().any(|s| s.is_live())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(s: &str) -> ProfileId {
        ProfileId(s.into())
    }
    fn mic(s: &str) -> MicId {
        MicId(s.into())
    }

    /// A test source that yields a constant while "live"; can be toggled dead (device drop).
    struct Const {
        val: f32,
        live: bool,
    }
    impl AudioInput for Const {
        fn read(&mut self, out: &mut [f32]) -> InputStatus {
            if !self.live {
                out.iter_mut().for_each(|s| *s = 0.0);
                return InputStatus::Silent;
            }
            out.iter_mut().for_each(|s| *s = self.val);
            InputStatus::Live
        }
        fn is_live(&self) -> bool {
            self.live
        }
    }

    #[test]
    fn policy_gates_by_profile() {
        let mut p = MicPolicy::new();
        p.allow(mic("built-in"), prof("alice"));
        assert!(p.permits(&prof("alice"), &mic("built-in")));
        assert!(!p.permits(&prof("bob"), &mic("built-in")));
        assert!(!p.permits(&prof("alice"), &mic("usb")));
    }

    #[test]
    fn broker_grants_only_matching_present_mics() {
        let mut policy = MicPolicy::new();
        policy.allow(mic("m1"), prof("alice"));
        let mut b = MicBroker::new(policy);
        // mic not present yet
        assert_eq!(b.request(&prof("alice"), &mic("m1"), b"secret"), Err(Denied::Unavailable));
        b.set_present([mic("m1")]);
        // wrong profile
        assert_eq!(b.request(&prof("bob"), &mic("m1"), b"secret"), Err(Denied::NotPermitted));
        // permitted + present → granted
        let grant = b.request(&prof("alice"), &mic("m1"), b"secret").unwrap();
        assert_eq!(grant.profile, prof("alice"));
        assert_eq!(b.sharers(&mic("m1")), vec![prof("alice")]);
        b.release(&grant);
        assert!(b.sharers(&mic("m1")).is_empty());
    }

    #[test]
    fn grant_seals_and_opens_frames_only_with_matching_key() {
        let mut policy = MicPolicy::new();
        policy.allow(mic("m1"), prof("alice"));
        let mut b = MicBroker::new(policy);
        b.set_present([mic("m1")]);
        let grant = b.request(&prof("alice"), &mic("m1"), b"alice-secret").unwrap();
        let frame = vec![0.1f32, -0.2, 0.3];
        let sealed = grant.seal_frame(&frame);
        let opened = grant.open_frame(&sealed).unwrap();
        for (a, b) in frame.iter().zip(opened.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
        // a different profile-secret derives a different key → can't open
        let mut b2 = MicBroker::new({
            let mut p = MicPolicy::new();
            p.allow(mic("m1"), prof("mallory"));
            p
        });
        b2.set_present([mic("m1")]);
        let other = b2.request(&prof("mallory"), &mic("m1"), b"mallory-secret").unwrap();
        assert!(other.open_frame(&sealed).is_none(), "wrong profile can't decrypt");
    }

    #[test]
    fn fallback_chain_prefers_live_then_degrades_to_silence() {
        let mut buf = [9.9f32; 4];
        // primary live → Live, primary's value
        let mut chain = FallbackChain::new(vec![
            Box::new(Const { val: 1.0, live: true }),
            Box::new(Const { val: 2.0, live: true }),
        ]);
        assert_eq!(chain.read(&mut buf), InputStatus::Live);
        assert_eq!(buf, [1.0; 4]);
        // primary dead → Fallback, secondary's value
        let mut chain = FallbackChain::new(vec![
            Box::new(Const { val: 1.0, live: false }),
            Box::new(Const { val: 2.0, live: true }),
        ]);
        assert_eq!(chain.read(&mut buf), InputStatus::Fallback);
        assert_eq!(buf, [2.0; 4]);
        // all dead → Silent, zeros (never blocks/panics)
        let mut chain = FallbackChain::new(vec![
            Box::new(Const { val: 1.0, live: false }),
            Box::new(Silence),
        ]);
        assert_eq!(chain.read(&mut buf), InputStatus::Silent);
        assert_eq!(buf, [0.0; 4]);
    }

    #[test]
    fn output_fallback_drops_gracefully_when_no_sink() {
        let mut out = OutputFallback::new(vec![Box::new(NullSink), Box::new(NullSink)]);
        assert!(!out.write(&[0.1, 0.2]), "no live sink → frame dropped, not an error");
        assert!(!out.is_live());
    }
}
