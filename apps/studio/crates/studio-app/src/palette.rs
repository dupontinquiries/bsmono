//! palette — every command as a PATH, fuzzy-found, keyboard-first.
//!
//! `C-p` opens it. Type a few letters, pick a verb, and if the verb needs a
//! target the palette asks for that too — "route" → which track → which bus
//! (or a new one). That step structure is the whole design, and it is not
//! decoration:
//!
//! * **A command is a path of choices, resolved by matching text.** Which
//!   means the same machine that serves the keyboard serves anything else
//!   that produces words — [`Palette::resolve`] takes a `&["route",
//!   "vocals", "drums"]` and returns the finished [`Act`] with no UI at all,
//!   which is the seam voice mode plugs into later without a second
//!   implementation of anything.
//! * **Every leaf is an ordinary edit.** The palette builds `Op`s and shell
//!   verbs the keyboard already has; it holds no private powers, the same
//!   bargain `studio-copilot` makes.
//! * **The keybinding is shown beside the entry.** A palette that teaches
//!   the shortcut makes itself unnecessary for the things you do often,
//!   which is the correct ambition for one — the tooltip lesson again.
//!
//! Scoring is a subsequence match with bonuses for word starts and
//! contiguity: "rtb" finds "route track to bus", "volup" finds "volume up".
//! It is pure and tested, because a fuzzy finder that ranks badly is worse
//! than an alphabetical list.

use studio_core::{BusId, Op, Project, TrackId};

/// What a completed path does. Ops apply as one undo group; the rest are
/// shell verbs the keyboard also reaches.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Ops(Vec<Op>),
    /// Feed a character to the existing keymap — one source of truth for
    /// anything that already HAS a key, so the palette can never drift
    /// from the keyboard.
    Key(char),
    Panel(studio_egui::touch::Panel),
    /// Shell-level verbs with no op and no key of their own.
    Shell(ShellVerb),
}

/// The transcript gutter's filters, as something a menu can carry (the
/// dictionary itself is read from a file the shell asks for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordFilterKind { All, Filler, Unique, Dictionary }

/// The stabilizer flavours, as something a menu can carry (the engine's own
/// `Flavor` holds a tracked path and is not `Copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilizeKind { Still, Smooth, Dejerk }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellVerb {
    /// Open a different project. A SHELL verb, not an `Op`: replacing
    /// the document is not an edit TO the document, and routing it
    /// through the op log would try to send "we are now a different
    /// project" to collaborators of the project being left.
    OpenProject,
    /// Add a track below the selected clip's track (or at the bottom).
    AddTrack { video: bool },
    /// Open the native file picker for a project.
    OpenProjectBrowse,
    /// Open the project at this place in `open_project::nearby()`.
    OpenProjectFound(u16),
    /// With unsaved changes: save, then open the waiting project.
    OpenSaveThen,
    /// With unsaved changes: drop them and open the waiting project.
    OpenDiscard,
    /// With unsaved changes: stay on this project.
    OpenCancel,
    /// Collaboration. All shell verbs for the same reason — they change
    /// the TEAM or this device's view, and neither is a `Project` field,
    /// so none of them can be an `Op`.
    CollabInvite,
    CollabRemove,
    /// Put this session's id (the carrier key, readable) on the clipboard
    /// and in the status line, so it can be read to someone.
    CopySessionId,
    /// Open the join window: LAN sessions, or an id typed in.
    JoinSession,
    /// Select every clip of a composition.
    SelectGroup(studio_core::GroupId),
    /// Follow a collaborator's viewport. Local only: what I am looking
    /// at is not part of the project, and syncing it would let one
    /// person drag everybody else's screen around.
    CollabFollow,
    CollabUnfollow,
    /// Ask peers for the full op log again.
    CollabResync,
    /// The voice call over the session's wire: join/leave, mute, and
    /// push-to-talk mode. Shell verbs like the rest of collaboration —
    /// who can hear whom is not a fact about the music.
    /// Cut a camera group to a control channel's level. A shell verb, not
    /// an `Op` list, because the plan depends on the MIX: only the shell
    /// holds the decoded audio the control channel is rendered from.
    MulticamLevel { group: studio_core::GroupId, bus: BusId },
    /// Stabilize the selected clip with one of the flavours. A shell verb:
    /// it renders a file in the background and then relinks the clip.
    /// Show a camera group as a grid in the program monitor, or `None` to
    /// go back to the cut.
    Multiview(Option<studio_core::GroupId>),
    /// Relink every source to a folder of originals, picked with the native
    /// dialog. A shell verb: it has to ask the desktop for a folder, which
    /// is not something an `Op` can do.
    RelinkToFolder,
    /// Redraw every waveform from one band of the spectrum. A shell verb:
    /// the peaks are the shell's, built from decoded audio it owns.
    PeakBand(studio_audio::Band),
    /// Load whisper transcripts from a folder and show them under the clips.
    LoadTranscripts,
    /// Place `source[from_ms..to_ms]` at the playhead. A shell verb because
    /// the playhead is the shell's, not the project's.
    InsertRange { source: studio_core::SourceId, from_ms: u64, to_ms: u64 },
    /// Which words the transcript gutter shows.
    WordFilter(WordFilterKind),
    /// Fold the selected clips into a kaleidoscope of `segments` wedges;
    /// 0 takes it off.
    Kaleidoscope(u32),
    /// Drive `bus` from the selected scene's stage signal through response
    /// preset `preset` (see `stage::PRESETS`).
    StageBind { bus: BusId, preset: u8 },
    /// Measure the look of the picture under the playhead and hold it.
    StyleCopy,
    /// A first cut from the transcripts (`assembly`), at the playhead, on a
    /// new track, reviews as markers.
    Assemble,
    /// Draft outline.txt from the footage (`assembly::draft_story`).
    DraftStory,
    /// Check the timeline against its documents and footage (`story_check`).
    CheckStory,
    /// Survey the video on the timeline for touch-up (`vfx_setup`).
    VfxSurvey,
    /// Give the selected clips the held look, at `pct`% strength.
    StylePaste(u32),
    /// What the selected clips' effects do after the clips end: `ring_ms` of
    /// tail, the last `fade_ms` of it faded. 0 ring is a hard cut at the
    /// clip end. Milliseconds so the verb stays `Copy + Eq`.
    ClipTail { ring_ms: u32, fade_ms: u32 },
    TouchUpStabilize(StabilizeKind),
    /// Clean-plate the selected clip: remove something that crosses it,
    /// using `radius` frames each side. `moving` warps the neighbours onto
    /// the current frame first, for a shot where the camera travels.
    TouchUpCleanPlate { radius: usize, moving: bool },
    /// Rescue a take: punch in by `zoom_pct`% and follow where the movement is.
    TouchUpFollow { zoom_pct: u32 },
    /// Composite particles into the selected clip: preset 0 dirt, 1 dust,
    /// 2 smoke; driven by a control channel's sound, or steady.
    TouchUpParticles { preset: u8, drive: Option<BusId>, ride: bool },
    /// Particles riding points the operator places by hand: opens the window.
    TouchUpParticlesPlace { preset: u8, drive: Option<BusId> },
    CallToggle,
    CallMute,
    CallPushToTalk,
    /// Chord tools on the roll's channel: a harmony `n` scale degrees
    /// away (2 = third, 4 = fifth, 7 = octave, negative below), and every
    /// chord broken into a run in this order.
    Harmonize(i8),
    Arpeggiate(studio_live::Arp),
    /// The arrangement loop range, from the selection; and off.
    LoopSelection,
    LoopOff,
    /// Takes: the selected lane clips copied onto their chain's root;
    /// the lanes under the take target muted or unmuted.
    CompTakes,
    MuteTakeLanes(bool),
    RecordTake,
    /// Show or hide the out-of-key dimming in the piano roll.
    ToggleKeyGuide,
    MarkerHere,
    VoiceVocab(u8),
    Find(u8),
    GotoMarker(studio_core::Ticks),
    GotoStart,
    GotoEnd,
    MixMaster,
    /// Mint an automation curve gated to the selected clips (or bound
    /// always to the strip's lane when nothing is selected) — see
    /// `Studio::new_curve_on_selection`.
    NewCurve,
    /// A curve gated to the group the selected clip is in: a ride over
    /// the composition's hull.
    NewCurveOnGroup,
    /// Cut the selection on the rack grid every N steps (4 = beat, 16 = bar).
    SplitOnGrid(u8),
    /// Every delay's time to N sixteenths at the working tempo — the
    /// sync `instantiate` cannot do, since it has a rate and no tempo.
    SyncDelay(u8),
    /// The strip's binding gets its own copy of its curve.
    MakeCurveUnique,
    /// Overlap policy for the selected clip's track (every track with none selected).
    Overlap(studio_core::Overlap),
    ArchiveDialog,
    ExportDialog,
    /// The music video: the timeline's video clips over the mixdown,
    /// through ffmpeg, next to the project file.
    ExportVideo,
    Settings,
    ToggleWorkbench,
    ToggleLibrary,
    /// Make the timeline show this song (`None` = the whole project).
    SwitchSong(Option<studio_core::GroupId>),
    /// Put the shell in one of the named rooms (`workspace::LAYOUTS`
    /// order — the same index `C-1`..`C-4` reaches).
    Workspace(u8),
    /// Publish the CURRENT rack pattern to the machine-wide cache
    /// (`pattern_pack::cache_dir()`), where every project's palette can
    /// import it. Acts on the pattern being edited — the same "verbs act
    /// on where you are" simplification 'split at playhead' uses.
    SharePattern,
    /// Import the i-th pack from the cache's name-sorted listing. The
    /// index is resolved against a FRESH listing at act time; a pack
    /// removed between the menu and the choice simply misses, with a
    /// status line, never imports the wrong one silently (names are
    /// re-checked).
    ImportPack(u8),
    /// Open the grouping graph -- every composition, tangled or solo, as
    /// nodes and edges (see `studio_core::Project::group_tangles`). The
    /// "cause and effect" visual, before deciding anything about limiting
    /// or tiering group membership.
    ShowGroupGraph,
}

/// One offer in the current step.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub label: String,
    /// The keybinding, when this thing has one — shown right-aligned so
    /// the palette teaches its way out of being needed.
    pub hint: Option<String>,
    pub then: Then,
}

