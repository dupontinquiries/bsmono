//! transcribe — VAD, the dual-tier whisper race, and the fallback path.
//!
//! Lifted from voiceterm/src/voice.rs. `record_and_transcribe_inner`
//! had its Tier1 acoustic-index / speaker-aware section REMOVED --
//! see maintenance/C18_TODO.md sections 13/18/19 for why: that path
//! is opt-in (WEB2QML_TIER1_PROMOTE, unset by default) and pulls in
//! ~4,969 lines across audio_profile/phonetic/acoustic_index/speakers,
//! none of which this crate's own doc scopes it to own. A consuming
//! app that wants the Tier1 fast path runs it as a PRE-CHECK in front
//! of this module's transcribe, not inside it.

use crate::model::{ModelTier, model_path};
#[cfg(feature = "voice")]
use crate::model::{ensure_models_loaded, FORCE_TINY_TIER, FAST_FINALIZE, TINY_CTX, BASE_CTX, LARGE_CTX, TINY_STATE, BASE_STATE, LARGE_STATE};
#[cfg(feature = "voice")]
use crate::capture::{RING_BUFFER, CANCEL_FLAG, FINISH_FLAG, ensure_capture_running, begin_capture};
#[cfg(feature = "voice")]
use crate::capture::TARGET_RATE;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::sync::mpsc;
use std::sync::Arc;

/// Repointed onto voice-kit, same as voiceterm's own copy: the shingle-scan
/// logic was a byte-for-byte duplicate of
/// `voice_kit::hallucination::trim_hallucination_loop`. Kept as a thin
/// wrapper (not a bare re-export) so the `clog!` diagnostic survives --
/// voice-kit itself stays logging-free since it is shared with rusty-board,
/// which has its own diagnostics story.
fn trim_hallucination_loop(s: &str) -> String {
    let trimmed = voice_kit::hallucination::trim_hallucination_loop(s);
    if trimmed != s.trim() {
        crate::clog!("whisper", "trimmed hallucination loop: {:?} -> {:?}", s, trimmed);
    }
    trimmed
}

/// v0.1.225: gate for per-chunk VAD trace. Returns true when
/// `WEB2QML_VAD_DEBUG` is set to "1" or "true". Cached so the
/// env-var lookup happens once per process. Used by the VAD
/// loop to emit a line per evaluation showing rms/zcr/decision.
///
/// Diagnostic-only — leave off in normal operation.
fn vad_debug_enabled() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        match std::env::var("WEB2QML_VAD_DEBUG") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => {
                crate::clog!("vad-debug", "enabled — per-chunk VAD trace will spam the log");
                true
            }
            _ => false,
        }
    })
}



#[cfg(feature = "voice")]
#[allow(dead_code)] // public API kept for future use; current code uses _inner directly
fn record_and_transcribe_with_audio() -> Result<(String, Vec<f32>), String> {
    record_and_transcribe_inner()
}

#[cfg(feature = "voice")]
#[allow(dead_code)] // public API kept for future use; current code uses _inner directly
fn record_and_transcribe() -> Result<String, String> {
    record_and_transcribe_inner().map(|(text, _)| text)
}


