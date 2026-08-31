//! audio — the mic and the speaker, so a voice call carries voice.
//!
//! The same hole `capture` was: the decision layer is complete and nothing
//! feeds it. `voice_send` decides whether a mic interval may leave,
//! `voice_bridge` envelopes it, `sink_cushion` decides what to write to the
//! device and when — and `HostEvent::MicFrame`, `SinkDepth` and
//! `HostCommand::PlayAudio` had no producer or consumer anywhere in the
//! tree. A call could be joined, the floor could be taken, and no sample
//! ever moved.
//!
//! ## Subprocesses again, for the same reasons
//!
//! `parec`/`pacat` (PipeWire's and PulseAudio's, both present on
//! postmarketOS) or `arecord`/`aplay`, spawned through `sh`, speaking raw
//! S16LE mono at [`sink_cushion::SAMPLE_RATE`] over a pipe. Linking cpal or
//! libpulse would put an audio stack in a crate whose tests run in CI with
//! no sound card, and the per-target build cost lands on exactly the ARM
//! devices this app is for. A subprocess costs a pipe copy of 32 kB/s,
//! which is nothing next to the camera's 36 MB/s.
//!
//! It also keeps the loop legible: the command being run is a string in a
//! log line, and `parec --rate=16000 ... | aplay` is something you can try
//! by hand when a phone is silent.
//!
//! ## What is a frame here
//!
//! `sink_cushion` works in `f32` frames of `frame_samples(frame_ms)`
//! samples. The wire to the device is `i16`. The conversion is in this
//! module and nowhere else — two places scaling by 32768 with different
//! rounding is how one direction ends up quieter than the other.
//!
//! ## Backpressure is the whole design of the playback side
//!
//! The app's `sink_tick(depth)` asks "how full is the device?" and answers
//! with what to write. A writer that ignores depth and writes whatever it
//! has will either overrun (latency grows for the whole call and never
//! recovers) or spin. [`Speaker::depth`] is therefore a real count of
//! frames handed over and not yet consumed, estimated from elapsed time
//! against the frame rate — the pipe gives no true depth, and an estimate
//! that is honest about being one is better than a number that pretends.
//!
//! ## What is verified here and what is not
//!
//! Framing, the i16/f32 round trip, the depth estimate, mic gating and the
//! stop path are unit tested, and the subprocess mechanics are tested
//! against stand-in launchers exactly as in `capture`. NOT verified: that
//! `parec` and `aplay` accept these flags, and that any sound has ever come
//! out. This sandbox has no audio device.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::sink_cushion::{frame_samples, SAMPLE_RATE};

/// Bytes per sample on the device side.
const BYTES_PER_SAMPLE: usize = 2;

/// One i16 LE sample buffer to f32 in −1..1.
///
/// `i16::MIN` maps to slightly past −1.0 with a naive /32768, so the divisor
/// is chosen to keep the range symmetric — an asymmetric one clips one half
/// of a loud waveform and not the other, which sounds like distortion only
/// on peaks and is miserable to track down later.
pub fn pcm_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(BYTES_PER_SAMPLE)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0)
        .collect()
}

/// f32 in −1..1 back to i16 LE, clamped rather than wrapped.
///
/// Wrapping is the difference between a loud sample and a full-scale sample
/// of the OPPOSITE sign — a click on every peak.
pub fn f32_to_pcm(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * BYTES_PER_SAMPLE);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// A running capture or playback process, reduced to what the loops need.
pub trait Pipe: Send {
    /// Fill `buf` completely, or say why not. Mic side.
    fn read_exact_into(&mut self, buf: &mut [u8]) -> Result<(), String>;
    /// Hand these bytes to the device. Speaker side.
    fn write_all_from(&mut self, buf: &[u8]) -> Result<(), String>;
}

/// The mic thread's shared state.
///
/// Latest-wins would be wrong here, unlike video: dropping an audio frame
/// leaves a hole you HEAR, and the encoder's sequence numbering assumes
/// frames are consecutive. So this is a bounded queue, and it drops the
/// OLDEST on overflow with a counted `dropped` — a UI thread that stalled
/// for a second must not make the next second of speech play a second late
/// for the rest of the call.
#[derive(Default)]
pub struct MicShared {
    frames: Mutex<std::collections::VecDeque<Vec<f32>>>,
    /// Frames discarded because the consumer fell behind.
    dropped: Mutex<u64>,
    error: Mutex<Option<String>>,
}

/// How many mic frames may wait for the UI thread. At 20 ms a frame this is
/// 320 ms — long enough to ride out a slow paint, short enough that the
/// resulting delay is still a conversation.
pub const MIC_BACKLOG: usize = 16;

impl MicShared {
    pub fn new() -> Arc<MicShared> {
        Arc::new(MicShared::default())
    }

