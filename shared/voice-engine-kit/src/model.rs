//! model — whisper tier selection and lazy loading.
//!
//! Lifted from voiceterm/src/voice.rs. All crate:: references here
//! (clog, voice_lang, whisper_pool) resolve at THIS crate's root
//! unchanged from voiceterm's own — both are mounted as direct
//! siblings of this module, matching where voiceterm had them, so
//! nothing needed rewriting (unlike the av1tuner gm:: extraction
//! earlier this session, where the target module was one level
//! deeper than the source).
//!
//! Not moved: parse_for_lang and the six voice_intent modules it
//! dispatches to (6,690 lines) -- see voice_lang.rs's own doc for
//! why. This module only uses voice_lang::current() and
//! whisper_lang_code().

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelTier { Tiny, Base, HighAccuracy }

/// Resolve the model file path for the given tier. Prefers the quantized
/// variant if it exists (faster, same accuracy), else falls back to fp16.
/// Returns None if no file exists for this tier.
fn model_file(tier: ModelTier) -> Option<PathBuf> {
    // The canonical store first, then this app's historical directory.
    //
    // These had drifted apart INSIDE ONE APP: voice-kit resolves through
    // models-kit (`~/models/whisper`) and this function looked only in
    // `~/.config/web2qml/models`, so a model sitting in the canonical store
    // was invisible here and the app would offer to download one it already
    // had. The legacy directory stays on the search path -- an existing
    // install must keep working -- but it is no longer the only place looked.
    let legacy = PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
        .join(".config/web2qml/models");
    let dir = legacy;
    // v0.1.652: language-aware model selection. The .en models are
    // English-ONLY — they ignore a non-"en" language request and
    // transcribe other languages phonetically as English (e.g. Spanish
    // "abre terminal" → "abreit terminal"). So when the configured
    // language isn't English, prefer the MULTILINGUAL variants (no .en
    // suffix); only fall back to .en with a warning, since it can't
    // actually transcribe the language.
    let non_english = crate::voice_lang::current() != "en";
    let candidates: &[&str] = match (tier, non_english) {
        (ModelTier::Tiny, false) => &["ggml-tiny.en-q5_1.bin", "ggml-tiny.en.bin"],
        (ModelTier::Tiny, true)  => &["ggml-tiny-q5_1.bin", "ggml-tiny.bin", "ggml-tiny.en-q5_1.bin", "ggml-tiny.en.bin"],
        (ModelTier::Base, false) => &["ggml-base.en-q5_1.bin", "ggml-base.en.bin"],
        (ModelTier::Base, true)  => &["ggml-base-q5_1.bin", "ggml-base.bin", "ggml-base.en-q5_1.bin", "ggml-base.en.bin"],
        // Large-v3-turbo is multilingual already (no .en variant).
        (ModelTier::HighAccuracy, _) => &[
            "ggml-large-v3-turbo-q5_0.bin",
            "ggml-large-v3-turbo.bin",
        ],
    };
    for name in candidates {
        // models-kit's search path first (canonical `~/models/whisper`, plus
        // the legacy spots it knows about), then this app's own directory.
        let p = match models_kit::find("whisper", name) {
            Some(found) => found,
            None => dir.join(name),
        };
        if p.exists() {
            // warn if a non-English language ends up on an English-only model
            if non_english && name.contains(".en") {
                crate::clog!("whisper",
                    "language is '{}' but only an English-only model ({}) is available — \
                     it will transcribe {} phonetically as English. Install a multilingual \
                     model (e.g. ggml-base-q5_1.bin, no .en) for correct transcription.",
                    crate::voice_lang::current(), name, crate::voice_lang::current());
            }
            return Some(p);
        }
    }
    None
}

/// Honor WEB2QML_FORCE_MODEL=tiny|tiny.en|base|base.en. Returns Some(tier)
/// when the env var is set, None when no override is in effect.
fn forced_tier() -> Option<ModelTier> {
    let v = std::env::var("WEB2QML_FORCE_MODEL").ok()?;
    match v.as_str() {
        "tiny" | "tiny.en" => Some(ModelTier::Tiny),
        "base" | "base.en" => Some(ModelTier::Base),
        "large" | "large-v3-turbo" | "turbo" | "high" | "high-accuracy" =>
            Some(ModelTier::HighAccuracy),
        _ => None,
    }
}

/// Back-compat for system_info.rs / other call sites that just want SOMETHING
/// to point at. Returns the lowest tier that exists, falling back to base.
pub fn model_path() -> PathBuf {
    if let Some(t) = forced_tier() {
        if let Some(p) = model_file(t) { return p; }
    }
    if let Some(p) = model_file(ModelTier::Base) { return p; }
    if let Some(p) = model_file(ModelTier::Tiny) { return p; }
    // No file — return a guess so callers can format an error message.
    // The guess names the CANONICAL location, because this string ends up in
    // an error telling someone where to put a model. Pointing them at the
    // legacy directory teaches the wrong convention to every new install.
    models_kit::resolve("whisper", "ggml-base.en.bin")
}

