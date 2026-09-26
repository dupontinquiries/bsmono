//! The audio-thread-facing half of Studio: owns the authoritative `Project`,
//! `History`, and `Transport`, lowers every `Command` through the seam
//! `studio-command` defines, and publishes a read-only snapshot for whatever
//! renders or mixes against it.
//!
//! This crate's model half — `EngineState`, `Command` lowering,
//! snapshot-swap instead of a shared mutable project, an SPSC ring instead
//! of a mutex — is proven without a device, and stays that way: nothing
//! about `EngineState::apply` changed to add real-time support, because it
//! never needed to. What DID need building, and now exists in [`device`],
//! is the thing this comment used to say belonged "one layer down,
//! `studio-io`, not built yet": [`device::DeviceEngine`] wires this crate's
//! own `EngineState`/`CommandReceiver` to `studio_audio::StreamingRenderer`
//! and `studio_io::AudioIo`, so a real device callback (or `studio_io::
//! TestIo`, for testing without hardware) can actually drive this engine.
//!
//! ## Why snapshot-swap
//!
//! An audio callback must never block on a lock the UI thread might be
//! holding — that is how a UI stutter becomes an audible glitch. So the UI
//! thread owns the only mutable `Project`, and after every command that
//! changes it, publishes a fresh `Arc<Project>` via `arc-swap`. Whatever reads
//! for rendering (a future `studio-audio` mixdown call on the audio thread)
//! loads the arc once per buffer and works from that snapshot for the whole
//! buffer, so a mid-buffer edit is simply "visible next buffer" rather than
//! "visible half-way through this one," which is the only sane place to draw
//! that line.
//!
//! Note the snapshot only publishes on `Command::Edit` — `Transport` and
//! `Engine` commands change state the render path reads elsewhere (transport
//! position, cue level), and republishing the whole project on every cue-knob
//! twist would be one clone per touch event for no reason.
//!
//! ## Why an SPSC ring, not a channel with an allocator behind it
//!
//! `rtrb` is a lock-free single-producer/single-consumer ring with a fixed
//! capacity, so pushing into it never allocates and never blocks — the two
//! properties a realtime consumer needs from anything that feeds it. Today
//! the consumer here is the same thread as the producer (there is no realtime
//! thread yet), but the ring is exercised across a real second thread in the
//! tests below so the SPSC contract is actually proven, not just assumed from
//! the crate's docs.

use std::sync::Arc;

pub mod device;

use arc_swap::ArcSwap;
use studio_command::{Command, CommandTap, EngineCmd, Outcome, TransportCmd};
use studio_core::{History, Project};
use studio_live::Tempo;

/// The mutable session state, owned by the UI thread.
pub struct EngineState {
    pub project: Project,
    pub history: History,
    pub transport: studio_live::transport::Transport,
    pub tempo: Tempo,
    /// Beats per bar, needed to turn `TransportCmd::CountIn(bars)` into ticks.
    /// Not `Tempo`'s job — a project can be in 3/4 without changing bpm.
    pub beats_per_bar: u32,

    /// Ephemeral engine-side state that `EngineCmd` touches. Never saved,
    /// never on the wire — this is exactly the split `studio-command`'s docs
    /// describe.
    pub cue_level: f32,
    pub monitor_retune: bool,
    previewing: std::collections::BTreeSet<usize>,

    /// The current rack, replaced whole by `Command::Rack` — see that
    /// variant's docs on why it's a third kind of state alongside `Op`s and
    /// ephemeral commands. A plain field, not snapshotted through an
    /// `ArcSwap` the way `project` is: nothing here yet reads it from a
    /// realtime audio thread (pattern playback isn't wired into this
    /// engine's render path), so there's no torn-read hazard to guard
    /// against — if/when that changes, this is exactly the field that would
    /// need the same snapshot treatment `project` already has, not a
    /// redesign.
    pub rack: Option<studio_live::Rack>,

    tap: CommandTap,
    snapshot: Arc<ArcSwap<Project>>,
}

impl EngineState {
    pub fn new(project: Project, rate: u32, tap_capacity: usize) -> EngineState {
        let snapshot = Arc::new(ArcSwap::from_pointee(project.clone()));
        EngineState {
            project,
            history: History::default(),
            transport: studio_live::transport::Transport::new(rate),
            tempo: Tempo::default(),
            beats_per_bar: 4,
            cue_level: 1.0,
            monitor_retune: false,
            previewing: std::collections::BTreeSet::new(),
            rack: None,
            tap: CommandTap::new(tap_capacity),
            snapshot,
        }
    }