#[cfg(feature = "voice")]
pub(crate) fn record_and_transcribe_inner() -> Result<(String, Vec<f32>), String> {
    use std::time::{Duration, Instant};

    // Make sure stream is running and capture is enabled
    ensure_capture_running()?;
    begin_capture();

    // VAD-based recording with ADAPTIVE thresholds.
    //
    // Stop criteria:
    //   - Hard cap: 12 seconds (was 7s — too short for spelling URLs)
    //   - Min record: 1.2s before any early-stop is allowed
    //   - Early stop: after min_record AND we've heard speech, IF we observe
    //                 silence_to_stop ms of low-energy audio.
    //
    // Adaptive calibration: the first 0.5s of detected speech sets the
    // "speech baseline" (average RMS during active speech). The silence
    // threshold then becomes baseline * 0.35, with a floor at 0.005.
    // This adapts to quiet speakers, loud speakers, distant mics, close mics.
    //
    // Silence tolerance: 1100ms. Longer than before because:
    //   - Spelled letters ("ay ... el ... ex") have 400-700ms pauses
    //   - Thinking pauses ("open... uh... github") need to not cut off
    //   - DTW-aware: extra silence before whisper costs ~50ms; cutting off mid-word costs reparse.
    let start = Instant::now();
    let max_record = Duration::from_secs_f32(12.0);
    // v0.1.122: one-shot fast-finalize for confirm-listen flow (yes/no).
    // QML sets the FAST_FINALIZE flag right before voice_listen_dual,
    // we read+clear here and tighten the VAD timings for this single
    // recording.
    let fast = FAST_FINALIZE.swap(false, std::sync::atomic::Ordering::SeqCst);
    let min_record = if fast {
        Duration::from_millis(600)
    } else {
        Duration::from_secs_f32(1.2)
    };
    // VAD tail silence: how long after the last speech sample we wait
    // before declaring the utterance over. v0.1.91 reduced from 1100ms
    // to 825ms (25% trim per user request) — most of the perceived-
    // latency cost between "I stopped talking" and "transcription
    // begins" comes from this silence wait. 825ms is still well above
    // a typical inter-word pause (200-400ms) but pulls the latency
    // budget tighter. If users report transcripts being cut mid-word,
    // dial back toward 950ms.
    //
    // v0.1.122: when fast-finalize is armed (confirm-listen yes/no
    // path), tail drops to 450ms. Short replies don't have
    // mid-utterance pauses.
    let silence_to_stop = if fast {
        Duration::from_millis(450)
    } else {
        Duration::from_millis(825)
    };
    if fast {
        crate::clog!("voice", "fast-finalize armed: min_record=600ms silence_to_stop=450ms");
    }

    // Initial threshold — used until we've calibrated.
    //
    // voiceterm's own version scales this baseline by a noise floor
    // measured from background-listen silence samples
    // (`audio_profile::noise_floor()`). That module is not part of this
    // crate's core -- confirmed too heavy to pull in (707 lines, itself
    // depending on `phonetic`'s 1,860) the same way `acoustic_index` was,
    // per `maintenance/C18_TODO.md` sections 13/18/19. This crate always
    // uses the unchanged baseline; a consuming app that wants the
    // adaptive threshold computes its own noise floor and would need a
    // way to feed it in -- NOT built here, since that is a real API
    // surface (a parameter on this function, or another hook) that
    // deserves its own decision rather than a silent partial port.
    let initial_speech_rms: f32 = 0.008;
    eprintln!("Voice VAD: initial speech_rms={:.4} (fixed baseline, no adaptive noise floor in this crate)",
        initial_speech_rms);
    let mut speech_rms = initial_speech_rms;
    let mut last_speech: Option<Instant> = None;
    let mut had_speech = false;

    // Calibration state: collect peak RMS during first ~0.6s of detected speech
    // to set adaptive threshold.
    let mut speech_rms_samples: Vec<f32> = Vec::with_capacity(16);
    let mut calibrated = false;
    let mut first_speech_at: Option<Instant> = None;

    loop {
        std::thread::sleep(Duration::from_millis(80));
        // User-cancelled: stop right away, drop the audio
        if CANCEL_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
            CANCEL_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
            // Mark capture done so the buffer drains cleanly
            if let Some(ring) = RING_BUFFER.get() {
                if let Ok(mut g) = ring.lock() {
                    g.capturing = false;
                    let _ = g.drain_into_mono();  // discard
                }
            }
            return Err("cancelled by user".into());
        }
        // User force-finished (tapped "done"): stop now, KEEP audio
        if FINISH_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
            FINISH_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
            eprintln!("Voice: user-finished at {:.1}s", start.elapsed().as_secs_f32());
            break;
        }
        let elapsed = start.elapsed();
        if elapsed >= max_record { break; }

        // Get last 100ms of audio for VAD
        let ring = RING_BUFFER.get().ok_or("Ring buffer not initialized")?;
        let (rms, zcr, src_rate, src_ch) = {
            let g = ring.lock().map_err(|e| format!("Ring lock: {}", e))?;
            let window_samples = (g.source_rate as usize / 10) * g.source_channels.max(1);
            let take = window_samples.min(g.samples.len());
            let start_idx = g.samples.len().saturating_sub(take);
            let mut sum_sq = 0.0f64;
            let mut n = 0u64;
            // v0.1.130: zero-crossing count over the same 100ms window.
            // Used as a coarse voicing gate alongside RMS. Speech ZCR
            // (per 100ms) typically falls 50-300 at 44.1kHz × 1ch;
            // sustained drums/bass lines have very low ZCR (long
            // periodic crossings); cymbals/static have very high
            // (thousands). Music sits somewhere depending on
            // material. This is a coarse filter — not a substitute
            // for a proper frequency-domain voicing detector — but
            // it knocks out the easy cases (subway rumble, kick
            // drums, sustained hum) with one extra comparison per
            // sample. No FFT, no allocation.
            //
            // v0.1.225 bugfix: when the source is stereo, the
            // interleaved buffer is L0 R0 L1 R1 L2 R2... Treating
            // this as a continuous mono signal for ZCR is wrong —
            // independent L/R signals create spurious "crossings"
            // at every channel boundary. On the user's MacBook
            // (44.1k stereo) this inflated ZCR from speech-band
            // (~50-300 per 100ms) to 2100-3000, blowing past the
            // 30..800 voicing gate and rejecting every voice
            // chunk as "non-voice noise."
            //
            // Fix: stride through one channel only (the left, by
            // convention). RMS sum is also restricted to that
            // channel so the energy estimate stays consistent
            // with the crossings analyzed.
            //
            // Previous sample's sign for crossing detection. Starts
            // at 0 so a leading positive sample doesn't false-count.
            let mut prev: f64 = 0.0;
            let mut crossings: u64 = 0;
            let stride = g.source_channels.max(1);
            let mut i = start_idx;
            while i < g.samples.len() {
                let s = g.samples[i] as f64;
                sum_sq += s * s; n += 1;
                if (s >= 0.0) != (prev >= 0.0) && n > 1 {
                    crossings += 1;
                }
                prev = s;
                i += stride;
            }
            let rms = if n > 0 { (sum_sq / n as f64).sqrt() as f32 } else { 0.0 };
            // ZCR normalized to crossings per 100ms (the window length).
            // At source_rate 44.1k, window is 4410 samples per channel
            // = 100ms regardless of channel count, since we now stride
            // by channels. n counts only the channel we sampled.
            let zcr = if n > 0 {
                (crossings as f32 / n as f32) * (g.source_rate as f32 / 10.0)
            } else { 0.0 };
            (rms, zcr, g.source_rate, g.source_channels)
        };
        let _ = (src_rate, src_ch);

        // v0.1.130: voicing gate. RMS passes the energy bar but ZCR
        // tells us whether that energy looks speech-shaped. Acceptable
        // speech band: 30 < ZCR < 800 crossings/sec at 44.1kHz mono.
        // The wide tolerance is intentional — strict ZCR rejects
        // legitimate speech that happens to have unusual phonetic
        // makeup (e.g. lots of fricatives push ZCR up, lots of
        // vowels push it down). We're filtering OBVIOUSLY non-speech.
        //
        // Below 30: sustained low-frequency hum (subway rumble, AC
        // noise, kick drums in adjacent room).
        // Above 800: high-frequency static, cymbals, electrical
        // interference.
        // Both bands could COINCIDE with speech in mixed scenes; we
        // accept that false negative rate as the cost of cleaner
        // VAD in noisy environments.
        let voicing_ok = zcr >= 30.0 && zcr <= 800.0;
        let is_speech = rms > speech_rms && voicing_ok;
        // v0.1.225: optional per-chunk VAD trace. Set
        // WEB2QML_VAD_DEBUG=1 to emit one line per VAD evaluation
        // showing the raw rms, zcr, gate thresholds, and decision.
        // Lets the user (or future-Claude reading a transcript)
        // see WHY chunks are being rejected, and tune the
        // thresholds against real captured audio rather than
        // synthetic test cases.
        //
        // Caveat: spams the log heavily — only enable for
        // diagnostic sessions, not normal use.
        if vad_debug_enabled() {
            crate::clog!("vad-debug", "rms={:.4} (thr={:.4}) zcr={:.0}/100ms (band 30..800) ch={} → {}",
                rms, speech_rms, zcr, src_ch,
                if is_speech { "ACCEPT" }
                else if !voicing_ok { "reject (zcr out of band)" }
                else { "reject (below rms threshold)" });
        }
        // Surface rejected-but-loud chunks once per VAD session so
        // we can see WHY a noisy environment isn't triggering speech.
        // Throttle to first occurrence to avoid log spam.
        if rms > speech_rms && !voicing_ok && first_speech_at.is_none() {
            eprintln!("Voice VAD: rejected loud chunk (rms={:.4} zcr={:.0}/s) — non-voice noise",
                rms, zcr);
        }
        if is_speech {
            last_speech = Some(Instant::now());
            had_speech = true;
            if first_speech_at.is_none() { first_speech_at = Some(Instant::now()); }

            // Calibration: collect samples for the first 600ms of speech, then
            // compute adaptive threshold.
            if !calibrated {
                speech_rms_samples.push(rms);
                if let Some(fsa) = first_speech_at {
                    if fsa.elapsed() >= Duration::from_millis(600) && speech_rms_samples.len() >= 4 {
                        // Use median of collected speech RMS as baseline (robust to outliers)
                        let mut sorted = speech_rms_samples.clone();
                        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        let median = sorted[sorted.len() / 2];
                        // Threshold = 35% of median observed speech, floor at 0.005,
                        // cap at 0.022. The cap was 0.030 prior to v0.1.79 but real-
                        // world testing showed it was high enough to swallow quiet
                        // confirmation utterances ("yes", "no") spoken at lower volume
                        // than the original calibrating burst. 0.022 still rejects
                        // most non-speech ambient while letting natural-volume
                        // confirmations through.
                        let new_thresh = (median * 0.35).max(0.005).min(0.022);
                        eprintln!("Voice VAD: calibrated speech_rms = {:.4} (was {:.4}, median speech RMS {:.4})",
                            new_thresh, speech_rms, median);
                        speech_rms = new_thresh;
                        calibrated = true;
                    }
                }
            }
        }

        // Allow early stop only after min_record AND we've heard speech
        if elapsed >= min_record && had_speech {
            if let Some(t) = last_speech {
                if t.elapsed() >= silence_to_stop {
                    eprintln!("Voice: VAD stop at {:.1}s ({}ms silence, threshold {:.4})",
                        elapsed.as_secs_f32(), t.elapsed().as_millis(), speech_rms);
                    break;
                }
            }
        }
    }

    // Stop capturing and drain
    let ring = RING_BUFFER.get().ok_or("Ring buffer not initialized")?;
    let (raw_mono, source_rate) = {
        let mut g = ring.lock().map_err(|e| format!("Ring lock: {}", e))?;
        g.capturing = false;
        (g.drain_into_mono(), g.source_rate)
    };

    // Resample to 16kHz
    let mut audio = if source_rate == TARGET_RATE { raw_mono.clone() } else { resample(&raw_mono, source_rate, TARGET_RATE) };
    eprintln!("Voice: captured {} mono samples → {} 16kHz samples ({:.1}s)",
        raw_mono.len(), audio.len(), audio.len() as f32 / TARGET_RATE as f32);

    if audio.is_empty() {
        return Err("No audio captured".into());
    }

    // High-pass to kill rumble + DC. Significantly improves whisper
    // transcription on rooms with HVAC, traffic, fans, or 50/60Hz
    // mains buzz. Cheap (~1 mult per sample). Applied to a copy
    // because we ALSO want the unfiltered audio for the phonetic
    // fingerprint store later (filtering changes fingerprint
    // characteristics, and we want fingerprints to match raw mic
    // input not filtered audio).
    high_pass_80hz(&mut audio);
    // v0.1.783: optional gentle gain (opt-in WEB2QML_VOICE_GAIN=1). Same
    // conservative normalization as the background path.
    if std::env::var("WEB2QML_VOICE_GAIN").map(|v| v == "1").unwrap_or(false) {
        let g = normalize_gain(&mut audio);
        if g != 1.0 { crate::clog!("voice", "gain x{:.2}", g); }
    }
    if audio.len() < 8000 {
        return Err(format!("Too little audio: {} samples", audio.len()));
    }

    let max_amp = audio.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    if max_amp < 0.001 {
        return Err(format!("Audio silent (max amp {:.6})", max_amp));
    }

    // Pad to whisper-friendly minimum 1s if needed
    let mut audio_padded = audio;
    while audio_padded.len() < TARGET_RATE as usize {
        audio_padded.push(0.0);
    }

    // Tier1 acoustic-index fast path REMOVED here (voiceterm has it,
    // this crate does not) -- see the module doc for why. Consuming
    // apps that want it run their own pre-check before calling into
    // this function; this crate always transcribes.
    ensure_models_loaded()?;
    let trans_start = Instant::now();
    let raw_cleaned = transcribe_with_dual_model(&audio_padded)?;
    // Trim repetition loops. Whisper occasionally emits "command, run
    // cargo test, run cargo test, run cargo test, ..." when audio
    // includes silence after a confident phrase -- its decoder enters a
    // fixation loop. Detect the loop and truncate. The result is just
    // the unique prefix, no padding.
    let cleaned = trim_hallucination_loop(&raw_cleaned);
    let xc_secs = trans_start.elapsed().as_secs_f32();
    eprintln!("Voice: transcribed in {:.3}s: {:?}", xc_secs, cleaned);

    Ok((cleaned, audio_padded))
}


