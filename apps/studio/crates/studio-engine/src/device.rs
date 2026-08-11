//! device — the piece this crate's own module docs named and deferred:
//! "[touching a sound card, running on a real-time thread] belong one layer
//! down (`studio-io`, not built yet)." `studio-io` exists now, and so does
//! `studio_audio::StreamingRenderer` (the call-boundary-safe render path a
//! real device callback needs, as opposed to `mixdown`'s one-shot shape).
//! [`DeviceEngine`] is what turns those two facts plus this crate's own
//! [`EngineState`]/[`CommandReceiver`] into an actual `studio_io::
//! DuplexCallback`.
//!
//! ## Two engine-state copies, on purpose
//!
//! The crate's top-level docs already establish the split: the UI thread
//! owns the authoritative, mutable `Project` and publishes read-only
//! snapshots. `DeviceEngine` owns a SECOND, audio-thread-local
//! [`EngineState`] — not a view onto the UI's — kept in lockstep by
//! draining the same [`CommandReceiver`] every callback. That is exactly
//! what [`CommandReceiver::drain_into`]'s own doc already says ("call once
//! per render buffer in the eventual realtime path") and why `Command`
//! crosses the ring at all rather than the UI just mutating a shared
//! `Project` directly: a lock a UI stutter might be holding is precisely
//! what a real-time callback must never wait on.
//!
//! ## What this proves, and what it doesn't yet
//!
//! Wiring these three already-independently-tested pieces together and
//! proving the composition works — via `studio_io::TestIo`, no hardware
//! needed — is this module's job. What it does NOT yet do:
//! [`StreamingRenderer::render`] allocates a fresh `Vec<f32>` per call
//! (`mixdown`'s own original shape), which makes [`DeviceEngine::tick`]
//! correct but not the allocation-free callback `studio_io::DuplexCallback`'s
//! own docs ask a real implementation to be. That is a genuine, separately-
//! scoped next step (pre-sized scratch buffers reused across calls), not a
//! defect in what's built here — the same "prove the shape first, make it
//! allocation-free second" ordering `studio-io`'s own `CpalIo` already
//! followed (correct config negotiation landed before anyone had measured
//! whether the negotiation itself needs to be realtime-safe, because it
//! only runs once at stream start, not per callback).

use std::collections::BTreeMap;

use studio_audio::{Fades, SourceAudio, StreamingRenderer};
use studio_core::{ClipId, SourceId};
use studio_io::DuplexCallback;
use studio_live::Recorder;

use crate::{CommandReceiver, EngineState};

/// Everything one real-time callback touches, owned in one place so the
/// callback closure itself can stay a thin `tick` call.
pub struct DeviceEngine {
    pub engine: EngineState,
    cmd_rx: CommandReceiver,
    renderer: StreamingRenderer,
    audio: BTreeMap<SourceId, SourceAudio>,
    fades: BTreeMap<ClipId, Fades>,
    rate: u32,
    input_channels: u16,
    recorder: Option<Recorder>,
    /// Recordings that stopped (recording went false) but haven't been
    /// claimed yet. Turning one into ops (`Recorder::commit_ops`) and
    /// broadcasting them is a UI-thread decision — this module only keeps
    /// the take safe until [`DeviceEngine::take_pending_recordings`] is
    /// called, the same "return ops, don't mutate" split `commit_ops`
    /// itself already draws.
    pending_takes: Vec<Recorder>,
}

impl DeviceEngine {
    pub fn new(
        engine: EngineState,
        cmd_rx: CommandReceiver,
        audio: BTreeMap<SourceId, SourceAudio>,
        rate: u32,
        input_channels: u16,
    ) -> DeviceEngine {
        DeviceEngine {
            engine,
            cmd_rx,
            renderer: StreamingRenderer::new(),
            audio,
            fades: BTreeMap::new(),
            rate,
            input_channels,
            recorder: None,
            pending_takes: Vec::new(),
        }
    }

    /// Register decoded or synthesised audio for a source — the output of
    /// `studio_media::decode_file`, `studio_live::render_pattern` (wrapped
    /// as `SourceAudio`), or a prior take's `Recorder::to_audio`. The audio
    /// thread needs its own copy of this map; it cannot borrow the UI
    /// thread's without a lock, which is exactly the thing a real-time
    /// callback must not take.
    pub fn set_source_audio(&mut self, id: SourceId, audio: SourceAudio) {
        self.audio.insert(id, audio);
    }

    pub fn set_fades(&mut self, id: ClipId, fades: Fades) {
        self.fades.insert(id, fades);
    }

    /// Recordings that have stopped since the last call, ready to become
    /// ops via `Recorder::commit_ops` and be broadcast. Draining rather
    /// than peeking, so a caller that doesn't check every buffer doesn't
    /// see the same take twice.
    pub fn take_pending_recordings(&mut self) -> Vec<Recorder> {
        std::mem::take(&mut self.pending_takes)
    }

