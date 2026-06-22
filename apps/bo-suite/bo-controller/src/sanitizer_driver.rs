// src/sanitizer_driver.rs
//
// Spawns a system audio-player subprocess (paplay, then aplay as fallback)
// and continuously pipes generated sanitizer audio (shaped noise +
// phase-tracked anti-tones) to its stdin. The result: the user hears the
// frequency sanitizer continuously through their default audio output,
// independent of whatever else is playing.
//
// Lifecycle:
//   1. SanitizerDriver::start() spawns the subprocess + a generator thread.
//      The generator thread holds a Sanitizer and pushes 16-bit mono PCM
//      to the subprocess's stdin in small chunks (defaults to 200ms).
//   2. SanitizerDriver::stop() drops the driver, which:
//        - signals the generator thread to exit
//        - closes the subprocess's stdin (causing it to exit cleanly)
//        - kills the subprocess if it doesn't exit within a timeout
//   3. SanitizerDriver::retune() updates the anti-tone fundamental on the
//      fly without restarting the subprocess (the generator thread picks
//      up the new frequency on its next chunk).
//
// Honest scope: This piggybacks on the local OS audio output, which
// means the sanitizer is mixed into whatever the user is hearing on their
// default sink. For multi-speaker distribution (the bo-mux mesh case),
// the audio would need to flow through the daemon's per-channel routes
// instead — that's a separate path documented in the masker module.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::masker::{MaskerPreset, Sanitizer};

const SAMPLE_RATE: u32 = 48_000;
const CHUNK_MS: u32 = 200;

/// Audio-player backend the driver will try, in order. Each variant maps
/// to a command and the appropriate flags to read 48 kHz mono 16-bit PCM
/// from stdin.
#[derive(Debug, Clone, Copy)]
enum Player {
    Paplay,
    Aplay,
}

impl Player {
    fn build(&self) -> Command {
        #[cfg(unix)]
        use std::os::unix::process::CommandExt;
        let mut c = match self {
            Player::Paplay => {
                let mut c = Command::new("paplay");
                c.args(["--raw", "--format=s16le", "--rate=48000", "--channels=1"]);
                c
            }
            Player::Aplay => {
                let mut c = Command::new("aplay");
                c.args(["-q", "-f", "S16_LE", "-r", "48000", "-c", "1"]);
                c
            }
        };
        c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        // PR_SET_PDEATHSIG so the cue process dies when the controller
        // does — same invariant as the music-playback paths.
        #[cfg(unix)]
        unsafe {
            c.pre_exec(|| {
                let r = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                if r != 0 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            });
        }
        c
    }

    fn detect() -> Option<Self> {
        for p in [Player::Paplay, Player::Aplay] {
            if which(p.cmd_name()).is_some() { return Some(p); }
        }
        None
    }

    fn cmd_name(&self) -> &'static str {
        match self { Player::Paplay => "paplay", Player::Aplay => "aplay" }
    }
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

/// Handle to a running sanitizer subprocess + generator thread.
pub struct SanitizerDriver {
    child: Arc<Mutex<Option<Child>>>,
    stop_flag: Arc<AtomicBool>,
    freq_bits: Arc<AtomicU32>,    // f32 stored as u32 bits for lock-free update
    thread: Option<JoinHandle<()>>,
    player_name: &'static str,
    /// Shared predictor state. The audio thread mixes its inverse-template
    /// output into the sanitizer output sample-by-sample. The pattern
    /// runner (owned by QtState) updates the template atomically as it
    /// learns from mic input.
    pub predictor_state: Arc<crate::predictor::PredictorState>,
}

impl SanitizerDriver {
    /// Start the sanitizer. Returns Err if no system audio player is
    /// available, or if the subprocess fails to spawn.
    pub fn start(preset: MaskerPreset, antitone_freq: Option<f32>, gain_db: f32)
        -> Result<Self, String>
    {
        let player = Player::detect()
            .ok_or_else(|| "no system audio player found (paplay/aplay)".to_string())?;
        let mut cmd = player.build();
        let mut child = cmd.spawn()
            .map_err(|e| format!("spawn {}: {}", player.cmd_name(), e))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| "subprocess stdin not available".to_string())?;

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let stop_flag = Arc::new(AtomicBool::new(false));
        // Encode initial freq as f32 bits; 0.0 means "no anti-tones".
        let initial_freq = antitone_freq.unwrap_or(0.0);
        let freq_bits = Arc::new(AtomicU32::new(initial_freq.to_bits()));
        // Predictor starts dormant (no template). The pattern runner
        // fills it when adaptive mode is enabled.
        let predictor_state = Arc::new(crate::predictor::PredictorState::new(0.5));

        let thread = {
            let stop = stop_flag.clone();
            let freq = freq_bits.clone();
            let pstate = predictor_state.clone();
            thread::spawn(move || run_generator(stdin, stop, freq, preset, gain_db, pstate))
        };