#[cfg(feature = "voice")]
pub(crate) fn transcribe_with_dual_model(audio: &[f32]) -> Result<String, String> {
    use std::sync::mpsc;
    use std::time::{Instant, Duration};

    // Force-tiny check: when QML armed this for a yes/no confirmation,
    // bypass the dispatch loop entirely and just call Tiny. One-shot:
    // we clear the flag here so subsequent normal queries get the full
    // priority cascade.
    let force_tiny = FORCE_TINY_TIER.swap(false, std::sync::atomic::Ordering::SeqCst);
    if force_tiny {
        if let Some(tiny_arc) = TINY_CTX.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
        {
            crate::clog!("voice", "force-tiny tier (yes/no context)");
            return run_one(&tiny_arc, audio, ModelTier::Tiny);
        }
        crate::clog!("voice", "force-tiny requested but Tiny not loaded — falling through");
    }

    // v0.1.280 (voice speed): adaptive grace window. Short
    // utterances (< 2s of audio) are commands like "scroll
    // down", "open settings", "undo" — Tiny is reliable on
    // these and waiting up to 1000ms for Base to overtake is
    // mostly wasted latency. Longer utterances are sentences
    // (dictation, URLs spelled out, longer commands) where
    // Base's higher accuracy matters and the grace is worth
    // it.
    //
    // Audio is 16kHz mono f32. 2 seconds = 32000 samples.
    //
    // Tuning:
    //   ≤ 2s : 300ms  — barely-noticeable grace, but still
    //                   gives Base a fighting chance if it's
    //                   already in flight on a fast machine.
    //   2-5s : 700ms  — moderate (mixed shorter sentences).
    //   > 5s : 1000ms — original behavior for dictation-like.
    let grace_ms: u64 = if audio.len() <= 32_000 {
        300
    } else if audio.len() <= 80_000 {
        700
    } else {
        1000
    };

    // Collect all loaded contexts in priority order: HighAccuracy beats
    // Base which beats Tiny. We dispatch to all and pick the highest-
    // priority result available within the grace window.
    let pairs: Vec<(ModelTier, Arc<whisper_rs::WhisperContext>)> = [
        (ModelTier::HighAccuracy, &LARGE_CTX),
        (ModelTier::Base, &BASE_CTX),
        (ModelTier::Tiny, &TINY_CTX),
    ].iter().filter_map(|(tier, cell)| {
        cell.get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
            .map(|arc| (*tier, arc))
    }).collect();

    if pairs.is_empty() {
        return Err("No model loaded".into());
    }

    // Single-tier fast path — no thread fan-out.
    if pairs.len() == 1 {
        let (tier, ctx) = &pairs[0];
        return run_one(ctx, audio, *tier);
    }

    // Multi-tier: dispatch every loaded tier in parallel, collect by
    // priority. Tier priority is the index in `pairs` — index 0 wins.
    let (tx, rx) = mpsc::channel::<(usize, ModelTier, Result<String, String>)>();
    let t_start = Instant::now();
    for (idx, (tier, ctx)) in pairs.iter().enumerate() {
        let tx = tx.clone();
        let ctx = ctx.clone();
        let audio_v = audio.to_vec();
        let tier = *tier;
        std::thread::spawn(move || {
            let r = run_one(&ctx, &audio_v, tier);
            let _ = tx.send((idx, tier, r));
        });
    }
    drop(tx); // Original tx dropped; channel closes when all spawned tx clones finish.

    // Result collection. We track the best-priority successful result
    // seen so far. Strategy:
    //   1. Wait for the FIRST result to arrive (no timeout).
    //   2. Note which tier landed and its priority index.
    //   3. If it's the best tier (idx 0), return immediately — nothing
    //      better is coming.
    //   4. Otherwise wait up to GRACE_MS for any higher-priority tier
    //      to land. If one does, use it. If not, return the best we have.
    let mut best: Option<(usize, String)> = None;
    let first = match rx.recv() {
        Ok(r) => r,
        Err(_) => return Err("All models failed (channel closed)".into()),
    };
    crate::clog!("whisper", "{:?} returned at +{:.3}s",
        first.1, t_start.elapsed().as_secs_f32());
    match first.2 {
        Ok(text) => best = Some((first.0, text)),
        Err(e) => crate::clog!("whisper", "{:?} failed: {}", first.1, e),
    }
    // If the first result is the BEST tier (idx 0), return immediately.
    // No grace needed because nothing better is in flight.
    if let Some((0, text)) = &best {
        crate::clog!("whisper", "using {:?} (best tier, arrived first)", pairs[0].0);
        return Ok(text.clone());
    }

    // Grace loop: keep collecting results, upgrade `best` when a higher-
    // priority result arrives. Stop when we hit GRACE_MS from t_start
    // OR when we've heard from all tiers OR when we have idx 0.
    let deadline = t_start + Duration::from_millis(grace_ms);
    loop {
        let now = Instant::now();
        if now >= deadline { break; }
        let timeout = deadline - now;
        match rx.recv_timeout(timeout) {
            Ok((idx, tier, r)) => {
                crate::clog!("whisper", "{:?} returned at +{:.3}s",
                    tier, t_start.elapsed().as_secs_f32());
                match r {
                    Ok(text) => {
                        let cur_idx = best.as_ref().map(|(i, _)| *i).unwrap_or(usize::MAX);
                        if idx < cur_idx {
                            best = Some((idx, text));
                            if idx == 0 {
                                // Top tier landed in the grace window — done.
                                crate::clog!("whisper", "using {:?} (top tier, within grace)", tier);
                                return Ok(best.unwrap().1);
                            }
                        }
                    }
                    Err(e) => crate::clog!("whisper", "{:?} failed: {}", tier, e),
                }
            }
            Err(_) => break, // Timeout or channel closed.
        }
    }

    match best {
        Some((idx, text)) => {
            crate::clog!("whisper", "using {:?} (grace window expired)", pairs[idx].0);
            Ok(text)
        }
        None => Err("All models failed".into()),
    }
}