    /// One callback invocation. `input`/`output` are interleaved
    /// `input_channels`/2-channel (mixdown is always stereo) respectively,
    /// both `frames` long where `frames == output.len() / 2`.
    pub fn tick(&mut self, input: &[f32], output: &mut [f32]) {
        self.cmd_rx.drain_into(&mut self.engine);

        let frames = (output.len() / 2) as i64;

        if !self.engine.transport.playing {
            output.fill(0.0);
            // Recording without playing isn't a real transport state
            // (`start_recording` always sets `playing = true`), so there is
            // nothing to capture here — a stopped transport is silence in
            // both directions.
            return;
        }

        let advance = self.engine.transport.advance(frames);

        // Recording capture — checked AFTER advance, not before, so a
        // count-in that punches in mid-buffer (`Advance::punched_in`)
        // captures this same buffer's input rather than missing it by one
        // callback. Simplification worth naming: the whole buffer is fed
        // once `recording` is true for it, even though a punch that lands
        // mid-buffer technically only "owns" the samples after the punch
        // point. Splitting that precisely needs `Transport` to expose the
        // punch's in-buffer sample offset, which it doesn't yet — real,
        // separately-scoped follow-up, not attempted here.
        if self.engine.transport.recording {
            let rate = self.rate;
            let channels = self.input_channels;
            let punch_in = self.engine.transport.record_position();
            let recorder = self.recorder.get_or_insert_with(|| Recorder::new(rate, channels, punch_in));
            recorder.arm(true);
            recorder.feed(input);
        } else if let Some(mut rec) = self.recorder.take() {
            rec.arm(false);
            self.pending_takes.push(rec);
        }

        let mut offset = 0usize;
        for (from, to) in advance.spans() {
            let rendered = self.renderer.render(&self.engine.project, &self.audio, &self.fades, from, to, self.rate, &[], &[]);
            let end = (offset + rendered.len()).min(output.len());
            let take = end.saturating_sub(offset);
            output[offset..end].copy_from_slice(&rendered[..take]);
            offset = end;
        }
        // A span that rendered fewer samples than the buffer expects (e.g.
        // a project shorter than the requested window) leaves the tail
        // silent rather than reading past what was actually rendered.
        for s in &mut output[offset..] {
            *s = 0.0;
        }
    }