/// What choosing does: finish, or ask the next question.
#[derive(Debug, Clone, PartialEq)]
pub enum Then {
    Do(Act),
    /// Ask for a target; the chosen target completes the verb.
    Ask(Ask),
}

/// The wizards. Each is "a verb missing one argument", and each knows how
/// to enumerate its candidates from the project — no verb invents targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Which composition to select whole.
    SelectGroup,
    /// Which root, then which scale — the two-step the key needs.
    KeyRoot,
    KeyScale(u8),
    /// Which marker to jump to.
    JumpMarker,
    /// Which genre vocabulary the voice fastpath should listen with.
    VoiceVocab,
    /// How to search the timeline.
    FindHow,
    /// Which track to hide (mute is the model's own word for it).
    MuteTrack2,
    /// Which track's gain to change, and by how many dB.
    TrackGain(i8),
    /// Which track to route…
    RouteTrackPick,
    /// …and then to which bus (this variant carries the chosen track).
    RouteBusPick(TrackId),
    BusGain(i8),
    /// Which bus to tap with a control channel — a new silent bus fed by a
    /// pre-fader send, for driving something (a meter, a sidechain, the
    /// multicam switcher) without adding anything to what is heard.
    ControlTapFrom,
    /// Which camera group to cut, and then how often to cut it — the
    /// multicam switcher driven by the project's own beat grid.
    MulticamGroup,
    MulticamEvery(studio_core::GroupId),
    /// The level-driven variant: which camera group, then which control
    /// channel drives it.
    MulticamLevelGroup,
    MulticamLevelBus(studio_core::GroupId),
    /// Which stabilizer flavour to run on the selected clip.
    StabilizeFlavor,
    /// Which camera group to watch as a grid.
    MultiviewGroup,
    /// Which part of the spectrum the waveforms should show.
    PeakBandChoice,
    /// What should happen to the selected clips' reverb/delay at the clip end.
    ClipTailChoice,
    /// Which words to show in the transcript gutter.
    WordFilterChoice,
    /// Insert by content: search transcript lines (or type `C0057 12-18`).
    InsertFind,
    /// How many wedges for the kaleidoscope.
    KaleidoChoice,
    /// Which project to open: the picker, or one found nearby.
    OpenProjectPick,
    /// Audio or video, for a new track.
    AddTrackKind,
    /// The live project has unsaved changes: what to do before opening another.
    UnsavedBeforeOpen,
    /// Which bus the scene's stage should drive, then how it answers.
    StageBus,
    /// How strongly to paste the held look.
    StyleStrength,
    StageResponse(BusId),
    /// How to trim a chosen line before it goes in.
    InsertTrim { source: studio_core::SourceId, start_ms: u64, end_ms: u64 },
    /// From the chosen line to a later line in the same source.
    InsertTo { source: studio_core::SourceId, start_ms: u64 },
    /// Whether the camera moves during the shot being cleaned, then how many
    /// frames each side to look at.
    CleanPlateCamera,
    /// How far to punch in when following the motion.
    FollowZoom,
    /// Which particles, then what drives them.
    ParticlePreset,
    ParticleDrive(u8),
    /// Pinned to the frame, or riding what is under the emitter.
    ParticleRide(u8, Option<BusId>),
    CleanPlateWindow(bool),
    /// Which bus feeds which — the input side, chosen from the receiver.
    BusInputPick(BusId),
    MuteTrack,
    /// Which song (group) to make the timeline show — or "whole project"
    /// to leave song mode. No permanent home in the chrome yet (there is
    /// no song-switcher tab), so this lives here until one exists; the
    /// palette is explicitly the fallback for exactly that case.
    SwitchSong,
    /// Which song to set a key for…
    SongForKey,
    /// …then which root (carries the chosen song)…
    SongKeyRootPick(studio_core::GroupId),
    /// …then which scale (carries song and root).
    SongKeyScale(studio_core::GroupId, u8),
    /// Which song to set a tempo for…
    SongForTempo,
    /// …then which BPM (carries the chosen song) — the same shape as
    /// the key's root/scale walk, one step shorter.
    SongTempoPick(studio_core::GroupId),
    /// Which shared pack to import — enumerated from the cache, the one
    /// candidate list that reads the filesystem rather than the project
    /// (the shelf IS a directory; pretending otherwise would just move
    /// the read somewhere less visible).
    SharedPack,
}

/// The BPMs the tempo step offers. A curated ladder, not a free-text
/// field: the palette is choices matched by typing, and typing "140"
/// against this list IS entering a BPM — while staying resolvable by
/// voice and never needing a numeric parser with its empty/garbage
/// cases. Spans the genres the rack ships kits for (70 half-time trap
/// through 174 DnB); an in-between tempo is one `SetGroupTempo` op away
/// for a future numeric entry, on the same op this path already uses.
pub const TEMPO_LADDER: [u16; 16] =
    [60, 70, 75, 80, 85, 90, 95, 100, 110, 120, 128, 130, 140, 150, 160, 174];

fn db(gain: f32, delta_db: i8) -> f32 {
    (gain * 10f32.powf(delta_db as f32 / 20.0)).clamp(0.0, 8.0)
}

