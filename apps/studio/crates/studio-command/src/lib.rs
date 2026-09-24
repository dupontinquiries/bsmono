//! The seam between "something wants to change the session" and the two
//! places that change can go: `studio_core::Op` (persistent — history, doc,
//! wire) or ephemeral engine state (transport position, cue level, a
//! monitor-fx knob). Every producer of intent — touch, Helix keys, voice, the
//! copilot, an incoming collab packet — emits a `Command` and nothing else.
//!
//! The lowering rule is the whole design: **if it must survive save or reach
//! a collaborator, it is an `Op`; otherwise it is an `EngineCmd`.** Getting
//! this wrong in either direction is a real bug, not a style choice — an Op
//! that should have been ephemeral bloats the log and the wire; an EngineCmd
//! that should have been an Op silently fails to save or to sync.
//!
//! `CommandTap` is the other half: every command that goes through, plus its
//! terminal outcome, is mirrored onto a bounded ring. That ring is (a) a Log
//! panel so the internal loop stays visible, (b) the deterministic fixture
//! for end-to-end tests — replay a scripted command file, assert the tap
//! matches exactly — and (c) the copilot's context feed. One seam, four
//! features.

use serde::{Deserialize, Serialize};
use studio_core::{Op, Ticks, TrackId};

/// Something that changes the session. The only thing any producer emits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Persistent: goes through `History`, the `Doc`, and the wire.
    Edit(Op),
    Transport(TransportCmd),
    Engine(EngineCmd),
    /// Replace the whole rack. Neither an `Op` (patterns aren't part of the
    /// op log — see `studio_collab::RackSync`'s docs on why a rack syncs as
    /// a whole-value register instead) nor ephemeral in the `EngineCmd`
    /// sense (a flipped pattern is real work worth keeping, not a listening
    /// choice like a cue-level twiddle) — a third kind of thing this seam
    /// carries, for a third kind of state. This is what closes the loop
    /// between `studio_copilot::Proposal.rack` (a proposed replacement) and
    /// an actual apply: without this variant, a copilot recipe that
    /// mutates a rack had a `Proposal` to show but no seam to apply it
    /// through — the exact gap this variant exists to close.
    Rack(studio_live::Rack),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TransportCmd {
    Play,
    Stop,
    Seek(Ticks),
    SetLoop { start: Ticks, end: Ticks, enabled: bool },
    Record(bool),
    /// Bars of count-in before the punch point. 0 disables it.
    CountIn(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineCmd {
    /// 0..=1 or dB-linear gain on the monitor cue mix — never printed.
    CueLevel(f32),
    /// Toggle live autotune on the input monitor path (the print-side
    /// instance is a `studio_core::Effect` and travels as an Op instead).
    MonitorRetune(bool),
    /// Note-repeat / pad preview: fires a voice for audition without
    /// touching the pattern. Committing the result is a separate Op.
    PreviewVoice { channel: usize, vel: f32 },
    StopPreview { channel: usize },
}

impl Command {
    /// True if this command must be applied through `Project::apply` and is
    /// therefore undoable, saved, and broadcast to collaborators — OR is a
    /// `Rack` replacement, which is equally real, kept state (just synced
    /// through `RackSync`'s whole-value register instead of the op log, not
    /// ephemeral the way `Transport`/`Engine` commands are).
    pub fn is_persistent(&self) -> bool {
        matches!(self, Command::Edit(_) | Command::Rack(_))
    }

    pub fn kind(&self) -> CommandKind {
        match self {
            Command::Edit(op) => match op {
                Op::AddTrack { .. } => CommandKind::EditAddTrack,
                Op::RemoveTrack { .. } => CommandKind::EditRemoveTrack,
                Op::SetTrackFlags { .. } => CommandKind::EditSetTrackFlags,
                Op::SetTrackGain { .. } => CommandKind::EditSetTrackGain,
                Op::AddClip { .. } => CommandKind::EditAddClip,
                Op::RemoveClip { .. } => CommandKind::EditRemoveClip,
                Op::MoveClip { .. } => CommandKind::EditMoveClip,
                Op::TrimClip { .. } => CommandKind::EditTrimClip,
                Op::SetGain { .. } => CommandKind::EditSetGain,
                Op::SetEffects { .. } => CommandKind::EditSetEffects,
                Op::AddSource { .. } => CommandKind::EditAddSource,
                Op::RemoveSource { .. } => CommandKind::EditRemoveSource,
                Op::AddBus { .. } => CommandKind::EditAddBus,
                Op::RemoveBus { .. } => CommandKind::EditRemoveBus,
                Op::SetBusGain { .. } => CommandKind::EditSetBusGain,
                Op::SetBusPan { .. } => CommandKind::EditSetBusPan,
                Op::SetBusFlags { .. } => CommandKind::EditSetBusFlags,
                Op::SetBusSilent { .. } => CommandKind::EditSetBusSilent,
                Op::SetSourcePath { .. } => CommandKind::EditRelinkSource,
                Op::SetBusChain { .. } => CommandKind::EditSetBusChain,
                Op::SetBusSends { .. } => CommandKind::EditSetBusSends,
                Op::SetRoute { .. } => CommandKind::EditSetRoute,
                Op::SetAutoPoint { .. } => CommandKind::EditSetAutoPoint,
                Op::RemoveAutoPoint { .. } => CommandKind::EditRemoveAutoPoint,
                Op::SetBusParent { .. } => CommandKind::EditSetBusParent,
                Op::SetGroup { .. } => CommandKind::EditSetGroup,
                Op::RemoveGroup { .. } => CommandKind::EditRemoveGroup,
                Op::SetProvenance { .. } => CommandKind::EditSetProvenance,
                Op::RemoveProvenance { .. } => CommandKind::EditRemoveProvenance,
                Op::SetFingerprint { .. } => CommandKind::EditSetFingerprint,
                Op::RemoveFingerprint { .. } => CommandKind::EditRemoveFingerprint,
                Op::Group { .. } => CommandKind::EditGroupedOps,
                Op::SetKey { .. } => CommandKind::EditSetKey,
                Op::SetGroupKey { .. } => CommandKind::EditSetGroupKey,
                Op::SetGroupTempo { .. } => CommandKind::EditSetGroupTempo,
                Op::SetGroupActive { .. } => CommandKind::EditSetGroupActive,
                Op::SetGroupWindow { .. } => CommandKind::EditSetGroupWindow,
                Op::SetClipFade { .. } => CommandKind::EditSetClipFade,
                Op::SetMarker { .. } => CommandKind::EditSetMarker,
                Op::RemoveMarker { .. } => CommandKind::EditRemoveMarker,
                Op::ClearMarkers => CommandKind::EditClearMarkers,
                Op::SetCurve { .. } => CommandKind::EditSetCurve,
                Op::RemoveCurve { .. } => CommandKind::EditRemoveCurve,
                Op::SetCurvePoint { .. } => CommandKind::EditSetCurvePoint,
                Op::RemoveCurvePoint { .. } => CommandKind::EditRemoveCurvePoint,
                Op::SetBinding { .. } => CommandKind::EditSetBinding,
                Op::RemoveBinding { .. } => CommandKind::EditRemoveBinding,
            },
            Command::Transport(t) => match t {
                TransportCmd::Play => CommandKind::TransportPlay,
                TransportCmd::Stop => CommandKind::TransportStop,
                TransportCmd::Seek(_) => CommandKind::TransportSeek,
                TransportCmd::SetLoop { .. } => CommandKind::TransportLoop,
                TransportCmd::Record(_) => CommandKind::TransportRecord,
                TransportCmd::CountIn(_) => CommandKind::TransportCountIn,
            },
            Command::Engine(e) => match e {
                EngineCmd::CueLevel(_) => CommandKind::EngineCueLevel,
                EngineCmd::MonitorRetune(_) => CommandKind::EngineMonitorRetune,
                EngineCmd::PreviewVoice { .. } => CommandKind::EnginePreviewVoice,
                EngineCmd::StopPreview { .. } => CommandKind::EngineStopPreview,
            },
            Command::Rack(_) => CommandKind::RackReplace,
        }
    }

    /// True when applying this command destroys state a confirm word should
    /// gate before voice or a stray touch can fire it. Reversible commands
    /// (everything else) execute immediately with an undo toast instead —
    /// running the *wrong* workflow on someone's timeline is worse than
    /// running none, but gating every command equally would make the fast
    /// path pointless.
    pub fn is_destructive(&self) -> bool {
        spec_for(self.kind()).destructive
    }
}

/// A convenience builder so callers don't have to know `TrackId`'s field is
/// public — kept tiny on purpose; this crate does not want an ergonomics
/// layer beyond what the seam itself needs.
pub fn mute_track(id: TrackId, muted: bool, locked: bool) -> Command {
    Command::Edit(Op::SetTrackFlags { id, muted, locked })
}

/// Every distinct shape of command, used purely as a metadata lookup key —
/// this is the enum the voice grammar and the copilot's tool schema are
/// compiled from, so a new `Command` variant that forgets an entry here is a
/// missing-match compile error, not a silent grammar gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    /// Open a different project. Distinct from the startup path, which
    /// can only load whatever it was launched with.
    ProjectOpen,
    /// Collaboration. These edit the TEAM, not the document — a member
    /// list is not a `Project` field, so none of them can be an `Op`.
    /// That is why they are their own group rather than Edit variants.
    CollabInvite,
    CollabRemove,
    /// Follow a collaborator's viewport. Local only: what I am looking
    /// at is not part of the project, and syncing it would let one
    /// person drag everyone else's screen around.
    CollabFollow,
    CollabUnfollow,
    /// Ask for the full op log again.
    CollabResync,
    EditAddTrack,
    EditRemoveTrack,
    EditSetTrackFlags,
    EditAddClip,
    EditRemoveClip,
    EditMoveClip,
    EditTrimClip,
    EditSetGain,
    EditSetEffects,
    EditAddSource,
    EditRemoveSource,
    EditAddBus,
    EditRemoveBus,
    EditSetTrackGain,
    EditSetBusGain,
    EditSetBusPan,
    EditSetBusFlags,
    /// See `studio_core::Bus::silent`: a bus that renders and then feeds
    /// nothing, so it can be read as a control signal.
    EditSetBusSilent,
    /// Point a source at a different file — proxies to camera originals,
    /// or media that moved.
    EditRelinkSource,
    EditSetBusChain,
    EditSetBusSends,
    EditSetRoute,
    EditSetAutoPoint,
    EditRemoveAutoPoint,
    EditSetBusParent,
    EditSetGroup,
    EditRemoveGroup,
    EditSetProvenance,
    EditRemoveProvenance,
    EditSetFingerprint,
    EditRemoveFingerprint,
    EditGroupedOps,
    EditSetKey,
    EditSetGroupKey,
    EditSetGroupTempo,
    EditSetGroupActive,
    /// Trimming a folded composition — two windows over one member list.
    EditSetGroupWindow,
    /// A clip's fade in/out. Both ends, because dragging a corner is one
    /// gesture and two commands would let undo leave half a crossfade.
    EditSetClipFade,
    EditSetMarker,
    EditRemoveMarker,
    EditClearMarkers,
    EditSetCurve,
    EditRemoveCurve,
    EditSetCurvePoint,
    EditRemoveCurvePoint,
    EditSetBinding,
    EditRemoveBinding,
    TransportPlay,
    TransportStop,
    TransportSeek,
    TransportLoop,
    TransportRecord,
    TransportCountIn,
    EngineCueLevel,
    EngineMonitorRetune,
    EnginePreviewVoice,
    EngineStopPreview,
    RackReplace,
}

impl CommandKind {
    /// Every variant, in table order. Kept as an explicit list (rather than
    /// derived by a proc macro) so the "one entry per kind" test below is
    /// reading real duplicated intent, not trusting generated code to agree
    /// with itself.
    pub const ALL: &'static [CommandKind] = &[
        CommandKind::EditAddTrack,
        CommandKind::EditRemoveTrack,
        CommandKind::EditSetTrackFlags,
        CommandKind::EditAddClip,
        CommandKind::EditRemoveClip,
        CommandKind::EditMoveClip,
        CommandKind::EditTrimClip,
        CommandKind::EditSetGain,
        CommandKind::EditSetEffects,
        CommandKind::EditAddSource,
        CommandKind::EditRemoveSource,
        CommandKind::EditAddBus,
        CommandKind::EditRemoveBus,
        CommandKind::EditSetTrackGain,
        CommandKind::EditSetBusGain,
        CommandKind::EditSetBusPan,
        CommandKind::EditSetBusFlags,
        CommandKind::EditSetBusSilent,
        CommandKind::EditRelinkSource,
        CommandKind::EditSetBusChain,
        CommandKind::EditSetBusSends,
        CommandKind::EditSetRoute,
        CommandKind::TransportPlay,
        CommandKind::TransportStop,
        CommandKind::TransportSeek,
        CommandKind::TransportLoop,
        CommandKind::TransportRecord,
        CommandKind::TransportCountIn,
        CommandKind::EngineCueLevel,
        CommandKind::EngineMonitorRetune,
        CommandKind::EnginePreviewVoice,
        CommandKind::EngineStopPreview,
        CommandKind::RackReplace,
        CommandKind::ProjectOpen,
        CommandKind::CollabInvite,
        CommandKind::CollabRemove,
        CommandKind::CollabFollow,
        CommandKind::CollabUnfollow,
        CommandKind::CollabResync,
    ];
}