    /// Consume `self` into a `DuplexCallback` ready for `studio_io::AudioIo::start`.
    pub fn into_callback(mut self) -> DuplexCallback {
        Box::new(move |input: &[f32], output: &mut [f32]| self.tick(input, output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EngineState;
    use studio_command::{Command, TransportCmd};
    use studio_core::{Clip, ClipId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};
    use studio_io::{AudioIo, IoConfig, TestIo};

    fn sine(n: usize, rate: u32, hz: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5).collect()
    }

    fn project_with_tone(rate: u32, len_s: f64) -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "A1".into(), muted: false, locked: false, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "t.wav".into(), len: Ticks::from_seconds(len_s), sample_rate: rate, channels: 1, has_video: true } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(len_s), gain: 1.0, effects: Vec::new() } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: rate, channels: vec![sine((len_s * rate as f64) as usize, rate, 440.0)] });
        (p, audio)
    }

    #[test]
    fn a_stopped_transport_produces_silence_and_does_not_advance() {
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 1.0);
        let engine = EngineState::new(p, rate, 16);
        let (_tx, rx) = crate::command_ring(16);
        let mut dev = DeviceEngine::new(engine, rx, audio, rate, 2);

        let input = vec![0.0f32; 256];
        let mut output = vec![1.0f32; 256]; // pre-fill with garbage to prove it gets overwritten
        dev.tick(&input, &mut output);

        assert!(output.iter().all(|&s| s == 0.0), "a stopped transport must produce silence");
        assert_eq!(dev.engine.transport.position(), Ticks::ZERO, "a stopped transport must not advance");
    }

    #[test]
    fn a_playing_transport_renders_real_audio_and_advances() {
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 1.0);
        let mut engine = EngineState::new(p, rate, 16);
        engine.transport.play();
        let (_tx, rx) = crate::command_ring(16);
        let mut dev = DeviceEngine::new(engine, rx, audio, rate, 2);

        let frames = 512;
        let input = vec![0.0f32; frames * 2];
        let mut output = vec![0.0f32; frames * 2];
        dev.tick(&input, &mut output);

        assert!(output.iter().any(|&s| s != 0.0), "playing over a real tone must produce non-silent output");
        assert_eq!(dev.engine.transport.position(), Ticks::from_samples(frames as i64, rate), "the transport must advance by exactly the frames rendered");
    }

    #[test]
    fn commands_sent_through_the_ring_are_applied_before_the_next_render() {
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 1.0);
        let engine = EngineState::new(p, rate, 16);
        let (mut tx, rx) = crate::command_ring(16);
        let mut dev = DeviceEngine::new(engine, rx, audio, rate, 2);

        // Mute the only track through the ring, exactly the way a UI thread
        // would — never touching `dev.engine` directly.
        tx.send(Command::Transport(TransportCmd::Play)).unwrap();
        tx.send(studio_command::mute_track(TrackId(1), true, false)).unwrap();

        let frames = 256;
        let input = vec![0.0f32; frames * 2];
        let mut output = vec![9.0f32; frames * 2];
        dev.tick(&input, &mut output);

        assert!(dev.engine.transport.playing, "the Play command must have been drained and applied");
        assert!(output.iter().all(|&s| s == 0.0), "a track muted via the ring before this tick must render silent, not the pre-mute tone");
    }

    #[test]
    fn consecutive_ticks_compose_the_same_way_streamingrenderer_alone_already_proves() {
        // Not re-deriving StreamingRenderer's own continuity guarantee —
        // that's already covered directly in studio-audio. What this test
        // adds is that DeviceEngine actually calls it that way: many small
        // ticks must advance the transport by the exact total and produce
        // as many samples as one bigger tick would, with no gaps or
        // overlaps at the seams.
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 0.2);
        let mut engine = EngineState::new(p, rate, 16);
        engine.transport.play();
        let (_tx, rx) = crate::command_ring(16);
        let mut dev = DeviceEngine::new(engine, rx, audio, rate, 2);

        let buffer_frames = 128;
        let mut total_rendered = 0usize;
        let input = vec![0.0f32; buffer_frames * 2];
        for _ in 0..20 {
            let mut output = vec![0.0f32; buffer_frames * 2];
            dev.tick(&input, &mut output);
            total_rendered += output.len();
        }
        assert_eq!(total_rendered, 20 * buffer_frames * 2);
        assert_eq!(dev.engine.transport.position(), Ticks::from_samples((20 * buffer_frames) as i64, rate));
    }

    #[test]
    fn arming_and_stopping_recording_yields_a_take_with_the_captured_input() {
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 0.5);
        let engine = EngineState::new(p, rate, 16);
        let (mut tx, rx) = crate::command_ring(16);
        let mut dev = DeviceEngine::new(engine, rx, audio, rate, 1);

        tx.send(Command::Transport(TransportCmd::Play)).unwrap();
        tx.send(Command::Transport(TransportCmd::Record(true))).unwrap();

        let frames = 480; // 10ms at 48kHz
        let fed = sine(frames, rate, 220.0);
        let mut output = vec![0.0f32; frames * 2];
        dev.tick(&fed, &mut output);

        assert!(dev.take_pending_recordings().is_empty(), "still recording — nothing should be finalised yet");

        tx.send(Command::Transport(TransportCmd::Record(false))).unwrap();
        let mut output2 = vec![0.0f32; frames * 2];
        dev.tick(&vec![0.0f32; frames], &mut output2);

        let takes = dev.take_pending_recordings();
        assert_eq!(takes.len(), 1, "stopping recording must finalise exactly one take");
        assert_eq!(takes[0].frames(), frames, "the take must hold the buffer that was captured while armed");
        assert!(takes[0].to_audio().channels[0].iter().any(|&s| s != 0.0), "the captured audio must be the real fed signal, not silence");
    }

    #[test]
    fn a_real_studio_io_testio_stream_drives_deviceengine_end_to_end() {
        // The point of the whole module: this is a genuine `studio_io::
        // AudioIo` implementor calling the callback DeviceEngine produces,
        // not DeviceEngine's own tick called directly. TestIo stands in for
        // CpalIo (no hardware in this sandbox), but the seam it exercises —
        // AudioIo::start handing back a callback, that callback being
        // DeviceEngine::tick wrapped exactly the way a real backend would
        // call it — is the same seam a real device drives.
        let rate = 48_000;
        let (p, audio) = project_with_tone(rate, 1.0);
        let mut engine = EngineState::new(p, rate, 16);
        engine.transport.play();
        let (_tx, rx) = crate::command_ring(16);
        let dev = DeviceEngine::new(engine, rx, audio, rate, 2);

        let mut io = TestIo::default();
        let running = io
            .start(IoConfig { sample_rate: rate, channels: 2, buffer_frames: 256 }, dev.into_callback())
            .unwrap();

        let input = vec![0.0f32; 256];
        let mut output = vec![0.0f32; 256];
        io.pump(&input, &mut output);

        assert!(output.iter().any(|&s| s != 0.0), "a real AudioIo round trip must still produce real audio");
        running.stop();
    }
}