/// The verbs, before any typing. Order is the tie-break for equal scores,
/// so the common things sit first.
pub fn root() -> Vec<Choice> {
    let c = |label: &str, hint: Option<&str>, then: Then| Choice {
        label: label.into(), hint: hint.map(str::to_owned), then };
    vec![
        c("play / pause", Some("space"), Then::Do(Act::Key(' '))),
        // Sessions: the id is what a friend needs to join; the LAN needs
        // nothing but the window.
        c("session id (copy)", None, Then::Do(Act::Shell(ShellVerb::CopySessionId))),
        c("join session (LAN or by id)", None, Then::Do(Act::Shell(ShellVerb::JoinSession))),
        // Compositions: make one from the selection (the key the timeline
        // already has), or select one's clips as a unit.
        c("group selected clips", Some("g"), Then::Do(Act::Key('g'))),
        c("select composition's clips", None, Then::Ask(Ask::SelectGroup)),
        // Key and scale — the data flow the roll's guide reads.
        c("set project key", None, Then::Ask(Ask::KeyRoot)),
        c("switch song", None, Then::Ask(Ask::SwitchSong)),
        c("set song key", None, Then::Ask(Ask::SongForKey)),
        c("set song tempo", None, Then::Ask(Ask::SongForTempo)),
        c("share pattern with other projects", None,
          Then::Do(Act::Shell(ShellVerb::SharePattern))),
        c("import shared pattern", None, Then::Ask(Ask::SharedPack)),
        c("voice vocabulary (genre words)", None, Then::Ask(Ask::VoiceVocab)),
        c("toggle out-of-key dimming", None,
          Then::Do(Act::Shell(ShellVerb::ToggleKeyGuide))),
        // Markers: the one request both the FL Studio and kdenlive
        // communities make in the same words.
        c("drop a marker here", None, Then::Do(Act::Shell(ShellVerb::MarkerHere))),
        c("show transcripts under clips (load a folder of .srt)",
          Some("what is being said, in the gutter under each clip"),
          Then::Do(Act::Shell(ShellVerb::LoadTranscripts))),
        c("story: draft an outline from the footage",
          Some("scenes from what is said; edit outline.txt, then assemble"),
          Then::Do(Act::Shell(ShellVerb::DraftStory))),
        c("vfx: survey the video on the timeline",
          Some("shake, flicker, short handles -> vfx.md and markers"),
          Then::Do(Act::Shell(ShellVerb::VfxSurvey))),
        c("constraints: check the timeline against its documents",
          Some("outline, constraints.txt, .lrc, and what the takes rule out"),
          Then::Do(Act::Shell(ShellVerb::CheckStory))),
        c("assemble a first cut from the transcripts",
          Some("order from outline.txt / script.txt; reviews as markers"),
          Then::Do(Act::Shell(ShellVerb::Assemble))),
        c("insert a clip at the playhead by what is said (or a time range)",
          Some("search the transcripts, or type C0057 12.5-18"),
          Then::Ask(Ask::InsertFind)),
        c("style: copy the look under the playhead", Some("then paste it onto other clips"),
          Then::Do(Act::Shell(ShellVerb::StyleCopy))),
        c("style: paste the copied look onto the selected clips", Some("each clip is matched from its own look"),
          Then::Ask(Ask::StyleStrength)),
        c("stage: let the selected scene drive a bus (music / sfx / rhythm)",
          Some("one arc per scene; each bus answers it its own way"),
          Then::Ask(Ask::StageBus)),
        c("kaleidoscope the selected clip", Some("mirror, 4, 6, 8 or 12 wedges — or off"),
          Then::Ask(Ask::KaleidoChoice)),
        c("transcript gutter: filter words (fillers / unique / a word list)",
          Some("find the ums to cut, or skim for the names and key lines"),
          Then::Ask(Ask::WordFilterChoice)),
        c("reverb/delay tail at the clip end (cut or ring out)",
          Some("hard cut at the edit, or let the space decay past it"),
          Then::Ask(Ask::ClipTailChoice)),
        c("waveforms: show one band of the spectrum",
          Some("see the low end under a voice, or the sibilance over it"),
          Then::Ask(Ask::PeakBandChoice)),
        c("relink media to a folder (proxies → originals)",
          Some("only the paths move; every cut stays where it is"),
          Then::Do(Act::Shell(ShellVerb::RelinkToFolder))),
        c("multiview a camera group (watch every angle)", Some("or go back to the cut"),
          Then::Ask(Ask::MultiviewGroup)),
        c("stabilize the selected clip", Some("still / smooth / de-jerk"),
          Then::Ask(Ask::StabilizeFlavor)),
        // "punch in" last: in the middle, "punch in and ... motion" spelled
        // p-i-a-n-o and outranked the piano roll for anyone typing "piano"
        c("particles: dirt, dust or smoke in the selected clip", Some("moved by a control channel's sound, or steady"),
          Then::Ask(Ask::ParticlePreset)),
        c("rescue a take: follow the motion (punch in)",
          Some("cover a weak take by focusing on what moves — a hand, a turn"),
          Then::Ask(Ask::FollowZoom)),
        c("clean plate the selected clip (remove something crossing it)",
          Some("a car, a passer-by — anything that does not stay"),
          Then::Ask(Ask::CleanPlateCamera)),
        c("cut a camera group on the beat (multicam)",
          Some("switch angles on the beat grid"), Then::Ask(Ask::MulticamGroup)),
        c("cut a camera group to a control channel (multicam)",
          Some("loud passages get the busier angle"), Then::Ask(Ask::MulticamLevelGroup)),
        c("control channel from a bus (dummy channel)",
          Some("a silent tap that drives things without being heard"),
          Then::Ask(Ask::ControlTapFrom)),
        c("open project", Some("the file picker, or a project found nearby"),
          Then::Ask(Ask::OpenProjectPick)),
        // Collaboration. Grouped together and named with "collaborator"
        // in each label so a fuzzy search for that word finds the whole
        // group — the palette matches on text, so the shared word IS the
        // grouping mechanism.
        c("invite collaborator", Some("share this project"),
          Then::Do(Act::Shell(ShellVerb::CollabInvite))),
        c("remove collaborator", None, Then::Do(Act::Shell(ShellVerb::CollabRemove))),
        c("follow collaborator", Some("jump to where they are editing"),
          Then::Do(Act::Shell(ShellVerb::CollabFollow))),
        c("stop following collaborator", None,
          Then::Do(Act::Shell(ShellVerb::CollabUnfollow))),
        c("resync with collaborators", Some("re-request the op log"),
          Then::Do(Act::Shell(ShellVerb::CollabResync))),
        // The call. "collaborator" again, so the group stays one search.
        c("call collaborators (join / leave the voice call)", None,
          Then::Do(Act::Shell(ShellVerb::CallToggle))),
        c("mute / unmute the call microphone", None,
          Then::Do(Act::Shell(ShellVerb::CallMute))),
        c("push-to-talk on the call (hold the talk button)", None,
          Then::Do(Act::Shell(ShellVerb::CallPushToTalk))),
        // The roll's chord tools.
        c("harmonize the roll: add thirds", Some("in the project key, or major"),
          Then::Do(Act::Shell(ShellVerb::Harmonize(2)))),
        c("harmonize the roll: add fifths", None, Then::Do(Act::Shell(ShellVerb::Harmonize(4)))),
        c("harmonize the roll: add an octave above", None, Then::Do(Act::Shell(ShellVerb::Harmonize(7)))),
        c("harmonize the roll: add an octave below", None, Then::Do(Act::Shell(ShellVerb::Harmonize(-7)))),
        c("arpeggiate the roll's chords: up", Some("one step per note"),
          Then::Do(Act::Shell(ShellVerb::Arpeggiate(studio_live::Arp::Up)))),
        c("arpeggiate the roll's chords: down", None, Then::Do(Act::Shell(ShellVerb::Arpeggiate(studio_live::Arp::Down)))),
        c("arpeggiate the roll's chords: up and down", None, Then::Do(Act::Shell(ShellVerb::Arpeggiate(studio_live::Arp::UpDown)))),
        // Loop and takes.
        c("loop the selected clips' span", Some("record inside it for a take per pass"),
          Then::Do(Act::Shell(ShellVerb::LoopSelection))),
        c("loop off", None, Then::Do(Act::Shell(ShellVerb::LoopOff))),
        c("comp: use the selected takes on the track", Some("copies each onto its chain's root"),
          Then::Do(Act::Shell(ShellVerb::CompTakes))),
        c("mute the take lanes under this track", None, Then::Do(Act::Shell(ShellVerb::MuteTakeLanes(true)))),
        c("unmute the take lanes under this track", None, Then::Do(Act::Shell(ShellVerb::MuteTakeLanes(false)))),
        c("find in the timeline", None, Then::Ask(Ask::FindHow)),
        c("jump to marker", None, Then::Ask(Ask::JumpMarker)),
        c("delete all markers", None, Then::Do(Act::Ops(vec![Op::ClearMarkers]))),
        c("hide (mute) a track", None, Then::Ask(Ask::MuteTrack2)),
        c("split every clip at the playhead", Some("s"), Then::Do(Act::Key('s'))),
        c("record a take (microphone)", Some("C-r"),
          Then::Do(Act::Shell(ShellVerb::RecordTake))),
        c("playhead to start", Some("g g"), Then::Do(Act::Shell(ShellVerb::GotoStart))),
        c("playhead to end", Some("g e"), Then::Do(Act::Shell(ShellVerb::GotoEnd))),
        c("undo", Some("u"), Then::Do(Act::Key('u'))),
        c("redo", Some("U"), Then::Do(Act::Key('U'))),
        c("track volume up 3 dB", None, Then::Ask(Ask::TrackGain(3))),
        c("track volume down 3 dB", None, Then::Ask(Ask::TrackGain(-3))),
        c("add a track below", Some("under the selected clip's track, or at the bottom"), Then::Ask(Ask::AddTrackKind)),
        c("route track to bus", None, Then::Ask(Ask::RouteTrackPick)),
        c("bus volume up 3 dB", None, Then::Ask(Ask::BusGain(3))),
        c("bus volume down 3 dB", None, Then::Ask(Ask::BusGain(-3))),
        c("mute / unmute track", Some("m"), Then::Ask(Ask::MuteTrack)),
        c("mix & master pass", None, Then::Do(Act::Shell(ShellVerb::MixMaster))),
        c("align selected clips", Some("A"), Then::Do(Act::Key('A'))),
        c("quantize selected clips", Some("Q"), Then::Do(Act::Key('Q'))),
        c("group into a composition", Some("g"), Then::Do(Act::Key('g'))),
        c("ungroup", Some("G"), Then::Do(Act::Key('G'))),
        c("show grouping graph (debug)", None, Then::Do(Act::Shell(ShellVerb::ShowGroupGraph))),
        c("split at playhead", Some("s"), Then::Do(Act::Key('s'))),
        c("delete selection", Some("d"), Then::Do(Act::Key('d'))),
        c("yank selection", Some("y"), Then::Do(Act::Key('y'))),
        c("paste at playhead (with automation)", Some("p"), Then::Do(Act::Key('p'))),
        c("automation curve on selected clips", None, Then::Do(Act::Shell(ShellVerb::NewCurve))),
        c("automation curve over the selected clip's group", Some("the whole composition, gap included"),
          Then::Do(Act::Shell(ShellVerb::NewCurveOnGroup))),
        c("split selection on beats", None, Then::Do(Act::Shell(ShellVerb::SplitOnGrid(4)))),
        c("split selection on bars", None, Then::Do(Act::Shell(ShellVerb::SplitOnGrid(16)))),
        // Delay sync: the four lengths a producer reaches for, the trap
        // default first.
        c("sync delay to tempo: dotted eighth", None, Then::Do(Act::Shell(ShellVerb::SyncDelay(3)))),
        c("sync delay to tempo: eighth", None, Then::Do(Act::Shell(ShellVerb::SyncDelay(2)))),
        c("sync delay to tempo: quarter", None, Then::Do(Act::Shell(ShellVerb::SyncDelay(4)))),
        c("sync delay to tempo: sixteenth", None, Then::Do(Act::Shell(ShellVerb::SyncDelay(1)))),
        c("make curve unique (strip's binding)", None, Then::Do(Act::Shell(ShellVerb::MakeCurveUnique))),
        c("overlap: newest wins (trim what it lands on)", None, Then::Do(Act::Shell(ShellVerb::Overlap(studio_core::Overlap::Trim)))),
        c("overlap: shadow lane (a track beside it, same bus)", None, Then::Do(Act::Shell(ShellVerb::Overlap(studio_core::Overlap::Lane)))),
        c("overlap: refuse", None, Then::Do(Act::Shell(ShellVerb::Overlap(studio_core::Overlap::Refuse)))),
        c("save archive…", None, Then::Do(Act::Shell(ShellVerb::ArchiveDialog))),
        c("export…", None, Then::Do(Act::Shell(ShellVerb::ExportDialog))),
        c("export the music video (mp4 next to the project)", Some("video clips cut over the mixdown, via ffmpeg"),
          Then::Do(Act::Shell(ShellVerb::ExportVideo))),
        c("settings…", None, Then::Do(Act::Shell(ShellVerb::Settings))),
        c("workspace: edit (arrange & trim)", Some("C-1"),
          Then::Do(Act::Shell(ShellVerb::Workspace(0)))),
        c("workspace: beat (rack & samples)", Some("C-2"),
          Then::Do(Act::Shell(ShellVerb::Workspace(1)))),
        c("workspace: mix (faders)", Some("C-3"),
          Then::Do(Act::Shell(ShellVerb::Workspace(2)))),
        c("workspace: perform (workbench)", Some("C-4"),
          Then::Do(Act::Shell(ShellVerb::Workspace(3)))),
        c("toggle workbench layout", None, Then::Do(Act::Shell(ShellVerb::ToggleWorkbench))),
        c("toggle library sidebar", None, Then::Do(Act::Shell(ShellVerb::ToggleLibrary))),
        c("go to timeline", None, Then::Do(Act::Panel(studio_egui::touch::Panel::Timeline))),
        c("go to rack", None, Then::Do(Act::Panel(studio_egui::touch::Panel::Rack))),
        c("go to mixer", None, Then::Do(Act::Panel(studio_egui::touch::Panel::Mixer))),
        c("go to program (piano roll)", None,
          Then::Do(Act::Panel(studio_egui::touch::Panel::Program))),
    ]
}

