// src/speakers.rs
//
// Multi-user speaker identification. v0.1.116 foundations for the
// collaboration mode the user described: multiple speakers shown
// as circles on screen, each pulsing when their voice is detected,
// supporting expert/learner handoffs during paired sessions.
//
// Design:
//
// - **Speaker registry**: a small list of registered speakers, each
//   with name, color (for UI), and reference TonalProfile (averaged
//   from confirmed samples). Persisted to disk so the registry
//   survives restarts.
//
// - **Identification**: given an utterance's TonalProfile, scan
//   the registry and return (best_match_name, similarity_score).
//   Similarity uses the existing TonalProfile::similarity which
//   weights pitch + activity. Threshold 0.65 for "this is them";
//   below = unknown speaker.
//
// - **Registration**: `add_speaker(name, profile)` adds or updates.
//   For accuracy we average across multiple samples — `update_speaker`
//   blends new profiles into the existing one. After ~5 samples the
//   profile stabilizes.
//
// - **Detection mode**: foreground voice path can call `identify()`
//   to know who's speaking; the QML overlay reads this via a
//   qt_method polled every ~200ms during recording.
//
// What's NOT in v0.1.116:
//
// - Speaker-specific command palettes / permissions. Currently all
//   speakers have the same command access. Future work: per-speaker
//   privilege levels (read-only mode for learners?).
// - Cross-session continuity: each session starts fresh detection
//   from the persisted registry; we don't track which speaker said
//   what within a session.
// - Speaker diarization across an utterance. We treat each utterance
//   as ONE speaker (the one whose profile matches best). If two
//   people overlap in one VAD segment, we'd misidentify.
//
// Storage format:
//
//   ~/.config/web2qml/speakers.toml:
//
//     [[speaker]]
//     name = "Alice"
//     color = "#5599dd"
//     pitch_hz = 220.0
//     rms = 0.045
//     activity = 0.72
//     spectral_centroid_hz = 1850.0
//     voicing_prob = 0.68
//     energy_low = 0.18
//     energy_mid = 0.55
//     energy_high = 0.27
//     sample_count = 8
//
// Threading: state is a Mutex. All operations are short.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::phonetic::TonalProfile;

/// v0.1.185: process-local counter for speaker ID generation.
/// Combined with unix-ms-timestamp, gives readable monotonic IDs
/// like "spk_1778731373450_001" that don't collide within a
/// session even if multiple speakers register in the same ms.
/// Counter resets at process start; that's fine because the
/// timestamp part advances across restarts.
static ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a new speaker ID. Format: `spk_<unix_ms>_<seq3>`.
/// Stable, monotonic, human-readable, no external crate needed.
pub fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("spk_{}_{:03}", ms, seq)
}

/// v0.1.137: per-speaker rhythm/timing stats accumulated from
/// confirmed utterances. Pure data collection at this stage —
/// nothing in the identification or matching paths reads these
/// values yet. Exposed via `speaker_cadence` qt_method for
/// debug introspection.
///
/// Why we collect: a person's normal-volume speech and their
/// whispered speech have very different tonal profiles (pitch
/// flattens, formants shift) but their cadence is more stable
/// across volume. Eventually this can become a secondary signal
/// for identification when tonal similarity is borderline — but
/// only once we have enough data to know whether cadence values
/// actually distinguish people in real use (vs. being too noisy
/// to be useful).
///
/// Values:
/// - `syllables_per_sec`: rolling EMA. Computed per-utterance as
///   `word_count * 1.4 / utterance_duration_sec` (the 1.4 factor
///   is a rough English-syllables-per-word average; better than
///   nothing without language-specific phonemizers).
/// - `mean_pause_ms`: rolling EMA of average inter-word gap within
///   utterances. Within-utterance only; between-utterance pauses
///   would conflate with thinking time.
/// - `sample_count`: number of utterances rolled in. Used to weight
///   the EMA so early samples don't dominate.
/// - `last_updated_at`: unix ms of most recent update.
#[derive(Clone, Debug, Default)]
pub struct Cadence {
    pub syllables_per_sec: f32,
    pub mean_pause_ms: f32,
    pub sample_count: u32,
    pub last_updated_at: u64,
}

impl Cadence {
    /// Roll a new utterance's cadence values into the rolling EMA.
    /// Weight is min(sample_count, 20) so values stabilize after
    /// ~20 utterances but later samples still contribute.
    pub fn update(&mut self, new_syl: f32, new_pause_ms: f32) {
        let n = self.sample_count.min(20).max(1) as f32;
        // EMA: new = old * n/(n+1) + sample * 1/(n+1)
        let blend = 1.0 / (n + 1.0);
        if self.sample_count == 0 {
            self.syllables_per_sec = new_syl;
            self.mean_pause_ms = new_pause_ms;
        } else {
            self.syllables_per_sec = self.syllables_per_sec * (1.0 - blend) + new_syl * blend;
            self.mean_pause_ms = self.mean_pause_ms * (1.0 - blend) + new_pause_ms * blend;
        }
        self.sample_count += 1;
        self.last_updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
    }
}

pub struct Speaker {
    /// v0.1.185: stable identifier, separate from the (mutable)
    /// `name`. Generated on creation (`spk_<unix_ms>_<counter>`)
    /// and never changed. Future code that needs to reference
    /// a speaker durably — e.g. cards attributing content to a
    /// speaker — uses this, not name. Name is for display.
    ///
    /// Naming is mutable: rename "Me" → "Dad" without breaking
    /// any back-references. Internal lookups today are still
    /// largely name-based (legacy); migrating those to id-based
    /// is filed for later.
    ///
    /// Format chosen for readability when inspecting the toml:
    ///   "spk_1778731373450_001"
    /// 26-ish chars, monotonic, no crypto needed (we have at
    /// most ~5 speakers per device; collision essentially
    /// impossible). Future ships may swap this format for a
    /// public-key-derived identifier if cross-device signing
    /// becomes needed; the field semantics don't change.
    pub id: String,
    pub name: String,
    pub color: String,         // hex like "#5599dd"
    pub profile: TonalProfile,
    pub sample_count: u32,     // # of samples averaged into profile
    /// Whitelist flag. v0.1.116: when false, the voice path rejects
    /// utterances from this speaker. Default true (everyone allowed
    /// unless explicitly disabled). Useful for:
    ///   - Single-user noise rejection: register your own profile,
    ///     leave allowed=true; later when partner joins, register
    ///     them with allowed=false to silence side-channel chatter.
    ///   - Multi-user with role separation: expert allowed, observer
    ///     not allowed; observer's voice doesn't drive commands but
    ///     still appears in the live preview UI for context.
    pub allowed: bool,
    /// v0.1.133: when `allowed=false`, this further controls how
    /// the speaker's audio is handled.
    ///
    /// - `deep_muted = false` (default): "background mute". Audio
    ///   still gets routed and transcribed; appears italic + dim in
    ///   the background channel (v0.1.131). User sees what was said
    ///   but it doesn't drive commands.
    /// - `deep_muted = true`: "deep mute". Audio dropped at the
    ///   chunk-routing stage. No whisper inference, no transcript,
    ///   not even active-speaker pulse. Useful when the user wants
    ///   to ignore a speaker entirely (noisy colleague on a call).
    ///
    /// Has no effect when `allowed = true`. UI toggle: long-press
    /// the speaker circle.
    pub deep_muted: bool,
    /// v0.1.137: rolling cadence stats. Updated from confirmed
    /// (high-confidence identified) utterances only — see
    /// voice.rs result drain.
    pub cadence: Cadence,
}