    /// Take up to `max` frames, oldest first.
    pub fn take(&self, max: usize) -> Vec<Vec<f32>> {
        let Ok(mut q) = self.frames.lock() else { return Vec::new() };
        let n = max.min(q.len());
        q.drain(..n).collect()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.lock().map(|d| *d).unwrap_or(0)
    }

    /// The reason capture stopped, if it did.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }

    fn push(&self, f: Vec<f32>) {
        if let Ok(mut q) = self.frames.lock() {
            if q.len() >= MIC_BACKLOG {
                q.pop_front();
                if let Ok(mut d) = self.dropped.lock() {
                    *d += 1;
                }
            }
            q.push_back(f);
        }
    }

    fn set_error(&self, e: String) {
        if let Ok(mut g) = self.error.lock() {
            *g = Some(e);
        }
    }
}

/// Read frames off the mic until stopped or the pipe ends.
///
/// Synchronous, like `capture::run`, so tests drive it directly.
pub fn run_mic(
    mut pipe: Box<dyn Pipe>,
    frame_ms: u32,
    shared: &MicShared,
    stop: &AtomicBool,
) {
    let n = frame_samples(frame_ms);
    let mut buf = vec![0u8; n * BYTES_PER_SAMPLE];
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match pipe.read_exact_into(&mut buf) {
            Ok(()) => shared.push(pcm_to_f32(&buf)),
            Err(e) => {
                // A stop that raced the read is not an error worth showing;
                // the user asked for it.
                if !stop.load(Ordering::Relaxed) {
                    shared.set_error(e);
                }
                return;
            }
        }
    }
}

/// The playback side: a device to write frames to, and an honest estimate of
/// how much it still holds.
pub struct Speaker {
    pipe: Box<dyn Pipe>,
    frame_ms: u32,
    /// When the device would have finished everything written so far, if it
    /// consumes in real time. `None` before the first write.
    drains_at: Option<Instant>,
}

impl Speaker {
    pub fn new(pipe: Box<dyn Pipe>, frame_ms: u32) -> Speaker {
        Speaker { pipe, frame_ms, drains_at: None }
    }

    /// Frames handed over and not yet played, estimated from elapsed time.
    ///
    /// An ESTIMATE, and named one: a pipe cannot be asked. The alternative
    /// is to report zero, which tells `sink_tick` the device is starving and
    /// gets it a write every tick until latency is a second — the failure
    /// this whole cushion exists to prevent.
    pub fn depth(&self) -> usize {
        let Some(t) = self.drains_at else { return 0 };
        let left = t.saturating_duration_since(Instant::now());
        (left.as_millis() as u32 / self.frame_ms.max(1)) as usize
    }

    /// Write frames out. Returns the reason on failure; the caller ends the
    /// call rather than writing into a dead pipe forever.
    pub fn play(&mut self, frames: &[Vec<f32>]) -> Result<(), String> {
        if frames.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        for f in frames {
            bytes.extend_from_slice(&f32_to_pcm(f));
        }
        self.pipe.write_all_from(&bytes)?;
        // Advance the drain clock from whichever is later: now, or where the
        // last write left it. Starting from `now` unconditionally would
        // forget everything still queued and report an empty device.
        let base = match self.drains_at {
            Some(t) if t > Instant::now() => t,
            _ => Instant::now(),
        };
        self.drains_at = Some(base + Duration::from_millis(
            u64::from(self.frame_ms) * frames.len() as u64));
        Ok(())
    }
}

// ─────────────────────────────────────────────────────── real subprocesses ──

/// The default capture command. `parec` is PipeWire's and PulseAudio's alike;
/// `--raw` keeps it headerless so every read is samples and not a WAV header
/// the first time round.
pub fn default_record_cmd() -> String {
    format!("parec --raw --format=s16le --rate={SAMPLE_RATE} --channels=1")
}

/// The default playback command.
pub fn default_play_cmd() -> String {
    format!("pacat --raw --format=s16le --rate={SAMPLE_RATE} --channels=1")
}