/// The candidates for a pending question, read from the project — "new
/// bus" is offered by the routing step itself, because the answer to
/// "which bus" is legitimately "one that does not exist yet" and making
/// the user leave the palette to create it would defeat the palette.
pub fn candidates(ask: Ask, project: &Project) -> Vec<Choice> {
    let plain = |label: String, then: Then| Choice { label, hint: None, then };
    match ask {
        Ask::SelectGroup => project.groups.iter()
            .map(|(id, g)| plain(format!("{} ({} clips)", g.name, g.clips.len()),
                                 Then::Do(Act::Shell(ShellVerb::SelectGroup(*id)))))
            .collect(),
        Ask::KeyRoot => studio_core::PITCH_NAMES.iter().enumerate()
            .map(|(i, n)| plain((*n).to_string(), Then::Ask(Ask::KeyScale(i as u8))))
            .collect(),
        Ask::KeyScale(root) => [studio_core::Scale::Major, studio_core::Scale::Minor,
                                studio_core::Scale::Dorian, studio_core::Scale::Mixolydian,
                                studio_core::Scale::Chromatic].iter()
            .map(|s| plain(s.name().to_string(), Then::Do(Act::Ops(vec![
                Op::SetKey { key: Some(studio_core::Key { root, scale: *s }) }]))))
            .collect(),
        Ask::SwitchSong => {
            let mut out: Vec<Choice> = project.groups.iter()
                .map(|(id, g)| plain(g.name.clone(),
                    Then::Do(Act::Shell(ShellVerb::SwitchSong(Some(*id))))))
                .collect();
            // Always offered, even with zero groups — the way back to the
            // one camera every project starts in, and the only path back
            // once a song has been picked.
            out.push(plain("whole project".into(),
                Then::Do(Act::Shell(ShellVerb::SwitchSong(None)))));
            out
        }
        Ask::SharedPack => {
            let shelf = crate::pattern_pack::list(&crate::pattern_pack::cache_dir());
            if shelf.is_empty() {
                return vec![plain("(nothing shared yet — 'share pattern' \
                                   publishes the one you're on)".into(),
                    Then::Do(Act::Shell(ShellVerb::ImportPack(u8::MAX))))];
            }
            shelf.into_iter().enumerate()
                .map(|(i, (name, _))| plain(name,
                    Then::Do(Act::Shell(ShellVerb::ImportPack(i as u8)))))
                .collect()
        }
        Ask::SongForTempo => project.groups.iter()
            .map(|(id, g)| plain(
                match &g.tempo {
                    // Show the song's current tempo the way the key step
                    // shows its key — the list doubles as a readout.
                    Some(t) => format!("{} ({:.0} bpm)", g.name, t.bpm_at(studio_core::Ticks::ZERO)),
                    None => g.name.clone(),
                },
                Then::Ask(Ask::SongTempoPick(*id))))
            .collect(),
        Ask::SongTempoPick(song) => {
            let mut out: Vec<Choice> = TEMPO_LADDER.iter()
                .map(|bpm| plain(format!("{bpm} bpm"), Then::Do(Act::Ops(vec![
                    Op::SetGroupTempo { id: song,
                        tempo: Some(studio_core::TempoMap::constant(*bpm as f32)) }]))))
                .collect();
            // The way back out: a song whose tempo override should go away
            // again. `None` here means "no opinion", the same meaning the
            // key's None carries — never "wrong".
            out.push(plain("clear (follow the project)".into(),
                Then::Do(Act::Ops(vec![Op::SetGroupTempo { id: song, tempo: None }]))));
            out
        }
        Ask::SongForKey => project.groups.iter()
            .map(|(id, g)| plain(
                match g.key {
                    Some(k) => format!("{} ({})", g.name, k.name()),
                    None => g.name.clone(),
                },
                Then::Ask(Ask::SongKeyRootPick(*id))))
            .collect(),
        Ask::SongKeyRootPick(song) => studio_core::PITCH_NAMES.iter().enumerate()
            .map(|(i, n)| plain((*n).to_string(), Then::Ask(Ask::SongKeyScale(song, i as u8))))
            .collect(),
        Ask::SongKeyScale(song, root) => [studio_core::Scale::Major, studio_core::Scale::Minor,
                                studio_core::Scale::Dorian, studio_core::Scale::Mixolydian,
                                studio_core::Scale::Chromatic].iter()
            .map(|s| plain(s.name().to_string(), Then::Do(Act::Ops(vec![
                Op::SetGroupKey { id: song,
                    key: Some(studio_core::Key { root, scale: *s }) }]))))
            .collect(),
        Ask::FindHow => vec![
            plain("by name".into(), Then::Do(Act::Shell(ShellVerb::Find(0)))),
            plain("by sound (fingerprint, then verify)".into(),
                  Then::Do(Act::Shell(ShellVerb::Find(1)))),
            plain("silent tracks and clips".into(),
                  Then::Do(Act::Shell(ShellVerb::Find(2)))),
        ],
        Ask::VoiceVocab => crate::voice::Domain::ALL.iter().enumerate()
            .map(|(i, d)| plain(d.label().to_string(),
                                Then::Do(Act::Shell(ShellVerb::VoiceVocab(i as u8)))))
            .collect(),
        Ask::JumpMarker => project.markers.iter()
            .map(|m| plain(format!("{} ({:.1}s)", m.name, m.at.to_seconds()),
                           Then::Do(Act::Shell(ShellVerb::GotoMarker(m.at)))))
            .collect(),
        Ask::MuteTrack2 => project.tracks.iter().map(|t| {
            plain(format!("{}{}", t.name, if t.muted { " (hidden)" } else { "" }),
                  Then::Do(Act::Ops(vec![
                      Op::SetTrackFlags { id: t.id, muted: !t.muted, locked: t.locked }])))
        }).collect(),
        Ask::TrackGain(delta) => project.tracks.iter().map(|t| {
            plain(t.name.clone(), Then::Do(Act::Ops(vec![
                Op::SetTrackGain { id: t.id, gain: db(t.gain, delta) }])))
        }).collect(),
        Ask::MuteTrack => project.tracks.iter().map(|t| {
            plain(format!("{}{}", t.name, if t.muted { " (muted)" } else { "" }),
                  Then::Do(Act::Ops(vec![
                      Op::SetTrackFlags { id: t.id, muted: !t.muted, locked: t.locked }])))
        }).collect(),
        Ask::RouteTrackPick => project.tracks.iter()
            .filter(|t| t.kind == studio_core::TrackKind::Audio)
            .map(|t| plain(t.name.clone(), Then::Ask(Ask::RouteBusPick(t.id))))
            .collect(),
        Ask::RouteBusPick(track) => {
            let mut out: Vec<Choice> = project.busses.iter().map(|(id, b)| {
                plain(b.name.clone(), Then::Do(Act::Ops(vec![
                    Op::SetRoute { track, bus: Some(*id) }])))
            }).collect();
            out.push(plain("master (no bus)".into(), Then::Do(Act::Ops(vec![
                Op::SetRoute { track, bus: None }]))));
            // The new bus is created and routed to in ONE group, so an
            // undo of a mistake does not leave an orphan bus behind.
            let fresh = BusId(project.busses.keys().map(|b| b.0).max().unwrap_or(0) + 1);
            out.push(plain("new bus".into(), Then::Do(Act::Ops(vec![
                Op::AddBus { bus: studio_core::Bus {
                    id: fresh, name: format!("bus {}", fresh.0), parent: None,
                    gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false } },
                Op::SetRoute { track, bus: Some(fresh) },
            ]))));
            out
        }
        Ask::MulticamGroup => project.groups.iter()
            .filter(|(id, _)| studio_video::multicam::from_group(project, **id).is_ok())
            .map(|(id, g)| plain(g.name.clone(), Then::Ask(Ask::MulticamEvery(*id))))
            .collect(),
        Ask::MulticamEvery(group) => {
            let Ok((angles, start, end)) = studio_video::multicam::from_group(project, group) else {
                return vec![plain("that group is not cuttable any more".into(), Then::Do(Act::Ops(vec![])))];
            };
            // The beat grid: the camera group's own tempo if it has one,
            // else the project's first song that does, else nothing — and
            // when there is no grid at all the step says so instead of
            // inventing 120 BPM and cutting to a beat nobody set.
            let tempo = project.groups.get(&group).and_then(|g| g.tempo.clone())
                .or_else(|| project.groups.values().find_map(|g| g.tempo.clone()));
            let Some(tempo) = tempo else {
                return vec![plain("no tempo set — set a song tempo first".into(), Then::Do(Act::Ops(vec![])))];
            };
            let beats = studio_video::multicam::beats_from_tempo(&tempo, start, end);
            let span = (end.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
            let track = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
            let first_clip = project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
            [1usize, 2, 4, 8].iter().map(|every| {
                let shots = studio_video::multicam::plan(
                    &angles,
                    &studio_video::multicam::Timing::OnBeats { beats: beats.clone(), every: *every },
                    &studio_video::multicam::Choice::Cycle,
                    studio_video::multicam::Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 },
                    span);
                // The new track and every cut land as one group, so one
                // undo takes the whole switch back — a half-undone multicam
                // cut is worse than none.
                let mut ops = vec![Op::AddTrack { track: studio_core::Track {
                    id: track, kind: studio_core::TrackKind::Video,
                    name: format!("multicam /{every}"), muted: false, locked: false,
                    gain: 1.0, route: None }, at: project.tracks.len() }];
                ops.extend(studio_video::multicam::layout(&shots, &angles, track, start, first_clip));
                Choice {
                    label: match every { 1 => "every beat".into(), n => format!("every {n} beats") },
                    hint: Some(format!("{} cuts", shots.len())),
                    then: Then::Do(Act::Ops(ops)),
                }
            }).collect()
        }
        Ask::InsertTrim { source, start_ms, end_ms } => {
            let src_len = project.sources.get(&source)
                .map(|s| crate::insert_by_content::ms(s.len)).unwrap_or(end_ms);
            let go = |a: u64, b: u64| Then::Do(Act::Shell(ShellVerb::InsertRange {
                source, from_ms: a, to_ms: b.min(src_len).max(a + 1) }));
            vec![
                Choice { label: "just the line".into(),
                         hint: Some(format!("{:.2}–{:.2} s", start_ms as f64 / 1e3, end_ms as f64 / 1e3)),
                         then: go(start_ms, end_ms) },
                Choice { label: "the line with ½ s either side".into(),
                         hint: Some("line times are whole lines; a cut needs air".into()),
                         then: go(start_ms.saturating_sub(500), end_ms + 500) },
                Choice { label: "from here to a later line…".into(),
                         hint: Some("pick where it ends".into()),
                         then: Then::Ask(Ask::InsertTo { source, start_ms }) },
                Choice { label: "5 s from the line".into(), hint: None, then: go(start_ms, start_ms + 5_000) },
                Choice { label: "10 s from the line".into(), hint: None, then: go(start_ms, start_ms + 10_000) },
            ]
        }
        Ask::InsertFind | Ask::InsertTo { .. } => Vec::new(), // need transcripts: see Palette::choices
        Ask::StyleStrength => [(100u32, "the full look"), (75, "most of it — keeps some of the clip"), (50, "halfway — a nod to it"), (0, "remove the look")]
            .into_iter().map(|(p, what)| Choice {
                label: if p == 0 { "off".into() } else { format!("{p}%") }, hint: Some(what.into()),
                then: Then::Do(Act::Shell(ShellVerb::StylePaste(p))),
            }).collect(),
        Ask::StageBus => project.busses.iter().filter(|(_, b)| !b.silent)
            .map(|(id, b)| plain(b.name.clone(), Then::Ask(Ask::StageResponse(*id)))).collect(),
        Ask::StageResponse(bus) => crate::stage::PRESETS.iter().enumerate().map(|(i, p)| Choice {
            label: p.label.into(), hint: Some(p.what.into()),
            then: Then::Do(Act::Shell(ShellVerb::StageBind { bus, preset: i as u8 })),
        }).collect(),
        Ask::AddTrackKind => vec![
            Choice { label: "audio track".into(), hint: None, then: Then::Do(Act::Shell(ShellVerb::AddTrack { video: false })) },
            Choice { label: "video track".into(), hint: None, then: Then::Do(Act::Shell(ShellVerb::AddTrack { video: true })) },
        ],
        Ask::OpenProjectPick => {
            let mut v = vec![Choice { label: "browse… (file picker)".into(), hint: Some("the desktop's own file dialog".into()),
                then: Then::Do(Act::Shell(ShellVerb::OpenProjectBrowse)) }];
            v.extend(crate::open_project::nearby().iter().enumerate().map(|(i, p)| {
                let (name, folder) = crate::open_project::label(p);
                Choice { label: name, hint: Some(folder), then: Then::Do(Act::Shell(ShellVerb::OpenProjectFound(i as u16))) }
            }));
            v
        }
        Ask::UnsavedBeforeOpen => vec![
            Choice { label: "save, then open".into(), hint: Some("keeps this project's changes".into()),
                     then: Then::Do(Act::Shell(ShellVerb::OpenSaveThen)) },
            Choice { label: "discard changes and open".into(), hint: Some("this project goes back to its last save".into()),
                     then: Then::Do(Act::Shell(ShellVerb::OpenDiscard)) },
            Choice { label: "keep working here".into(), hint: None, then: Then::Do(Act::Shell(ShellVerb::OpenCancel)) },
        ],
        Ask::KaleidoChoice => [
            (0, "off", "back to the plain picture"),
            (2, "mirror (2)", "one fold down the middle"),
            (4, "4 wedges", "a cross"),
            (6, "6 wedges", "the classic kaleidoscope"),
            (8, "8 wedges", "a rose window"),
            (12, "12 wedges", "lace — busy, use briefly"),
        ].into_iter().map(|(n, label, what)| Choice {
            label: label.into(), hint: Some(what.into()),
            then: Then::Do(Act::Shell(ShellVerb::Kaleidoscope(n))),
        }).collect(),
        Ask::WordFilterChoice => [
            (WordFilterKind::All, "all words", "what is being said — the default"),
            (WordFilterKind::Filler, "filler words only", "um, uh, you know, o sea — the places to cut"),
            (WordFilterKind::Unique, "unique words only", "names, places, the words said once or twice"),
            (WordFilterKind::Dictionary, "words from a list…", "a file, one word per line: names, key terms"),
        ].into_iter().map(|(k, label, what)| Choice {
            label: label.into(), hint: Some(what.into()),
            then: Then::Do(Act::Shell(ShellVerb::WordFilter(k))),
        }).collect(),
        Ask::ClipTailChoice => [
            (0, 0, "cut at the clip end", "the effect stops dead with the clip — a hard cut"),
            (1_000, 0, "ring 1 s, then cut", "decay past the edit, stop at a known point"),
            (2_000, 1_000, "ring 2 s, fade the last 1 s", "a natural decay under the next shot"),
            (4_000, 2_000, "ring 4 s, fade the last 2 s", "a long space: a hall, a stairwell"),
            (8_000, 0, "ring out fully (8 s)", "let the effect finish on its own"),
        ].into_iter().map(|(ring_ms, fade_ms, label, what)| Choice {
            label: label.into(), hint: Some(what.into()),
            then: Then::Do(Act::Shell(ShellVerb::ClipTail { ring_ms, fade_ms })),
        }).collect(),
        Ask::PeakBandChoice => [
            (studio_audio::Band::Full, "everything (the usual waveform)"),
            (studio_audio::Band::Low, "low — kick, bass, rumble, room"),
            (studio_audio::Band::Mid, "mid — voices and most instruments"),
            (studio_audio::Band::High, "high — consonants, cymbals, air"),
        ].into_iter().map(|(b, what)| Choice {
            label: b.label().to_string(),
            hint: Some(what.into()),
            then: Then::Do(Act::Shell(ShellVerb::PeakBand(b))),
        }).collect(),
        Ask::MultiviewGroup => {
            let mut out: Vec<Choice> = project.groups.iter()
                .filter(|(id, _)| studio_video::multicam::from_group(project, **id).is_ok())
                .map(|(id, g)| plain(g.name.clone(), Then::Do(Act::Shell(ShellVerb::Multiview(Some(*id))))))
                .collect();
            out.push(plain("back to the cut".into(), Then::Do(Act::Shell(ShellVerb::Multiview(None)))));
            out
        }
        Ask::StabilizeFlavor => vec![
            Choice { label: "smooth-cam (keep the move, lose the shake)".into(),
                     hint: Some("a walk-and-talk or a handheld push".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpStabilize(StabilizeKind::Smooth))) },
            Choice { label: "still-cam (lock it off)".into(),
                     hint: Some("a locked shot that was handheld by accident".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpStabilize(StabilizeKind::Still))) },
            Choice { label: "punch-in and de-jerk (least crop)".into(),
                     hint: Some("a deliberate pan with a bump in it".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpStabilize(StabilizeKind::Dejerk))) },
        ],
        Ask::ParticlePreset => [(0u8, "floating dirt", "slow upward drift with spin — lifting off"),
                                 (1, "spaceship dust", "a glowing burst that spreads and slows"),
                                 (2, "creepy smoke", "soft puffs curling in a swirl")]
            .into_iter().map(|(k, label, what)| Choice { label: label.into(), hint: Some(what.into()),
                then: Then::Ask(Ask::ParticleDrive(k)) }).collect(),
        Ask::ParticleDrive(preset) => {
            let mut v = vec![Choice { label: "steady (no drive)".into(), hint: Some("the same all the way through".into()),
                then: Then::Ask(Ask::ParticleRide(preset, None)) }];
            // only control channels: a silent bus is read, not heard, which is
            // what a drive is -- see Bus::silent
            v.extend(project.busses.values().filter(|b| b.silent).map(|b| Choice {
                label: format!("driven by '{}'", b.name), hint: Some("its loudness moves emission, swirl and glow".into()),
                then: Then::Ask(Ask::ParticleRide(preset, Some(b.id))) }));
            v
        }
        Ask::ParticleRide(preset, drive) => vec![
            Choice { label: "pinned to the frame".into(), hint: Some("stays where it is placed".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpParticles { preset, drive, ride: false })) },
            Choice { label: "riding what's under it".into(), hint: Some("tracks the subject under the emitter and follows it".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpParticles { preset, drive, ride: true })) },
            Choice { label: "riding points I place…".into(), hint: Some("tap the subject yourself".into()),
                     then: Then::Do(Act::Shell(ShellVerb::TouchUpParticlesPlace { preset, drive })) },
        ],
        Ask::FollowZoom => [(130u32, "1.3x — keep most of the frame"), (160, "1.6x — the gesture fills it"),
                             (200, "2x — tight on the movement")].into_iter().map(|(z, what)| Choice {
            label: format!("{:.1}x", z as f32 / 100.0), hint: Some(what.into()),
            then: Then::Do(Act::Shell(ShellVerb::TouchUpFollow { zoom_pct: z })),
        }).collect(),
        Ask::CleanPlateCamera => vec![
            Choice { label: "the camera is locked off".into(),
                     hint: Some("tripod, or already stabilized".into()),
                     then: Then::Ask(Ask::CleanPlateWindow(false)) },
            Choice { label: "the camera moves".into(),
                     hint: Some("the neighbouring frames are lined up first".into()),
                     then: Then::Ask(Ask::CleanPlateWindow(true)) },
        ],
        Ask::CleanPlateWindow(moving) => [3usize, 6, 10].iter().map(|r| Choice {
            // The window has to outlast the thing crossing: a wider one
            // recovers more, costs more, and is likelier to include a real
            // change in the background.
            label: format!("±{r} frames"),
            hint: Some(match r {
                3 => "something fast — a car, a cyclist".into(),
                6 => "a walking passer-by".into(),
                _ => "something slow; check the report for what it could not fill".to_string(),
            }),
            then: Then::Do(Act::Shell(ShellVerb::TouchUpCleanPlate { radius: *r, moving })),
        }).collect(),
        Ask::MulticamLevelGroup => project.groups.iter()
            .filter(|(id, _)| studio_video::multicam::from_group(project, **id).is_ok())
            .map(|(id, g)| plain(g.name.clone(), Then::Ask(Ask::MulticamLevelBus(*id))))
            .collect(),
        Ask::MulticamLevelBus(group) => {
            let taps: Vec<Choice> = project.busses.values().filter(|b| b.silent)
                .map(|b| plain(b.name.clone(),
                               Then::Do(Act::Shell(ShellVerb::MulticamLevel { group, bus: b.id }))))
                .collect();
            if taps.is_empty() {
                // Every bus would work as a driver, but a non-silent one is
                // heard as well as read; pointing at the command that makes
                // a proper tap is more use than listing busses that would
                // double the mix.
                return vec![plain("no control channel yet — make one first".into(),
                                  Then::Ask(Ask::ControlTapFrom))];
            }
            taps
        }
        Ask::ControlTapFrom => project.busses.iter()
            .filter(|(_, b)| !b.silent)  // tapping a tap is not a thing
            .map(|(id, b)| {
                let fresh = BusId(project.busses.keys().map(|b| b.0).max().unwrap_or(0) + 1);
                let mut sends = b.sends.clone();
                sends.push(studio_core::Send { to: fresh, level: 1.0, pre_fader: true });
                // One group: the tap, its silence and the send land together,
                // so an undo cannot leave an audible duplicate of the bus.
                plain(b.name.clone(), Then::Do(Act::Ops(vec![
                    Op::AddBus { bus: studio_core::Bus {
                        id: fresh, name: format!("{} ctrl", b.name), parent: None,
                        gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: true } },
                    Op::SetBusSends { id: *id, sends },
                ])))
            }).collect(),
        Ask::BusGain(delta) => project.busses.iter().map(|(id, b)| {
            plain(b.name.clone(), Then::Do(Act::Ops(vec![
                Op::SetBusGain { id: *id, gain: db(b.gain, delta) }])))
        }).collect(),
        Ask::BusInputPick(into) => project.tracks.iter()
            .filter(|t| t.kind == studio_core::TrackKind::Audio && t.route != Some(into))
            .map(|t| plain(format!("track {}", t.name), Then::Do(Act::Ops(vec![
                Op::SetRoute { track: t.id, bus: Some(into) }]))))
            .collect(),
    }
}