#[derive(Default)]
struct Registry {
    speakers: Vec<Speaker>,
    loaded: bool,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

fn registry_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/web2qml/speakers.toml"))
}

/// Default color palette for new speakers (cycled by index).
///
/// v0.1.211 (ship 2.6.6): no longer the primary path for new speakers
/// — `color_from_profile` derives a color from the voice's pitch and
/// other tonal features. DEFAULT_COLORS is now the fallback when:
///   - the profile is empty / all zeros (pitch_hz == 0)
///   - we need a stable seed for cases where no profile is available
///     (e.g. import-from-other-device, or admin-edit pre-profile)
const DEFAULT_COLORS: &[&str] = &[
    "#5599dd", // blue
    "#e08030", // orange
    "#88c070", // green
    "#cc66cc", // purple
    "#dd8855", // copper
    "#66bbcc", // teal
];

/// v0.1.211 (ship 2.6.6): derive a stable per-speaker color from
/// their voice's tonal features. Replaces the round-robin
/// DEFAULT_COLORS assignment for new speakers.
///
/// v0.1.251: widened pitch range and added spectral centroid
/// modulation. Original v0.1.211 implementation used pitch
/// clamp 80-350Hz (matching human voice FUNDAMENTAL range).
/// But our ZCR-based pitch_hz detector in phonetic.rs counts
/// zero crossings divided by 2, which on real speech with
/// harmonics produces values much higher than f0 — typically
/// 400-1500Hz across speakers. Result: two distinct speakers
/// (e.g. Kessler @ ~680Hz, Allison @ ~975Hz) both clamped to
/// the 350Hz endpoint → same hue → both rendered the same red.
///
/// Hardware test in v0.1.249 confirmed this with two speakers
/// both showing as red dots. Fix:
///   1. Widened pitch range to 80-1500Hz to cover what our
///      detector actually produces.
///   2. Added spectral centroid modulation: ±60° hue rotation
///      based on centroid in 200-3500Hz range. This separates
///      two speakers with similar pitch via timbre.
///
/// Mapping rationale:
///   - **Base hue** ← pitch_hz. Pitch is the most-distinctive
///     single voice characteristic; mapping it to hue (the
///     dimension humans discriminate most easily) means voices
///     that sound different also LOOK different in the UI.
///     Range 80-1500 Hz mapped to 0..360°. Above 1500 wraps
///     back through red.
///   - **Hue offset** ← spectral_centroid_hz. Voices with the
///     same pitch but different timbre (e.g. brighter vs
///     darker voicing) get separated by ±60° rotation.
///     Centroid range 200-3500Hz mapped linearly to -60..+60°.
///   - **Saturation** ← voicing_prob. More clearly voiced
///     speech gets more saturated colors. Whispered or
///     borderline-noise samples get desaturated. Range 0.50
///     to 0.75; even unvoiced never goes fully gray (the
///     speaker still gets recognizable accent).
///   - **Lightness** ← fixed 0.55. Mid-tone, readable on
///     both dark and light themes without per-theme adjust.
///
/// Returns a hex string like "#5599dd". When the profile has
/// no signal (pitch_hz < 50 — silence or pre-profile state),
/// falls back to a deterministic color from DEFAULT_COLORS
/// keyed by a stable seed (passed by the caller, typically
/// the speaker count or id-hash). This keeps the function
/// pure / no-side-effect.
pub fn color_from_profile(profile: &TonalProfile, fallback_seed: usize) -> String {
    // Empty / no-signal profile: deterministic palette pick.
    // pitch < 50Hz is below human voice range — treat as missing.
    if profile.pitch_hz < 50.0 {
        return DEFAULT_COLORS[fallback_seed % DEFAULT_COLORS.len()].to_string();
    }
    // Pitch → base hue. Map 80..1500 Hz to 0..360°. Clamp
    // outside that range to the endpoints so extreme outliers
    // don't wrap into misleading colors. The widened range is
    // chosen to match our ZCR-based pitch_hz detector which
    // counts zero crossings (sensitive to harmonics), not the
    // fundamental — see comment on phonetic.rs:445.
    let p = profile.pitch_hz.clamp(80.0, 1500.0);
    let base_hue = (p - 80.0) / (1500.0 - 80.0) * 360.0;
    // Spectral centroid → hue offset. Centroid range 200-3500Hz
    // (covering low-energy speech up through bright/sibilant).
    // Map to -60..+60° rotation. This gives same-pitch speakers
    // with different timbre distinct colors. If centroid is 0
    // (default / unset profile field), offset is also 0.
    let centroid = profile.spectral_centroid_hz.clamp(0.0, 3500.0);
    let hue_offset = if centroid > 0.0 {
        let c_norm = (centroid - 200.0).max(0.0) / (3500.0 - 200.0);
        (c_norm * 120.0) - 60.0 // map [0..1] → [-60..+60]
    } else {
        0.0
    };
    let hue = (base_hue + hue_offset).rem_euclid(360.0);
    // Voicing → saturation. 0..1 input → 0.50..0.75 output.
    let v = profile.voicing_prob.clamp(0.0, 1.0);
    let sat = 0.50 + v * 0.25;
    // Fixed lightness keeps readability across themes.
    let light = 0.55;
    let (r, g, b) = hsl_to_rgb(hue, sat, light);
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

/// HSL → RGB conversion. Hue in degrees [0, 360); sat, light in
/// [0, 1]. Output bytes [0, 255]. Standard formula; no external
/// crate dependency.
fn hsl_to_rgb(h_deg: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = (h_deg / 60.0) % 6.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = if h_prime < 1.0 { (c, x, 0.0) }
        else if h_prime < 2.0 { (x, c, 0.0) }
        else if h_prime < 3.0 { (0.0, c, x) }
        else if h_prime < 4.0 { (0.0, x, c) }
        else if h_prime < 5.0 { (x, 0.0, c) }
        else { (c, 0.0, x) };
    let m = l - c / 2.0;
    let r = ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (r, g, b)
}

/// Add a new speaker or update an existing one. If `name` already
/// exists in the registry, the new profile is BLENDED into the
/// existing one (weighted by sample_count). Otherwise a new speaker
/// is created with a color picked from DEFAULT_COLORS.
///
/// Returns the registered speaker name.
pub fn add_speaker(name: &str, profile: TonalProfile) -> String {
    let name = name.trim().to_string();
    if name.is_empty() { return String::new(); }
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.name == name) {
            // Blend: weighted average. New sample contributes
            // 1/(sample_count+1) of the new profile.
            let n = s.sample_count as f32;
            let m = n + 1.0;
            s.profile.pitch_hz = (s.profile.pitch_hz * n + profile.pitch_hz) / m;
            s.profile.rms = (s.profile.rms * n + profile.rms) / m;
            s.profile.activity = (s.profile.activity * n + profile.activity) / m;
            s.profile.spectral_centroid_hz = (s.profile.spectral_centroid_hz * n
                + profile.spectral_centroid_hz) / m;
            s.profile.voicing_prob = (s.profile.voicing_prob * n + profile.voicing_prob) / m;
            s.profile.energy_low = (s.profile.energy_low * n + profile.energy_low) / m;
            s.profile.energy_mid = (s.profile.energy_mid * n + profile.energy_mid) / m;
            s.profile.energy_high = (s.profile.energy_high * n + profile.energy_high) / m;
            s.sample_count += 1;
            crate::clog!("speakers", "updated {:?} (n={})", name, s.sample_count);
        } else {
            // v0.1.211: derive color from the voice profile instead
            // of the prior round-robin index. fallback_seed is the
            // speaker count, so when the profile is missing (e.g.
            // pitch == 0 sentinel) we still get a stable color that
            // isn't all the same blue.
            let color = color_from_profile(&profile, r.speakers.len());
            r.speakers.push(Speaker {
                // v0.1.185: stable id at creation. Never changes
                // for the lifetime of this speaker entry.
                id: new_id(),
                name: name.clone(),
                color,
                profile,
                sample_count: 1,
                allowed: true,
                deep_muted: false,
                cadence: Cadence::default(),
            });
            crate::clog!("speakers", "registered {:?} (#{} total)", name, r.speakers.len());
        }
        drop(r);
        let _ = save();
    }
    name
}