/// Check if voice input is available/// short (yes/no/cancel/the candidate name). Tiny is fast enough
/// to do this 200-400ms instead of Base's 1000ms+, so the popup
/// closes feeling responsive.
///
/// We don't use this for normal voice queries because Tiny's accuracy
/// drops on longer phrases. The hint is opt-in, set ONLY by QML
/// paths that know the next utterance will be short.
#[cfg(feature = "voice")]
pub static FORCE_TINY_TIER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "voice")]
pub fn set_force_tiny_tier(v: bool) {
    FORCE_TINY_TIER.store(v, std::sync::atomic::Ordering::SeqCst);
}

/// One-shot "fast finalize" flag (v0.1.122). When QML knows the
/// next utterance will be short (yes/no/cancel/single-word
/// confirmation), it sets this to true. record_and_transcribe_inner
/// reads + clears it on entry and adjusts:
///   - silence_to_stop: 825ms → 450ms
///   - min_record: 1200ms → 600ms
///
/// Result: confirm-listen flow shaves ~600-750ms off the perceived
/// "I said yes, why is it still waiting" delay. Single-word
/// utterances finish ~1.2s instead of ~1.9s on a typical recording.
///
/// Trade-off: if the user actually says a long sentence during what
/// QML expected to be a confirmation (rare), VAD might cut off
/// prematurely. The same flag is one-shot — bigger commands hit the
/// normal voice path with the standard timings.
#[cfg(feature = "voice")]
pub static FAST_FINALIZE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "voice")]
pub fn set_fast_finalize(v: bool) {
    FAST_FINALIZE.store(v, std::sync::atomic::Ordering::SeqCst);
}

/// Memory cost when both loaded: ~220MB for fp16, ~88MB for quantized.
/// Acceptable on desktop, expensive on mobile — mobile usually only has
/// tiny installed via setup-voice.sh, so only tiny gets loaded.
#[cfg(feature = "voice")]
pub(crate) static TINY_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
#[cfg(feature = "voice")]
pub(crate) static BASE_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
/// Optional third tier for systems fast enough to run a Large model in
/// the grace window. ggml-large-v3-turbo is the practical candidate
/// (~6x faster than vanilla large-v3 thanks to reduced decoder layers).
/// Loaded only if a matching file exists in the model dir; absent on
/// most systems including all phones.
///
/// The dispatcher tries HighAccuracy first when picking results, but
/// it falls back to Base or Tiny if HighAccuracy hasn't returned within
/// the grace window. So loading it on a slow system doesn't block —
/// it just gets skipped at decision time. The wasted inference cost
/// is the price of the optionality.
#[cfg(feature = "voice")]
pub(crate) static LARGE_CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();

// ─── Per-tier WhisperState caches ───
//
// whisper-rs 0.16's WhisperState owns its data (no borrow on
// WhisperContext at the API level), so we can cache it across calls.
// This avoids the ~210MB allocation churn (kv_self + kv_cross +
// compute_buffer_encode + compute_buffer_decode) that happens on
// every state creation — visible in the user's logs as a wall of
// `whisper_init_state:` lines on every transcribe.
//
// Pattern: Mutex<Option<WhisperState>>. On call:
//   1. Take it out of the Option (.take())
//   2. If None (first call), create a new one
//   3. Run inference
//   4. Put it back (.replace(state))
//
// The Mutex serializes calls per tier. Foreground + bg-listen are
// already serialized at a higher level (bg-listen skips while
// foreground is recording), so contention is rare.
//
// Memory cost: keeping state alive between calls keeps the ~210MB
// of compute buffers resident. That's RAM we're spending to save
// 100-300ms per call. Worth it on any machine with > 1GB free.
//
// State is reset implicitly by each .full() call — whisper-rs
// clears segments and KV state at the start of inference.
#[cfg(feature = "voice")]
pub(crate) static TINY_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
#[cfg(feature = "voice")]
pub(crate) static BASE_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
#[cfg(feature = "voice")]
pub(crate) static LARGE_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();