/// Fuzzy score: `None` is no match. Higher is better. A subsequence match
/// with bonuses for matching at a word start and for runs — enough to make
/// "rtb" rank "route track to bus" above anything accidental, and cheap
/// enough to run over every entry on every keystroke.
pub fn score(query: &str, label: &str) -> Option<i32> {
    if query.is_empty() { return Some(0); }
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let l: Vec<char> = label.to_lowercase().chars().collect();
    let mut qi = 0usize;
    let mut total = 0i32;
    let mut run = 0i32;
    for (i, ch) in l.iter().enumerate() {
        if qi >= q.len() { break; }
        if *ch == q[qi] {
            let word_start = i == 0
                || matches!(l[i - 1], ' ' | '-' | '_' | '(' | '/');
            total += 1 + if word_start { 6 } else { 0 } + run;
            run += 2;
            qi += 1;
        } else {
            run = 0;
        }
    }
    if qi < q.len() { return None; }
    // Shorter labels win ties: "undo" over "undo the last mix pass".
    Some(total - (l.len() as i32) / 8)
}

/// Rank choices against a query, best first, ties keeping list order.
pub fn filter<'a>(choices: &'a [Choice], query: &str) -> Vec<&'a Choice> {
    let mut scored: Vec<(i32, usize, &Choice)> = choices.iter().enumerate()
        .filter_map(|(i, c)| score(query, &c.label).map(|s| (s, i, c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

/// The palette's whole state: where in the path we are, and what has been
/// typed for the current step.
#[derive(Debug, Clone, Default)]
pub struct Palette {
    /// `None` at the root, `Some` while a wizard is asking.
    pub pending: Option<Ask>,
    /// Breadcrumb of chosen labels — shown, and what a voice transcript
    /// would read back.
    pub path: Vec<String>,
    pub query: String,
    pub selected: usize,
    /// The transcripts, snapshotted when the palette opens — the one input
    /// the project does not hold. Shared, not copied per keystroke.
    pub words: std::sync::Arc<std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>>,
}

/// The find step: a typed time range first (it is exactly what was asked
/// for), then every transcript line. The palette's own fuzzy filter narrows
/// the lines by the typed words, so this lists them all and labels each with
/// its text, source and time.
fn insert_find(project: &Project, words: &std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>,
               query: &str) -> Vec<Choice> {
    use crate::insert_by_content as ibc;
    let mut out = Vec::new();
    if let Some((source, a, b)) = ibc::parse_range(project, query) {
        out.push(Choice {
            // the label IS the query, so the fuzzy filter always keeps it
            label: query.trim().to_string(),
            hint: Some(format!("insert {} {:.2}–{:.2} s", ibc::source_name(project, source),
                               a.to_seconds(), b.to_seconds())),
            then: Then::Do(Act::Shell(ShellVerb::InsertRange { source, from_ms: ibc::ms(a), to_ms: ibc::ms(b) })),
        });
    }
    if words.is_empty() && out.is_empty() {
        out.push(Choice { label: "no transcripts loaded — type a range like C0057 12.5-18".into(),
                          hint: None, then: Then::Do(Act::Ops(vec![])) });
    }
    // Transcript lines are filtered by WORDS before the palette's fuzzy
    // ranking sees them. Fuzzy subsequence matching is right for command
    // names and wrong for dialogue: "gloria" matches any long line that
    // happens to contain g…l…o…r…i…a in order, and on the Fragile Thing card
    // that kept dozens of unrelated lines under the real hits.
    let wanted: Vec<String> = query.split_whitespace().map(studio_voice::normalize)
        .filter(|w| !w.is_empty()).collect();
    for l in ibc::lines(words) {
        if !wanted.is_empty() {
            let have: Vec<String> = l.text.split_whitespace().map(studio_voice::normalize).collect();
            if !wanted.iter().all(|w| have.iter().any(|h| h.starts_with(w.as_str()))) { continue; }
        }
        out.push(Choice {
            label: format!("“{}”", l.text),
            hint: Some(format!("{} {:.1} s", ibc::source_name(project, l.source), l.start.to_seconds())),
            then: Then::Ask(Ask::InsertTrim { source: l.source, start_ms: ibc::ms(l.start), end_ms: ibc::ms(l.end) }),
        });
    }
    out
}

/// Later lines of the same source, each ending the range there.
fn insert_to(project: &Project, words: &std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>,
             source: studio_core::SourceId, start_ms: u64) -> Vec<Choice> {
    use crate::insert_by_content as ibc;
    let from = ibc::from_ms(start_ms);
    words.get(&source).map(|segs| segs.iter().filter(|s| s.end > from).map(|s| Choice {
        label: format!("…to “{}”", s.text),
        hint: Some(format!("{:.1} s long", (s.end.saturating_sub(from)).to_seconds())),
        then: Then::Do(Act::Shell(ShellVerb::InsertRange { source, from_ms: start_ms, to_ms: ibc::ms(s.end) })),
    }).collect()).unwrap_or_else(|| {
        let _ = project;
        Vec::new()
    })
}

impl Palette {
    pub fn open() -> Palette { Palette::default() }

    /// Open with the transcripts, so insert-by-content can search them.
    pub fn open_with(words: std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>) -> Palette {
        Palette { words: std::sync::Arc::new(words), ..Palette::default() }
    }

    pub fn choices(&self, project: &Project) -> Vec<Choice> {
        match self.pending {
            None => root(),
            Some(Ask::InsertFind) => insert_find(project, &self.words, &self.query),
            Some(Ask::InsertTo { source, start_ms }) => insert_to(project, &self.words, source, start_ms),
            Some(ask) => candidates(ask, project),
        }
    }

    /// Take a choice: either finish (returning the act) or descend into the
    /// next question, clearing the query for fresh typing.
    pub fn choose(&mut self, choice: &Choice) -> Option<Act> {
        self.path.push(choice.label.clone());
        self.query.clear();
        self.selected = 0;
        match &choice.then {
            Then::Do(act) => Some(act.clone()),
            Then::Ask(ask) => { self.pending = Some(*ask); None }
        }
    }

    /// Resolve a whole path headlessly — the seam voice mode plugs into:
    /// each word-group picks the best match at its step, and the walk ends
    /// in an [`Act`] or in `None` if any step matched nothing. No UI, no
    /// second implementation of the wizard.
    pub fn resolve(project: &Project, steps: &[&str]) -> Option<Act> {
        let mut p = Palette::open();
        for step in steps {
            let choices = p.choices(project);
            let ranked = filter(&choices, step);
            let best = ranked.first()?;
            if let Some(act) = p.choose(&(*best).clone()) {
                return Some(act);
            }
        }
        None // ran out of words mid-wizard: an honest nothing
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_collab_commands_are_all_findable_by_one_word() {
        // The palette matches on text, so the shared word IS the
        // grouping mechanism — there is no category field. If a label
        // ever drops "collaborator", that command silently leaves the
        // group and nobody notices until they cannot find it.
        // Bound: `filter` borrows the slice, so a temporary would not
        // outlive the call.
        let r = root();
        let found = filter(&r, "collaborator");
        assert!(found.len() >= 4, "only found {}: {:?}", found.len(),
                found.iter().map(|c| &c.label).collect::<Vec<_>>());
    }

    #[test]
    fn insert_by_content_finds_a_line_trims_it_and_offers_a_typed_range() {
        use studio_core::{Op, Source, SourceId, Ticks};
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "/card/2026-09-10_0905_C0057.mkv",
            Ticks::from_seconds(54.0), 48_000, 2, true) }).unwrap();
        let seg = |a: f64, b: f64, t: &str| studio_voice::Segment { start: Ticks::from_seconds(a),
            end: Ticks::from_seconds(b), text: t.into(), speaker: None, confidence: 1.0 };
        let mut words = std::collections::BTreeMap::new();
        words.insert(SourceId(1), vec![seg(1.0, 3.0, "Good morning, I'm Gloria."),
                                       seg(3.0, 5.0, "This is Max."),
                                       seg(8.0, 12.0, "You will be with other kids.")]);
        let mut pal = Palette::open_with(words);
        assert!(!filter(&root(), "insert").is_empty());
        pal.pending = Some(Ask::InsertFind);

        // words: the fuzzy filter narrows the lines
        pal.query = "gloria".into();
        let all = pal.choices(&p);
        let hits = filter(&all, &pal.query);
        assert_eq!(hits[0].label, "“Good morning, I'm Gloria.”", "{:?}", hits.iter().map(|c| &c.label).collect::<Vec<_>>());
        assert_eq!(hits.len(), 1, "only lines that contain the word — no scattered-letter matches");
        let Then::Ask(trim @ Ask::InsertTrim { start_ms: 1000, end_ms: 3000, .. }) = hits[0].then else { panic!("{:?}", hits[0].then) };

        // trim: just the line, with handles, or to a later line
        pal.pending = Some(trim);
        pal.query.clear();
        let t = pal.choices(&p);
        let range = |c: &Choice| match c.then { Then::Do(Act::Shell(ShellVerb::InsertRange { from_ms, to_ms, .. })) => (from_ms, to_ms), _ => (0, 0) };
        assert_eq!(range(&t[0]), (1000, 3000), "just the line");
        assert_eq!(range(&t[1]), (500, 3500), "half a second either side");
        let Then::Ask(to @ Ask::InsertTo { start_ms: 1000, .. }) = t[2].then else { panic!() };
        pal.pending = Some(to);
        let later = pal.choices(&p);
        assert_eq!(later.len(), 3, "every line that ends after the start");
        assert_eq!(range(&later[2]), (1000, 12000), "from Gloria's line to the end of 'other kids'");

        // a typed range is offered first and survives the filter
        pal.pending = Some(Ask::InsertFind);
        pal.query = "C0057 12.5-18".into();
        let r = pal.choices(&p);
        let kept = filter(&r, &pal.query);
        assert_eq!(range(kept[0]), (12500, 18000), "{:?}", kept.iter().map(|c| &c.label).collect::<Vec<_>>());
    }

    #[test]
    fn the_word_filter_offers_every_mode_with_all_first() {
        let r = root();
        assert!(!filter(&r, "filler").is_empty(), "reachable by the job's own word");
        assert!(!filter(&r, "transcript").is_empty());
        let modes: Vec<WordFilterKind> = candidates(Ask::WordFilterChoice, &Project::default()).iter()
            .map(|c| match c.then { Then::Do(Act::Shell(ShellVerb::WordFilter(k))) => k, _ => panic!() })
            .collect();
        assert_eq!(modes, vec![WordFilterKind::All, WordFilterKind::Filler,
                               WordFilterKind::Unique, WordFilterKind::Dictionary],
                   "all first: the way back from a filtered gutter");
    }

    #[test]
    fn the_tail_chooser_offers_a_hard_cut_first_and_every_ring_says_what_it_is_for() {
        let r = root();
        assert!(!filter(&r, "reverb").is_empty() && !filter(&r, "tail").is_empty());
        let choices = candidates(Ask::ClipTailChoice, &Project::default());
        let got: Vec<(u32, u32)> = choices.iter().map(|c| match c.then {
            Then::Do(Act::Shell(ShellVerb::ClipTail { ring_ms, fade_ms })) => (ring_ms, fade_ms),
            _ => panic!("{:?}", c.label),
        }).collect();
        assert_eq!(got[0], (0, 0), "a hard cut is the first answer — it is the way back");
        assert!(got.iter().any(|(r, f)| *r > 0 && *f == 0), "a ring that ends in a hard cut");
        assert!(got.iter().any(|(r, f)| *r > 0 && *f > 0), "a ring that fades");
        assert!(got.iter().all(|(r, f)| f <= r), "no fade outlasts its ring");
        assert!(choices.iter().all(|c| c.hint.is_some()));
    }

    #[test]
    fn the_band_chooser_offers_every_band_and_says_what_each_is_for() {
        let r = root();
        assert!(!filter(&r, "waveform").is_empty(), "reachable by the word on screen");
        assert!(!filter(&r, "band").is_empty());
        let bands = candidates(Ask::PeakBandChoice, &Project::default());
        let got: Vec<studio_audio::Band> = bands.iter().map(|c| match c.then {
            Then::Do(Act::Shell(ShellVerb::PeakBand(b))) => b,
            _ => panic!("{:?}", c.label),
        }).collect();
        assert_eq!(got, vec![studio_audio::Band::Full, studio_audio::Band::Low,
                             studio_audio::Band::Mid, studio_audio::Band::High],
                   "full first: the way out of a filtered view is the first thing offered");
        assert!(bands.iter().all(|c| c.hint.is_some()),
                "a band name is not self-explanatory to someone who does not mix");
    }

    #[test]
    fn the_stabilizer_offers_its_flavours_and_names_what_each_is_for() {
        let r = root();
        assert!(!filter(&r, "stabilize").is_empty(), "reachable by its own word");
        let flavours = candidates(Ask::StabilizeFlavor, &Project::default());
        assert_eq!(flavours.len(), 3);
        // Every flavour says what it is FOR, not just what it is called: the
        // difference between them is a judgement about the shot.
        assert!(flavours.iter().all(|c| c.hint.is_some()), "{:?}", flavours.iter().map(|c| &c.label).collect::<Vec<_>>());
        let kinds: Vec<StabilizeKind> = flavours.iter().map(|c| match c.then {
            Then::Do(Act::Shell(ShellVerb::TouchUpStabilize(k))) => k,
            _ => panic!("{:?}", c.label),
        }).collect();
        assert_eq!(kinds, vec![StabilizeKind::Smooth, StabilizeKind::Still, StabilizeKind::Dejerk],
                   "smooth first: it is the one that fits most shots");
    }

    #[test]
    fn the_clean_plate_asks_about_the_camera_before_the_window() {
        let r = root();
        assert!(!filter(&r, "clean plate").is_empty());
        let cam = candidates(Ask::CleanPlateCamera, &Project::default());
        assert_eq!(cam.len(), 2, "locked off, or moving");
        // The camera question comes FIRST because it changes what the window
        // means: on a moving shot the neighbours must be lined up before they
        // can stand in for the covered pixels.
        let moving: Vec<bool> = cam.iter().map(|c| match c.then {
            Then::Ask(Ask::CleanPlateWindow(m)) => m,
            _ => panic!("{:?}", c.label),
        }).collect();
        assert_eq!(moving, vec![false, true]);
        for m in [false, true] {
            let w = candidates(Ask::CleanPlateWindow(m), &Project::default());
            assert_eq!(w.len(), 3);
            let asked: Vec<(usize, bool)> = w.iter().map(|c| match c.then {
                Then::Do(Act::Shell(ShellVerb::TouchUpCleanPlate { radius, moving })) => (radius, moving),
                _ => panic!("{:?}", c.label),
            }).collect();
            assert_eq!(asked, vec![(3, m), (6, m), (10, m)], "the camera answer is carried through");
            assert!(w.iter().all(|c| c.hint.is_some()), "each window says what it is for");
        }
    }

    #[test]
    fn a_multicam_cut_lands_on_the_beat_alternates_angles_and_undoes_whole() {
        use studio_core::*;
        let mut p = Project::default();
        for id in 1..=2u64 {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video,
                name: format!("angle {id}"), muted: false, locked: false, gain: 1.0, route: None },
                at: (id - 1) as usize }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(id), path: format!("/card/C{id}.mkv"),
                len: Ticks::from_seconds(120.0), sample_rate: 48_000, channels: 2, has_video: true,
                kind: SourceKind::File } }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
                pos: Ticks::from_seconds(4.0), src_in: Ticks::from_seconds(4.0 + id as f64),
                len: Ticks::from_seconds(8.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();

        // No tempo yet: the step says so rather than inventing one.
        let no_grid = candidates(Ask::MulticamEvery(GroupId(1)), &p);
        assert!(no_grid[0].label.contains("no tempo"), "{:?}", no_grid[0].label);

        p.apply(&Op::SetGroupTempo { id: GroupId(1), tempo: Some(TempoMap::constant(120.0)) }).unwrap();
        assert_eq!(candidates(Ask::MulticamGroup, &p).len(), 1, "the camera group is offered");
        let every = candidates(Ask::MulticamEvery(GroupId(1)), &p);
        assert_eq!(every.len(), 4, "every 1/2/4/8 beats");
        let Then::Do(Act::Ops(ops)) = every[1].then.clone() else { panic!() };  // every 2 beats = 1 s
        let inv = p.apply(&Op::Group { ops }).expect("applies");

        let cuts: Vec<&Clip> = p.clips.values().filter(|c| c.track == TrackId(3)).collect();
        assert!(cuts.len() >= 7, "8 s at a cut a second: {}", cuts.len());
        for c in &cuts {
            assert!((c.pos.to_seconds().fract()).abs() < 1e-6, "on the beat grid: {c:?}");
            // each cut reads its own moment in its own angle's file
            let offset = if c.source == SourceId(1) { 1.0 } else { 2.0 };
            assert!((c.src_in.to_seconds() - c.pos.to_seconds() - offset).abs() < 1e-6, "{c:?}");
        }
        let mut by_pos: Vec<&&Clip> = cuts.iter().collect();
        by_pos.sort_by_key(|c| c.pos);
        assert!(by_pos.windows(2).all(|w| w[0].source != w[1].source), "angles alternate");
        p.apply(&inv).expect("undoes");
        assert!(p.tracks.iter().all(|t| t.id != TrackId(3)), "one undo takes the track too");
        assert_eq!(p.clips.len(), 2, "and every cut");
    }

    #[test]
    fn a_control_tap_is_silent_fed_pre_fader_and_lands_in_one_undo() {
        let mut p = project();
        let before = studio_audio::mixdown(&p, &Default::default(), &Default::default(),
            studio_core::Ticks::ZERO, studio_core::Ticks::from_seconds(0.2), 48_000, &[], &[]);
        let choices = candidates(Ask::ControlTapFrom, &p);
        assert!(!choices.is_empty(), "there is a bus to tap");
        let Then::Do(Act::Ops(ops)) = choices[0].then.clone() else { panic!("{:?}", choices[0].then) };
        let inv = p.apply(&Op::Group { ops: ops.clone() }).expect("applies");
        let tap = p.busses.values().find(|b| b.silent).expect("a silent tap");
        let source = p.busses.values().find(|b| !b.silent && b.sends.iter().any(|s| s.to == tap.id))
            .expect("fed by a send");
        assert!(source.sends.iter().find(|s| s.to == tap.id).unwrap().pre_fader,
                "a control tap hears the bus, not its fader moves");
        // it changes nothing about what is heard
        let after = studio_audio::mixdown(&p, &Default::default(), &Default::default(),
            studio_core::Ticks::ZERO, studio_core::Ticks::from_seconds(0.2), 48_000, &[], &[]);
        assert_eq!(before, after, "a control channel must be inaudible");
        // and it is one undo, not two
        p.apply(&inv).expect("undoes");
        assert!(p.busses.values().all(|b| !b.silent), "the tap is gone");
        // tapping a tap is not offered
        p.apply(&Op::Group { ops }).unwrap();
        let again = candidates(Ask::ControlTapFrom, &p);
        assert!(again.iter().all(|c| !c.label.ends_with("ctrl")), "{:?}", again.iter().map(|c| &c.label).collect::<Vec<_>>());
    }

    #[test]
    fn opening_a_project_is_reachable_from_the_palette() {
        let r = root();
        assert!(!filter(&r, "open project").is_empty());
    }
    use super::*;
    use studio_core::{Bus, Track, TrackKind};

    fn project() -> Project {
        let mut p = Project::default();
        for (i, name) in ["vocals", "drums", "bass"].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1),
                kind: TrackKind::Audio, name: (*name).into(), muted: false,
                locked: false, gain: 1.0, route: None }, at: i }).unwrap();
        }
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "drum bus".into(),
            parent: None, gain: 1.0, chain: vec![], sends: vec![],
            muted: false, silent: false } }).unwrap();
        p
    }

    #[test]
    fn fuzzy_ranking_puts_the_obvious_thing_first() {
        let r = root();
        let top = |q: &str| filter(&r, q).first().map(|c| c.label.clone()).unwrap_or_default();
        assert_eq!(top("rtb"), "route track to bus", "initials of the words");
        assert_eq!(top("undo"), "undo", "exact short label beats longer matches");
        assert_eq!(top("mixer"), "go to mixer");
        assert_eq!(top("master"), "mix & master pass");
        assert_eq!(top("piano"), "go to program (piano roll)");
        assert!(filter(&r, "zzzz").is_empty(), "no match is empty, not everything");
        assert_eq!(filter(&r, "").len(), r.len(), "an empty query shows the menu");
    }

    #[test]
    fn the_route_wizard_walks_track_then_bus_and_can_make_one() {
        let p = project();
        // The path a keyboard walks: verb, track, bus.
        let act = Palette::resolve(&p, &["route", "vocals", "drum bus"]).expect("resolves");
        assert_eq!(act, Act::Ops(vec![Op::SetRoute { track: TrackId(1),
                                                     bus: Some(BusId(1)) }]));
        // "new bus" creates AND routes in one group, so undoing a mistake
        // leaves no orphan bus behind.
        let Some(Act::Ops(ops)) = Palette::resolve(&p, &["route", "bass", "new bus"])
            else { panic!("expected ops") };
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], Op::AddBus { .. }));
        assert!(matches!(ops[1], Op::SetRoute { track: TrackId(3), bus: Some(BusId(2)) }));
        // Stopping mid-wizard is None, not a half-applied guess.
        assert!(Palette::resolve(&p, &["route"]).is_none());
        assert!(Palette::resolve(&p, &["route", "nonesuch"]).is_none());
    }

    fn project_with_groups() -> Project {
        let mut p = project();
        p.apply(&Op::AddSource { source: studio_core::Source { id: studio_core::SourceId(1),
            path: "x".into(), len: studio_core::Ticks::from_seconds(10.0), sample_rate: 48_000,
            channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: studio_core::Clip { id: studio_core::ClipId(1),
            source: studio_core::SourceId(1), track: TrackId(1), pos: studio_core::Ticks::ZERO,
            src_in: studio_core::Ticks::ZERO, len: studio_core::Ticks::from_seconds(1.0),
            gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetGroup { id: studio_core::GroupId(1), name: "Track 1".into(),
                                clips: vec![studio_core::ClipId(1)] }).unwrap();
        p
    }

    #[test]
    fn switch_song_offers_every_group_plus_the_way_back() {
        let p = project_with_groups();
        let act = Palette::resolve(&p, &["switch song", "track 1"]).expect("resolves");
        assert_eq!(act, Act::Shell(ShellVerb::SwitchSong(Some(studio_core::GroupId(1)))));
        // The way back exists even though "whole project" was never a
        // group — this is the one candidate list that must offer an
        // option NOT drawn from `project.groups`, or there would be no
        // path out of song mode from the palette at all.
        let back = Palette::resolve(&p, &["switch song", "whole project"]).expect("resolves");
        assert_eq!(back, Act::Shell(ShellVerb::SwitchSong(None)));
    }

    #[test]
    fn switch_song_offers_the_way_back_even_with_zero_groups() {
        // A project with no songs yet must still resolve "whole project" —
        // it is not just the alternative to a group, it is the default,
        // and a palette that only shows it once a group exists would make
        // the very first group irreversible from here.
        let p = project();
        let act = Palette::resolve(&p, &["switch song", "whole project"]).expect("resolves");
        assert_eq!(act, Act::Shell(ShellVerb::SwitchSong(None)));
    }


    /// C12-14f's palette wiring: "set song tempo" mirrors "set song key"'s
    /// walk (one step shorter — song, then BPM), lands on Op::SetGroupTempo
    /// with a constant TempoMap, and offers the way OUT (clear -> None) the
    /// same way switch-song always offers "whole project".
    #[test]
    fn set_song_tempo_walks_song_then_bpm_and_clears() {
        let p = project_with_groups();
        let act = Palette::resolve(&p, &["set song tempo", "track 1", "140"])
            .expect("resolves");
        assert_eq!(act, Act::Ops(vec![Op::SetGroupTempo {
            id: studio_core::GroupId(1),
            tempo: Some(studio_core::TempoMap::constant(140.0)) }]));
        // Never the project-wide tempo — same discipline as the key test.
        let clear = Palette::resolve(&p, &["set song tempo", "track 1", "clear"])
            .expect("resolves");
        assert_eq!(clear, Act::Ops(vec![Op::SetGroupTempo {
            id: studio_core::GroupId(1), tempo: None }]),
            "clear means no opinion, the meaning None already has");
    }

    /// The workspaces resolve by name and carry the same index the C-1..C-4
    /// chord reaches — the palette teaching the shortcut, as its hints
    /// column promises.
    #[test]
    fn workspaces_resolve_by_room_name() {
        let p = project();
        assert_eq!(Palette::resolve(&p, &["workspace beat"]).expect("resolves"),
                   Act::Shell(ShellVerb::Workspace(1)));
        assert_eq!(Palette::resolve(&p, &["workspace mix"]).expect("resolves"),
                   Act::Shell(ShellVerb::Workspace(2)));
    }

    #[test]
    fn set_song_key_walks_song_then_root_then_scale() {
        let p = project_with_groups();
        let act = Palette::resolve(&p, &["set song key", "track 1", "d", "dorian"])
            .expect("resolves");
        assert_eq!(act, Act::Ops(vec![Op::SetGroupKey { id: studio_core::GroupId(1),
            key: Some(studio_core::Key { root: 2, scale: studio_core::Scale::Dorian }) }]));
        // This must be Op::SetGroupKey, never the project-wide Op::SetKey —
        // the whole reason C12-14a exists as its own op rather than reusing
        // the project-wide one is that these two must never be confused,
        // in either direction.
        assert!(!matches!(act, Act::Ops(ref ops) if matches!(ops[0], Op::SetKey { .. })));
    }

    #[test]
    fn volume_steps_are_decibels_on_the_chosen_track() {
        let p = project();
        let Some(Act::Ops(ops)) = Palette::resolve(&p, &["volume up", "drums"])
            else { panic!("expected ops") };
        let Op::SetTrackGain { id, gain } = ops[0] else { panic!("gain op") };
        assert_eq!(id, TrackId(2));
        assert!((20.0 * gain.log10() - 3.0).abs() < 0.01, "+3 dB, got {gain}");
        let Some(Act::Ops(down)) = Palette::resolve(&p, &["volume down", "drums"])
            else { panic!("expected ops") };
        let Op::SetTrackGain { gain, .. } = down[0] else { panic!("gain op") };
        assert!((20.0 * gain.log10() + 3.0).abs() < 0.01, "-3 dB, got {gain}");
    }

    #[test]
    fn a_verb_with_a_key_says_which_key_and_delegates_to_it() {
        // The palette must never reimplement a keyboard verb: entries that
        // have a key CARRY that key, so the two cannot drift, and the hint
        // teaches the shortcut that makes the palette unnecessary.
        let r = root();
        for (label, key) in [("undo", 'u'), ("split at playhead", 's'),
                             ("align selected clips", 'A'), ("yank selection", 'y'),
                             ("paste at playhead (with automation)", 'p')] {
            let c = r.iter().find(|c| c.label == label).unwrap();
            assert_eq!(c.then, Then::Do(Act::Key(key)));
            assert!(c.hint.is_some(), "'{label}' has a key and must show it");
        }
        // Multi-key hints ("g g") name a keymap SEQUENCE and are checked
        // by the keymap's own tests, not here; this loop covers the
        // single-key delegations, which are the ones that could silently
        // diverge from the keyboard.
        // And every hint that exists must be a key the keymap or the
        // shell's own interception actually handles — one source of truth.
        let intercepted = ['A', 'Q', ',', '.', '<', '>', 'g', 'G', 'n', 'D', '[', ']'];
        for c in &r {
            let Some(Act::Key(ch)) = (match &c.then {
                Then::Do(a) => Some(a.clone()), _ => None }) else { continue };
            let mut k = studio_egui::keys::Keymap::new();
            let bound = matches!(k.feed(studio_egui::keys::Key::c(ch)),
                                 studio_egui::keys::Resolution::Act(_));
            assert!(bound || intercepted.contains(&ch),
                    "'{}' claims key '{ch}' that nothing handles", c.label);
        }
    }
    #[test]
    fn sessions_and_compositions_are_reachable_from_the_root() {
        let p = Project::default();
        let got = Palette::resolve(&p, &["session id"]);
        assert!(matches!(got, Some(Act::Shell(ShellVerb::CopySessionId))), "{got:?}");
        assert!(matches!(Palette::resolve(&p, &["join session"]), Some(Act::Shell(ShellVerb::JoinSession))));
        assert!(matches!(Palette::resolve(&p, &["group selected"]), Some(Act::Key('g'))));
        let p = project_with_groups();
        let labels: Vec<String> = candidates(Ask::SelectGroup, &p).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, vec!["Track 1 (1 clips)"]);
        assert_eq!(Palette::resolve(&p, &["select composition's clips", "Track 1"]),
                   Some(Act::Shell(ShellVerb::SelectGroup(studio_core::GroupId(1)))), "{labels:?}");
    }

}