/// v0.1.259: voice-profile dedup check. Returns the name of an
/// existing speaker whose profile matches `profile` above the
/// dedup threshold (0.85, higher than the 0.65 identify threshold
/// — we want HIGH confidence before suggesting a merge), or None.
///
/// QML's registration popup calls this BEFORE add_speaker to
/// detect the v0.1.251 hardware bug case: same person registering
/// twice with different name strings (e.g. "Allison" + "Allison
/// Martin"), creating duplicate entries pointing to the same
/// human voice.
///
/// The 0.85 threshold is intentionally well above 0.65 (the
/// identify threshold). The cost of a false positive (saying
/// "this is the same person" when it isn't) is a confusing UI
/// prompt; the cost of a false negative (missing a true dedup
/// case) is the duplicate-entry bug. We bias toward false
/// negatives by setting threshold higher; users with similar
/// voices won't trigger spurious merge prompts.
///
/// Excludes self-match: if `proposed_name` matches an existing
/// speaker exactly, return None (caller will fall through to
/// add_speaker which already handles the same-name update path
/// via blending).
pub fn propose_dedup(proposed_name: &str, profile: &TonalProfile) -> Option<(String, f32)> {
    let trimmed = proposed_name.trim();
    ensure_loaded();
    let r = registry().lock().ok()?;
    if r.speakers.is_empty() { return None; }
    let mut best: Option<(String, f32)> = None;
    for s in &r.speakers {
        // Skip exact name match (existing same-name update path).
        if s.name == trimmed { continue; }
        let sim = profile.similarity(&s.profile);
        match &best {
            None => best = Some((s.name.clone(), sim)),
            Some((_, b_sim)) if sim > *b_sim => best = Some((s.name.clone(), sim)),
            _ => {}
        }
    }
    match best {
        Some((name, sim)) if sim >= 0.85 => Some((name, sim)),
        _ => None,
    }
}

/// Identify which registered speaker matches the given profile.
///
/// Returns (name, similarity_score) for the best match, or None if
/// the registry is empty or no speaker scores above threshold (0.65).
///
/// The threshold balances:
///   - Too low → mis-identification when an unknown person speaks
///   - Too high → frequent unknown-speaker results for registered
///     users whose voice varies slightly day-to-day
///
/// 0.65 captures the same-speaker case based on pitch + activity
/// similarity. The TonalProfile::similarity function is the truth
/// source for this scoring.
pub fn identify(profile: &TonalProfile) -> Option<(String, f32)> {
    // Utterance-level acceptance threshold (0.65). The per-chunk separator calls
    // identify_at with a lower threshold for better recall on short noisy chunks.
    identify_at(profile, 0.65)
}

/// v0.1.855: core matcher with a caller-supplied acceptance threshold. The
/// ambiguity guard (winner must lead the runner-up by a clear margin) is applied
/// regardless of threshold — so a LOWER threshold improves recall without ever
/// merging two close speakers. identify() passes 0.65 (utterance-level); the
/// per-chunk separator passes 0.55 (chunk profiles are noisier, hysteresis then
/// stabilizes). This is the "best match regardless of the fixed 0.65 gate" the
/// separator wanted.
pub fn identify_at(profile: &TonalProfile, threshold: f32) -> Option<(String, f32)> {
    ensure_loaded();
    let r = registry().lock().ok()?;
    if r.speakers.is_empty() { return None; }
    // v0.1.496: track the TOP TWO matches, not just the best. The
    // coarse tonal similarity (pitch 70% + activity 30%) barely
    // separates different people with similar pitch — measured: two
    // different adult males at 110 vs 125 Hz score 0.90, while the
    // same person day-to-day scores ~0.97. A bare "best above threshold"
    // therefore SILENTLY MERGES different speakers (the reported
    // bug): person B's audio gets attributed to person A, corrupting
    // A's per-speaker learning. We can't fully fix discrimination
    // without the MFCC fingerprint (formants distinguish same-pitch
    // speakers) — that's a pipeline change — but we CAN refuse to
    // guess when the call is ambiguous.
    let mut best: Option<(String, f32)> = None;
    let mut second: Option<f32> = None;
    for s in &r.speakers {
        let sim = profile.similarity(&s.profile);
        match &best {
            None => best = Some((s.name.clone(), sim)),
            Some((_, b_sim)) if sim > *b_sim => {
                second = Some(*b_sim);
                best = Some((s.name.clone(), sim));
            }
            Some((_, b_sim)) => {
                // not a new best — is it a new second?
                if second.map_or(true, |sc| sim > sc) && sim <= *b_sim {
                    second = Some(sim);
                }
            }
        }
    }
    match best {
        Some((name, sim)) if sim >= threshold => {
            // Ambiguity guard: if a SECOND registered speaker also
            // scores close to the best, we can't confidently say which
            // — returning the "best" would be a coin-flip that risks a
            // wrong merge. Require the winner to lead the runner-up by
            // a clear margin; otherwise return None (caller treats as
            // unknown → prompts rather than mis-attributing).
            if let Some(sc) = second {
                const MARGIN: f32 = 0.08;
                if sim - sc < MARGIN {
                    crate::clog!("speakers",
                        "identify ambiguous: {:?} {:.3} vs runner-up {:.3} (margin<{}) → unknown",
                        name, sim, sc, MARGIN);
                    return None;
                }
            }
            Some((name, sim))
        }
        _ => None,
    }
}

