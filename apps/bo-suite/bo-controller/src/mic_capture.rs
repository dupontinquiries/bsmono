// src/mic_capture.rs
#![allow(dead_code)]
//
// Microphone capture via a `parec` (or `arecord`) subprocess. Reads raw
// 48 kHz mono 16-bit PCM from stdout into a fixed-size ring buffer.
// Callers grab the most-recent N samples for analysis.
//
// Why subprocess instead of a Rust audio library (cpal etc.):
//   - parec speaks to PipeWire and PulseAudio transparently.
//   - PipeWire's pulse-server compatibility layer enforces quantum-aligned
//     reads, so the timing jitter is in the few-millisecond range without
//     us doing anything special.
//   - No extra crate / no extra runtime threads beyond our reader.
//   - Falls back to ALSA's arecord when neither PA nor PW is present.
//
// Ring buffer semantics: writes from the subprocess are batched into
// chunks (whatever parec hands us — typically a quantum, ~21 ms at the
// default settings). On read, callers ask for the most recent N samples,
// which may span multiple chunks. Writes wrap around; old data is lost
// when the buffer fills.
//
// HONEST LATENCY note: the parec subprocess buffers internally; the ring
// buffer adds a few more chunks. Round-trip mic→analysis is typically
// 60-120 ms. That's fine for predictive cancellation (we're predicting
// the next period, not reacting to live audio) but useless for true ANC.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Sample rate of the captured stream. Matches the sanitizer driver's
/// output rate so we don't need any resampling between them.
pub const CAPTURE_SAMPLE_RATE: u32 = 48_000;

/// Default ring buffer size: 1 second of mono samples.
const DEFAULT_BUFFER_SAMPLES: usize = CAPTURE_SAMPLE_RATE as usize;

/// Subprocess backend; tried in order.
#[derive(Debug, Clone, Copy)]
enum CaptureBackend { Parec, Arecord }

impl CaptureBackend {
    fn build(&self) -> Command {
        let mut c = match self {
            CaptureBackend::Parec => {
                let mut c = Command::new("parec");
                c.args([
                    "--raw",
                    "--format=s16le",
                    "--rate=48000",
                    "--channels=1",
                    // Keep server-side buffer modest so capture is timely.
                    "--latency-msec=20",
                ]);
                c
            }
            CaptureBackend::Arecord => {
                let mut c = Command::new("arecord");
                c.args([
                    "-q",
                    "-D", "default",
                    "-f", "S16_LE",
                    "-r", "48000",
                    "-c", "1",
                    "-t", "raw",
                ]);
                c
            }
        };
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        c
    }

    fn detect() -> Option<Self> {
        for b in [CaptureBackend::Parec, CaptureBackend::Arecord] {
            if which(b.cmd_name()).is_some() { return Some(b); }
        }
        None
    }

    fn cmd_name(&self) -> &'static str {
        match self { CaptureBackend::Parec => "parec", CaptureBackend::Arecord => "arecord" }
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

/// Shared ring buffer of f32 samples in [-1.0, 1.0]. The reader thread
/// writes here; callers read via `latest_samples()`.
struct Ring {
    buf: Vec<f32>,
    head: usize,    // next write position
    filled: bool,   // true once we've wrapped at least once
}

impl Ring {
    fn new(capacity: usize) -> Self {
        // Capacity must be ≥1 — pushing into an empty buffer would
        // panic on the first sample because we'd try to index buf[0]
        // on a zero-length Vec. Clamp here rather than at the call
        // sites since this is a private constructor with one job.
        let capacity = capacity.max(1);
        Self { buf: vec![0.0; capacity], head: 0, filled: false }
    }

    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buf[self.head] = s;
            self.head += 1;
            if self.head >= self.buf.len() {
                self.head = 0;
                self.filled = true;
            }
        }
    }

    /// Snapshot the most recent `n` samples in chronological order.
    /// Returns fewer if we haven't accumulated `n` yet.
    fn latest(&self, n: usize) -> Vec<f32> {
        let available = if self.filled { self.buf.len() } else { self.head };
        let take = n.min(available);
        if take == 0 { return Vec::new(); }
        let mut out = Vec::with_capacity(take);
        // Read backwards from `head - 1` for `take` samples, then reverse.
        let start = (self.head + self.buf.len() - take) % self.buf.len();
        if start + take <= self.buf.len() {
            out.extend_from_slice(&self.buf[start..start + take]);
        } else {
            let first_part = self.buf.len() - start;
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..take - first_part]);
        }
        out
    }
}

pub struct MicCapture {
    child: Arc<Mutex<Option<Child>>>,
    ring: Arc<Mutex<Ring>>,
    samples_written: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    backend_name: &'static str,
}

impl MicCapture {
    pub fn start() -> Result<Self, String> {
        Self::start_with_capacity(DEFAULT_BUFFER_SAMPLES)
    }