#[cfg(feature = "voice")]
fn run_one(ctx: &whisper_rs::WhisperContext, audio: &[f32], tier: ModelTier) -> Result<String, String> {
    let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(4);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // v0.1.110: configurable language. Default "en". For non-English
    // users, set WEB2QML_VOICE_LANG (e.g. "es", "fr", "ja"). Only
    // takes effect if a multilingual model is loaded — tiny.en /
    // base.en are English-only and will ignore non-"en" requests.
    //
    // We pass the configured code straight to whisper. Whisper falls
    // back to auto-detect if the code is unrecognized.
    let lang_code = crate::voice_lang::whisper_lang_code();
    params.set_language(lang_code);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_suppress_nst(true);
    // Initial prompt: heavily English-biased. For non-English users
    // this prompt actually HARMS recognition by anchoring whisper on
    // English. Only set the prompt when we're in English mode.
    if crate::voice_lang::current() == "en" {
        params.set_initial_prompt(
            "open github, navigate to apple.com, search for rust async, scroll down, \
             go back, switch to terminal, run git status, alx.sh, news.ycombinator.com, \
             lore.kernel.org, sourcehut, web2qml, ollama, tinyllama, helix editor, \
             show history, show menu, find on page, copy URL, edit URL, A B C D E F, \
             cd tilde slash code, cd tilde slash documents slash work, \
             cd dot dot slash src, cd slash etc slash hosts, \
             go to tilde slash projects, navigate to tilde slash downloads, \
             change directory to slash usr slash local slash bin, \
             control C, control Z, escape colon W Q, save and quit, \
             exit helix, quit vim, kill the running process, \
             open first error, open next error, open next warning, \
             show second commit, show third one, run cargo build, run cargo test"
        );
    }

    // Take the cached state for this tier (or None on first call).
    // We hold the Mutex across the inference to serialize within the
    // tier — concurrent foreground+bg calls on the same tier would
    // race state internals otherwise. Different tiers run independent
    // inferences in parallel because they have separate state caches.
    let cache = match tier {
        ModelTier::Tiny         => &TINY_STATE,
        ModelTier::Base         => &BASE_STATE,
        ModelTier::HighAccuracy => &LARGE_STATE,
    };
    let cell = cache.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().map_err(|e| format!("state lock {:?}: {:?}", tier, e))?;

    // Lazy-init the state on first call; reuse on subsequent calls.
    // This is the speedup vs creating a fresh state every time —
    // saves the ~210MB allocation visible in whisper_init_state logs.
    let mut state = match guard.take() {
        Some(s) => s,
        None => ctx.create_state().map_err(|e| format!("State {:?}: {:?}", tier, e))?,
    };

    let result = state.full(params, audio).map_err(|e| format!("Transcribe {:?}: {:?}", tier, e));

    // Always put the state back, even on error — we want to reuse
    // it even after a failed inference (the state's buffers are
    // still valid; whisper-rs resets them on next .full() call).
    let n = state.full_n_segments();
    let text: String = (0..n)
        .filter_map(|i| state.get_segment(i))
        .filter_map(|seg| seg.to_str().ok().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    *guard = Some(state);
    drop(guard);

    result?;
    Ok(strip_whisper_artifacts(text.trim()))
}



fn strip_whisper_artifacts(s: &str) -> String {
    // Trim trailing/leading commas and stray punctuation that whisper
    // routinely adds. Examples: ", DNF, search python" → "DNF search
    // python", "test," → "test", ", home page," → "home page". Trailing
    // commas are particularly dangerous because they get appended to
    // shell command arguments when the recognized text is sent through.
    //
    // We do NOT strip mid-string commas — those CAN be intentional in
    // sentences like "list X, Y, Z". The pipe-chain parser handles the
    // ", ... | ..." case separately.
    let mut out = s.to_string();
    let trim_chars: &[char] = &[',', '.', '!', '?', ';', ':', ' ', '\t', '\n', '\r'];
    out = out.trim_matches(trim_chars).to_string();
    // After trim, also collapse the very-common whisper pattern ", word"
    // → "word" at the start (whisper sometimes encodes a quiet breath
    // as a leading comma + space + the actual word). The trim above
    // already handles plain ", " but if there's whitespace between the
    // comma and the next word that survived, normalize it here.
    while out.starts_with(", ") || out.starts_with(",  ") {
        out = out[2..].trim_start().to_string();
    }
    let s = out.as_str();
    let mut out = s.to_string();
    let artifacts = [
        "[BLANK_AUDIO]", "[NO_SPEECH]", "(silence)", "[silence]",
        "[Music]", "[music]", "[MUSIC]", "(music)",
        "(speaking in foreign language)", "[speaking in foreign language]",
        "(coughing)", "(laughing)", "(applause)", "[Applause]",
        "(typing)", "(door closing)", ". . .", "...",
    ];
    for a in &artifacts {
        out = out.replace(a, "");
    }
    let out = out.trim();

    // Whisper hallucinates leading discourse markers when the audio starts with
    // non-speech sound (chime tail, room noise, breath). These are NOT real
    // speech — strip them. Only strips at the start, only when followed by space.
    // Conservative: capitalised "And " stays (could be a real sentence start),
    // we only strip lowercase variants which whisper emits for hallucinations.
    let leading_hallucinations = [
        "and ", "And ", "so ", "So ", "uh ", "Uh ", "um ", "Um ",
        "okay ", "Okay ", "you ", "You ",
    ];
    let mut cleaned = out.to_string();
    // Strip up to 2 leading hallucination tokens (sometimes whisper emits "And so ...")
    for _ in 0..2 {
        let mut stripped = false;
        for h in &leading_hallucinations {
            if let Some(rest) = cleaned.strip_prefix(h) {
                eprintln!("Voice: stripping whisper hallucination prefix {:?}", h);
                cleaned = rest.to_string();
                stripped = true;
                break;
            }
        }
        if !stripped { break; }
    }
    // Path-token translation. Whisper transcribes paths as English words
    // ("tilde slash code slash kwin") because that's how people speak them.
    // Translate these into actual path syntax (~/Code/kwin) so all downstream
    // parsers see clean text. This is the right layer for this transform —
    // doing it here means every consumer (cd matcher, cmd-index, entity
    // resolver) gets clean input without each one re-implementing the same
    // English-to-path mapping. It also positions us for a custom voice model
    // later: we can train a model that natively outputs ~/Code instead of
    // "tilde slash code", and at that point this pass becomes a no-op.
    let cleaned = translate_path_tokens(&cleaned);
    // Normalize stray spaces around dots in numeric sequences. Whisper
    // routinely transcribes "one ninety two dot one sixty dot two dot one
    // seventy" as "192 .160.2.170" — a number, a space, then ".160.2.170"
    // glued together. We want IP addresses, version numbers, and similar
    // numeric-dot sequences to come out as "192.160.2.170".
    //
    // Rule: collapse "<digit><spaces>.<digit>" → "<digit>.<digit>" and
    // "<digit>.<spaces><digit>" → "<digit>.<digit>". Only when both sides
    // are digits — preserves prose like "I have 5 . The next one..." (which
    // a user wouldn't say anyway, but the constraint keeps us safe).
    let cleaned = normalize_numeric_dots(&cleaned);

    cleaned.trim().to_string()
}


fn normalize_numeric_dots(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Look for: digit, then whitespace, then '.', then digit → emit "X.Y"
        if c.is_ascii_digit() && i + 1 < bytes.len() {
            // Find the end of any whitespace gap
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            if j < bytes.len() && bytes[j] == b'.' && j + 1 < bytes.len()
                && bytes[j+1].is_ascii_digit() && j > i + 1 {
                // Stray-space-before-dot pattern. Collapse the gap.
                out.push(c as char);
                out.push('.');
                i = j + 1;
                continue;
            }
        }
        // Also: digit, then '.', then whitespace, then digit → "X.Y"
        if c.is_ascii_digit() && i + 2 < bytes.len() && bytes[i+1] == b'.'
            && (bytes[i+2] == b' ' || bytes[i+2] == b'\t') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            if j < bytes.len() && bytes[j].is_ascii_digit() {
                out.push(c as char);
                out.push('.');
                i = j;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

fn translate_path_tokens(input: &str) -> String {
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.is_empty() { return input.to_string(); }

    // Strip trailing sentence-end punctuation that whisper appends
    // ("kwin." → "kwin"). We do this by trimming each word's trailing
    // ASCII period/comma/!/? before matching. This isn't lossy — if a
    // user actually says "dot" they spell it out, and that's a separate
    // token. The only false-positive would be a path that legitimately
    // ends with "." which is rare and recoverable.
    fn trim_punct(w: &str) -> &str {
        w.trim_end_matches(|c: char| c == '.' || c == ',' || c == '!' || c == '?')
    }

    let mut out_parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let raw = trim_punct(words[i]);
        let lc = raw.to_lowercase();
        // Is this word the start of a path-token run? Whisper has phonetic
        // confusions: "tilda" and "tilly" for "tilde" are common. We accept
        // these as the start of a path run too.
        let is_path_start = matches!(lc.as_str(),
            "tilde" | "tilda" | "tilly" | "slash" | "backslash" | "dot");
        if !is_path_start {
            out_parts.push(words[i].to_string());
            i += 1;
            continue;
        }
        // Build the path. Continue as long as the next token is a path-
        // token OR an alphanumeric word that's CONTIGUOUS in path-shape
        // (no path token between component words ends the run).
        let mut path = String::new();
        let mut last_was_separator = false;  // whether the last token wrote a separator (slash/tilde/dot)
        while i < words.len() {
            let raw = trim_punct(words[i]);
            let lc = raw.to_lowercase();
            match lc.as_str() {
                "tilde" | "tilda" | "tilly" => { path.push('~'); last_was_separator = true; }
                "slash" => { path.push('/'); last_was_separator = true; }
                "backslash" => { path.push('\\'); last_was_separator = true; }
                "dot" => {
                    if i + 1 < words.len() && trim_punct(words[i+1]).to_lowercase() == "dot" {
                        path.push_str("..");
                        i += 1;
                    } else {
                        path.push('.');
                    }
                    last_was_separator = true;
                }
                "dash" | "hyphen" => { path.push('-'); last_was_separator = false; }
                "underscore" => { path.push('_'); last_was_separator = false; }
                _ => {
                    // Plain word. Three possibilities:
                    //  1. last_was_separator=true: clearly a path component
                    //     (e.g. after "slash"). Consume.
                    //  2. last_was_separator=false but NEXT word is a path-
                    //     separator (slash/dot/tilde): the path run is
                    //     continuing and this word is a fragment whisper
                    //     split off the previous component (e.g. "kde" got
                    //     transcribed as "keg E" → next is "slash kwin").
                    //     Glue this fragment to the previous component
                    //     directly (no space) and continue.
                    //  3. last_was_separator=false and next word is also
                    //     non-separator: this is post-path English. Stop
                    //     the run BEFORE consuming this word.
                    if last_was_separator {
                        path.push_str(&capitalize_known_dir(&lc));
                        last_was_separator = false;
                    } else {
                        let next_is_sep = i + 1 < words.len() && {
                            let n = trim_punct(words[i+1]).to_lowercase();
                            n == "slash" || n == "tilde" || n == "tilda" || n == "tilly"
                                || n == "dot" || n == "backslash"
                        };
                        if next_is_sep {
                            // Glue fragment to the existing path component.
                            // Don't capitalize (it's a fragment, not a known-dir name).
                            path.push_str(&lc);
                        } else {
                            break;
                        }
                    }
                }
            }
            i += 1;
        }
        out_parts.push(path);
    }

    out_parts.join(" ")
}


fn capitalize_known_dir(w: &str) -> String {
    match w {
        "code" => "Code".into(),
        "documents" => "Documents".into(),
        "downloads" => "Downloads".into(),
        "desktop" => "Desktop".into(),
        "pictures" => "Pictures".into(),
        "videos" => "Videos".into(),
        "music" => "Music".into(),
        "public" => "Public".into(),
        "templates" => "Templates".into(),
        _ => w.to_string(),
    }
}


fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate { return input.to_vec(); }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        if src_idx >= input.len() { break; }
        let frac = src_pos - src_idx as f64;
        let next_idx = (src_idx + 1).min(input.len() - 1);
        let s = input[src_idx] as f64 * (1.0 - frac) + input[next_idx] as f64 * frac;
        out.push(s as f32);
    }
    out
}