/// Get the color associated with a registered speaker. Returns the
/// default-palette fallback if name not registered.
pub fn color_for(name: &str) -> String {
    ensure_loaded();
    if let Ok(r) = registry().lock() {
        if let Some(s) = r.speakers.iter().find(|s| s.name == name) {
            return s.color.clone();
        }
    }
    DEFAULT_COLORS[0].to_string()
}

/// Fast check: is the registry empty? Used as an early-exit on hot
/// paths so we don't do TonalProfile computation when no speakers
/// are registered (the very common case for solo users). v0.1.122.
pub fn is_empty() -> bool {
    ensure_loaded();
    registry().lock().map(|r| r.speakers.is_empty()).unwrap_or(true)
}

/// v0.1.288: number of registered speakers. Used by the voice
/// preview thread to gate per-speaker streaming. Single-speaker
/// sessions don't need per-speaker disambiguation (only one
/// person ever talking), so the per-speaker streaming whisper
/// pass is duplicate work vs the foreground voice_listen_dual
/// authoritative transcript. On mobile (single worker pool,
/// limited CPU), this is choppiness the user actually sees.
pub fn count() -> usize {
    ensure_loaded();
    registry().lock().map(|r| r.speakers.len()).unwrap_or(0)
}

/// List all registered speakers. Each entry: name + color (newline-
/// separated, fields tab-separated).
pub fn list() -> Vec<(String, String)> {
    ensure_loaded();
    registry().lock().ok()
        .map(|r| r.speakers.iter()
            .map(|s| (s.name.clone(), s.color.clone()))
            .collect())
        .unwrap_or_default()
}

/// v0.1.185: list speakers including their stable IDs. For callers
/// that need to reference a speaker durably (cards, captures, any
/// future code attaching data to a speaker over time).
pub fn list_with_ids() -> Vec<(String, String, String)> {
    ensure_loaded();
    registry().lock().ok()
        .map(|r| r.speakers.iter()
            .map(|s| (s.id.clone(), s.name.clone(), s.color.clone()))
            .collect())
        .unwrap_or_default()
}

/// v0.1.185: rename a speaker by id. Safe to call even when other
/// code has stored references to the old name — those references
/// can be reconciled via `lookup_id_by_name` or migrated to use
/// id directly. The speaker's identity (id) doesn't change; only
/// its display name does.
///
/// Returns true if a speaker with this id existed and was renamed.
pub fn rename_by_id(id: &str, new_name: &str) -> bool {
    let new_name = new_name.trim();
    if id.is_empty() || new_name.is_empty() { return false; }
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.id == id) {
            let old = s.name.clone();
            s.name = new_name.to_string();
            crate::clog!("speakers", "renamed id={} '{}' → '{}'", id, old, new_name);
            drop(r);
            let _ = save();
            return true;
        }
    }
    false
}

/// v0.1.262: merge two registered speakers into one. The source
/// speaker (`source_name`) is REMOVED from the registry after
/// its voice profile is blended into the target (`target_name`).
///
/// Used by the Settings → Speakers manual-merge UI to clean up
/// duplicate entries created before v0.1.259's dedup-on-register
/// (e.g. the Allison + Allison Martin pair in user's existing
/// registry).
///
/// Merge semantics:
///   - Voice profile: weighted average by sample_count (same as
///     add_speaker's blend path for same-name re-registration).
///   - Target's allowed and deep_muted flags WIN. User chose the
///     target; their settings stay. Source's allowed flag is
///     discarded. (This prevents the security hole of merging
///     a blocked speaker INTO an allowed one and inadvertently
///     unblocking them.)
///   - Target's color stays.
///   - Source is removed from registry.
///   - Source's stable id is no longer resolvable; audit log
///     entries that reference it will have `spk_id=<old_id>`
///     pointing nowhere — but the human-readable `spk=<Name>`
///     field is preserved in those entries already, so the
///     historical attribution remains legible.
///
/// Returns true if the merge succeeded, false if either speaker
/// name is unknown or if source == target.
pub fn merge_speakers(source_name: &str, target_name: &str) -> bool {
    let source = source_name.trim();
    let target = target_name.trim();
    if source.is_empty() || target.is_empty() { return false; }
    if source == target {
        crate::clog!("speakers", "merge: source == target {:?}, no-op", source);
        return false;
    }
    ensure_loaded();
    let mut r = match registry().lock() {
        Ok(r) => r,
        Err(_) => return false,
    };

    // Find both, extract source's profile + sample_count by
    // index so we can drain it after the blend.
    let source_idx = match r.speakers.iter().position(|s| s.name == source) {
        Some(i) => i,
        None => {
            crate::clog!("speakers", "merge: source {:?} not found", source);
            return false;
        }
    };
    let target_idx = match r.speakers.iter().position(|s| s.name == target) {
        Some(i) => i,
        None => {
            crate::clog!("speakers", "merge: target {:?} not found", target);
            return false;
        }
    };
    if source_idx == target_idx {
        // Shouldn't happen given source != target check, but
        // defensive.
        return false;
    }

    // Extract source data (clone what we need; we'll remove the
    // entry after blending into target).
    let src_profile = r.speakers[source_idx].profile;
    let src_samples = r.speakers[source_idx].sample_count;

    // Blend source into target. Weighted average by sample
    // counts, mirroring add_speaker's same-name blend logic.
    {
        let t = &mut r.speakers[target_idx];
        let s_count = src_samples as f32;
        let t_count = t.sample_count as f32;
        let total = (s_count + t_count).max(1.0);
        t.profile.pitch_hz = (t.profile.pitch_hz * t_count + src_profile.pitch_hz * s_count) / total;
        t.profile.rms = (t.profile.rms * t_count + src_profile.rms * s_count) / total;
        t.profile.activity = (t.profile.activity * t_count + src_profile.activity * s_count) / total;
        t.profile.spectral_centroid_hz = (t.profile.spectral_centroid_hz * t_count
            + src_profile.spectral_centroid_hz * s_count) / total;
        t.profile.voicing_prob = (t.profile.voicing_prob * t_count
            + src_profile.voicing_prob * s_count) / total;
        t.profile.energy_low = (t.profile.energy_low * t_count
            + src_profile.energy_low * s_count) / total;
        t.profile.energy_mid = (t.profile.energy_mid * t_count
            + src_profile.energy_mid * s_count) / total;
        t.profile.energy_high = (t.profile.energy_high * t_count
            + src_profile.energy_high * s_count) / total;
        t.sample_count = t.sample_count.saturating_add(src_samples);
    }

    // Remove source. After the swap_remove the indices may
    // shift, but we're done indexing.
    r.speakers.remove(source_idx);

    crate::clog!("speakers", "merged {:?} → {:?} (new sample_count={})",
        source, target, r.speakers.iter().find(|s| s.name == target)
            .map(|s| s.sample_count).unwrap_or(0));
    drop(r);
    let _ = save();
    true
}