    /// A cheap, independently-owned handle to the current snapshot. Clone
    /// this once (it's an `Arc`) and hand it to whatever renders — every
    /// clone sees the same published project until the next `Edit` swaps it.
    pub fn snapshot_handle(&self) -> Arc<ArcSwap<Project>> {
        Arc::clone(&self.snapshot)
    }

    /// Lower one command. This is the only entry point every producer —
    /// touch, keys, voice, copilot, an incoming collab op — is meant to call.
    pub fn apply(&mut self, cmd: Command) -> Outcome {
        let outcome = match &cmd {
            Command::Edit(op) => match self.history.apply(&mut self.project, op) {
                Ok(()) => {
                    // Publish only on a real, persistent change — see module
                    // docs on why Transport/Engine commands don't do this.
                    self.snapshot.store(Arc::new(self.project.clone()));
                    Outcome::Applied
                }
                Err(e) => Outcome::Rejected(format!("{e:?}")),
            },
            Command::Transport(t) => {
                self.apply_transport(t);
                Outcome::Applied
            }
            Command::Engine(e) => {
                self.apply_engine(e);
                Outcome::Applied
            }
            Command::Rack(rack) => {
                self.rack = Some(rack.clone());
                Outcome::Applied
            }
        };
        self.tap.record(&cmd, outcome.clone());
        outcome
    }

    fn apply_transport(&mut self, t: &TransportCmd) {
        match *t {
            TransportCmd::Play => self.transport.play(),
            TransportCmd::Stop => self.transport.stop(),
            TransportCmd::Seek(pos) => self.transport.set_position(pos),
            TransportCmd::SetLoop { start, end, enabled } => {
                self.transport.loop_range = if enabled { Some((start, end)) } else { None };
            }
            TransportCmd::Record(true) => {
                let at = self.transport.position();
                self.transport.start_recording(at);
            }
            TransportCmd::Record(false) => {
                // Transport has no "stop recording, keep playing" primitive —
                // `stop()` halts playback too, which a mid-take punch-out
                // should not do. `recording` is a public field for exactly
                // this: drop the arm without touching the clock.
                self.transport.recording = false;
            }
            TransportCmd::CountIn(bars) => {
                self.transport.count_in = if bars == 0 {
                    studio_core::Ticks::ZERO
                } else {
                    self.tempo.bar_len(self.beats_per_bar).saturating_mul(bars as i64)
                };
            }
        }
    }

    fn apply_engine(&mut self, e: &EngineCmd) {
        match *e {
            EngineCmd::CueLevel(v) => self.cue_level = v.clamp(0.0, 1.0),
            EngineCmd::MonitorRetune(on) => self.monitor_retune = on,
            EngineCmd::PreviewVoice { channel, .. } => {
                self.previewing.insert(channel);
            }
            EngineCmd::StopPreview { channel } => {
                self.previewing.remove(&channel);
            }
        }
    }

    pub fn is_previewing(&self, channel: usize) -> bool {
        self.previewing.contains(&channel)
    }

    pub fn tap(&self) -> &CommandTap {
        &self.tap
    }
}

/// `Ticks` doesn't expose a `saturating_mul` — bars can be larger than 1, and
/// `bar_len` already returns a `Ticks`, so this is the one arithmetic step
/// `studio-core` doesn't hand us. Kept local rather than proposing a new
/// method upstream for a single call site.
trait TicksMulExt {
    fn saturating_mul(self, n: i64) -> Self;
}
impl TicksMulExt for studio_core::Ticks {
    fn saturating_mul(self, n: i64) -> Self {
        studio_core::Ticks(self.0.saturating_mul(n))
    }
}

/// The UI-thread producer side of the command ring.
pub struct CommandSender(rtrb::Producer<Command>);

/// The consumer side — `device::DeviceEngine` is the real audio-thread
/// attachment point this doc used to describe as future work; its own
/// `tick` calls `drain_into` once per callback, exactly the cadence this
/// type's design always assumed.
pub struct CommandReceiver(rtrb::Consumer<Command>);

/// A fixed-capacity, non-allocating, single-producer/single-consumer ring for
/// `Command`. Full pushes are rejected rather than blocking — a producer that
/// floods this is a bug to surface, not to paper over by growing the ring.
pub fn command_ring(capacity: usize) -> (CommandSender, CommandReceiver) {
    let (p, c) = rtrb::RingBuffer::new(capacity);
    (CommandSender(p), CommandReceiver(c))
}

#[derive(Debug, PartialEq)]
pub struct RingFull;

impl CommandSender {
    pub fn send(&mut self, cmd: Command) -> Result<(), RingFull> {
        self.0.push(cmd).map_err(|_| RingFull)
    }
}