/// Single-pole high-pass filter to attenuate sub-80Hz content. Real
/// speech has fundamentals from ~85Hz (deep male voice) up to ~255Hz
/// (children); harmonics carry intelligibility upward. Sub-80Hz energy
/// is almost always non-speech: AC mains hum (50/60Hz + harmonics),
/// HVAC rumble, traffic, electrical buzz from poor cables, USB noise.
///
/// The filter:
///     y[n] = α * (y[n-1] + x[n] - x[n-1])
///
/// where α = exp(-2π·fc / fs) for cutoff fc and sample rate fs.
/// At fc=80Hz, fs=16000Hz, α ≈ 0.969. This is a very light touch —
/// barely audible on speech, big effect on rumble.
///
/// Applied to the 16kHz resampled signal just before whisper.
/// Cheap: 2 mults + 2 adds per sample, ~32k ops on a 6s capture.
pub fn high_pass_80hz(input: &mut [f32]) {
    if input.len() < 2 { return; }
    // Coefficient for ~80Hz cutoff at 16kHz.
    // α = exp(-2π·80/16000) = exp(-0.0314) ≈ 0.96904
    const ALPHA: f32 = 0.969;
    let mut prev_x = input[0];
    let mut prev_y = 0.0f32;
    for s in input.iter_mut() {
        let x = *s;
        let y = ALPHA * (prev_y + x - prev_x);
        *s = y;
        prev_x = x;
        prev_y = y;
    }
}