        Ok(Self {
            child: child_arc,
            stop_flag,
            freq_bits,
            thread: Some(thread),
            player_name: player.cmd_name(),
            predictor_state,
        })
    }

    /// Update the anti-tone fundamental on the fly. The generator thread
    /// picks this up on its next chunk boundary (≤ CHUNK_MS later).
    /// Pass 0.0 to disable anti-tones entirely (noise-only).
    pub fn retune(&self, hz: f32) {
        self.freq_bits.store(hz.to_bits(), Ordering::Relaxed);
    }

    pub fn player_name(&self) -> &'static str { self.player_name }
}

impl Drop for SanitizerDriver {
    fn drop(&mut self) {
        // Sequencing matters here because the generator thread can be
        // blocked in stdin.write_all (paused subprocess, full pipe
        // buffer, SIGSTOPped child) — in which case it never re-checks
        // stop_flag. We need to break that block before joining.
        //
        // Order:
        //   1. Set stop_flag so the thread exits on its NEXT loop iter
        //      (the one that would happen after write_all returns).
        //   2. Kill the subprocess. This closes the OS-side pipe, so
        //      any in-progress write_all on stdin returns EPIPE, the
        //      thread breaks out of its loop, and the join becomes
        //      non-blocking.
        //   3. Join the thread (now guaranteed to complete promptly).
        //   4. Reap the killed subprocess to avoid a zombie.
        self.stop_flag.store(true, Ordering::Relaxed);

        // Kill the subprocess FIRST — without this, a paused paplay
        // would leave the generator thread blocked in write_all and
        // the join below would hang forever.
        let mut zombie_child: Option<std::process::Child> = None;
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                zombie_child = Some(c);
            }
        }

        // Now the thread can exit; join with a small timeout via
        // a sentinel join+sleep loop. std::thread doesn't have join
        // with timeout natively, so we approximate with try-join via
        // a one-shot channel — but a single straight join is fine
        // now that we killed the subprocess.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }

        // Reap the dead subprocess so it doesn't sit as a zombie.
        if let Some(mut c) = zombie_child {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_millis(500) {
                match c.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            // Already killed; if try_wait keeps returning None for
            // half a second, the kernel will still reap it later when
            // the Child is dropped. Nothing more we can usefully do.
        }
    }
}

/// Generator thread: produces 16-bit mono PCM chunks and writes them to
/// the subprocess's stdin. Picks up frequency changes between chunks.
/// When the predictor has a learned template loaded (adaptive mode),
/// its inverse-phase output is mixed into each sample.
fn run_generator(
    mut stdin: std::process::ChildStdin,
    stop: Arc<AtomicBool>,
    freq_bits: Arc<AtomicU32>,
    preset: MaskerPreset,
    gain_db: f32,
    predictor_state: Arc<crate::predictor::PredictorState>,
) {
    use std::io::Write;
    let read_freq = || {
        let bits = freq_bits.load(Ordering::Relaxed);
        let hz = f32::from_bits(bits);
        if hz > 0.0 { Some(hz) } else { None }
    };
    let mut current_freq = read_freq();
    let mut sanitizer = Sanitizer::new(preset, SAMPLE_RATE, gain_db, current_freq);
    let mut predictor = crate::predictor::Predictor::new(predictor_state.clone());

    // Pre-allocate per-chunk PCM buffer so we don't reallocate in the hot loop.
    let chunk_samples = (SAMPLE_RATE as u64 * CHUNK_MS as u64 / 1000) as usize;
    let mut pcm = Vec::with_capacity(chunk_samples * 2);

    while !stop.load(Ordering::Relaxed) {
        // Check whether the user retuned; if so, push the new fundamental
        // through to the running sanitizer (no need to reallocate it).
        let new_freq = read_freq();
        if new_freq != current_freq {
            current_freq = new_freq;
            if !sanitizer.set_antitone_frequency(new_freq.unwrap_or(0.0)) && new_freq.is_some() {
                sanitizer = Sanitizer::new(preset, SAMPLE_RATE, gain_db, new_freq);
            }
        }

        pcm.clear();
        for _ in 0..chunk_samples {
            // Combine masker noise + anti-tones (from sanitizer) with the
            // predictor's inverse-template output (from mic-driven learner).
            // The predictor returns 0 when dormant, so adaptive mode is
            // strictly additive — turning it off just means template=empty.
            let s_mix = sanitizer.next_sample() + predictor.next_sample();
            let s_clamped = s_mix.clamp(-1.0, 1.0);
            let i16_val = (s_clamped * 32_767.0) as i16;
            pcm.extend_from_slice(&i16_val.to_le_bytes());
        }
        if stdin.write_all(&pcm).is_err() {
            // Subprocess gone (probably killed). Exit cleanly.
            break;
        }
    }
    drop(stdin);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_existing_command() {
        // /bin/sh almost always exists on any Linux test env.
        assert!(which("sh").is_some());
    }

    #[test]
    fn which_returns_none_for_garbage() {
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}
