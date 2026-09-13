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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellVerb {
    /// Open a different project. A SHELL verb, not an `Op`: replacing
    /// the document is not an edit TO the document, and routing it
    /// through the op log would try to send "we are now a different
    /// project" to collaborators of the project being left.
    OpenProject,
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
        c("open project", Some("switch to another project"),
          Then::Do(Act::Shell(ShellVerb::OpenProject))),
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
        c("route track to bus", None, Then::Ask(Ask::RouteTrackPick)),
        c("bus volume up 3 dB", None, Then::Ask(Ask::BusGain(3))),
        c("bus volume down 3 dB", None, Then::Ask(Ask::BusGain(-3))),
        c("mute / unmute track", Some("m"), Then::Ask(Ask::MuteTrack)),
        c("mix & master pass", None, Then::Do(Act::Shell(ShellVerb::MixMaster))),
        c("align selected clips", Some("A"), Then::Do(Act::Key('A'))),
        c("quantize selected clips", Some("Q"), Then::Do(Act::Key('Q'))),
        c("group into a composition", Some("g"), Then::Do(Act::Key('g'))),
        c("ungroup", Some("G"), Then::Do(Act::Key('G'))),
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
                    gain: 1.0, chain: vec![], sends: vec![], muted: false } },
                Op::SetRoute { track, bus: Some(fresh) },
            ]))));
            out
        }
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
}

impl Palette {
    pub fn open() -> Palette { Palette::default() }

    pub fn choices(&self, project: &Project) -> Vec<Choice> {
        match self.pending {
            None => root(),
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
            muted: false } }).unwrap();
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
            channels: 1, has_video: false } }).unwrap();
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
        let intercepted = ['A', 'Q', ',', '.', '<', '>', 'g', 'G'];
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