/// v0.1.260: list all registered speaker names. Used by the
/// natural-language speaker-claim matcher (speaker_claim.rs)
/// to fuzzy-match a Whisper-transcribed claim against the
/// registry.
pub fn all_names() -> Vec<String> {
    ensure_loaded();
    registry().lock().ok()
        .map(|r| r.speakers.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

/// v0.1.185: find a speaker's id given their current name. Useful
/// for migrating callers that still hold name-keyed references.
pub fn lookup_id_by_name(name: &str) -> Option<String> {
    ensure_loaded();
    registry().lock().ok().and_then(|r| {
        r.speakers.iter().find(|s| s.name == name).map(|s| s.id.clone())
    })
}

/// v0.1.259: name-keyed rename wrapper. Looks up the id by current
/// name, then calls rename_by_id. Returns true on success, false
/// if no speaker has the given current name. Used by the
/// dedup-on-register flow (QML's registration popup), where the
/// caller has the name string from propose_dedup.
pub fn rename_by_name(current_name: &str, new_name: &str) -> bool {
    let id = match lookup_id_by_name(current_name) {
        Some(id) => id,
        None => {
            crate::clog!("speakers", "rename_by_name: no speaker named {:?}", current_name);
            return false;
        }
    };
    rename_by_id(&id, new_name)
}

/// v0.1.185: find a speaker's current name given their stable id.
/// Reverse of lookup_id_by_name. Returns None if the id doesn't
/// match any registered speaker.
pub fn name_for_id(id: &str) -> Option<String> {
    ensure_loaded();
    registry().lock().ok().and_then(|r| {
        r.speakers.iter().find(|s| s.id == id).map(|s| s.name.clone())
    })
}

/// Remove a speaker by name. Returns true on success.
pub fn remove_speaker(name: &str) -> bool {
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        let before = r.speakers.len();
        r.speakers.retain(|s| s.name != name);
        let removed = r.speakers.len() < before;
        if removed {
            drop(r);
            let _ = save();
        }
        return removed;
    }
    false
}

/// Is this speaker on the whitelist? Returns true for unknown names
/// (open-by-default — caller must use known names to gate strictly).
pub fn is_allowed(name: &str) -> bool {
    ensure_loaded();
    if name.is_empty() { return true; }
    if let Ok(r) = registry().lock() {
        if let Some(s) = r.speakers.iter().find(|s| s.name == name) {
            return s.allowed;
        }
    }
    true
}

/// Toggle whitelist status. Returns true if a speaker matched and
/// the change was saved.
pub fn set_allowed(name: &str, allowed: bool) -> bool {
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.name == name) {
            if s.allowed != allowed {
                s.allowed = allowed;
                drop(r);
                let _ = save();
                crate::clog!("speakers", "{:?} allowed={}", name, allowed);
            }
            return true;
        }
    }
    false
}

/// v0.1.133: read the deep_muted flag. Only meaningful when
/// `allowed=false` — for allowed speakers, deep_muted has no
/// effect on audio routing.
pub fn is_deep_muted(name: &str) -> bool {
    ensure_loaded();
    if name.is_empty() { return false; }
    if let Ok(r) = registry().lock() {
        if let Some(s) = r.speakers.iter().find(|s| s.name == name) {
            return s.deep_muted;
        }
    }
    false
}

/// v0.1.133: toggle deep_muted. Returns true on success.
/// Caller is responsible for any "allowed" state changes — this
/// only flips deep_muted independently. Typical use: when user
/// long-presses a muted speaker circle, flip deep_muted; if user
/// long-presses an allowed speaker, the UI first sets allowed=false
/// then deep_muted=true.
pub fn set_deep_muted(name: &str, deep_muted: bool) -> bool {
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.name == name) {
            if s.deep_muted != deep_muted {
                s.deep_muted = deep_muted;
                drop(r);
                let _ = save();
                crate::clog!("speakers", "{:?} deep_muted={}", name, deep_muted);
            }
            return true;
        }
    }
    false
}

/// v0.1.137: roll a new utterance's cadence values into the given
/// speaker's rolling stats. Caller computes the per-utterance values
/// from whisper output (word count, duration, inter-word gaps) and
/// passes them in. Caller should only invoke this when the speaker
/// was identified with reasonable confidence — otherwise misidentified
/// speech poisons the cadence average.
///
/// Returns true if speaker was found and updated. Saves to disk
/// after update (cadence persists across sessions for stability).
///
/// Note: save() acquires the registry lock independently, so we drop
/// our lock first to avoid contention.
pub fn update_cadence(name: &str, syllables_per_sec: f32, mean_pause_ms: f32) -> bool {
    if name.is_empty() { return false; }
    // Reject obviously wrong inputs that would poison the EMA:
    // negative or super-high syllable rates indicate measurement
    // glitches (clock skew, broken segment timestamps).
    if !syllables_per_sec.is_finite() || syllables_per_sec < 0.5 || syllables_per_sec > 20.0 {
        crate::clog!("cadence", "rejected update for {:?}: rate={:.2} out of [0.5, 20] range",
            name, syllables_per_sec);
        return false;
    }
    if !mean_pause_ms.is_finite() || mean_pause_ms < 0.0 || mean_pause_ms > 5000.0 {
        crate::clog!("cadence", "rejected update for {:?}: pause={:.0}ms out of [0, 5000] range",
            name, mean_pause_ms);
        return false;
    }
    ensure_loaded();
    let mut should_save = false;
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.name == name) {
            s.cadence.update(syllables_per_sec, mean_pause_ms);
            should_save = true;
        }
    }
    if should_save {
        let _ = save();
        return true;
    }
    false
}