/// v0.1.783: gentle pre-whisper gain for quiet captures (soft speech, distant
/// mic, low input gain). Whisper is fairly level-robust but not level-INVARIANT,
/// and very low amplitude loses detail. This scales toward a target RMS but is
/// deliberately conservative so it can't hurt:
///   - never ATTENUATES (gain ≥ 1) — loud audio is left alone;
///   - a hard PEAK CEILING clamps the gain so the signal can never clip;
///   - MAX_GAIN caps boost so a near-silent / noise-only buffer isn't blown up
///     into amplified hiss (which whisper would hallucinate words from);
///   - pure silence (peak below a floor) is skipped entirely.
/// Returns the gain applied (1.0 = untouched) so the caller can log it. Pure +
/// cheap (one accumulating pass, one scale pass). Opt-in via WEB2QML_VOICE_GAIN.
pub fn normalize_gain(input: &mut [f32]) -> f32 {
    if input.is_empty() { return 1.0; }
    let mut peak = 0.0f32;
    let mut sumsq = 0.0f64;
    for &s in input.iter() {
        let a = s.abs();
        if a > peak { peak = a; }
        sumsq += (s as f64) * (s as f64);
    }
    if peak < 1e-4 { return 1.0; }                 // silence — don't amplify the floor
    let rms = (sumsq / input.len() as f64).sqrt() as f32;
    const TARGET_RMS: f32 = 0.10;                  // comfortable speech level
    const PEAK_CEIL: f32 = 0.95;                   // clip guard
    const MAX_GAIN: f32 = 4.0;                     // bound noise amplification
    if rms >= TARGET_RMS { return 1.0; }           // already loud enough
    let gain_rms = TARGET_RMS / rms.max(1e-6);
    let gain_peak = PEAK_CEIL / peak;              // gain that would just reach the ceiling
    let gain = gain_rms.min(gain_peak).min(MAX_GAIN).max(1.0);
    if (gain - 1.0).abs() < 1e-3 { return 1.0; }   // nothing meaningful to do
    for s in input.iter_mut() { *s *= gain; }
    gain
}