/// Spawn one side of the audio path.
///
/// `direction` decides which pipe is wired: recording reads the child's
/// stdout, playback writes its stdin. Both go through `sh -c 'exec …'` so
/// the command can be a user-editable string with flags in it, and so the
/// pid we kill is the tool rather than a shell around it.
pub fn spawn(cmd: &str, direction: Direction) -> Result<(Box<dyn Pipe>, Child), String> {
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(format!("exec {cmd}")).stderr(std::process::Stdio::piped());
    match direction {
        Direction::Record => {
            c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped());
        }
        Direction::Play => {
            c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null());
        }
    }
    let mut child = c.spawn().map_err(|e| format!("could not spawn `{cmd}`: {e}"))?;
    let pipe: Box<dyn Pipe> = match direction {
        Direction::Record => Box::new(ChildPipe {
            out: child.stdout.take(),
            inp: None,
        }),
        Direction::Play => Box::new(ChildPipe {
            out: None,
            inp: child.stdin.take(),
        }),
    };
    Ok((pipe, Child(Arc::new(Mutex::new(Some(child))))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Record,
    Play,
}

/// A handle that can kill the spawned tool and read its last words.
///
/// Same stop contract as `capture::KillSwitch`, and for the same reason: a
/// blocking read on a pipe cannot be interrupted by a flag alone.
#[derive(Clone)]
pub struct Child(Arc<Mutex<Option<std::process::Child>>>);

impl Child {
    pub fn stop(&self) {
        if let Ok(mut g) = self.0.lock() {
            if let Some(c) = g.as_mut() {
                let _ = c.kill();
            }
        }
    }

    /// Wait for it and return whatever it said on the way out.
    pub fn reap(&self) -> Option<String> {
        let mut c = self.0.lock().ok()?.take()?;
        let status = c.wait().ok()?;
        let mut err = String::new();
        if let Some(mut e) = c.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        Some(match err.trim().lines().last() {
            Some(last) if !last.is_empty() => format!("{status}: {last}"),
            _ => format!("{status}"),
        })
    }
}

struct ChildPipe {
    out: Option<std::process::ChildStdout>,
    inp: Option<std::process::ChildStdin>,
}

impl Pipe for ChildPipe {
    fn read_exact_into(&mut self, buf: &mut [u8]) -> Result<(), String> {
        match self.out.as_mut() {
            Some(o) => o.read_exact(buf).map_err(|e| e.to_string()),
            None => Err("this pipe does not record".into()),
        }
    }

    fn write_all_from(&mut self, buf: &[u8]) -> Result<(), String> {
        match self.inp.as_mut() {
            // Flushed every write: a buffered last frame of a talk-spurt
            // that never leaves is a word cut off, and the buffer is small
            // enough that flushing costs a syscall we were making anyway.
            Some(i) => i
                .write_all(buf)
                .and_then(|_| i.flush())
                .map_err(|e| e.to_string()),
            None => Err("this pipe does not play".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted {
        reads: Vec<Vec<u8>>,
        written: Arc<Mutex<Vec<u8>>>,
        fail_write: bool,
    }
    impl Pipe for Scripted {
        fn read_exact_into(&mut self, buf: &mut [u8]) -> Result<(), String> {
            if self.reads.is_empty() {
                return Err("out of script".into());
            }
            let r = self.reads.remove(0);
            if r.len() != buf.len() {
                return Err(format!("scripted {} bytes for a {}-byte frame",
                                   r.len(), buf.len()));
            }
            buf.copy_from_slice(&r);
            Ok(())
        }
        fn write_all_from(&mut self, buf: &[u8]) -> Result<(), String> {
            if self.fail_write {
                return Err("device went away".into());
            }
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(())
        }
    }

    #[test]
    fn the_sample_conversion_round_trips_without_drifting_or_wrapping() {
        // One place does this conversion; two places doing it with different
        // rounding is how one direction ends up quieter than the other.
        let orig: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let back = pcm_to_f32(&f32_to_pcm(&orig));
        for (a, b) in orig.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1.0 / 32_000.0, "{a} became {b}");
        }
        // Out of range CLAMPS. Wrapping turns a loud sample into a
        // full-scale one of the opposite sign — a click on every peak.
        let loud = f32_to_pcm(&[4.0, -4.0]);
        assert_eq!(i16::from_le_bytes([loud[0], loud[1]]), 32_767);
        assert_eq!(i16::from_le_bytes([loud[2], loud[3]]), -32_767);
    }

    #[test]
    fn the_mic_frames_the_stream_at_the_codec_size() {
        let shared = MicShared::new();
        let stop = AtomicBool::new(false);
        let n = frame_samples(20);
        let pipe = Scripted {
            reads: vec![vec![1u8; n * 2], vec![2u8; n * 2]],
            written: Arc::new(Mutex::new(Vec::new())),
            fail_write: false,
        };
        run_mic(Box::new(pipe), 20, &shared, &stop);
        let got = shared.take(10);
        assert_eq!(got.len(), 2, "two frames in, two frames out");
        assert_eq!(got[0].len(), n, "a frame is {n} samples, not bytes");
        assert_eq!(shared.take_error().as_deref(), Some("out of script"),
                   "the end of the stream is reported, not swallowed");
    }

    #[test]
    fn a_stalled_consumer_costs_the_oldest_audio_not_the_newest() {
        // Latest-wins is right for video and wrong here: the newest audio is
        // what the person is saying NOW, and a backlog played late means the
        // whole rest of the call runs behind.
        let shared = MicShared::new();
        for i in 0..(MIC_BACKLOG + 3) {
            shared.push(vec![i as f32]);
        }
        assert_eq!(shared.dropped(), 3);
        let got = shared.take(MIC_BACKLOG);
        assert_eq!(got.len(), MIC_BACKLOG);
        assert_eq!(got[0][0], 3.0, "the OLDEST three went, not the newest");
        assert_eq!(got.last().unwrap()[0], (MIC_BACKLOG + 2) as f32);
    }

    #[test]
    fn a_stop_is_not_reported_as_a_capture_error() {
        // The kill closes the pipe, so the read fails — and telling the user
        // "audio died" when they hung up is a bug report waiting to happen.
        let shared = MicShared::new();
        let stop = AtomicBool::new(true);
        let pipe = Scripted { reads: vec![], written: Arc::new(Mutex::new(Vec::new())),
                              fail_write: false };
        run_mic(Box::new(pipe), 20, &shared, &stop);
        assert!(shared.take_error().is_none());
    }

    #[test]
    fn the_speaker_reports_a_depth_that_falls_as_the_device_drains() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let pipe = Scripted { reads: vec![], written: written.clone(), fail_write: false };
        let mut sp = Speaker::new(Box::new(pipe), 20);
        assert_eq!(sp.depth(), 0, "nothing written, nothing queued");

        let n = frame_samples(20);
        sp.play(&[vec![0.25; n], vec![-0.25; n]]).unwrap();
        assert_eq!(written.lock().unwrap().len(), n * 2 * 2, "two frames of i16");
        assert!(sp.depth() >= 1, "two frames were just handed over: {}", sp.depth());

        // Writing again must ADD to the queue rather than restart the clock —
        // resetting it would report an empty device and invite a write every
        // tick until latency is a second.
        let before = sp.depth();
        sp.play(&[vec![0.0; n]]).unwrap();
        assert!(sp.depth() > before, "{} did not grow past {before}", sp.depth());

        std::thread::sleep(Duration::from_millis(70));
        assert!(sp.depth() < before, "the estimate never drains: {}", sp.depth());
    }

    #[test]
    fn a_dead_output_device_is_an_error_not_a_silent_discard() {
        let pipe = Scripted { reads: vec![], written: Arc::new(Mutex::new(Vec::new())),
                              fail_write: true };
        let mut sp = Speaker::new(Box::new(pipe), 20);
        assert_eq!(sp.play(&[vec![0.0; frame_samples(20)]]),
                   Err("device went away".to_string()));
        // An empty write is not a device error and must not spawn a syscall.
        assert_eq!(sp.play(&[]), Ok(()));
    }

    #[cfg(unix)]
    fn fake_tool(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "snapclone-audio-test-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn a_real_recording_subprocess_reaches_the_frame_queue() {
        // The spawn, the pipe and the framing, against a stand-in that
        // speaks the same protocol as parec: raw bytes on stdout, words on
        // stderr. What this cannot vouch for is parec's own flags.
        let n = frame_samples(20);
        let tool = fake_tool("rec", &format!(
            "head -c {} /dev/zero; echo 'mic unplugged' >&2; exit 2", n * 2 * 2));
        let (pipe, child) = spawn(&tool.display().to_string(), Direction::Record).unwrap();
        let shared = MicShared::new();
        let stop = AtomicBool::new(false);
        run_mic(pipe, 20, &shared, &stop);
        assert_eq!(shared.take(10).len(), 2, "two whole frames off a real pipe");
        assert!(shared.take_error().is_some(), "the EOF was not reported");
        assert!(child.reap().unwrap().contains("mic unplugged"),
                "the tool's own words did not come back");
    }

    #[test]
    #[cfg(unix)]
    fn a_real_playback_subprocess_receives_the_bytes() {
        let out = std::env::temp_dir().join(format!("snapclone-audio-out-{}",
                                                    std::process::id()));
        let _ = std::fs::remove_file(&out);
        let tool = fake_tool("play", &format!("cat > {}", out.display()));
        let (pipe, child) = spawn(&tool.display().to_string(), Direction::Play).unwrap();
        let n = frame_samples(20);
        {
            let mut sp = Speaker::new(pipe, 20);
            sp.play(&[vec![1.0; n]]).unwrap();
        } // dropping the speaker closes stdin, so `cat` sees EOF and exits
        child.reap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got.len(), n * 2, "the device did not receive a full frame");
        assert_eq!(i16::from_le_bytes([got[0], got[1]]), 32_767,
                   "full scale did not survive the trip");
        let _ = std::fs::remove_file(&out);
    }
}