/// v0.1.137: read a speaker's current cadence stats for debug/UI display.
/// Returns None if speaker not found OR if no samples have been rolled in yet.
pub fn cadence_for(name: &str) -> Option<Cadence> {
    ensure_loaded();
    if let Ok(r) = registry().lock() {
        if let Some(s) = r.speakers.iter().find(|s| s.name == name) {
            if s.cadence.sample_count > 0 {
                return Some(s.cadence.clone());
            }
        }
    }
    None
}

/// v0.1.368: reset a speaker's cadence stats to the default
/// (zero samples). Useful when a user's speaking style has
/// shifted (illness, new microphone, recovery from voice strain)
/// and the rolling EMA no longer reflects current cadence
/// reality. After reset, the next ~20 utterances rebuild the
/// EMA from scratch.
///
/// Returns true if the speaker was found and cadence zeroed.
/// Returns false if the named speaker doesn't exist.
///
/// Persists to speakers.toml.
pub fn reset_cadence_for(name: &str) -> bool {
    ensure_loaded();
    if let Ok(mut r) = registry().lock() {
        if let Some(s) = r.speakers.iter_mut().find(|s| s.name == name) {
            // Replace the whole struct rather than mutating fields
            // one at a time — keeps us correct if Cadence gains
            // new fields in the future. Default zeros everything.
            s.cadence = Cadence::default();
            drop(r);
            let _ = save();
            crate::clog!("speakers", "{:?} cadence reset", name);
            return true;
        }
    }
    false
}

fn ensure_loaded() {
    if let Ok(r) = registry().lock() {
        if r.loaded { return; }
    }
    let _ = load();
}