impl CommandReceiver {
    /// Drain everything currently queued into `engine`, in order. Call once
    /// per render buffer in the eventual realtime path.
    pub fn drain_into(&mut self, engine: &mut EngineState) -> usize {
        let mut n = 0;
        while let Ok(cmd) = self.0.pop() {
            engine.apply(cmd);
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_command::{mute_track, CommandKind};
    use studio_core::{Op, Ticks, Track, TrackId, TrackKind};

    fn empty_project() -> Project {
        Project { name: "t".into(), ..Default::default() }
    }

    fn add_track_cmd(id: u64) -> Command {
        Command::Edit(Op::AddTrack {
            track: Track { id: TrackId(id), kind: TrackKind::Audio, name: format!("T{id}"), muted: false, locked: false, gain: 1.0, route: None },
            at: 0,
        })
    }

    #[test]
    fn edit_applies_and_publishes_snapshot() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        let snap = engine.snapshot_handle();
        assert_eq!(snap.load().tracks.len(), 0, "snapshot should reflect the project at construction");

        let outcome = engine.apply(add_track_cmd(1));
        assert_eq!(outcome, Outcome::Applied);
        assert_eq!(snap.load().tracks.len(), 1, "an Edit must republish the snapshot synchronously");
        assert_eq!(engine.project.tracks.len(), 1);
    }

    #[test]
    fn transport_and_engine_commands_never_touch_the_snapshot() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        let snap = engine.snapshot_handle();
        let before = Arc::as_ptr(&snap.load_full());

        engine.apply(Command::Transport(TransportCmd::Play));
        engine.apply(Command::Engine(EngineCmd::CueLevel(0.8)));

        let after = Arc::as_ptr(&snap.load_full());
        assert_eq!(before, after, "ephemeral commands must not republish the project — see module docs");
        assert!(engine.transport.playing);
        assert_eq!(engine.cue_level, 0.8);
    }

    #[test]
    fn rejected_edit_does_not_publish_or_corrupt_state() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        let before_ptr = Arc::as_ptr(&engine.snapshot_handle().load_full());

        // No such track — must be rejected, not panic, not silently no-op the doc.
        let outcome = engine.apply(mute_track(TrackId(99), true, false));
        assert!(matches!(outcome, Outcome::Rejected(_)));
        assert_eq!(engine.project.tracks.len(), 0);
        assert_eq!(
            Arc::as_ptr(&engine.snapshot_handle().load_full()),
            before_ptr,
            "a rejected edit must not publish a snapshot"
        );

