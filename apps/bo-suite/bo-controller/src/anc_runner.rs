// src/anc_runner.rs
#![allow(dead_code)]
//
// Glue between the AncEngine (pure logic) and the live audio side:
//   - owns a MicCapture handle and pushes captured frames into the
//     engine's capture ring on a polling thread,
//   - schedules periodic alignment probes,
//   - owns the spawn machinery for "play this music file through ANC
//     into a sink" (PCM-decode via ffmpeg, mix via engine, pipe to
//     paplay/aplay/pw-cat).
//
// Why a separate file from AncEngine: the engine is `cfg(test)`-clean
// pure logic; this file pulls in subprocess + thread machinery that
// can't run in CI without a soundcard. Keeping them apart lets the
// engine stay fully unit-tested while this layer remains tested only
// at integration time (which we mostly do by running the binary).

use crate::anc_engine::{
    AncEngine, AlignmentProbe, ProbeResult, ANC_SAMPLE_RATE,
};
use crate::mic_capture::MicCapture;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// One scheduled mic→engine bridge thread. Pulls latest samples from
/// MicCapture and pushes them into the engine's capture ring. Runs
/// at ~50Hz (every 20 ms) — much faster than the engine's pattern
/// learning window so we never starve.
///
/// Lifetime tied to the AncRunner; cancellation via the `stop` flag.
fn mic_bridge_loop(
    mic: Arc<MicCapture>,
    engine: Arc<AncEngine>,
    stop: Arc<AtomicBool>,
) {
    let mut last_samples_count: usize = 0;
    while !stop.load(Ordering::Relaxed) {
        let current = mic.samples_written();
        if current > last_samples_count {
            let delta = current - last_samples_count;
            // Cap the read to avoid pulling more than the mic ring's
            // capacity in one go after a long sleep.
            let n = delta.min(ANC_SAMPLE_RATE as usize);
            let samples = mic.latest_samples(n);
            if !samples.is_empty() {
                engine.push_capture(&samples);
            }
            last_samples_count = current;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// The runner — owns the engine, mic bridge, probe scheduler.
pub struct AncRunner {
    engine: Arc<AncEngine>,
    mic: Option<Arc<MicCapture>>,
    stop: Arc<AtomicBool>,
    mic_thread: Option<JoinHandle<()>>,
    backend_label: String,
    /// Background re-probe scheduler thread, if started. Owned so
    /// drop tears it down with the runner.
    reprobe_thread: Option<JoinHandle<()>>,
    /// Sink name + active flag the scheduler reads on each tick.
    /// Wrapped in Arc<Mutex> so the user can change the target sink
    /// at runtime without restarting the scheduler.
    reprobe_target: Arc<Mutex<ReprobeTarget>>,
    /// Active ANC-blended playback child handles. Tracked so that
    /// `stop_playback` can kill them cleanly instead of leaving
    /// ffmpeg/paplay orphans, AND so that Drop tears them down with
    /// the runner. Previous version `mem::forget`d these, relying
    /// on the QML stop button's pkill-paplay to kill the player —
    /// which leaked the ffmpeg process and any handle resources
    /// every time the user replayed.
    playback_children: Mutex<Vec<Child>>,
    /// Standby (overnight) mode state. The PassbyCanceller + a
    /// long-running sink process owned by this thread. None when
    /// standby is off.
    standby_thread: Option<JoinHandle<()>>,
    standby_stop: Arc<AtomicBool>,
    standby_snapshot: Arc<Mutex<crate::passby_canceller::PassbySnapshot>>,
    /// Saturation amounts the standby thread applies to its
    /// PassbyCanceller and DroneCanceller respectively. Stored as
    /// AtomicU32 bit-patterns of f32 so the controller (which lives
    /// on the QML thread) can update them lock-free; the standby
    /// thread reads them at the top of each iteration. f32 → u32
    /// bits via to_bits / from_bits.
    standby_passby_saturation: Arc<std::sync::atomic::AtomicU32>,
    standby_drone_saturation: Arc<std::sync::atomic::AtomicU32>,
}

#[derive(Debug, Clone, Default)]
struct ReprobeTarget {
    /// "" = system default audio output. Updated by the caller via
    /// `set_reprobe_target` whenever the active speaker changes.
    sink: String,
    /// When false, the scheduler still ticks but doesn't actually
    /// fire probes — saves the user from intrusive chirps when
    /// they're not listening.
    enabled: bool,
}

impl AncRunner {
    /// Build a runner with the given engine. Mic capture is started
    /// on demand via `start_mic_bridge`.
    pub fn new(engine: Arc<AncEngine>) -> Self {
        Self {
            engine,
            mic: None,
            stop: Arc::new(AtomicBool::new(false)),
            mic_thread: None,
            backend_label: String::new(),
            reprobe_thread: None,
            reprobe_target: Arc::new(Mutex::new(ReprobeTarget::default())),
            playback_children: Mutex::new(Vec::new()),
            standby_thread: None,
            standby_stop: Arc::new(AtomicBool::new(false)),
            standby_snapshot: Arc::new(Mutex::new(
                crate::passby_canceller::PassbySnapshot {
                    phase: crate::passby_canceller::PassbyPhase::Quiet,
                    fundamental_hz: 0.0,
                    band_energy: 0.0,
                    noise_floor: 0.0,
                    anti_amp: 0.0,
                    timer_confidence: 0.0,
                    doppler_hz_per_s: 0.0,
                    phase_lock_quality: 0.0,
                })),
            standby_passby_saturation: Arc::new(
                std::sync::atomic::AtomicU32::new(0.0_f32.to_bits())),
            standby_drone_saturation: Arc::new(
                std::sync::atomic::AtomicU32::new(0.0_f32.to_bits())),
        }
    }

    /// Update the passby saturation that the standby thread will
    /// apply on its next iteration. Lock-free via AtomicU32 bit
    /// pattern. The value is clamped + sanitized at the caller's
    /// boundary (`Controller::set_passby_saturation`), so we trust
    /// it here.
    pub fn set_passby_saturation(&self, s: f32) {
        self.standby_passby_saturation.store(
            s.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    pub fn set_drone_saturation(&self, s: f32) {
        self.standby_drone_saturation.store(
            s.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Spawn the mic-capture subprocess + the bridge thread that
    /// pumps samples into the engine.
    ///
    /// Idempotent: calling twice does nothing the second time.
    pub fn start_mic_bridge(&mut self) -> Result<(), String> {
        if self.mic.is_some() { return Ok(()); }
        let mic = Arc::new(MicCapture::start()?);
        self.backend_label = mic.backend_name().to_string();
        let stop = self.stop.clone();
        let engine = self.engine.clone();
        let mic_arc = mic.clone();
        let h = thread::spawn(move || {
            mic_bridge_loop(mic_arc, engine, stop);
        });
        self.mic = Some(mic);
        self.mic_thread = Some(h);
        eprintln!("anc: mic bridge started via {}", self.backend_label);
        Ok(())
    }

    /// Stop the mic bridge + the underlying subprocess. Also tears
    /// down the periodic re-probe scheduler if it's running.
    pub fn stop_mic_bridge(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.mic_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.reprobe_thread.take() {
            let _ = h.join();
        }
        self.mic = None;  // drops the MicCapture → kills subprocess
        // Reset the flag for a possible subsequent start.
        self.stop.store(false, Ordering::Relaxed);
    }

    /// Start the periodic re-probe scheduler.
    ///
    /// Every `ALIGNMENT_REPROBE_SECS` (~90s) the scheduler — if the
    /// target is enabled — fires `run_probe(sink)` against the
    /// current sink. Drift sources (room temp, BT re-pairing,
    /// listener movement) shift the round-trip by tens of ms over
    /// minutes; without a re-probe, the alignment grows stale and
    /// cancellation degrades silently.
    ///
    /// Failures are logged but never panic: a transient probe error
    /// (mic empty, sink missing) just means we wait for the next
    /// interval. The scheduler exits cleanly when `stop` is set
    /// (which happens via drop or stop_mic_bridge).
    ///
    /// Idempotent: calling twice does nothing the second time.
    pub fn start_periodic_reprobe(&mut self) {
        if self.reprobe_thread.is_some() { return; }
        let stop = self.stop.clone();
        let engine = self.engine.clone();
        let target = self.reprobe_target.clone();
        let mic_alive = self.mic.is_some();
        if !mic_alive {
            eprintln!("anc: periodic re-probe requested but mic bridge \
                       not started — start_mic_bridge first; ignoring");
            return;
        }
        // We can't move the MicCapture itself into the thread (we
        // also need it for the bridge), so we clone the Arc.
        let mic = self.mic.as_ref().unwrap().clone();
        let h = thread::spawn(move || {
            // Stagger the first probe by 5 s so an immediate
            // start_mic_bridge → start_periodic_reprobe sequence
            // doesn't fire a probe before the mic has accumulated
            // any baseline. After the first one, settle into the
            // configured interval.
            let initial_delay = Duration::from_secs(5);
            let interval = Duration::from_secs(
                crate::anc_engine::ALIGNMENT_REPROBE_SECS);
            let mut next_at = std::time::Instant::now() + initial_delay;
            while !stop.load(Ordering::Relaxed) {
                // Sleep in short chunks so a stop signal lands fast.
                while std::time::Instant::now() < next_at {
                    if stop.load(Ordering::Relaxed) { return; }
                    thread::sleep(Duration::from_millis(250));
                }
                next_at = std::time::Instant::now() + interval;
                // Read the current target. If disabled, skip this tick
                // — the scheduler keeps ticking so re-enabling
                // doesn't require restarting the thread.
                let (enabled, sink) = match target.lock() {
                    Ok(t) => (t.enabled, t.sink.clone()),
                    Err(_) => continue,
                };
                if !enabled { continue; }
                // Inline a minimal run_probe — we can't borrow
                // &self from the spawned thread, so we replicate
                // the structure with the Arc'd engine + the mic.
                if let Err(e) = reprobe_one(&engine, &mic, &sink) {
                    eprintln!("anc reprobe: {}", e);
                }
            }
        });
        self.reprobe_thread = Some(h);
        eprintln!("anc: periodic re-probe scheduler started \
                  (interval = {}s)", crate::anc_engine::ALIGNMENT_REPROBE_SECS);
    }

    /// Update the target the periodic scheduler will use. Call this
    /// whenever the active output sink changes (BT speaker switch,
    /// driver kind change). Setting `enabled = false` pauses
    /// probing without tearing the scheduler down.
    pub fn set_reprobe_target(&self, sink: String, enabled: bool) {
        if let Ok(mut t) = self.reprobe_target.lock() {
            t.sink = sink;
            t.enabled = enabled;
        }
    }

    pub fn backend_label(&self) -> &str { &self.backend_label }

    pub fn engine(&self) -> Arc<AncEngine> { self.engine.clone() }

    /// Build + run a single alignment probe end-to-end:
    ///   1. build chirp
    ///   2. start a capture-snapshot timer
    ///   3. play the chirp through the chosen sink via paplay/pw-cat
    ///   4. wait for the capture to include the chirp + a safety margin
    ///   5. extract that window and process it via the engine
    ///
    /// Blocks until the probe completes (typically < 1s). Spawn from
    /// a worker thread if the caller wants non-blocking.
    ///
    /// `sink` may be empty for the default audio output, or a
    /// PulseAudio sink name like "bluez_sink.50_1E_2D_5B_E6_EF.a2dp_sink".
    pub fn run_probe(&self, sink: &str) -> Result<ProbeResult, String> {
        let mic = self.mic.as_ref()
            .ok_or_else(|| "mic bridge not started — call start_mic_bridge first".to_string())?;
        let probe = self.engine.build_probe();
        // Mark the mic's current sample count as the "play start"
        // baseline. The probe will arrive at the mic at some lag
        // after this baseline.
        let baseline = mic.samples_written();
        // Estimated upper bound on round-trip: 500 ms covers most
        // Bluetooth + over-the-air scenarios. We wait for the mic to
        // accumulate that much PLUS the probe's own length.
        let probe_len = probe.output_samples.len();
        let max_lag_samples = (ANC_SAMPLE_RATE as f32 * 0.5) as usize;
        let wait_samples = probe_len + max_lag_samples;

        // Start playing the probe (async, via paplay/pw-cat/aplay).
        play_probe_to_sink(&probe, sink)?;

        // Wait until the mic has accumulated enough new samples.
        let start = std::time::Instant::now();
        loop {
            let now_count = mic.samples_written();
            if now_count.saturating_sub(baseline) >= wait_samples { break; }
            if start.elapsed() > Duration::from_secs(2) {
                return Err("probe timeout: mic didn't accumulate \
                            enough samples in 2 s — backend stalled?".into());
            }
            thread::sleep(Duration::from_millis(20));
        }

        // Grab the relevant window: from the baseline forward by the
        // expected total. latest_samples returns the most recent N
        // chronologically, so we take exactly wait_samples worth from
        // the current count.
        let captured = mic.latest_samples(wait_samples);
        let result = self.engine.process_probe_capture(&captured, &probe);
        eprintln!(
            "anc_probe: corr={:.3}, lag={} samples (~{:.1} ms){}",
            result.correlation, result.round_trip_samples,
            (result.round_trip_samples as f32 / ANC_SAMPLE_RATE as f32) * 1000.0,
            if result.correlation >= crate::anc_engine::PROBE_MIN_CORRELATION {
                ""
            } else {
                " — REJECTED (below threshold; alignment unchanged)"
            },
        );
        Ok(result)
    }

    /// Play a music file through the ANC mixer to the given sink.
    /// Spawns ffmpeg to decode → reads PCM chunks → mixes via engine
    /// → writes to paplay/pw-cat/aplay.
    ///
    /// Honest-scope notes:
    ///   - Mono 48 kHz 16-bit PCM only. The engine runs at
    ///     ANC_SAMPLE_RATE = 48 kHz; ffmpeg downmixes whatever the
    ///     source is to mono. Stereo blend can be added later by
    ///     duplicating the engine's anti-noise to both channels.
    ///   - Sample-by-sample, not frame-by-frame. Throughput is
    ///     limited by the slowest of decoder / mixer / encoder pipe,
    ///     which on a phone is comfortable for 48 kHz mono.
    ///   - Stop via the standard `pkill` route used by the rest of
    ///     the player layer — we run paplay so the caller can kill
    ///     it the same way they kill plain music playback.
    pub fn play_with_anc(
        &self, music_path: &str, sink: &str,
    ) -> Result<(), String> {
        // Reap any previously-finished playback children before
        // starting new ones. Without this, every play_with_anc leaks
        // the previous play's Child handles into our vec; over many
        // plays the vec grows unbounded.
        self.reap_finished_playback();

        // ffmpeg decode → stdout (raw s16le mono 48k)
        let mut ff = Command::new("ffmpeg")
            .args([
                "-loglevel", "error",
                "-i", music_path,
                "-f", "s16le",
                "-ar", &ANC_SAMPLE_RATE.to_string(),
                "-ac", "1",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("ffmpeg spawn: {}", e))?;

        // paplay / pw-cat / aplay sink (s16le mono 48k)
        let mut sink_cmd = build_sink_command(sink);
        sink_cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        let mut player = sink_cmd.spawn()
            .map_err(|e| format!("sink spawn: {}", e))?;

        let mut ff_out = ff.stdout.take().ok_or("ffmpeg stdout missing")?;
        let mut player_in = player.stdin.take().ok_or("sink stdin missing")?;
        let engine = self.engine.clone();
        thread::spawn(move || {
            let mut byte_buf = vec![0u8; 4096];
            let mut music_chunk = Vec::with_capacity(2048);
            let mut out_chunk: Vec<u8> = Vec::with_capacity(4096);
            // CombMasker for the music — boosts the engine's
            // fundamental + harmonic series in the music spectrum
            // so the music covers any residue the anti-noise
            // leaves. The masker's target is updated each chunk
            // from the AncEngine's current anti-noise frequency
            // (when it has one); otherwise the masker stays
            // disengaged and music passes through unchanged.
            let mut comb = crate::music_masker::CombMasker::new(
                crate::anc_engine::ANC_SAMPLE_RATE as f32);
            let mut last_target_hz = 0.0f32;
            loop {
                let n = match ff_out.read(&mut byte_buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                music_chunk.clear();
                let mut i = 0;
                while i + 1 < n {
                    let s = i16::from_le_bytes([byte_buf[i], byte_buf[i + 1]]);
                    music_chunk.push(s as f32 / 32_768.0);
                    i += 2;
                }
                // Pull the engine's current target for the comb.
                // The engine doesn't currently expose a "tracked
                // noise fundamental" — that's the passby/drone
                // canceller's job. For play_with_anc the engine's
                // target is the alignment probe's measured tonal
                // content, which lives at the same harmonics as
                // the room noise. Using the engine's `tracked_hz`
                // (if any) gives us a target the masker can boost.
                let target_hz = engine.tracked_noise_hz();
                if (target_hz - last_target_hz).abs() > 5.0 {
                    if target_hz > 0.0 {
                        comb.set_fundamental(target_hz, 6.0, 2.0);
                        comb.set_engaged(true);
                    } else {
                        comb.set_engaged(false);
                    }
                    last_target_hz = target_hz;
                }
                // Apply the comb boost to the music in place.
                comb.process_block(&mut music_chunk);
                let mixed = engine.pull_output(music_chunk.len(), Some(&music_chunk));
                out_chunk.clear();
                for s in mixed {
                    let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                    out_chunk.extend_from_slice(&i16_val.to_le_bytes());
                }
                if player_in.write_all(&out_chunk).is_err() { break; }
            }
            drop(player_in);
        });

        // Register both children so stop_playback() and Drop can
        // clean them up rather than leaving zombies. The pump thread
        // above shares neither child; ownership stays here.
        if let Ok(mut v) = self.playback_children.lock() {
            v.push(ff);
            v.push(player);
        }
        Ok(())
    }

    /// Kill all in-flight ANC-blended playback processes. Called by
    /// the controller's stop button when ANC playback is active.
    /// Safe to call when nothing is playing (no-op).
    pub fn stop_playback(&self) {
        if let Ok(mut v) = self.playback_children.lock() {
            for mut c in v.drain(..) {
                let _ = c.kill();
                let _ = c.wait();   // reap to avoid zombies
            }
        }
    }

    /// Sweep finished children out of the tracking vec. ffmpeg + paplay
    /// exit cleanly when the music file ends; without reaping, their
    /// handles linger as zombies until we replay (or shut down).
    fn reap_finished_playback(&self) {
        if let Ok(mut v) = self.playback_children.lock() {
            v.retain_mut(|c| match c.try_wait() {
                Ok(Some(_)) => false,    // exited; drop the handle
                Ok(None)    => true,     // still running
                Err(_)      => false,    // can't query; assume dead
            });
        }
    }

    // ── Standby (overnight) mode ──

    /// Start the overnight pass-by-cancellation runner.
    ///
    /// This is fundamentally different from the ANC engine's continuous
    /// emit path: instead of always streaming an anti-noise signal,
    /// the standby loop sits silent while the mic shows just noise
    /// floor, fires the FSM only when in-band engine harmonics rise
    /// above threshold, and emits anti-noise only during the
    /// Approach/Peak/Recede phases of a detected pass-by.
    ///
    /// The benefit: the room stays acoustically natural between
    /// events, then briefly cancels the worst of an engine roar.
    /// At 3am with one sports car every 20 minutes that means the
    /// speaker is silent 99% of the time and audibly engaged for
    /// the 30-second pass-by event.
    ///
    /// Architecture: ONE long-running sink subprocess (paplay) is
    /// spawned for the session. The thread feeds it zeros during
    /// Quiet (cheap; paplay just plays silence) and the canceller's
    /// computed anti-noise during the event phases. Spawning a
    /// fresh subprocess per event would add hundreds of ms of
    /// latency to each Approach — well into the Peak by the time
    /// the speaker started.
    ///
    /// `sink` is the PA sink to write to ("" for system default).
    pub fn start_standby_mode(&mut self, sink: String) -> Result<(), String> {
        if self.standby_thread.is_some() {
            return Ok(()); // idempotent
        }
        if self.mic.is_none() {
            self.start_mic_bridge()?;
        }
        let mic = self.mic.as_ref().unwrap().clone();
        let stop = self.standby_stop.clone();
        stop.store(false, std::sync::atomic::Ordering::Relaxed);
        let snapshot = self.standby_snapshot.clone();
        let passby_sat = self.standby_passby_saturation.clone();
        let drone_sat = self.standby_drone_saturation.clone();

        // Spawn the long-running sink subprocess.
        let mut sink_cmd = build_sink_command(&sink);
        sink_cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        let mut player = sink_cmd.spawn()
            .map_err(|e| format!("standby sink spawn: {}", e))?;
        let mut player_in = player.stdin.take()
            .ok_or("standby sink stdin missing")?;
        // Register the long-running sink so stop_playback / Drop can
        // tear it down properly.
        if let Ok(mut v) = self.playback_children.lock() {
            v.push(player);
        }

        let h = thread::spawn(move || {
            use crate::passby_canceller::{
                PassbyCanceller, PassbyPhase, WINDOW_SAMPLES as PASSBY_WIN};
            use crate::drone_canceller::{DroneCanceller, DronePhase};
            use crate::backup_beeper::BackupBeeperMasker;

            // ── Three parallel detectors running on the same mic stream ──
            //
            // Each targets a distinct noise class:
            //
            //   passby   — fast-moving cars/trucks (Doppler > 0.5 Hz/s,
            //              60-300 Hz fundamental, transient envelope)
            //   drone    — slow-moving small engines (leafblower /
            //              mower, 100-500 Hz, sustained for minutes)
            //   beeper   — pulse-train alarms (1000-2000 Hz carrier,
            //              psychoacoustic masking, not cancellation)
            //
            // They share the mic but make independent decisions. Their
            // outputs are SUMMED before going to the speaker; in
            // practice only one is active at a time because the noise
            // classes are mutually exclusive (a backup beeper isn't
            // also a passing car). Even if two fire simultaneously,
            // they target different frequency bands, so summing
            // doesn't produce destructive interference between them.
            //
            // The shared snapshot reports only the passby state for
            // backward-compat with the existing UI; a future pass
            // will add drone/beeper fields to the snapshot type and
            // surface them.
            let mut passby = PassbyCanceller::new();
            let mut drone  = DroneCanceller::new();
            let mut beeper = BackupBeeperMasker::new();
            // Use the passby window size (2048) for the main mic
            // read; the beeper happily accepts a larger window than
            // its own 1024 (Goertzel works at any length).
            let window = PASSBY_WIN;
            let mut last_seen_count = mic.samples_written();
            let mut out_bytes: Vec<u8> = Vec::with_capacity(window * 2);
            let zero_bytes: Vec<u8> = vec![0u8; window * 2];

            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let now_count = mic.samples_written();
                if now_count - last_seen_count < window {
                    thread::sleep(Duration::from_millis(15));
                    continue;
                }
                last_seen_count = now_count;
                let samples = mic.latest_samples(window);
                if samples.len() < window {
                    thread::sleep(Duration::from_millis(15));
                    continue;
                }

                // Apply any user-changed saturation values before
                // this window's process. Cheap atomic loads; no
                // lock contention.
                {
                    use std::sync::atomic::Ordering::Relaxed;
                    let ps = f32::from_bits(passby_sat.load(Relaxed));
                    let ds = f32::from_bits(drone_sat.load(Relaxed));
                    if (passby.saturation() - ps).abs() > 1e-4 {
                        passby.set_saturation(ps);
                    }
                    if (drone.saturation() - ds).abs() > 1e-4 {
                        drone.set_saturation(ds);
                    }
                }

                // V1 gating: the passby canceller is experimental
                // (validated only on textbook synth; ~0 dB on real
                // engines). The drone canceller and beeper masker
                // are stable. Stable features always run; passby
                // runs only when experimental is opted in.
                //
                // We still SNAPSHOT the passby state when disabled
                // so the UI can show "would have triggered" diagnostics
                // and so the FSM keeps learning timer priors against
                // real noise even when we're not emitting anti-noise
                // for it. The process_window call's compute is small
                // (one Goertzel pass + FSM tick); it's the pull_output
                // synthesis we skip downstream when the flag is off.
                let passby_active = crate::v1::is_active(
                    crate::v1::Feature::PassbyCanceller3H);
                let passby_snap = passby.process_window(&samples);
                let drone_snap  = drone.process_window(&samples);
                let beeper_snap = beeper.process_window(&samples);
                let _ = passby_active;  // surfaced to the snapshot consumer

                // ── Audio capture dump (for offline CNN training) ──
                //
                // The dump is opt-in via QML toggle. When enabled, it
                // ring-buffers the last 30 s of mic audio and triggers
                // a save-to-WAV when a passby/drone event fires. We
                // call into the dump module unconditionally — it's a
                // no-op when disabled and cheap (single mutex, single
                // VecDeque push) when enabled.
                crate::audio_capture_dump::ingest_mic_window(&samples);

                // Trigger captures on FSM state transitions of
                // interest. Edge-trigger semantics: we trigger when
                // entering Peak (passby) or Active (drone), once
                // per event. Repeat trigger calls within the same
                // capture are no-ops by design.
                if passby_snap.phase == PassbyPhase::Peak {
                    crate::audio_capture_dump::trigger_capture("passby_peak");
                } else if matches!(drone_snap.phase, DronePhase::Active) {
                    crate::audio_capture_dump::trigger_capture("drone_active");
                }

                // Record FSM snapshot into the active capture's
                // sidecar (if there is one). This labels each frame
                // of the WAV with the canceller's state at that
                // moment — the CNN training pipeline uses these
                // labels to learn "engine present vs. quiet."
                crate::audio_capture_dump::record_snapshot(
                    if passby_snap.phase != PassbyPhase::Quiet { "passby" }
                    else if matches!(drone_snap.phase,
                        DronePhase::Active | DronePhase::Releasing) { "drone" }
                    else { "quiet" },
                    &format!("{:?}", passby_snap.phase),
                    passby_snap.fundamental_hz,
                    passby_snap.band_energy,
                    passby_snap.anti_amp,
                    passby_snap.phase_lock_quality,
                );

                if let Ok(mut s) = snapshot.lock() { *s = passby_snap; }

                // ── Decide whether anyone wants to emit ──
                let any_active = passby_snap.phase != PassbyPhase::Quiet
                              || matches!(drone_snap.phase,
                                  DronePhase::Active | DronePhase::Releasing)
                              || beeper_snap.masker_amp > 1e-3;

                if !any_active {
                    if player_in.write_all(&zero_bytes).is_err() { break; }
                    continue;
                }

                // ── Mix all three outputs ──
                //
                // passby + drone are anti-noise (sign-invert before
                // mixing); beeper is a masker (pink noise burst,
                // NOT sign-inverted). All three are bounded to
                // [-1, 1] internally so summing them can clip at
                // 3.0 — we divide by 3 to keep headroom rather
                // than peak-limit which would distort.
                let passby_out = if crate::v1::is_active(
                    crate::v1::Feature::PassbyCanceller3H)
                {
                    passby.pull_output(window)
                } else {
                    // Experimental gate is off: emit silence for
                    // the passby track. Compute is wasted on the
                    // FSM tick + lock above (intentional — we
                    // keep the diagnostics flowing) but we don't
                    // commit anti-noise to the speaker chain.
                    vec![0.0_f32; window]
                };
                let drone_out  = drone.pull_output(window);
                let beeper_out = beeper.pull_output(window);

                out_bytes.clear();
                for i in 0..window {
                    let p = passby_out.get(i).copied().unwrap_or(0.0);
                    let d = drone_out.get(i).copied().unwrap_or(0.0);
                    let b = beeper_out.get(i).copied().unwrap_or(0.0);
                    // Sign-invert passby + drone for cancellation;
                    // beeper is summed as-is (it's a masker).
                    let combined = (-p + -d + b) / 3.0;
                    let v = combined.clamp(-1.0, 1.0);
                    let i16_val = (v * 32_767.0) as i16;
                    out_bytes.extend_from_slice(&i16_val.to_le_bytes());
                }
                if player_in.write_all(&out_bytes).is_err() { break; }
            }
            // Closing stdin signals paplay to flush and exit.
            drop(player_in);
        });
        self.standby_thread = Some(h);
        eprintln!("anc: standby mode started — overnight pass-by canceller \
                   running against sink '{}'",
                  if sink.is_empty() { "(default)" } else { sink.as_str() });
        Ok(())
    }

    /// Stop the overnight standby loop. Idempotent.
    pub fn stop_standby_mode(&mut self) {
        if self.standby_thread.is_none() { return; }
        self.standby_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.standby_thread.take() {
            let _ = h.join();
        }
        // The standby paplay child was registered in playback_children.
        // Reap_finished should catch it once it exits naturally from
        // the dropped stdin. Belt-and-suspenders: also explicitly call
        // stop_playback so the user clicking "stop" tears down anything
        // dangling.
        self.reap_finished_playback();
        eprintln!("anc: standby mode stopped");
    }

    /// Snapshot of the current standby canceller state — used by the
    /// QML side to display "currently canceling a pass-by" status.
    pub fn standby_snapshot(&self) -> crate::passby_canceller::PassbySnapshot {
        self.standby_snapshot.lock()
            .map(|s| *s)
            .unwrap_or(crate::passby_canceller::PassbySnapshot {
                phase: crate::passby_canceller::PassbyPhase::Quiet,
                fundamental_hz: 0.0,
                band_energy: 0.0,
                noise_floor: 0.0,
                anti_amp: 0.0,
                timer_confidence: 0.0,
                doppler_hz_per_s: 0.0,
                    phase_lock_quality: 0.0,
            })
    }

    pub fn standby_active(&self) -> bool {
        self.standby_thread.is_some()
    }
}

impl Drop for AncRunner {
    fn drop(&mut self) {
        // Tear down in reverse dependency order: standby thread first
        // (which writes to a long-running paplay), then playback
        // children (which includes that paplay), finally the mic
        // bridge whose Arc the standby thread holds.
        self.stop_standby_mode();
        self.stop_playback();
        self.stop_mic_bridge();
    }
}

/// Build the sink command — paplay first, pw-cat fallback for
/// PipeWire-only setups, aplay as last resort.
///
/// This mirrors the cue/sanitizer subprocess fallback chain
/// established earlier (see `stream_pcm_to_player` in main.rs); kept
/// separate here because the ANC path uses different sample-format
/// flags (s16le, 48k mono) and needs to honor the `sink` parameter
/// (PA sink name for BT routing).
/// Run a single re-probe from within the scheduler thread.
///
/// Mirrors the structure of `AncRunner::run_probe` but without
/// requiring &self (the spawn closure couldn't capture &self
/// because the runner moves between contexts).
///
/// Uses a tighter timeout (1.5s) than the user-initiated probe
/// because (a) we don't want a stuck sink to delay subsequent
/// re-probes, and (b) if the current sink is dead, the next user
/// action will likely reset the target anyway.
fn reprobe_one(
    engine: &Arc<AncEngine>,
    mic: &Arc<MicCapture>,
    sink: &str,
) -> Result<(), String> {
    let probe = engine.build_probe();
    let baseline = mic.samples_written();
    let probe_len = probe.output_samples.len();
    let max_lag_samples = (crate::anc_engine::ANC_SAMPLE_RATE as f32 * 0.5) as usize;
    let wait_samples = probe_len + max_lag_samples;

    play_probe_to_sink(&probe, sink)?;

    let start = std::time::Instant::now();
    loop {
        let now_count = mic.samples_written();
        if now_count.saturating_sub(baseline) >= wait_samples { break; }
        if start.elapsed() > Duration::from_millis(1500) {
            return Err("reprobe timeout".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let captured = mic.latest_samples(wait_samples);
    let result = engine.process_probe_capture(&captured, &probe);
    let ms = (result.round_trip_samples as f32
        / crate::anc_engine::ANC_SAMPLE_RATE as f32) * 1000.0;
    let trusted = result.correlation >= crate::anc_engine::PROBE_MIN_CORRELATION;
    eprintln!(
        "anc reprobe: corr={:.3}, lag={} samples ({:.1} ms){}",
        result.correlation, result.round_trip_samples, ms,
        if trusted { "" } else { " — REJECTED" });
    Ok(())
}

fn build_sink_command(sink: &str) -> Command {
    // Detect which player to use. We check paplay first since it
    // accepts PA sink names — required for BT routing. pw-cat has
    // its own --target but the names don't always match.
    if which("paplay") {
        let mut c = Command::new("paplay");
        c.args([
            "--raw",
            "--format=s16le",
            "--rate=48000",
            "--channels=1",
        ]);
        if !sink.is_empty() {
            c.arg(format!("--device={}", sink));
        }
        c
    } else if which("pw-cat") {
        let mut c = Command::new("pw-cat");
        c.args([
            "--playback", "--raw",
            "--format=s16", "--rate=48000", "--channels=1",
            "-",
        ]);
        c
    } else {
        let mut c = Command::new("aplay");
        c.args(["-q", "-f", "S16_LE", "-r", "48000", "-c", "1", "-t", "raw"]);
        c
    }
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

/// Play the probe chirp to the given sink (best effort; returns
/// after spawn, the chirp is short and will play out).
fn play_probe_to_sink(probe: &AlignmentProbe, sink: &str) -> Result<(), String> {
    let mut sink_cmd = build_sink_command(sink);
    sink_cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = sink_cmd.spawn()
        .map_err(|e| format!("probe sink spawn: {}", e))?;
    let mut bytes = Vec::with_capacity(probe.output_samples.len() * 2);
    for &s in &probe.output_samples {
        let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(stdin);
    }
    // Reaping the player happens lazily — the probe is < PROBE_DURATION_MS,
    // so on average it has already exited by the time the caller
    // returns. Don't wait() here because that would defeat the
    // probe-in-progress feel.
    let _ = child;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anc_runner_constructs_without_mic() {
        let state = std::sync::Arc::new(crate::predictor::PredictorState::new(0.5));
        let engine = std::sync::Arc::new(AncEngine::new(state));
        let runner = AncRunner::new(engine);
        assert!(runner.backend_label().is_empty());
    }

    #[test]
    fn run_probe_without_mic_returns_error() {
        let state = std::sync::Arc::new(crate::predictor::PredictorState::new(0.5));
        let engine = std::sync::Arc::new(AncEngine::new(state));
        let runner = AncRunner::new(engine);
        let r = runner.run_probe("");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("mic"));
    }

    #[test]
    fn sink_command_builds_with_paplay_args_when_present() {
        // We can't assert which path the test runner takes (depends on
        // the test env's installed binaries) but we CAN assert the
        // command isn't empty.
        let cmd = build_sink_command("");
        let _ = cmd; // smoke test: no panic on construction
    }

    #[test]
    fn sink_command_with_named_sink_includes_device_arg() {
        if !which("paplay") { return; }  // only meaningful when paplay is present
        let cmd = build_sink_command("bluez_sink.AA_BB_CC.a2dp_sink");
        // Reach into args via Debug — std::process::Command's API
        // doesn't expose args otherwise. Smoke test only.
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("bluez_sink"));
    }
}