    pub fn start_with_capacity(capacity: usize) -> Result<Self, String> {
        let backend = CaptureBackend::detect()
            .ok_or_else(|| "no mic capture tool (parec or arecord) found".to_string())?;

        let mut cmd = backend.build();
        let mut child = cmd.spawn()
            .map_err(|e| format!("spawn {}: {}", backend.cmd_name(), e))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| "subprocess stdout unavailable".to_string())?;

        let ring = Arc::new(Mutex::new(Ring::new(capacity)));
        let samples_written = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let child_arc = Arc::new(Mutex::new(Some(child)));

        let thread = {
            let ring = ring.clone();
            let stop = stop.clone();
            let written = samples_written.clone();
            thread::spawn(move || reader_thread(stdout, ring, stop, written))
        };

        Ok(Self {
            child: child_arc,
            ring,
            samples_written,
            stop,
            thread: Some(thread),
            backend_name: backend.cmd_name(),
        })
    }

    /// Pull a snapshot of the most-recent `n` samples. Cheap copy; safe
    /// to call from any thread. The returned vec is in chronological
    /// order (oldest first).
    pub fn latest_samples(&self, n: usize) -> Vec<f32> {
        self.ring.lock().map(|r| r.latest(n)).unwrap_or_default()
    }

    /// Total samples received since start (monotonic counter — useful
    /// for the predictor to know how much new audio there is).
    pub fn samples_written(&self) -> usize {
        self.samples_written.load(Ordering::Relaxed)
    }

    pub fn backend_name(&self) -> &'static str { self.backend_name }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        // Same ordering as SanitizerDriver::drop: kill the subprocess
        // BEFORE joining the reader thread. If parec/arecord is paused
        // or stuck, the reader is blocked in stdout.read and can't
        // observe stop_flag. Killing the child closes the pipe, read
        // returns EOF, and the thread exits promptly.
        self.stop.store(true, Ordering::Relaxed);

        let mut zombie: Option<std::process::Child> = None;
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                zombie = Some(c);
            }
        }

        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }

        if let Some(mut c) = zombie {
            let _ = c.wait();
        }
    }
}

fn reader_thread(
    mut stdout: std::process::ChildStdout,
    ring: Arc<Mutex<Ring>>,
    stop: Arc<AtomicBool>,
    samples_written: Arc<AtomicUsize>,
) {
    // Read in chunks. 4096 bytes = 2048 16-bit samples ≈ 43 ms at 48 kHz.
    // Small enough to feel live, big enough to not thrash the lock.
    let mut buf = vec![0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        let n = match stdout.read(&mut buf) {
            Ok(0) => break,           // subprocess closed
            Ok(n) => n,
            Err(_) => break,
        };
        // Convert s16le → f32 in [-1, 1].
        let mut samples = Vec::with_capacity(n / 2);
        let mut i = 0;
        while i + 1 < n {
            let s = i16::from_le_bytes([buf[i], buf[i + 1]]);
            samples.push((s as f32) / 32_768.0);
            i += 2;
        }
        if let Ok(mut r) = ring.lock() {
            r.push(&samples);
        }
        samples_written.fetch_add(samples.len(), Ordering::Relaxed);
        // Be a little kind to the scheduler if the subprocess is fast.
        thread::sleep(Duration::from_micros(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_returns_chronological_latest() {
        let mut r = Ring::new(8);
        r.push(&[1.0, 2.0, 3.0, 4.0]);
        let snap = r.latest(4);
        assert_eq!(snap, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn ring_handles_wraparound() {
        let mut r = Ring::new(4);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);  // overwrites 1, 2
        let snap = r.latest(4);
        assert_eq!(snap, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn ring_returns_partial_before_full() {
        let r = Ring::new(8);
        let snap = r.latest(4);
        assert!(snap.is_empty());

        let mut r = Ring::new(8);
        r.push(&[1.0, 2.0]);
        let snap = r.latest(4);
        assert_eq!(snap, vec![1.0, 2.0]); // less than requested = available
    }

    #[test]
    fn ring_large_capacity() {
        let mut r = Ring::new(48_000);
        let chunk = vec![0.5_f32; 4096];
        for _ in 0..20 { r.push(&chunk); }
        let snap = r.latest(48_000);
        assert_eq!(snap.len(), 48_000);
        assert!(snap.iter().all(|&v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn ring_zero_capacity_does_not_panic() {
        // Pathological caller passes 0 — Ring::new should clamp up
        // to 1 rather than letting push() index into a zero-length
        // Vec.
        let mut r = Ring::new(0);
        r.push(&[0.1, 0.2, 0.3]);
        let snap = r.latest(10);
        // Only one slot — most recent sample wins.
        assert!(snap.len() <= 1);
    }

    #[test]
    fn which_finds_real_tools() {
        // /bin/sh is universal.
        assert!(which("sh").is_some());
        assert!(which("not-a-real-tool-xxx").is_none());
    }
}