        let entry = engine.tap().recent().last().unwrap();
        assert_eq!(entry.kind, CommandKind::EditSetTrackFlags);
        assert!(matches!(&entry.outcome, Outcome::Rejected(_)));
    }

    #[test]
    fn cue_level_clamps_into_unit_range() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        engine.apply(Command::Engine(EngineCmd::CueLevel(3.0)));
        assert_eq!(engine.cue_level, 1.0);
        engine.apply(Command::Engine(EngineCmd::CueLevel(-1.0)));
        assert_eq!(engine.cue_level, 0.0);
    }

    #[test]
    fn record_false_disarms_without_stopping_playback() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        engine.apply(Command::Transport(TransportCmd::Play));
        engine.apply(Command::Transport(TransportCmd::Record(true)));
        assert!(engine.transport.recording);
        assert!(engine.transport.playing);

        engine.apply(Command::Transport(TransportCmd::Record(false)));
        assert!(!engine.transport.recording, "punch-out must disarm");
        assert!(engine.transport.playing, "punch-out must not stop playback — that's a separate, explicit action");
    }

    #[test]
    fn count_in_bars_convert_through_tempo_and_time_signature() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        engine.tempo = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        engine.beats_per_bar = 4;
        engine.apply(Command::Transport(TransportCmd::CountIn(2)));
        // 120 bpm -> 0.5s/beat -> 4 beats/bar -> 2s/bar -> 2 bars = 4s.
        let expected = Ticks::from_seconds(4.0);
        assert_eq!(engine.transport.count_in, expected);
    }

    #[test]
    fn preview_voice_tracks_channel_until_stopped() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        assert!(!engine.is_previewing(3));
        engine.apply(Command::Engine(EngineCmd::PreviewVoice { channel: 3, vel: 1.0 }));
        assert!(engine.is_previewing(3));
        engine.apply(Command::Engine(EngineCmd::StopPreview { channel: 3 }));
        assert!(!engine.is_previewing(3));
    }

    #[test]
    fn tap_records_every_command_in_order() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        engine.apply(add_track_cmd(1));
        engine.apply(Command::Transport(TransportCmd::Play));
        engine.apply(Command::Engine(EngineCmd::CueLevel(0.5)));

        let kinds: Vec<_> = engine.tap().recent().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![CommandKind::EditAddTrack, CommandKind::TransportPlay, CommandKind::EngineCueLevel]
        );
    }

    #[test]
    fn command_rack_replaces_the_engines_rack_and_records_as_applied() {
        // The actual apply path for what studio-copilot's flip_the_drums /
        // change_song_key propose — this is the seam closing, not just the
        // Command variant existing. Before this, EngineState had nowhere at
        // all for a Rack to land, even in single-player use.
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        assert!(engine.rack.is_none());

        let mut rack = studio_live::Rack::default();
        rack.tempo_bpm = 174.0;
        let outcome = engine.apply(Command::Rack(rack.clone()));

        assert_eq!(outcome, Outcome::Applied);
        assert_eq!(engine.rack, Some(rack));
        let entry = engine.tap().recent().last().unwrap();
        assert_eq!(entry.kind, CommandKind::RackReplace);
        assert_eq!(entry.outcome, Outcome::Applied);
    }

    #[test]
    fn a_second_command_rack_fully_replaces_the_first_not_merges_with_it() {
        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        let mut first = studio_live::Rack::default();
        first.tempo_bpm = 120.0;
        engine.apply(Command::Rack(first));

        let mut second = studio_live::Rack::default();
        second.tempo_bpm = 90.0;
        engine.apply(Command::Rack(second.clone()));

        assert_eq!(engine.rack, Some(second), "the whole rack should be replaced, matching RackSync's own whole-value semantics — no merging of old and new");
    }

    /// The SPSC contract is only proven by actually crossing a thread —
    /// exercising the ring on one thread would not catch a `Send`/lifetime
    /// mistake that only breaks under real concurrent producer/consumer use.
    #[test]
    fn ring_moves_commands_across_a_real_thread_boundary() {
        let (mut tx, mut rx) = command_ring(64);
        let producer = std::thread::spawn(move || {
            for i in 0..50u64 {
                // Retry on a momentarily full ring rather than dropping —
                // the capacity (64) comfortably exceeds the burst (50), so
                // this loop is a safety net, not the expected path.
                while tx.send(add_track_cmd(i)).is_err() {
                    std::thread::yield_now();
                }
            }
        });

        let mut engine = EngineState::new(empty_project(), 48_000, 256);
        let mut drained = 0;
        while drained < 50 {
            drained += rx.drain_into(&mut engine);
            std::thread::yield_now();
        }
        producer.join().unwrap();

        assert_eq!(engine.project.tracks.len(), 50, "every command sent across the thread boundary must land");
    }

    #[test]
    fn command_rack_crosses_a_real_thread_boundary_intact() {
        // Command::Rack is the newest variant in the ring, carrying types
        // (studio_live::Rack -> Channel/Pattern -> BTreeMap<(usize,u32),
        // StepMod>) that only just gained Send-relevant derives this
        // session. "It compiles" doesn't prove it survives a real producer/
        // consumer thread split the way the existing ring test's own
        // docstring insists on — so this checks it directly rather than
        // assuming the property carries over from the Edit-only test above.
        let (mut tx, mut rx) = command_ring(8);
        let mut sent_rack = studio_live::Rack::default();
        sent_rack.tempo_bpm = 174.0;
        let mut pattern = studio_live::Pattern::new("beat", 16, 1);
        pattern.toggle(0, 0);
        pattern.set_mod(0, 0, studio_live::StepMod { vel: 0.8, ratchet: 3, tune: -2, slide: true, prob: 0.9, every: Some((0, 2)), nudge: 0.0 });
        sent_rack.patterns.push(pattern);
        let expected = sent_rack.clone();

        let producer = std::thread::spawn(move || {
            tx.send(Command::Rack(sent_rack)).expect("the ring has room for one small rack");
        });

        let mut engine = EngineState::new(empty_project(), 48_000, 16);
        let mut drained = 0;
        while drained < 1 {
            drained += rx.drain_into(&mut engine);
            std::thread::yield_now();
        }
        producer.join().unwrap();

        assert_eq!(engine.rack, Some(expected), "the rack, including its sparse StepMod, must arrive byte-for-byte intact across the thread boundary");
    }

    #[test]
    fn ring_rejects_rather_than_blocks_when_full() {
        let (mut tx, _rx) = command_ring(2);
        assert!(tx.send(add_track_cmd(1)).is_ok());
        assert!(tx.send(add_track_cmd(2)).is_ok());
        assert_eq!(tx.send(add_track_cmd(3)), Err(RingFull), "a full ring must reject, never block a caller");
    }
}
