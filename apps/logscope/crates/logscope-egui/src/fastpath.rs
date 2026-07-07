//! fastpath.rs — multi-tier whisper race, ported from voiceterm's dual-model dispatch. Loads whichever of
//! Tiny/Base/HighAccuracy are present and, on each utterance, races every loaded tier: if the best-quality
//! tier already loaded returns first, use it immediately; otherwise take whichever result lands first and
//! give better tiers a short adaptive grace window to overtake before falling back to what's in hand. Short
//! utterances (commands) get a short grace since Tiny is already reliable on them; longer ones (dictation)
//! get more room for Base/HighAccuracy's better accuracy to matter. Cached per-tier `WhisperState` avoids
//! the ~200MB alloc churn of creating a new state every call (relevant on CPU-only runs).
//!
//! Fully optional and additive: with only the primary (HighAccuracy) model loaded, `loaded_tiers().len()`
//! is 1 and callers fall back to the plain single-tier path — today's behavior, unchanged.

/// Whisper model quality tier. Ord by priority: HighAccuracy (0) is best, Tiny (2) is fastest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier { HighAccuracy = 0, Base = 1, Tiny = 2 }

/// Adaptive grace window (ms) by utterance length at 16kHz. Pure — unit-tested without whisper. Short
/// utterances are commands where Tiny is already reliable, so waiting long for a better tier mostly wastes
/// latency; longer ones are dictation where Base/HighAccuracy's extra accuracy is worth the wait.
pub fn grace_window_ms(n_samples_16k: usize) -> u64 {
    if n_samples_16k <= 32_000 { 300 }       // ≤ 2s: barely-noticeable grace
    else if n_samples_16k <= 80_000 { 700 }  // 2–5s: mixed shorter sentences
    else { 1000 }                            // > 5s: dictation-like
}

#[cfg(feature = "voice")]
pub use imp::*;
#[cfg(not(feature = "voice"))]
pub use stub::*;

#[cfg(feature = "voice")]
mod imp {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{mpsc, Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    static TINY: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
    static BASE: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
    static HIGH: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();
    // Cached WhisperState per tier — reused across calls (serialized by the Mutex) instead of recreated,
    // avoiding whisper's ~200MB kv_self/kv_cross/compute-buffer allocation on every single transcription.
    static TINY_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
    static BASE_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
    static HIGH_STATE: OnceLock<Mutex<Option<whisper_rs::WhisperState>>> = OnceLock::new();
    // Results queue: submit() spawns a thread that races the loaded tiers and pushes here; drain_results()
    // is polled from the UI thread, mirroring whisper_pool's Job/InferenceResult shape. The trailing bool is
    // `is_final`: a fast-tier PREVIEW is pushed with false (shows on the pearl instantly as a volatile partial),
    // then the resolved best result with true (commits the utterance).
    static RESULTS: OnceLock<Mutex<VecDeque<(String, String, u64, bool)>>> = OnceLock::new(); // (speaker, text, t_ms, is_final)