#[cfg(feature = "voice")]
pub(crate) fn ensure_models_loaded() -> Result<(), String> {
    // Tier loading policy (changed in v0.1.66 after multi-tier proved too
    // expensive on typical CPU desktops):
    //   - WEB2QML_FORCE_MODEL=tiny|base|turbo: load that tier ONLY
    //   - WEB2QML_PARALLEL_MODELS=1: load every tier whose file exists
    //     and run the parallel dispatcher
    //   - default: load Base if available, else Tiny. Single tier, no
    //     parallel dispatch. This is robust on midrange hardware where
    //     three concurrent whisper contexts can blow the CPU budget
    //     (turning a 0.5s tiny inference into a 4-12s tiny inference
    //     because all three tiers contend for the same cores).
    //
    // The fallback path (when neither env var is set):
    //   Base is preferred over Tiny because it's the better quality/
    //   speed point on a single-tier setup. If Base file is missing,
    //   fall back to Tiny.
    let force = forced_tier();
    let parallel_opt_in = std::env::var("WEB2QML_PARALLEL_MODELS")
        .map(|v| v == "1" || v == "true" || v == "yes")
        .unwrap_or(false);

    let (load_tiny, load_base, load_large) = if let Some(t) = force {
        (t == ModelTier::Tiny, t == ModelTier::Base, t == ModelTier::HighAccuracy)
    } else if parallel_opt_in {
        // User explicitly asked for parallel — load every tier whose
        // file exists. The dispatcher will fan out.
        crate::clog!("whisper", "WEB2QML_PARALLEL_MODELS=1 — loading all available tiers");
        (true, true, true)
    } else {
        // Default: single tier. Prefer Base, fall back to Tiny.
        // We probe file presence first to decide which one to load.
        let base_present = model_file(ModelTier::Base).is_some();
        if base_present {
            (false, true, false)
        } else {
            (true, false, false)
        }
    };

    if load_tiny { load_tier(ModelTier::Tiny)?; }
    if load_base { load_tier(ModelTier::Base)?; }
    if load_large { load_tier(ModelTier::HighAccuracy)?; }

    // v0.1.116: bring up the whisper worker pool. Uses the Tiny
    // context (smallest, fastest — good for streaming jobs). 1 worker
    // by default; future setting could let the user bump to 2 for
    // multi-speaker concurrency at the cost of extra ~210MB.
    //
    // Reading WEB2QML_WHISPER_WORKERS lets the user opt in to 2
    // workers without rebuilding. Capped at 4 by the pool.
    // v0.1.124: prefer Tiny for the pool (fast) but fall back to
    // Base if user didn't install Tiny. Previously the pool init
    // was gated strictly on TINY_CTX, leaving the pool uninitialized
    // for Base-only users — every per-speaker submit failed with
    // "pool not initialized".
    let pool_ctx = TINY_CTX.get()
        .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
        .or_else(|| BASE_CTX.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone())));
    if let Some(ctx) = pool_ctx {
        let n = std::env::var("WEB2QML_WHISPER_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        crate::whisper_pool::init_with_tiny_ctx(ctx, n);
    }

    // Sanity check: at least one must be loaded for transcription to work.
    let any_loaded = TINY_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false)
        || BASE_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false)
        || LARGE_CTX.get().map(|m| m.lock().map(|g| g.is_some()).unwrap_or(false)).unwrap_or(false);
    if !any_loaded {
        return Err(format!(
            "No whisper model present. Run setup-voice.sh to download (default: tiny+base quantized).\n\
             Looked in {}/ggml-{{tiny,base}}.en{{,-q5_1}}.bin and ggml-large-v3-turbo*.bin",
            PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
                .join(".config/web2qml/models").display()
        ));
    }
    Ok(())
}

/// Load one tier's model context if its file exists. No-op if already loaded.
/// Missing file is NOT an error — the dispatcher handles partial setups.
/// Returns Err only if the file exists but loading failed.
#[cfg(feature = "voice")]
#[cfg(feature = "voice")]
pub(crate) fn load_tier(tier: ModelTier) -> Result<(), String> {
    let cell = match tier {
        ModelTier::Tiny => &TINY_CTX,
        ModelTier::Base => &BASE_CTX,
        ModelTier::HighAccuracy => &LARGE_CTX,
    };
    let lock = cell.get_or_init(|| Mutex::new(None));
    let mut g = lock.lock().map_err(|e| format!("CTX lock: {}", e))?;
    if g.is_some() { return Ok(()); }
    let path = match model_file(tier) {
        Some(p) => p,
        None => {
            // No file for this tier — silently skip.
            return Ok(());
        }
    };
    crate::clog!("whisper", "loading {:?} from {}", tier, path.display());
    let start = std::time::Instant::now();
    // v0.1.283: was `path.to_str().unwrap()`. Would panic on a
    // non-UTF-8 path (extremely unlikely — would mean user's $HOME
    // or model dir has non-UTF-8 bytes — but defensive matters
    // because panic on startup is fatal whereas error-return
    // surfaces a clean message).
    let path_str = path.to_str()
        .ok_or_else(|| format!("Whisper init {:?}: model path is not UTF-8 ({})",
            tier, path.display()))?;
    let ctx = whisper_rs::WhisperContext::new_with_params(
        path_str,
        whisper_rs::WhisperContextParameters::default()
    ).map_err(|e| format!("Whisper init {:?}: {:?}", tier, e))?;
    *g = Some(Arc::new(ctx));
    crate::clog!("whisper", "{:?} ready in {:?}", tier, start.elapsed());
    Ok(())
}

/// Back-compat shim. Old call sites used this name. Routes to the new dual-
/// model loader.
#[cfg(feature = "voice")]
fn ensure_whisper_loaded() -> Result<(), String> {
    ensure_models_loaded()
}