fn load() -> std::io::Result<()> {
    let path = match registry_path() {
        Some(p) => p,
        None => return Ok(()),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            // No file yet — that's fine, mark loaded so we don't keep retrying.
            if let Ok(mut r) = registry().lock() {
                r.loaded = true;
            }
            return Ok(());
        }
    };
    let mut speakers = Vec::new();
    let mut cur: Option<Speaker> = None;
    // v0.1.185: track whether any speaker we loaded was missing an
    // id field — if so, we'll save back after load so the migration
    // persists. Avoids re-generating IDs on every restart.
    let mut migration_needed = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[[speaker]]" {
            if let Some(s) = cur.take() {
                if s.id.is_empty() { migration_needed = true; }
                speakers.push(s);
            }
            cur = Some(Speaker {
                // v0.1.185: id starts empty; either filled from
                // the toml `id` line below, or after parsing if
                // it's still empty we generate one.
                id: String::new(),
                name: String::new(),
                color: DEFAULT_COLORS[0].to_string(),
                profile: TonalProfile::default(),
                sample_count: 1,
                allowed: true,
                // v0.1.133: defaults to false. Older speakers.toml
                // files without a deep_muted line preserve current
                // behavior (background mute when allowed=false).
                deep_muted: false,
                // v0.1.137: defaults to zero-valued cadence. Files
                // without cadence lines load empty; cadence updates
                // on the next confirmed utterance for this speaker.
                cadence: Cadence::default(),
            });
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().trim_matches('"');
            if let Some(s) = cur.as_mut() {
                match key {
                    "id" => s.id = val.to_string(),
                    "name" => s.name = val.to_string(),
                    "color" => s.color = val.to_string(),
                    "sample_count" => { if let Ok(n) = val.parse::<u32>() { s.sample_count = n; } }
                    "allowed" => {
                        // Accept "true"/"false" + numeric "1"/"0"
                        s.allowed = matches!(val, "true" | "1" | "yes");
                    }
                    "deep_muted" => {
                        s.deep_muted = matches!(val, "true" | "1" | "yes");
                    }
                    "pitch_hz" => { if let Ok(n) = val.parse::<f32>() { s.profile.pitch_hz = n; } }
                    "rms" => { if let Ok(n) = val.parse::<f32>() { s.profile.rms = n; } }
                    "activity" => { if let Ok(n) = val.parse::<f32>() { s.profile.activity = n; } }
                    "spectral_centroid_hz" => { if let Ok(n) = val.parse::<f32>() { s.profile.spectral_centroid_hz = n; } }
                    "voicing_prob" => { if let Ok(n) = val.parse::<f32>() { s.profile.voicing_prob = n; } }
                    "energy_low" => { if let Ok(n) = val.parse::<f32>() { s.profile.energy_low = n; } }
                    "energy_mid" => { if let Ok(n) = val.parse::<f32>() { s.profile.energy_mid = n; } }
                    "energy_high" => { if let Ok(n) = val.parse::<f32>() { s.profile.energy_high = n; } }
                    // v0.1.137: cadence fields. Older files without
                    // these lines load Cadence::default() (zeros).
                    "cadence_syllables_per_sec" => {
                        if let Ok(n) = val.parse::<f32>() { s.cadence.syllables_per_sec = n; }
                    }
                    "cadence_mean_pause_ms" => {
                        if let Ok(n) = val.parse::<f32>() { s.cadence.mean_pause_ms = n; }
                    }
                    "cadence_sample_count" => {
                        if let Ok(n) = val.parse::<u32>() { s.cadence.sample_count = n; }
                    }
                    "cadence_last_updated_at" => {
                        if let Ok(n) = val.parse::<u64>() { s.cadence.last_updated_at = n; }
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(s) = cur.take() {
        if s.id.is_empty() { migration_needed = true; }
        speakers.push(s);
    }
    // v0.1.185: backfill ids for any speakers lacking them. After
    // this loop, every speaker has a stable id; if we generated
    // any, save back to disk so future loads skip the migration.
    for s in &mut speakers {
        if s.id.is_empty() {
            s.id = new_id();
            crate::clog!("speakers", "migrated speaker '{}' → id={}", s.name, s.id);
        }
    }
    // v0.1.251: color migration. Detect speakers with identical
    // colors — under the old narrow-pitch-range (80-350Hz) color
    // function, two voices with different fundamentals could both
    // get clamped to the same endpoint hue. Hardware test in
    // v0.1.249 showed two speakers both rendered as red dots.
    // Fix is in color_from_profile (widened range + centroid
    // modulation); migration recomputes colors for affected
    // registries so the fix applies retroactively.
    let mut color_migration_needed = false;
    {
        let mut seen: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(speakers.len());
        for s in &speakers {
            if !seen.insert(s.color.clone()) {
                color_migration_needed = true;
                break;
            }
        }
    }
    if color_migration_needed {
        for (idx, s) in speakers.iter_mut().enumerate() {
            let new_color = color_from_profile(&s.profile, idx);
            if new_color != s.color {
                crate::clog!("speakers", "recolored {:?}: {} → {}",
                    s.name, s.color, new_color);
                s.color = new_color;
            }
        }
        migration_needed = true;
    }
    if let Ok(mut r) = registry().lock() {
        r.speakers = speakers;
        r.loaded = true;
        crate::clog!("speakers", "loaded {} speakers from disk", r.speakers.len());
    }
    if migration_needed {
        // Save outside the registry lock so save() can re-lock.
        if let Err(e) = save() {
            crate::clog!("speakers", "migration save failed: {}", e);
        }
    }
    Ok(())
}

fn save() -> std::io::Result<()> {
    let path = match registry_path() {
        Some(p) => p,
        None => return Ok(()),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let r = registry().lock().map_err(|_|
        std::io::Error::new(std::io::ErrorKind::Other, "lock failed"))?;
    let mut out = String::with_capacity(256 * r.speakers.len() + 64);
    out.push_str("# Web2QML speaker registry. Edit names/colors freely;\n");
    out.push_str("# profile values are auto-tuned on each confirmation.\n\n");
    for s in &r.speakers {
        out.push_str("[[speaker]]\n");
        // v0.1.185: id written first so it's the most visible
        // field when humans inspect the file. Stable identifier;
        // editing this by hand would orphan back-references in
        // future card data. Name and color are safe to edit.
        out.push_str(&format!("id = \"{}\"\n", s.id));
        out.push_str(&format!("name = \"{}\"\n", s.name.replace('"', "\\\"")));
        out.push_str(&format!("color = \"{}\"\n", s.color));
        out.push_str(&format!("sample_count = {}\n", s.sample_count));
        out.push_str(&format!("allowed = {}\n", s.allowed));
        out.push_str(&format!("deep_muted = {}\n", s.deep_muted));
        out.push_str(&format!("pitch_hz = {}\n", s.profile.pitch_hz));
        out.push_str(&format!("rms = {}\n", s.profile.rms));
        out.push_str(&format!("activity = {}\n", s.profile.activity));
        out.push_str(&format!("spectral_centroid_hz = {}\n", s.profile.spectral_centroid_hz));
        out.push_str(&format!("voicing_prob = {}\n", s.profile.voicing_prob));
        out.push_str(&format!("energy_low = {}\n", s.profile.energy_low));
        out.push_str(&format!("energy_mid = {}\n", s.profile.energy_mid));
        out.push_str(&format!("energy_high = {}\n", s.profile.energy_high));
        // v0.1.137: cadence stats. Stored even when zero so files
        // always have the field present after first save.
        out.push_str(&format!("cadence_syllables_per_sec = {}\n", s.cadence.syllables_per_sec));
        out.push_str(&format!("cadence_mean_pause_ms = {}\n", s.cadence.mean_pause_ms));
        out.push_str(&format!("cadence_sample_count = {}\n", s.cadence.sample_count));
        out.push_str(&format!("cadence_last_updated_at = {}\n", s.cadence.last_updated_at));
        out.push('\n');
    }
    std::fs::write(&path, out)
}

/// Current speaker — most recently identified during this session.
/// QML polls this to drive circle-pulse animations.
static CURRENT_SPEAKER: Mutex<Option<String>> = Mutex::new(None);

/// v0.1.188 (1b-γ): ring of recent speaker identifications. Each
/// entry: (unix_ms_when_heard, speaker_name). Used by
/// `voices_heard_recent` to count distinct speakers in a recent
/// window — the signal that drives CoreadPrompt.
///
/// Size 32: enough to track a busy multi-speaker session without
/// growing unbounded. Overwrites oldest entries when full.
const RECENT_BUF_LEN: usize = 32;
static RECENT_HEARD: Mutex<RecentHeard> = Mutex::new(RecentHeard {
    buf: Vec::new(),
});

struct RecentHeard {
    /// (unix_ms, name) pairs. Maintained in insertion order;
    /// trimmed from front when len > RECENT_BUF_LEN.
    buf: Vec<(u64, String)>,
}

fn now_ms_for_speakers() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record that this name was just heard. Updates CURRENT_SPEAKER for
/// UI polling. Call from voice path after identification succeeds.
///
/// v0.1.188: also appends to RECENT_HEARD ring for voices_heard
/// counting.
pub fn note_active(name: &str) {
    if name.is_empty() {
        return;
    }
    if let Ok(mut g) = CURRENT_SPEAKER.lock() {
        let s = Some(name.to_string());
        if *g != s {
            *g = s;
        }
    }
    if let Ok(mut r) = RECENT_HEARD.lock() {
        r.buf.push((now_ms_for_speakers(), name.to_string()));
        // Trim from front if over capacity. We push to back,
        // truncate from front, so the ring keeps the most recent
        // RECENT_BUF_LEN entries.
        if r.buf.len() > RECENT_BUF_LEN {
            let excess = r.buf.len() - RECENT_BUF_LEN;
            r.buf.drain(0..excess);
        }
    }
}

/// Return the name of the most-recently-heard speaker (or empty).
pub fn current_active() -> String {
    CURRENT_SPEAKER.lock().ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

/// v0.1.282: stable id of the currently-active speaker. Empty
/// if no speaker is active. Used by the workspaces module
/// which keys its on-disk storage by speaker id (so renames
/// don't break links).
pub fn current_active_id() -> String {
    let name = match CURRENT_SPEAKER.lock().ok().and_then(|g| g.clone()) {
        Some(n) => n,
        None => return String::new(),
    };
    let g = match registry().lock() { Ok(g) => g, Err(_) => return String::new() };
    g.speakers.iter()
        .find(|s| s.name.eq_ignore_ascii_case(&name))
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// Clear the active speaker (called when recording ends).
pub fn clear_active() {
    if let Ok(mut g) = CURRENT_SPEAKER.lock() {
        *g = None;
    }
}

/// v0.1.188 (1b-γ): count distinct speakers heard within the last
/// `window_secs` seconds. Used by the Moment classifier to drive
/// CoreadPrompt — when >=2 distinct voices are in the window AND
/// the user is reading-paced, the system offers "read this together?"
///
/// Caveat: relies on voice fingerprint identification, which can
/// misclassify (the corpus arc is fixing this). False positives
/// (single user identified as two) lead to spurious CoreadPrompts;
/// these are dismissable and apply cooldown via the existing
/// suppression. False negatives (two users both identified as one)
/// just don't fire CoreadPrompt; harmless.
pub fn voices_heard_recent(window_secs: u64) -> usize {
    let now = now_ms_for_speakers();
    let window_ms = window_secs * 1000;
    let cutoff = now.saturating_sub(window_ms);
    if let Ok(r) = RECENT_HEARD.lock() {
        let mut seen: Vec<&str> = Vec::with_capacity(4);
        for (ts, name) in r.buf.iter() {
            if *ts < cutoff {
                continue;
            }
            if !seen.contains(&name.as_str()) {
                seen.push(name.as_str());
            }
        }
        seen.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    // v0.1.211 (ship 2.6.6): tests for the MFCC-derived color
    // function. Validates the mapping rules and edge cases.
    use super::*;
    use crate::phonetic::TonalProfile;

    fn make_profile(pitch: f32, voicing: f32) -> TonalProfile {
        let mut p = TonalProfile::default();
        p.pitch_hz = pitch;
        p.voicing_prob = voicing;
        p
    }

    #[test]
    fn color_from_profile_empty_falls_back_to_default_palette() {
        let p = TonalProfile::default(); // pitch_hz = 0
        let c = color_from_profile(&p, 0);
        assert_eq!(c, DEFAULT_COLORS[0]);
        let c2 = color_from_profile(&p, 1);
        assert_eq!(c2, DEFAULT_COLORS[1]);
        // Wrapping at palette length
        let c_wrap = color_from_profile(&p, DEFAULT_COLORS.len());
        assert_eq!(c_wrap, DEFAULT_COLORS[0]);
    }

    #[test]
    fn color_from_profile_emits_valid_hex() {
        let p = make_profile(150.0, 0.7);
        let c = color_from_profile(&p, 0);
        assert_eq!(c.len(), 7);
        assert!(c.starts_with('#'));
        // All 6 hex digits parseable
        let body = &c[1..];
        assert!(u32::from_str_radix(body, 16).is_ok(),
            "expected valid hex, got {}", c);
    }

    #[test]
    fn color_from_profile_pitch_changes_hue() {
        // Low pitch (bass) and high pitch (alto) should produce
        // visibly different colors, not just shades of one hue.
        let bass = make_profile(90.0, 0.7);
        let alto = make_profile(280.0, 0.7);
        let c_bass = color_from_profile(&bass, 0);
        let c_alto = color_from_profile(&alto, 0);
        assert_ne!(c_bass, c_alto);
    }

    #[test]
    fn color_from_profile_is_stable() {
        // Same input → same output. Critical: a speaker's color
        // must not drift between calls.
        let p = make_profile(180.0, 0.65);
        let a = color_from_profile(&p, 0);
        let b = color_from_profile(&p, 0);
        let c = color_from_profile(&p, 0);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn color_from_profile_clamps_extreme_pitch() {
        // Pitch above 1500 or below 80 still produces valid colors,
        // mapped to the endpoint hues.
        let very_low = make_profile(40.0, 0.5);
        // 40 is < 50 → falls back to DEFAULT_COLORS path. Verify
        // it doesn't panic.
        let _ = color_from_profile(&very_low, 0);
        // 60 is above the 50Hz floor for "no signal" but below
        // the 80Hz floor for hue mapping — should clamp to red.
        let edge_low = make_profile(60.0, 0.5);
        let c_edge = color_from_profile(&edge_low, 0);
        assert!(c_edge.starts_with('#'));
        // 2000Hz is above the 1500Hz upper clamp; should clamp to
        // the endpoint hue (wraps back through red).
        let very_high = make_profile(2000.0, 0.5);
        let c_high = color_from_profile(&very_high, 0);
        assert!(c_high.starts_with('#'));
    }

    fn make_profile_full(pitch: f32, voicing: f32, centroid: f32) -> TonalProfile {
        let mut p = TonalProfile::default();
        p.pitch_hz = pitch;
        p.voicing_prob = voicing;
        p.spectral_centroid_hz = centroid;
        p
    }

    #[test]
    fn color_v0_1_251_widened_pitch_range_separates_high_voices() {
        // v0.1.251 regression test for the Allison+Kessler bug.
        // Under the old 80-350Hz clamp, two voices above 350Hz
        // would both clamp to the same endpoint hue. The hardware
        // test in v0.1.249 showed real pitch_hz values around 680
        // (Kessler) and 975 (Allison). They MUST get distinct
        // colors under the new color function.
        let kessler = make_profile_full(680.0, 0.7, 1200.0);
        let allison = make_profile_full(975.0, 0.7, 1800.0);
        let c_k = color_from_profile(&kessler, 0);
        let c_a = color_from_profile(&allison, 1);
        assert_ne!(c_k, c_a,
            "Kessler and Allison from v0.1.249 hardware test must \
             get different colors; got both = {}", c_k);
    }

    #[test]
    fn color_v0_1_251_same_pitch_different_centroid_separated() {
        // Same pitch but different timbre (centroid) → different
        // hue. This is the centroid-modulation contribution.
        let bright = make_profile_full(180.0, 0.7, 2800.0);
        let dark = make_profile_full(180.0, 0.7, 600.0);
        let c_bright = color_from_profile(&bright, 0);
        let c_dark = color_from_profile(&dark, 1);
        assert_ne!(c_bright, c_dark,
            "Same-pitch speakers with different centroid must get \
             different colors; got both = {}", c_bright);
    }

    #[test]
    fn color_v0_1_251_zero_centroid_does_not_break_legacy_profiles() {
        // Older profiles loaded from disk before centroid was
        // tracked may have spectral_centroid_hz = 0. Color
        // function must still produce a valid color (no NaN,
        // no panic).
        let legacy = make_profile_full(200.0, 0.7, 0.0);
        let c = color_from_profile(&legacy, 0);
        assert_eq!(c.len(), 7);
        assert!(c.starts_with('#'));
        // Must be parseable hex.
        let body = &c[1..];
        assert!(u32::from_str_radix(body, 16).is_ok(),
            "expected valid hex, got {}", c);
    }

    #[test]
    fn hsl_to_rgb_pure_red() {
        let (r, g, b) = hsl_to_rgb(0.0, 1.0, 0.5);
        assert_eq!(r, 255);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn hsl_to_rgb_pure_green() {
        let (r, g, b) = hsl_to_rgb(120.0, 1.0, 0.5);
        assert_eq!(r, 0);
        assert_eq!(g, 255);
        assert_eq!(b, 0);
    }

    #[test]
    fn hsl_to_rgb_pure_blue() {
        let (r, g, b) = hsl_to_rgb(240.0, 1.0, 0.5);
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 255);
    }

    #[test]
    fn hsl_to_rgb_gray_at_zero_saturation() {
        // Any hue with sat=0 → gray (R=G=B).
        let (r, g, b) = hsl_to_rgb(0.0, 0.0, 0.5);
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn color_from_profile_voicing_affects_saturation() {
        // Same pitch, different voicing → different output.
        // (Low voicing = lower saturation = closer to gray.)
        let p_voiced = make_profile(180.0, 0.95);
        let p_whisper = make_profile(180.0, 0.10);
        let c_voiced = color_from_profile(&p_voiced, 0);
        let c_whisper = color_from_profile(&p_whisper, 0);
        assert_ne!(c_voiced, c_whisper);
    }
}