pub(crate) fn listen_external() -> String {
    // whisper-cpp / whisper-cli with arecord
    let model = model_path();
    let model_str = model.to_string_lossy();
    let cmd = format!(
        "arecord -d 5 -f S16_LE -r 16000 -c 1 /tmp/web2qml_voice.wav 2>/dev/null && \
         (which whisper-cli >/dev/null && whisper-cli -m '{}' -f /tmp/web2qml_voice.wav --no-prints 2>/dev/null | tail -1 || \
          which whisper-cpp >/dev/null && whisper-cpp -m '{}' -f /tmp/web2qml_voice.wav --no-prints 2>/dev/null | tail -1)",
        model_str, model_str
    );

    if let Ok(output) = std::process::Command::new("sh").arg("-c").arg(&cmd).output() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() { return text; }
    }

    // nerd-dictation fallback
    if let Ok(output) = std::process::Command::new("sh")
        .arg("-c")
        .arg("which nerd-dictation >/dev/null && timeout 5 nerd-dictation begin --output STDOUT 2>/dev/null")
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() { return text; }
        }
    }

    let model_path = model_path();
    if !model_path.exists() {
        format!("[Voice tools not available. Build with --features voice and download model to {}]", model_path.display())
    } else {
        "[Voice failed: arecord/whisper not in PATH]".into()
    }
}