    fn ctx_cell(t: Tier) -> &'static OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> {
        match t { Tier::Tiny => &TINY, Tier::Base => &BASE, Tier::HighAccuracy => &HIGH }
    }
    fn state_cell(t: Tier) -> &'static OnceLock<Mutex<Option<whisper_rs::WhisperState>>> {
        match t { Tier::Tiny => &TINY_STATE, Tier::Base => &BASE_STATE, Tier::HighAccuracy => &HIGH_STATE }
    }

    /// Load a tier from a model file on disk. Missing file / bad model → Err, tier just stays unloaded
    /// (the race silently runs with whichever tiers ARE present).
    pub fn load_tier(tier: Tier, path: &std::path::Path) -> Result<(), String> {
        let path_str = path.to_str().ok_or_else(|| format!("model path not UTF-8: {}", path.display()))?;
        let ctx = whisper_rs::WhisperContext::new_with_params(path_str, whisper_rs::WhisperContextParameters::default())
            .map_err(|e| format!("load {tier:?} ({}): {e:?}", path.display()))?;
        *ctx_cell(tier).get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Arc::new(ctx));
        crate::clog!("whisper", "{tier:?} tier ready ({})", path.display());
        Ok(())
    }

    /// Register an already-loaded context for a tier (shares the Arc — no second model decode). Used to
    /// fold the primary model `voice::init` already loaded into the race as the HighAccuracy tier.
    pub fn load_tier_ctx(tier: Tier, ctx: Arc<whisper_rs::WhisperContext>) {
        *ctx_cell(tier).get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(ctx);
    }

    /// Which tiers are currently loaded, in priority order.
    pub fn loaded_tiers() -> Vec<Tier> {
        [Tier::HighAccuracy, Tier::Base, Tier::Tiny].into_iter()
            .filter(|t| ctx_cell(*t).get().and_then(|m| m.lock().ok()).map(|g| g.is_some()).unwrap_or(false))
            .collect()
    }

    fn run_one(ctx: &Arc<whisper_rs::WhisperContext>, sc: &'static OnceLock<Mutex<Option<whisper_rs::WhisperState>>>,
               audio: &[f32], tier: Tier) -> Result<String, String> {
        let cell = sc.get_or_init(|| Mutex::new(None));
        // try_lock, NOT lock: each tier's cached WhisperState decodes ONE utterance at a time. If it's still
        // busy with a previous (slow) decode — the classic backlog when large-v3-turbo can't keep up — skip
        // this tier for this utterance instead of QUEUEING a thread blocked on the lock (which piles up and
        // stalls). The race just uses whichever tiers are free (tiny stays responsive).
        let mut guard = match cell.try_lock() {
            Ok(g) => g,
            Err(_) => return Err(format!("{tier:?} busy (previous decode still running) — skipped")),
        };
        let mut state = match guard.take() {
            Some(s) => s,
            None => ctx.create_state().map_err(|e| format!("create_state: {e:?}"))?,
        };
        let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_suppress_nst(true);
        params.set_language(Some(crate::whisper_pool::stt_lang_code()));
        // Bias whisper toward OUR command vocabulary (graph nav + coder dictation), not prose — measurably
        // helps short-command recognition, the case the fast Tiny/Base tiers exist for.
        params.set_initial_prompt(
            "go to twelve, focus on three, node seven, jump to twenty one, branch here, prune, \
             unprune, overview, show the palace, back to chat, best, stop, cancel, start tour, \
             stop tour, free cam, exit freecam, send it, code mode, prose mode, open paren, \
             close paren, semicolon, equals sign, dot, underscore, quote");
        state.full(params, audio).map_err(|e| format!("full(): {e:?}"))?;
        let n_segs = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n_segs {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(t) = seg.to_str() { text.push_str(t); }
            }
        }
        *guard = Some(state);
        Ok(text)
    }

    /// Race every loaded tier on the SAME audio; return the best-priority result available within the
    /// adaptive grace window. `on_prelim` is called with the FIRST (fastest) tier's text the moment it lands,
    /// so the UI can show words immediately instead of waiting out the grace / the slow tier. Single loaded
    /// tier = no thread fan-out, just runs it directly (it is both preview and final — no faster tier exists).
    fn transcribe_dual(audio: &[f32], mut on_prelim: impl FnMut(String)) -> Result<String, String> {
        let pairs: Vec<(Tier, Arc<whisper_rs::WhisperContext>, &'static OnceLock<Mutex<Option<whisper_rs::WhisperState>>>)> =
            [Tier::HighAccuracy, Tier::Base, Tier::Tiny].into_iter().filter_map(|t| {
                ctx_cell(t).get().and_then(|m| m.lock().ok().and_then(|g| g.clone())).map(|ctx| (t, ctx, state_cell(t)))
            }).collect();
        if pairs.is_empty() { return Err("no whisper tier loaded".into()); }
        if pairs.len() == 1 {
            let (tier, ctx, sc) = &pairs[0];
            return run_one(ctx, sc, audio, *tier);
        }

        // LOGSCOPE_STT_PREFER_SPEED=1 → use whichever tier lands FIRST, skipping the accuracy grace entirely
        // (best when the high-accuracy model — e.g. large-v3-turbo — is consistently slow). LOGSCOPE_STT_GRACE_MS
        // overrides the adaptive grace directly.
        let prefer_speed = std::env::var("LOGSCOPE_STT_PREFER_SPEED").map(|v| !v.is_empty() && v != "0").unwrap_or(false);
        let grace = std::env::var("LOGSCOPE_STT_GRACE_MS").ok().and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_millis).unwrap_or_else(|| Duration::from_millis(grace_window_ms(audio.len())));
        let (tx, rx) = mpsc::channel::<(usize, Tier, Result<String, String>)>();
        let t0 = Instant::now();
        for (idx, (tier, ctx, sc)) in pairs.iter().enumerate() {
            let (tx, ctx, sc, tier) = (tx.clone(), ctx.clone(), *sc, *tier);
            let audio_v = audio.to_vec();
            std::thread::spawn(move || { let r = run_one(&ctx, sc, &audio_v, tier); let _ = tx.send((idx, tier, r)); });
        }
        drop(tx);

        let mut best: Option<(usize, String)> = None;
        let mut previewed = false;
        let mut deadline: Option<Instant> = None; // set to (first successful result + grace)
        loop {
            let msg = match deadline {
                None => rx.recv().ok(),                                    // nothing yet → block for the first result
                Some(dl) => match dl.checked_duration_since(Instant::now()) {
                    Some(d) => rx.recv_timeout(d).ok(),                    // wait out the remaining grace for a better tier
                    None => None,                                         // grace elapsed
                },
            };
            let Some((idx, tier, r)) = msg else { break };
            match r {
                Ok(text) => {
                    crate::clog!("whisper", "{:?} returned at +{:.3}s", tier, t0.elapsed().as_secs_f32());
                    if idx == 0 { crate::clog!("whisper", "using {:?} (best tier)", tier); return Ok(text); } // top tier always wins
                    if !previewed { on_prelim(text.clone()); previewed = true; } // instant preview from the fastest tier
                    if best.as_ref().map(|(i, _)| idx < *i).unwrap_or(true) { best = Some((idx, text)); }
                    if prefer_speed { crate::clog!("whisper", "prefer-speed: using first result, skipping grace"); return Ok(best.unwrap().1); }
                    if deadline.is_none() { deadline = Some(Instant::now() + grace); } // start the grace on first success
                }
                Err(e) => crate::clog!("whisper", "{:?}: {e}", tier), // busy/failed tier — race on with the others
            }
        }
        best.map(|(_, t)| t).ok_or_else(|| "all tiers failed".into())
    }

    /// Submit an utterance to the tier race. Spawns one thread (which itself fans out across tiers);
    /// utterances are seconds apart so this is cheap. A fast-tier PREVIEW lands in `drain_results()` with
    /// is_final=false the moment it's ready, then the resolved result with is_final=true.
    pub fn submit(audio_16k: Vec<f32>, speaker: String) {
        let t_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        std::thread::spawn(move || {
            let push = |text: String, is_final: bool| {
                RESULTS.get_or_init(|| Mutex::new(VecDeque::new())).lock().unwrap().push_back((speaker.clone(), text, t_ms, is_final));
            };
            match transcribe_dual(&audio_16k, |prelim| push(prelim, false)) {
                Ok(text) => push(text, true),
                Err(e) => crate::clog!("whisper", "fastpath transcribe failed: {e}"),
            }
        });
    }

    /// Drain finished (speaker, text, t_ms, is_final) results — previews (is_final=false) then finals (true).
    pub fn drain_results() -> Vec<(String, String, u64, bool)> {
        RESULTS.get_or_init(|| Mutex::new(VecDeque::new())).lock().unwrap().drain(..).collect()
    }
}

#[cfg(not(feature = "voice"))]
mod stub {
    use super::Tier;
    pub fn load_tier(_tier: Tier, _path: &std::path::Path) -> Result<(), String> { Err("built without the `voice` feature".into()) }
    pub fn loaded_tiers() -> Vec<Tier> { Vec::new() }
    pub fn submit(_audio_16k: Vec<f32>, _speaker: String) {}
    pub fn drain_results() -> Vec<(String, String, u64, bool)> { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grace_window_scales_with_utterance_length() {
        assert_eq!(grace_window_ms(16_000), 300);   // 1s — short command
        assert_eq!(grace_window_ms(32_000), 300);   // exactly 2s — still short
        assert_eq!(grace_window_ms(32_001), 700);   // just over 2s
        assert_eq!(grace_window_ms(80_000), 700);   // exactly 5s
        assert_eq!(grace_window_ms(80_001), 1000);  // just over 5s — dictation
        assert_eq!(grace_window_ms(160_000), 1000); // 10s
    }

    #[test]
    fn tier_priority_order() {
        assert!((Tier::HighAccuracy as i32) < (Tier::Base as i32));
        assert!((Tier::Base as i32) < (Tier::Tiny as i32));
    }
}