/// Metadata for one `CommandKind`: its display/log name, the phrases voice
/// matches against (fed into `studio_voice`'s scorer alongside the user's
/// repertoire), and whether it needs the confirm word.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub kind: CommandKind,
    pub name: &'static str,
    pub phrases: &'static [&'static str],
    pub destructive: bool,
}

/// The compiled command table. Voice grammar and copilot tool schema both
/// read this — neither hand-maintains a separate list, which is the
/// mechanism that keeps coverage from drifting as commands are added.
pub static COMMAND_TABLE: &[CommandSpec] = &[
    CommandSpec { kind: CommandKind::ProjectOpen, name: "open project", phrases: &["open a project", "open the project", "switch project"], destructive: false },
    // `destructive: true` on invite and remove: both change who can read
    // and write the project. The flag drives a confirmation, and adding
    // a person to a session is exactly the kind of thing that should not
    // happen from a misheard voice phrase.
    CommandSpec { kind: CommandKind::CollabInvite, name: "invite collaborator", phrases: &["invite", "add a collaborator", "share this project"], destructive: true },
    CommandSpec { kind: CommandKind::CollabRemove, name: "remove collaborator", phrases: &["remove the collaborator", "kick"], destructive: true },
    CommandSpec { kind: CommandKind::CollabFollow, name: "follow collaborator", phrases: &["follow", "jump to where they are"], destructive: false },
    CommandSpec { kind: CommandKind::CollabUnfollow, name: "stop following", phrases: &["stop following", "unfollow"], destructive: false },
    // Not destructive: a resync REQUESTS the log and integrates it.
    // Integration is convergent by construction, so asking twice costs
    // bandwidth and changes nothing.
    CommandSpec { kind: CommandKind::CollabResync, name: "resync with collaborators", phrases: &["resync", "re-sync the session"], destructive: false },
    CommandSpec { kind: CommandKind::EditAddTrack, name: "add track", phrases: &["add a track", "new track"], destructive: false },
    CommandSpec { kind: CommandKind::EditRemoveTrack, name: "remove track", phrases: &["remove this track", "delete the track"], destructive: true },
    CommandSpec { kind: CommandKind::EditSetTrackFlags, name: "set track flags", phrases: &["mute the", "unmute the", "lock the track"], destructive: false },
    CommandSpec { kind: CommandKind::EditAddClip, name: "add clip", phrases: &["place a clip"], destructive: false },
    CommandSpec { kind: CommandKind::EditRemoveClip, name: "remove clip", phrases: &["delete this clip", "remove the clip"], destructive: true },
    CommandSpec { kind: CommandKind::EditMoveClip, name: "move clip", phrases: &["move this clip"], destructive: false },
    CommandSpec { kind: CommandKind::EditTrimClip, name: "trim clip", phrases: &["trim this clip"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetGain, name: "set gain", phrases: &["turn this up", "turn this down"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetEffects, name: "set effects", phrases: &["add an effect"], destructive: false },
    CommandSpec { kind: CommandKind::EditAddSource, name: "add source", phrases: &["import this"], destructive: false },
    CommandSpec { kind: CommandKind::EditRemoveSource, name: "remove source", phrases: &["remove this file", "delete the recording"], destructive: true },
    CommandSpec { kind: CommandKind::EditAddBus, name: "add bus", phrases: &["add a bus", "new bus"], destructive: false },
    CommandSpec { kind: CommandKind::EditRemoveBus, name: "remove bus", phrases: &["delete this bus", "remove the bus"], destructive: true },
    CommandSpec { kind: CommandKind::EditSetTrackGain, name: "set track gain", phrases: &["turn up the track", "turn down the track", "fader"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusGain, name: "set bus gain", phrases: &["turn up the bus", "turn down the bus"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusPan, name: "set bus pan", phrases: &["pan the bus left", "pan the bus right", "centre the bus"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusFlags, name: "set bus flags", phrases: &["mute the bus", "unmute the bus"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusSilent, name: "set bus silent", phrases: &["make this a control channel", "silence this bus but keep it", "use this as a dummy channel"], destructive: false },
    CommandSpec { kind: CommandKind::EditRelinkSource, name: "relink media", phrases: &["relink the media", "point this at the originals", "find the moved files"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusChain, name: "set bus chain", phrases: &["add a bus effect"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetBusSends, name: "set bus sends", phrases: &["send this to reverb", "send to delay"], destructive: false },
    CommandSpec { kind: CommandKind::EditSetRoute, name: "set route", phrases: &["route this to drums", "route to vocals"], destructive: false },
    CommandSpec { kind: CommandKind::TransportPlay, name: "play", phrases: &["play", "hit play"], destructive: false },
    CommandSpec { kind: CommandKind::TransportStop, name: "stop", phrases: &["stop", "hit stop"], destructive: false },
    CommandSpec { kind: CommandKind::TransportSeek, name: "seek", phrases: &["go to bar", "jump to"], destructive: false },
    CommandSpec { kind: CommandKind::TransportLoop, name: "set loop", phrases: &["loop bars", "loop this section"], destructive: false },
    CommandSpec { kind: CommandKind::TransportRecord, name: "record", phrases: &["record verse", "start recording"], destructive: false },
    CommandSpec { kind: CommandKind::TransportCountIn, name: "count-in", phrases: &["count me in"], destructive: false },
    CommandSpec { kind: CommandKind::EngineCueLevel, name: "cue level", phrases: &["cue up", "cue down"], destructive: false },
    CommandSpec { kind: CommandKind::EngineMonitorRetune, name: "monitor retune", phrases: &["turn on autotune", "autotune off"], destructive: false },
    CommandSpec { kind: CommandKind::EnginePreviewVoice, name: "preview voice", phrases: &[], destructive: false },
    CommandSpec { kind: CommandKind::EngineStopPreview, name: "stop preview", phrases: &[], destructive: false },
    // No phrases: unlike other table entries, a bare "replace the rack"
    // utterance can't be turned into a real `Command::Rack` by the fast
    // voice path alone — there's no rack payload to attach, only a
    // CommandKind marker. Building the actual replacement (which pattern,
    // what seed, how strong a flip) needs the copilot's context, which is
    // exactly why `studio_copilot::FakeCopilot` recognizes "flip the drums"
    // itself rather than this table doing it — see that crate's docs. This
    // entry exists so `RackReplace` has a name/destructive flag for the
    // caller applying whatever `Command::Rack` the copilot actually built.
    CommandSpec { kind: CommandKind::RackReplace, name: "replace rack", phrases: &[], destructive: true },
];

/// Look up a kind's spec. Panics on an unregistered kind — deliberately: a
/// `Command` whose kind has no table entry is a programming error (a new
/// variant added to `Command` without a matching table row), not a runtime
/// condition to handle gracefully. The exhaustiveness test below is what
/// should catch it long before this panic ever fires.
pub fn spec_for(kind: CommandKind) -> &'static CommandSpec {
    COMMAND_TABLE
        .iter()
        .find(|s| s.kind == kind)
        .unwrap_or_else(|| panic!("no CommandSpec registered for {kind:?}"))
}

/// A bounded, overwrite-oldest log of commands and their outcomes. Backs the
/// Log panel, the copilot's context feed, and the e2e test fixture (replay a
/// scripted command file, assert the tap matches expected exactly).
pub struct CommandTap {
    cap: usize,
    seq: u64,
    entries: std::collections::VecDeque<TapEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TapEntry {
    pub seq: u64,
    pub kind: CommandKind,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Applied,
    Rejected(String),
}

impl CommandTap {
    pub fn new(cap: usize) -> CommandTap {
        CommandTap { cap: cap.max(1), seq: 0, entries: std::collections::VecDeque::new() }
    }

    pub fn record(&mut self, cmd: &Command, outcome: Outcome) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(TapEntry { seq, kind: cmd.kind(), outcome });
        seq
    }

    pub fn recent(&self) -> impl Iterator<Item = &TapEntry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::TrackId;

    /// Every `CommandKind` variant must have exactly one row in
    /// `COMMAND_TABLE`. This is the test that keeps the voice grammar and
    /// copilot schema from silently losing coverage as commands are added —
    /// forgetting a row here is what `spec_for` would otherwise only catch
    /// at runtime, for whichever command happened to be exercised.
    #[test]
    fn every_kind_has_exactly_one_spec() {
        for kind in CommandKind::ALL {
            let matches = COMMAND_TABLE.iter().filter(|s| s.kind == *kind).count();
            assert_eq!(matches, 1, "{kind:?} has {matches} table entries, expected 1");
        }
        assert_eq!(
            COMMAND_TABLE.len(),
            CommandKind::ALL.len(),
            "COMMAND_TABLE has a row with no corresponding CommandKind::ALL entry"
        );
    }

    #[test]
    fn edit_is_persistent_transport_and_engine_are_not() {
        let edit = Command::Edit(Op::AddTrack {
            track: studio_core::Track { id: TrackId(0), kind: studio_core::TrackKind::Audio, name: "V".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0,
        });
        assert!(edit.is_persistent());
        assert!(!Command::Transport(TransportCmd::Play).is_persistent());
        assert!(!Command::Engine(EngineCmd::CueLevel(0.5)).is_persistent());
    }

    #[test]
    fn rack_replacement_is_persistent_like_edit_not_ephemeral_like_transport() {
        // Real, kept state — just synced through RackSync's whole-value
        // register (studio-collab) instead of the op log, not a listening
        // choice the way Transport/Engine commands are.
        let rack_cmd = Command::Rack(studio_live::Rack::default());
        assert!(rack_cmd.is_persistent());
        assert_eq!(rack_cmd.kind(), CommandKind::RackReplace);
    }

    #[test]
    fn rack_replacement_is_flagged_destructive() {
        // A whole-rack replace can lose real pattern work if applied by
        // accident, unlike a single StepMod tweak — worth a confirm word.
        let rack_cmd = Command::Rack(studio_live::Rack::default());
        assert!(rack_cmd.is_destructive());
    }

    #[test]
    fn removal_is_destructive_mute_is_not() {
        let remove = Command::Edit(Op::RemoveClip { id: studio_core::ClipId(1) });
        assert!(remove.is_destructive());
        let mute = mute_track(TrackId(0), true, false);
        assert!(!mute.is_destructive());
        assert!(!Command::Transport(TransportCmd::Record(true)).is_destructive());
    }

    #[test]
    fn tap_is_bounded_and_evicts_oldest() {
        let mut tap = CommandTap::new(3);
        for i in 0..5u64 {
            tap.record(&Command::Transport(TransportCmd::Seek(Ticks(i as i64))), Outcome::Applied);
        }
        assert_eq!(tap.len(), 3);
        let seqs: Vec<u64> = tap.recent().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![2, 3, 4], "oldest entries should have been evicted, newest 3 retained in order");
    }

    #[test]
    fn tap_records_rejection_reason() {
        let mut tap = CommandTap::new(4);
        tap.record(
            &Command::Edit(Op::RemoveTrack { id: TrackId(9) }),
            Outcome::Rejected("no such track".into()),
        );
        let entry = tap.recent().next().unwrap();
        assert_eq!(entry.kind, CommandKind::EditRemoveTrack);
        assert!(matches!(&entry.outcome, Outcome::Rejected(msg) if msg == "no such track"));
    }

    #[test]
    fn seq_is_monotonic_across_eviction() {
        let mut tap = CommandTap::new(2);
        let mut last = None;
        for _ in 0..10 {
            let seq = tap.record(&Command::Transport(TransportCmd::Play), Outcome::Applied);
            if let Some(prev) = last {
                assert!(seq > prev, "sequence must stay monotonic even as old entries are evicted");
            }
            last = Some(seq);
        }
    }
}
