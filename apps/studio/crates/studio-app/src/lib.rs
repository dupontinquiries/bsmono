//! studio-app — the shell that makes seven crates into an application.
//!
//! Studio had a model, a convergent op log, a wire, a timeline view, an audio
//! analyser and a full mastering chain — and **no way to start it**. Seven
//! libraries and no binary is not an application; it is a pile of well-tested
//! parts, and the gap is invisible from inside because every crate's tests pass.
//!
//! This is deliberately thin. It owns the state that spans crates — the project,
//! the transport, the session, which panel is showing — and delegates everything
//! else. Logic that lands here instead of in a library is logic that cannot be
//! tested headlessly, so the rule is: the shell wires, it does not decide.

use studio_collab::{session::{Session, Team}, Doc, SiteId};
use studio_core::{Project, Ticks};
use studio_egui::touch::{Panel, ViewMode};
use studio_live::transport::Transport;

/// Which file on disk is which clip, and what to do when it moves. The
/// content hash existed (collab's Manifest) and the path existed; nothing
/// joined them, so a moved sample failed as a decode error rather than as
/// "this file moved".
/// Dialogue/Music/Effects stems -- the shape a film mix is delivered in, so
/// the picture can be dubbed by replacing one of the three. `export`'s
/// per-track stems are the music convention and cannot answer that request.
/// Which of eight vocal takes is usable. `takes.rs` puts them on lanes and
/// `comp_ops` copies chosen spans; neither helps you choose, and soloing each
/// in turn is most of what comping costs.
/// The part of an Ozone-style display that is not drawing: meter ballistics,
/// a log frequency axis, and a rolling trace. The analysis existed; nothing
/// turned it into something an eye can read.
pub mod meter;
pub mod take_report;
pub mod stems_dme;
/// Curve-to-curve references, the cycles they allow, and the dummy that
/// repairs one. Cycles are accepted and reported rather than refused: an
/// editor builds the clip first and finds the loop after.
pub mod curve_refs;
pub mod media_map;
/// What a dropped file should become. The drop handler had one branch --
/// directory is a kit, everything else is a rack channel -- so a video
/// dropped on the timeline became a sampler hit.
pub mod drop_plan;
pub mod archive;
pub mod audition;
pub mod call;
pub mod capture;
/// Playing a synth, and getting a pattern back.
pub mod perform;
pub mod export;
pub mod upscale;
pub mod clip_inspector;
pub mod fx_editor;
pub mod find;
pub mod library;
pub mod looping;
pub mod midi;
pub mod mix_assistant;
pub mod multicam_cut;
pub mod relink;
pub mod transcripts;
pub mod overview;
pub mod insert_by_content;
pub mod stage;
pub mod kaleido_gallery;
pub mod signals_panel;
pub mod comic_job;
pub mod track_points;
pub mod wake;
pub mod open_project;
pub mod tracks;
pub mod thumbs;
pub mod pattern_clips;
#[cfg(test)]
mod abundance_tests;
#[cfg(test)]
mod fixture_tests;
pub mod touchup_job;
pub mod mixer_panel;
pub mod morph_pad;
pub mod program_panel;
pub mod palette;
pub mod project_dir;
pub mod viewport;
pub mod playhead_sim;
pub mod voice;
pub mod workspace;
pub mod rebinds;
pub mod scroll_edge;
pub mod pattern_pack;
pub mod rack_panel;
pub mod seq_header;
pub mod assembly;
pub mod deliver;
pub mod story_check;

/// Everything the running editor holds.
pub struct Studio {
    pub session: Session,
    pub transport: Transport,
    pub view: studio_egui::View,
    /// Camera (zoom/scroll) for each song-group, so switching between songs
    /// in an album restores where you were in THAT song rather than
    /// carrying one song's zoom onto another. Device-local, not synced —
    /// exactly the same category `GroupId`'s own doc comment already
    /// names for fold-open/closed state ("view state in the device's
    /// workspace sidecar"), so this deliberately lives here rather than
    /// in the `Project`/Op log: two collaborators are allowed to look at
    /// different parts of the same song at different zoom levels.
    /// Populated lazily by [`Studio::current_view_mut`]; a song never
    /// opened this session simply isn't a key yet.
    pub song_views: std::collections::BTreeMap<studio_core::GroupId, studio_egui::View>,
    /// Which song-group the timeline is currently showing. `None` means
    /// "the whole project as one timeline" — today's only behavior, and
    /// what every project with zero groups still gets, unchanged.
    pub active_song: Option<studio_core::GroupId>,
    pub panel: Panel,
    pub mode: ViewMode,
    pub selection: Vec<studio_core::ClipId>,
    /// Undo/redo as INVERSE OPS, not project snapshots — and routed back
    /// through [`Studio::edit`]'s stamping path when used, so an undo is an
    /// edit that travels to peers exactly like the edit it reverts. (The
    /// previous `studio_core::History` wiring mutated the project directly,
    /// bypassing the Doc's op log — which meant every local edit would have
    /// been LOST on the first remote-conflict replay. Collab correctness is
    /// why this shell exists on a `Doc` at all, so the log is now the only
    /// door.)
    undo_stack: Vec<studio_core::Op>,
    redo_stack: Vec<studio_core::Op>,
    /// Packets produced by local activity — stamped ops (including undos)
    /// and rack syncs — waiting for whatever carrier exists to drain them.
    /// The shell is transport-agnostic on purpose: this Vec IS the seam.
    outbox: Vec<studio_collab::wire::Packet>,
    /// Voice frames received, waiting for the shell's [`call::Call`] to
    /// take them. `Studio` owns the document and the wire's integration;
    /// it does not own the sink or the microphone, which is where a
    /// voice frame is going. Handing frames across this Vec keeps
    /// `integrate_packet` a pure function of the document, testable
    /// without a device — the same seam `outbox` is, pointing the other
    /// way.
    pub voice_inbox: Vec<studio_collab::wire::Packet>,
    /// Manifests for sources whose bytes this peer can serve.
    manifests: std::collections::BTreeMap<studio_core::SourceId, studio_collab::assets::Manifest>,
    /// Content hash -> where that media is on disk, and which source it became.
    ///
    /// Populated in `register_asset`, which is where the hash already exists.
    /// Two things need it: dropping the same file twice should reuse the
    /// source rather than mint a second one with a second copy of the audio,
    /// and a file that MOVED should be relinkable instead of failing to decode
    /// at playback with no indication that a path is the problem.
    pub media_map: crate::media_map::MediaMap,
    /// Which source each asset key resolved to, for the dedupe lookup.
    ///
    /// Separate from `media_map` because the map is about media on disk and
    /// this is about this project's sources — the same file in two projects is
    /// one entry there and two here.
    asset_sources: std::collections::BTreeMap<crate::media_map::AssetKey, studio_core::SourceId>,
    /// The content-addressed chunk store — shared across sources on purpose:
    /// a re-bounce that changed one bar re-ships one bar.
    chunk_store: std::collections::BTreeMap<studio_collab::assets::ChunkId, Vec<u8>>,
    /// Manifests still collecting chunks.
    pending_assets: std::collections::BTreeMap<studio_core::SourceId, studio_collab::assets::Manifest>,
    /// Media that finished arriving and decoded — the shell drains this into
    /// its audio/peaks maps. A queue rather than a callback so the headless
    /// tests can watch the same door the GUI uses.
    pub incoming_audio: Vec<(studio_core::SourceId, studio_audio::SourceAudio)>,
    /// Video sources whose audio still has to be pulled out with ffmpeg.
    /// Filled by `load_from`, emptied by the shell onto a worker thread:
    /// extraction is seconds per file, and doing it inline held the window
    /// closed for over a minute on a 77-clip card.
    pub pending_video_audio: Vec<(studio_core::SourceId, std::path::PathBuf)>,
    pub keys: studio_egui::keys::Keymap,
    /// Status line — what just happened, so an action never fails silently.
    pub status: String,

    // ── the live half ────────────────────────────────────────────────────
    /// The rack. Local to this Studio for now: `studio-collab` already has
    /// `Packet::Rack` (whole-value LWW) on the wire, so sharing it is a
    /// broadcast call away, but this shell does not make that call yet —
    /// named here rather than implied.
    pub rack: studio_live::Rack,
    pub current_pattern: usize,
    pub selected_channel: usize,
    pub lane: studio_egui::lanes::Lane,
    pub paint: studio_egui::grid::Paint,
    /// Play intent for the audition loop. Intent, not the device state: the
    /// shell syncs the sink to this every frame, so the headless tests below
    /// can drive the space bar without a sound card existing.
    pub rack_playing: bool,
    /// Set by the keymap's menu keys (Shift+Space, `:`), read and cleared by
    /// the shell, which owns the palette. The keymap had resolved both keys
    /// for a long time while nothing handled them: the corner hint said
    /// "S-space menu" and the key did nothing, leaving Ctrl+P — shown
    /// nowhere — as the only way in.
    pub palette_requested: bool,
    /// Set by the `r` key; the shell starts or stops a take (it owns the
    /// microphone), as it does for C-r.
    pub take_requested: bool,
    /// Set by any rack edit; the shell re-renders the loop and clears it.
    pub loop_dirty: bool,
    /// Set by any PROJECT edit (including undo/redo and a bounce); the shell
    /// re-renders the arrangement buffer before the next timeline playback.
    pub arrangement_dirty: bool,
    /// Set by [`Studio::seek`] whenever the playhead is moved by anything
    /// OTHER than the sink-follows-transport feedback line in `sync_sink`
    /// (scrub, GotoStart/GotoEnd, a marker jump, find-and-goto). Read and
    /// cleared by `sync_sink`, which otherwise has no way to tell "the user
    /// just seeked" apart from "nothing changed" -- `loaded`/
    /// `arrangement_dirty` alone cover a content change, not a bare
    /// position change within the SAME content, which is exactly what a
    /// pause-then-seek-backward-then-play is. Without this flag the sink's
    /// own internal cursor stays wherever it was left, play resumes it from
    /// there, and the very next tick's `transport.set_position` (fed by the
    /// sink's real position) silently snaps the playhead back -- the bug
    /// this field exists to close.
    pub seek_pending: bool,
    /// Video/UI-facing clock for an active arrangement loop, kept SEPARATE
    /// from the audio's own cycling position -- see `App::sync_sink`'s
    /// unwrap step. `loop_video_base` is the timeline position where the
    /// loop was (re)engaged; `loop_video_elapsed` accumulates real frames
    /// played since then, never reset by a wrap; `loop_last_raw_frame` is
    /// the last cycling sink frame observed, needed to detect the next
    /// wrap. Together they let video/the playhead ruler keep advancing
    /// like a normal timeline while the audio underneath repeats a short
    /// window forever -- "the timeline should function like a timeline"
    /// even when a beat pattern loops. Reset whenever the loop's buffer is
    /// (re)loaded (see `sync_sink`'s resync branch).
    pub loop_video_base: Ticks,
    pub loop_video_elapsed: i64,
    pub loop_last_raw_frame: Option<usize>,
    /// Solo'd tracks — monitoring state, deliberately NOT an op (see
    /// `mixer_panel`'s docs): it shapes what you hear and what you export,
    /// never what the project is.
    pub solo: Vec<studio_core::TrackId>,
    /// A fader mid-drag: (track, previewed value). Local by design — one
    /// `SetTrackGain` op lands when the drag ends.
    pub mixer_drag: Option<(studio_core::TrackId, f32)>,
    /// The clip inspector's fader mid-drag, same commit-on-release rule.
    pub clip_gain_drag: Option<(studio_core::ClipId, f32)>,
    /// The piano roll's resize-in-progress (a fresh note being drawn out,
    /// or an existing one grabbed by its edge).
    pub roll: studio_egui::roll::RollState,
    /// The step grid's and the piano roll's shared horizontal scroll, so a
    /// switch between the two views of a pattern keeps the same steps under
    /// your eye. `seq_scroll_owner` is the view that last wrote it; the other
    /// adopts it on the frame it appears (see `seq_header::adopt_scroll`).
    pub seq_scroll_x: f32,
    pub seq_scroll_owner: Option<Panel>,
    /// Which lane the automation strip under the timeline shows, or `None`
    /// for no strip. Local monitoring-adjacent state like `solo`: what you
    /// are LOOKING at is not a project fact.
    pub auto_target: Option<studio_core::AutoTarget>,
    /// Or which BINDING the strip shows — a curve through its gate, drawn
    /// at the gate's origin. Exclusive with `auto_target`: one strip, one
    /// thing. Same local-only status as the lane choice.
    pub auto_binding: Option<studio_core::BindingId>,
    /// The strip's point mid-drag — ghost until release, ops on release,
    /// the fader rule.
    pub auto_drag: Option<studio_egui::auto::PointDrag>,
    /// What `y` yanked: clip ids, read back at paste time so a paste
    /// copies the clip AS IT IS NOW (and its bindings), not a snapshot
    /// from whenever `y` was pressed. Local: your clipboard is not a
    /// project fact.
    pub clipboard: Vec<studio_core::ClipId>,
    /// What a placement does to the clips it lands on — the default and
    /// per-track overrides. Local (workspace sidecar): it decides which
    /// ops THIS editor emits, and every op it emits converges the same.
    pub overlap: studio_core::Overlap,
    pub overlap_tracks: std::collections::BTreeMap<studio_core::TrackId, studio_core::Overlap>,
    /// A take is rolling on this site — rides in presence so peers know.
    pub recording: bool,
    /// Workbench layout: the timeline stays on screen with the Rack /
    /// Program / Mixer in a resizable strip below it, instead of four
    /// full-screen tabs. The tabs remain the phone's layout; this is the
    /// desktop's "shown together".
    pub combined: bool,
    /// Compositions folded closed ON THIS DEVICE — view state beside
    /// `combined`, persisted in the workspace sidecar, never an op.
    pub closed_groups: std::collections::BTreeSet<studio_core::GroupId>,
    /// Out-of-key dimming in the roll — a VIEW preference (the key itself
    /// is the project's), so it rides in the sidecar like every other
    /// per-device choice.
    pub key_guide: bool,
    /// The mixer asked for a mastering analysis of this bus. The panel
    /// cannot run it — analysis needs the shell's decoded audio — so it
    /// leaves the request as a value, the way every other view here
    /// returns edits rather than performing them.
    pub mix_pass_request: Option<(studio_core::BusId, String)>,
    /// The bus faders' twin of `mixer_drag`.
    pub bus_drag: Option<(studio_core::BusId, f32)>,
    /// Motions (the Helix keymap) or Playing (the keyboard is a piano).
    pub input_mode: studio_egui::playkeys::InputMode,
    /// Octave shift for playing mode.
    pub octave: i8,
    /// Which edge an ambiguous timeline grab prefers; `alt-;` flips it.
    pub trim_side: studio_egui::TrimSide,
    /// Set by the panel's bounce button; the shell performs it.
    pub pending_bounce: bool,
    /// A pattern to land on the timeline next frame: (pattern index, time, track). Set by a
    /// library drop or ⇥; `pending_bounce` is the rack's "at the playhead" case of it.
    pub pending_place: Option<(usize, studio_core::Ticks, studio_core::TrackId)>,
    /// LIVE SUBPATTERNS. Sources the timeline got from a pattern, keyed to the pattern by
    /// NAME (indices move when patterns are deleted; names are what the user sees). When
    /// the rack changes, every linked source is re-rendered and the arrangement re-mixed,
    /// so the playhead passes through the pattern as it is NOW — through the same mixdown
    /// the export uses, which is what makes the preview mirror the render. Not persisted:
    /// a saved project holds the bounce's WAV, and a reopened project plays that frozen
    /// take until the pattern is placed again.
    pub pattern_links: std::collections::BTreeMap<studio_core::SourceId, String>,
    /// What this device's person is called in presence packets — per device
    /// (it rides the workspace sidecar), because the same account on a phone
    /// and a desktop may legitimately want "kes (phone)" vs "kes". Empty
    /// means "site N", never an error.
    pub display_name: String,
    /// Monotonic stamp for outgoing presence, so a reordered packet cannot
    /// resurrect a stale playhead on a peer — the receiving side's `seq`
    /// check is only as good as the sender's discipline here.
    presence_seq: u64,
    /// What we last shipped, compared by content to decide whether anything
    /// worth announcing changed.
    last_presence: Option<studio_collab::Presence>,
    last_presence_ms: i64,
    /// The effect editor, when one is open. View state on `Studio` rather
    /// than `App` because the clip inspector and the mixer — which only see
    /// `Studio` — are the two doors that open it.
    pub fx_editor: Option<fx_editor::FxEditor>,
    /// A session this device JOINED by id (palette → "join session"), which
    /// replaces the key derived from our own team + project name. The
    /// carrier keys its records by [`Studio::carrier_key`], so setting this
    /// is what "join" means at the wire.
    pub joined_key: Option<[u8; 20]>,
    /// Wakes the window when a worker the core started has news. Does
    /// nothing until the app installs one; headless use and tests keep that.
    pub waker: wake::Wake,
}

impl Studio {
    /// Move the playhead by any USER-INITIATED means -- scrub, GotoStart/
    /// GotoEnd, a marker jump, find-and-goto. Sets `seek_pending` so
    /// `App::sync_sink` knows to re-seek the sink even when nothing about
    /// WHAT is loaded changed, only WHERE in it. The sink-follows-transport
    /// feedback line in `sync_sink` must NOT go through this method (it
    /// would set `seek_pending` every single frame and force a reseek on
    /// every tick) -- it calls `self.transport.set_position` directly, on
    /// purpose, and that asymmetry is the whole fix.
    pub fn seek(&mut self, at: Ticks) {
        self.transport.set_position(at);
        self.seek_pending = true;
    }

    /// The 20-byte key the carrier publishes and reads under: a joined
    /// session's, else our own team + project's. One place, so the palette's
    /// "session id" and the wire agree by construction.
    pub fn carrier_key(&self) -> [u8; 20] {
        self.joined_key.unwrap_or_else(|| self.session.key())
    }

    /// Select every clip of a composition — the group becomes the thing the
    /// next nudge, delete or effect applies to.
    pub fn select_group(&mut self, id: studio_core::GroupId) {
        // Only what this comp SHOWS. Once a comp has been split the two halves
        // share one member list, so selecting `g.clips` would hand the left
        // half the right half's clips as well — and the next nudge would drag
        // material that is not on screen. See `group_members_in_window`.
        let picked = self.session.doc.project.group_members_in_window(id);
        if let Some(g) = self.session.doc.project.groups.get(&id) {
            let name = g.name.clone();
            self.status = format!("selected {} clips of {}", picked.len(), name);
            self.selection = picked;
        }
    }

    pub fn new(project: Project, site: SiteId, team: Team) -> Studio {
        let name = project.name.clone();
        Studio {
            session: Session::open(team, &name, Doc::new(site, project)),
            transport: Transport::new(48_000),
            fx_editor: None,
            joined_key: None,
            waker: wake::none(),
            view: studio_egui::View::default(),
            song_views: std::collections::BTreeMap::new(),
            active_song: None,
            panel: Panel::Timeline,
            mode: ViewMode::Split,
            selection: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            outbox: Vec::new(),
            voice_inbox: Vec::new(),
            manifests: Default::default(),
            media_map: Default::default(),
            asset_sources: Default::default(),
            chunk_store: Default::default(),
            pending_assets: Default::default(),
            incoming_audio: Vec::new(),
            pending_video_audio: Vec::new(),
            keys: studio_egui::keys::Keymap::new(),
            status: "ready".into(),
            rack: studio_live::Rack::trap_kit(),
            current_pattern: 0,
            selected_channel: 0,
            lane: studio_egui::lanes::Lane::default(),
            paint: studio_egui::grid::Paint::default(),
            rack_playing: false,
            palette_requested: false,
            take_requested: false,
            loop_dirty: true,
            arrangement_dirty: true,
            seek_pending: false,
            loop_video_base: Ticks::ZERO,
            loop_video_elapsed: 0,
            loop_last_raw_frame: None,
            solo: Vec::new(),
            mixer_drag: None,
            clip_gain_drag: None,
            roll: Default::default(),
            seq_scroll_x: 0.0,
            seq_scroll_owner: None,
            auto_target: None,
            auto_binding: None,
            auto_drag: None,
            clipboard: Vec::new(),
            overlap: studio_core::Overlap::default(),
            overlap_tracks: Default::default(),
            recording: false,
            combined: false,
            closed_groups: Default::default(),
            key_guide: true,
            mix_pass_request: None,
            bus_drag: None,
            input_mode: Default::default(),
            octave: 0,
            trim_side: studio_egui::TrimSide::default(),
            pending_bounce: false,
            pending_place: None,
            pattern_links: std::collections::BTreeMap::new(),
            display_name: String::new(),
            presence_seq: 0,
            last_presence: None,
            last_presence_ms: i64::MIN / 2,
        }
    }

    /// A demo project, so the binary starts with something on screen.
    ///
    /// An editor that opens empty gives a new user nothing to click, and an
    /// empty timeline is indistinguishable from a broken one.
    pub fn demo() -> Studio {
        use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};
        let mut p = Project { name: "untitled".into(), ..Default::default() };
        for (i, (kind, nm)) in [(TrackKind::Video, "V1"), (TrackKind::Audio, "A1"),
                                 (TrackKind::Audio, "A2")].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: (*nm).into(), muted: false, locked: false, gain: 1.0, route: None }, at: i }).ok();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "takes/demo.wav".into(),
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true, kind: studio_core::SourceKind::File } }).ok();
        for i in 0..6u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i + 1), source: SourceId(1),
                track: TrackId(i % 3 + 1), pos: Ticks::from_seconds(i as f64 * 3.5),
                src_in: Ticks::from_seconds(i as f64 * 2.0), len: Ticks::from_seconds(3.0),
                gain: 1.0, effects: Vec::new() } }).ok();
        }
        Studio::new(p, SiteId(1), Team::from_contacts("solo", &["sc-local".into()]))
    }

    /// Apply an edit through the Doc's op log — stamped, undoable, and
    /// pushed to the outbox — so undo works, peers converge, and the status
    /// line says so.
    /// Apply several ops as ONE undo step. A partial failure rolls the
    /// whole group back, the way `History::apply_group` does for the
    /// collab path: half a mastering pass is worse than none.
    pub fn edit_group(&mut self, ops: Vec<studio_core::Op>, what: &str) {
        let mut applied = Vec::new();
        for op in ops {
            match self.edit_inner(op) {
                Ok(inverse) => applied.push(inverse),
                Err(e) => {
                    // Undo what landed, in reverse, then say what happened.
                    for inv in applied.into_iter().rev() {
                        let _ = self.edit_inner(inv);
                    }
                    self.status = format!("{what} refused: {e:?} (nothing applied)");
                    return;
                }
            }
        }
        if applied.is_empty() {
            self.status = format!("{what}: nothing to do");
            return;
        }
        // One group, one undo: the inverses replay newest-first.
        let count = applied.len();
        self.undo_stack.push(studio_core::Op::Group {
            ops: applied.into_iter().rev().collect() });
        self.redo_stack.clear();
        self.status = format!("{what} — one undo reverses all {count}");
        self.arrangement_dirty = true;
    }

    pub fn edit(&mut self, op: studio_core::Op, what: &str) {
        match self.edit_inner(op) {
            Ok(inverse) => {
                self.undo_stack.push(inverse);
                self.redo_stack.clear();
                self.status = what.into();
                self.arrangement_dirty = true;
            }
            // An edit that cannot apply must SAY so. A silently dropped edit is
            // the worst failure an editor has: the user believes it worked.
            Err(e) => self.status = format!("{what} refused: {e:?}"),
        }
    }

    /// The one door to the project: pre-validate on a scratch copy (which is
    /// also what yields the inverse op — `Doc::local` doesn't return one),
    /// then stamp and log for real. Two applies and a clone per user
    /// gesture, the same per-edit cost `studio-engine`'s snapshot swap
    /// already accepts, bought here for the property that the log and the
    /// project can never disagree about what happened.
    fn edit_inner(&mut self, op: studio_core::Op)
        -> Result<studio_core::Op, studio_core::EditError>
    {
        let mut probe = self.session.doc.project.clone();
        let inverse = probe.apply(&op)?;
        let stamped = self.session.doc.local(op)?;
        self.outbox.push(studio_collab::wire::Packet::Op(stamped));
        Ok(inverse)
    }

    /// Run the selected clip's source through print-side autotune and land
    /// the result as a NEW source and clip on the same track, muting the
    /// original in place of deleting it.
    ///
    /// Print-side, not live: `studio_fx::autotune` is explicitly a
    /// whole-buffer pass (epoch continuity and the ratio track both need
    /// more context than a realtime callback has), so it is deliberately NOT
    /// registered as a chain effect kind — a chain effect that silently
    /// needed the whole take would be a lie about what chains are. This is
    /// the shape the module's own docs name as correct: bounce a take
    /// through it.
    ///
    /// A new source rather than an edit in place, because the tuned take is
    /// a different recording: the dry one stays on disk, stays undoable,
    /// stays something a peer can hear, and A/B is a mute toggle away.
    /// Channels are tuned independently (each with its own detector), which
    /// is right for the mono vocal takes this exists for; a wide stereo take
    /// can in principle drift between sides, and mixing to mono first would
    /// be a bigger lie than the drift.
    pub fn autotune_selected(
        &mut self,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
        scale: studio_fx::retune::Scale,
    ) -> Option<(studio_core::SourceId, studio_audio::SourceAudio)> {
        let Some(clip_id) = self.selection.first().copied() else {
            self.status = "autotune: select a clip first".into();
            return None;
        };
        let clip = self.session.doc.project.clips.get(&clip_id)?.clone();
        let Some(audio) = sources.get(&clip.source) else {
            self.status = "autotune: that clip's audio is not loaded".into();
            return None;
        };
        let rate = audio.sample_rate;
        let tuned: Vec<Vec<f32>> = audio.channels.iter().enumerate().map(|(i, ch)| {
            let mut a = studio_fx::autotune::AutotuneChannel::new(
                scale, rate as f32, 0xA07 + i as u64);
            a.process_buffer(ch)
        }).collect();
        let tuned = studio_audio::SourceAudio { sample_rate: rate, channels: tuned };

        let id = studio_core::SourceId(
            self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let new_clip = studio_core::ClipId(
            self.session.doc.project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let old = self.session.doc.project.sources.get(&clip.source)?.clone();
        // display_name(), not the path stem: a pattern source has no path, so
        // stemming it yields the "take" fallback and every tuned pattern in a
        // project collides on "take-N.wav". The name is the identity.
        let path = format!("tuned/{}-{}.wav", old.display_name(), id.0);
        self.edit(studio_core::Op::AddSource { source: studio_core::Source {
            id, path, len: studio_core::Ticks::from_samples(tuned.frames() as i64, rate),
            sample_rate: rate, channels: tuned.channels.len() as u16, has_video: false, kind: studio_core::SourceKind::File,
        } }, "tuned");
        // The tuned copy needs somewhere to live: the same track is occupied
        // by the dry take, so it goes to the first audio track with room —
        // the rule import already follows, rather than refusing outright.
        let end = studio_core::Ticks(clip.pos.0 + clip.len.0);
        let project = &self.session.doc.project;
        let track = project.tracks.iter()
            .filter(|t| t.kind == studio_core::TrackKind::Audio && !t.locked)
            .find(|t| !project.clips.values()
                .any(|c| c.track == t.id && c.pos < end && c.end() > clip.pos))
            .map(|t| t.id);
        let Some(track) = track else {
            self.status = "autotune: no audio track has room beside the take".into();
            return None;
        };
        self.edit(studio_core::Op::AddClip { clip: studio_core::Clip {
            id: new_clip, source: id, track, pos: clip.pos,
            src_in: clip.src_in, len: clip.len, gain: clip.gain,
            effects: clip.effects.clone(),
        } }, "tuned");
        self.selection = vec![new_clip];
        self.status = format!("tuned the take into '{}'",
            self.session.doc.project.tracks.iter().find(|t| t.id == track)
                .map(|t| t.name.clone()).unwrap_or_default());
        Some((id, tuned))
    }

    /// Write the whole session — project, edit history, rack — to `path`
    /// in the collab file format. Media is NOT embedded: sources keep their
    /// paths (the compressed originals and the bounce WAVs already on
    /// disk), which is what keeps a project file kilobytes while its
    /// samples stay wherever the library lives.
    pub fn save_to(&mut self, path: &std::path::Path) -> Result<(), String> {
        let file = studio_collab::file::SaveFile::from_project(
            &self.session.doc.project,
            self.session.doc.history().to_vec(),
            Some(self.rack.clone()),
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
        }
        std::fs::write(path, file.encode()).map_err(|e| format!("write {path:?}: {e}"))?;
        self.status = format!("saved {}", path.display());
        Ok(())
    }

    /// Open a saved session. A constructor rather than a mutation: loading
    /// replaces everything, and a half-replaced Studio is worse than none.
    ///
    /// Source media is re-decoded from the paths the file references,
    /// resolved RELATIVE TO THE FILE — a project folder that moves as a
    /// unit keeps working — and delivered through `incoming_audio`, the
    /// same door peer media uses, so the shell drains one queue no matter
    /// where sound comes from. Missing or undecodable files are counted in
    /// the status and their clips stay silent: a beat with one lost sample
    /// still opens.
    /// Open a single-file archive: all state, all audio, one file that
    /// opens whole or refuses whole. Embedded media feeds the same
    /// `incoming_audio` door a peer's does; where [`archive::find_upgrade`]
    /// locates a higher-quality per-device artifact under the archive's
    /// search paths, THAT is decoded instead — the proxy is the floor,
    /// never the ceiling.
    pub fn open_archive(path: &std::path::Path, site: SiteId, team: Team)
        -> Result<Studio, String>
    {
        let bytes = std::fs::read(path).map_err(|e| format!("read {path:?}: {e}"))?;
        let u = archive::unpack(&bytes)
            .ok_or_else(|| format!("{} is not a studio archive (or is damaged)",
                                   path.display()))?;
        // Authorized collaborators, ENFORCED: when the archive names a
        // roster, the session's team is derived from it — same id math a
        // hand-built team uses, so every holder of the archive lands on
        // one session and a join from outside it is refused at the door.
        // An archive with no roster keeps the caller's team: metadata that
        // was never written must not lock anyone out.
        let team = if u.meta.artists.is_empty() { team } else {
            Team::from_contacts(&u.save.name, &u.meta.artists)
        };
        let mut s = Studio::new(u.save.to_project(), site, team);
        if let Some(rack) = u.save.rack { s.rack = rack; }
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut upgraded = 0usize;
        let mut failed = 0usize;
        for m in u.media {
            let src_path = s.session.doc.project.sources.get(&m.source)
                .map(|src| src.path.clone()).unwrap_or_default();
            let device = archive::find_upgrade(&base, &u.meta.search_paths, &src_path)
                .and_then(|p| {
                    let hint = p.extension().and_then(|e| e.to_str())
                        .map(str::to_owned);
                    std::fs::read(&p).ok().map(|raw| (raw, hint))
                });
            let (raw, hint) = match device {
                Some((raw, hint)) => { upgraded += 1; (raw, hint) }
                None => (m.bytes, Some(m.ext)),
            };
            match studio_media::decode_bytes(raw.clone(), hint.as_deref()) {
                Ok(audio) => {
                    s.incoming_audio.push((m.source, audio));
                    s.register_asset(m.source, &raw);
                }
                Err(_) => failed += 1,
            }
        }
        s.outbox.clear();
        s.status = format!("opened archive: {} ({} upgraded from device{})",
            path.display(), upgraded,
            if failed > 0 { format!(", {failed} failed to decode") } else { String::new() });
        Ok(s)
    }

    /// The archive's pack half: state from the live project + rack,
    /// originals reassembled from the content-addressed chunk store, PCM
    /// from the caller's decode cache. Returns the bytes and the pack
    /// report — missing sources, dedup savings, near-duplicate pairs.
    pub fn pack_archive(
        &self,
        audio: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
        meta: &archive::ArchiveMeta,
    ) -> Result<(Vec<u8>, archive::PackReport), String> {
        let save = studio_collab::file::SaveFile::from_project(
            &self.session.doc.project, Vec::new(), Some(self.rack.clone()));
        let mut originals = std::collections::BTreeMap::new();
        for (id, manifest) in &self.manifests {
            if let Ok(bytes) = manifest.assemble(&self.chunk_store) {
                let ext = self.session.doc.project.sources.get(id)
                    .and_then(|s| std::path::Path::new(&s.path).extension())
                    .and_then(|e| e.to_str()).unwrap_or("bin").to_string();
                originals.insert(*id, (ext, bytes));
            }
        }
        archive::pack(&save, audio, &originals, meta)
    }

    /// Pull the audio out of queued video sources, one at a time, sending
    /// each as it lands so the timeline fills in progressively. `None` for
    /// a file with no audio stream (a picture, not a failure). Blocking: the
    /// shell runs it on a worker thread; tests run it inline.
    ///
    /// `wake` is called after every delivery. A timed repaint request alone
    /// did not reliably wake the window: seen on screen, the status sat at
    /// "1 to go" with the last clip long extracted, until the mouse moved.
    pub fn extract_video_audio(pending: Vec<(studio_core::SourceId, std::path::PathBuf)>,
                               tx: std::sync::mpsc::Sender<(studio_core::SourceId, Option<studio_audio::SourceAudio>)>,
                               wake: impl Fn()) {
        for (id, path) in pending {
            let audio = studio_video::extract_audio(&path).ok().flatten();
            if tx.send((id, audio)).is_err() { return; } // the shell went away
            wake();
        }
    }

    pub fn load_from(path: &std::path::Path, site: SiteId, team: Team)
        -> Result<Studio, String>
    {
        let bytes = std::fs::read(path).map_err(|e| format!("read {path:?}: {e}"))?;
        let file = studio_collab::file::SaveFile::decode(&bytes)
            .ok_or_else(|| format!("{} is not a studio file (or is damaged)", path.display()))?;
        let mut s = Studio::new(file.to_project(), site, team);
        if let Some(rack) = file.rack {
            s.rack = rack;
        }
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let sources: Vec<_> = s.session.doc.project.sources.values().cloned().collect();
        let mut missing = 0usize;
        for src in sources {
            // a pattern source has no file by design: the rack renders it
            // (pattern_clips), so it is neither read here nor counted missing
            if src.is_generator() { continue; }
            let p = base.join(&src.path);
            // Video is checked for, not READ: reading and content-hashing a
            // whole card of camera files inline (register_asset chunks and
            // hashes every byte) was the second thing holding the window
            // closed, after inline extraction — 77 proxies is 355 MB. The
            // worker needs only the path.
            if library::is_video_path(&p) {
                if p.is_file() { s.pending_video_audio.push((src.id, p)); } else { missing += 1; }
                continue;
            }
            let Ok(raw) = std::fs::read(&p) else { missing += 1; continue };
            let hint = p.extension().and_then(|e| e.to_str());
            // Video takes the ffmpeg door, as the import path already did —
            // symphonia cannot open an mkv, so decoding it here reported every
            // source of a reopened video project "missing" (77 of 77 on the
            // Fragile Thing card) and played it silent. But not HERE: the
            // first version of this fix extracted inline and the window did
            // not appear for over a minute. The path is queued; the shell
            // extracts on a worker and the audio arrives like a peer's does.
            match studio_media::decode_bytes(raw.clone(), hint) {
                Ok(audio) => {
                    s.incoming_audio.push((src.id, audio));
                    s.register_asset(src.id, &raw);
                }
                Err(_) => missing += 1,
            }
        }
        s.outbox.clear(); // registration is local state, not news for peers yet
        s.loop_dirty = true;
        s.arrangement_dirty = true;
        s.status = if missing == 0 {
            format!("opened {}", path.display())
        } else {
            format!("opened {} — {missing} source file(s) missing or undecodable",
                    path.display())
        };
        Ok(s)
    }

    /// The outgoing half of presence. [`Studio::integrate_packet`] has merged
    /// peers' whereabouts since the wire existed, but nothing ever SENT ours —
    /// two connected shells each drew an empty room, and the timeline's
    /// peer-playhead rendering was dead code between real instances. The shell
    /// calls this once per frame with the wall clock; it returns a packet when
    /// where-we-are changed (rate-limited so a scrub is ~10 packets a second,
    /// not sixty) or when the heartbeat is due, and `None` the rest of the
    /// time. The heartbeat matters independently of change: presence is the
    /// only sign of life the wire carries (there is no join packet), so a
    /// reader parked on one frame must still beat inside the sweep's
    /// `LIVENESS_MS` window or be swept as a ghost by every peer.
    pub fn presence_beat(&mut self, now_ms: i64, following: Option<studio_collab::SiteId>, view_mode: studio_collab::ViewMode) -> Option<studio_collab::wire::Packet> {
        const MIN_GAP_MS: i64 = 100;
        // Three beats per liveness window — one lost packet must not read
        // as a departure.
        const HEARTBEAT_MS: i64 = studio_collab::session::LIVENESS_MS / 3;
        // Compare BEFORE building: this runs every frame, and the idle
        // path — almost every frame — must not clone the selection and
        // format a name just to throw them away.
        let playhead = self.playhead();
        // The track being worked in, derived from the selection rather
        // than tracked separately: the first selected clip's home is
        // where a collision would happen.
        let active_track = self.selection.first().and_then(|id|
            self.session.doc.project.clips.get(id).map(|c| c.track));
        let changed = match &self.last_presence {
            None => true,
            Some(last) => last.playhead != playhead
                || last.selection != self.selection
                || last.active_track != active_track
                || last.recording != self.recording
                || last.following != following
                || last.view_mode != view_mode
                || (self.display_name.is_empty() != last.name.starts_with("site "))
                || (!self.display_name.is_empty() && last.name != self.display_name),
        };
        let due = now_ms - self.last_presence_ms >= HEARTBEAT_MS;
        if !(due || (changed && now_ms - self.last_presence_ms >= MIN_GAP_MS)) {
            return None;
        }
        let name = if self.display_name.is_empty() {
            format!("site {}", self.session.doc.site.0)
        } else { self.display_name.clone() };
        let cur = studio_collab::Presence {
            site: self.session.doc.site,
            name,
            playhead,
            selection: self.selection.clone(),
            active_track,
            seq: self.presence_seq + 1, recording: self.recording,
            following, view_mode,
        };
        self.presence_seq += 1;
        self.last_presence = Some(cur.clone());
        self.last_presence_ms = now_ms;
        Some(studio_collab::wire::Packet::Presence(cur))
    }

    /// Everything local since the last drain, for any carrier to ship.
    pub fn drain_outbox(&mut self) -> Vec<studio_collab::wire::Packet> {
        std::mem::take(&mut self.outbox)
    }

    /// A played note enters the pattern as a tuned step on the selected
    /// channel at the loop's current column — typing a melody into the 808
    /// lane, which is what the step grid's `tune` mod was minted for.
    ///
    /// Recording into the pattern rather than sounding a live voice is a
    /// deliberate scope line: a live voice needs a polyphonic one-shot mixer
    /// over the audition sink (named in `library`'s docs too), while step
    /// entry is playable TODAY and its result is exactly what the grid, the
    /// bounce, and a peer all already understand.
    /// Render one played note through the SELECTED channel's voice — the
    /// instrument under any keyboard is whatever channel the grid is
    /// editing. Goes through `apply_step_mod`, the same rule a sequenced
    /// step uses, so a played note and a programmed one at the same tune and
    /// velocity are the same sound.
    pub fn render_play_note(
        &self,
        note: &studio_egui::playkeys::PlayNote,
        rate: u32,
    ) -> Vec<f32> {
        use studio_live::Synth as _;
        let Some(ch) = self.rack.channels.get(self.selected_channel) else { return Vec::new() };
        let kind = studio_live::apply_step_mod(
            &ch.kind,
            &studio_live::StepMod { tune: note.semitone, vel: note.velocity,
                                    ..studio_live::StepMod::default() },
            None,
        );
        let sources = std::collections::BTreeMap::new();
        let mut synth = studio_live::BuiltinSynth { sources: &sources };
        // Two seconds is past every built-in voice's tail and short enough
        // that a flurry of notes cannot grow memory; voices self-retire.
        let mut out = vec![0.0f32; rate as usize * 2];
        synth.render(&kind, rate, &mut out);
        let g = ch.gain * note.velocity;
        for s in &mut out { *s *= g; }
        out
    }

    /// Which step a note played RIGHT NOW belongs to, wrapped into the
    /// current pattern. Quantised to the grid by construction: an unquantised
    /// step does not exist in a step sequencer, so this is not a setting.
    pub fn recording_step(&self) -> u32 {
        let steps = self.rack.patterns.get(self.current_pattern)
            .map(|p| p.steps).unwrap_or(16).max(1);
        // NEAREST step, not `Tempo::step_at`'s floor. Two reasons, one
        // musical and one mechanical: a note struck a hair BEFORE the beat
        // belongs on that beat, not the one before it; and Transport stores
        // position in frames, so a position set to an exact step boundary
        // reads back a fraction below it — flooring there records the whole
        // note one step early, which a test caught doing exactly that.
        let step = self.rack.tempo().step_len().0.max(1);
        let t = self.transport.position().0;
        let s = (t + step / 2).div_euclid(step);
        s.rem_euclid(steps as i64) as u32
    }

    pub fn play_note_at(&mut self, note: studio_egui::playkeys::PlayNote, step: u32) {
        let ch = self.selected_channel;
        let Some(p) = self.rack.patterns.get_mut(self.current_pattern) else { return };
        if ch >= p.rows.len() || step >= p.steps { return; }
        p.set(ch, step, true);
        let mut m = p.mod_at(ch, step);
        m.tune = note.semitone;
        // Analog travel becomes step velocity — the point of an analog
        // keyboard is that a soft press stays soft in the pattern.
        m.vel = note.velocity;
        p.set_mod(ch, step, m);
        self.loop_dirty = true;
        self.status = format!("note {:+} at step {}", note.semitone, step + 1);
    }

    /// Publish the rack to the Doc's LWW register when it has drifted — the
    /// one choke point between "the rack the panels mutate" and "the rack
    /// peers see". Called every shell frame; a compare of a small value is
    /// cheaper than being wrong once.
    pub fn sync_rack_if_changed(&mut self) {
        let differs = self.session.doc.rack.as_ref()
            .map(|r| r.rack != self.rack)
            .unwrap_or(true);
        if differs {
            let sync = self.session.doc.set_rack(self.rack.clone());
            self.outbox.push(studio_collab::wire::Packet::Rack(sync));
        }
    }

    /// Make a source's media servable: chunk it into the content-addressed
    /// store and announce the manifest, so peers who just saw the
    /// `AddSource` op can fetch exactly the chunks they lack.
    /// The source these exact bytes already became, if any.
    ///
    /// Answers "have I imported this file before" by CONTENT, so a sample
    /// renamed or copied to a second folder is recognised as the take it is.
    /// A path check would miss both, which is the case this exists for -- the
    /// same loop lives in `bounces/` and in the project folder more often than
    /// not.
    ///
    /// Only reports a source the project still has: an asset whose source was
    /// deleted must re-import rather than hand back an id nothing resolves.
    pub fn source_for_bytes(&self, bytes: &[u8]) -> Option<studio_core::SourceId> {
        let key = crate::media_map::AssetKey(studio_collab::assets::hash(bytes));
        let id = *self.asset_sources.get(&key)?;
        self.session.doc.project.sources.contains_key(&id).then_some(id)
    }

    pub fn register_asset(&mut self, source: studio_core::SourceId, bytes: &[u8]) {
        let (manifest, chunks) = studio_collab::assets::Manifest::build(bytes);
        // RECORD WHAT THIS FILE IS, here, because this is the one place the
        // content hash and the source id are both in hand. The path comes off
        // the Source rather than being threaded in as a parameter: it is
        // already stored there, and widening this signature would touch six
        // call sites to pass a value five of them would have to look up
        // anyway.
        let key = crate::media_map::AssetKey(manifest.asset);
        if let Some(src) = self.session.doc.project.sources.get(&source) {
            self.media_map.record(key, src.path.clone(), bytes.len() as u64);
        }
        self.asset_sources.insert(key, source);
        self.chunk_store.extend(chunks);
        self.outbox.push(studio_collab::wire::Packet::AssetManifest {
            source, manifest: manifest.clone(),
        });
        self.manifests.insert(source, manifest);
    }

    /// Ask the room for a source's media if we neither hold nor await it.
    fn maybe_request_asset(&mut self, source: studio_core::SourceId) {
        if !self.manifests.contains_key(&source) && !self.pending_assets.contains_key(&source) {
            self.outbox.push(studio_collab::wire::Packet::AssetRequest { source });
        }
    }

    /// A manifest's chunks are all here: assemble (verified), decode, queue.
    /// The manifest is promoted to servable EVEN IF decode fails — the bytes
    /// proved their integrity, so re-serving them is right; what they mean
    /// is a media problem, said out loud in the status.
    fn finish_asset(&mut self, source: studio_core::SourceId,
                    manifest: studio_collab::assets::Manifest) {
        let Ok(bytes) = manifest.assemble(&self.chunk_store) else { return };
        self.pending_assets.remove(&source);
        let hint = self.session.doc.project.sources.get(&source)
            .and_then(|s| s.path.rsplit('.').next().map(str::to_owned));
        match studio_media::decode_bytes(bytes, hint.as_deref()) {
            Ok(audio) => {
                self.incoming_audio.push((source, audio));
                self.status = "media arrived from a peer".into();
            }
            Err(e) => self.status = format!("peer media for source {} undecodable: {e}", source.0),
        }
        self.manifests.insert(source, manifest);
    }

    /// Feed one packet from a peer. Returns any reply packets (a `Hello`
    /// earns a catch-up; assets earn their fetch steps; the rest is silent).
    pub fn integrate_packet(&mut self, p: studio_collab::wire::Packet, now_ms: i64)
        -> Vec<studio_collab::wire::Packet>
    {
        use studio_collab::wire::Packet;
        match p {
            Packet::Op(s) => {
                // Idempotent and order-independent in the Doc; our only job
                // is refreshing what the remote edit made stale. Remote ops
                // never touch the LOCAL undo stack — you undo your own
                // edits, not your collaborator's.
                if self.session.doc.integrate(&s).is_ok() {
                    self.arrangement_dirty = true;
                }
                if let studio_core::Op::AddSource { source } = &s.op {
                    // The op travels light; the media follows on request.
                    self.maybe_request_asset(source.id);
                }
                Vec::new()
            }
            Packet::Catchup { ops } => {
                for s in &ops {
                    let _ = self.session.doc.integrate(s);
                    if let studio_core::Op::AddSource { source } = &s.op {
                        self.maybe_request_asset(source.id);
                    }
                }
                self.arrangement_dirty = true;
                Vec::new()
            }
            Packet::AssetRequest { source } => {
                match self.manifests.get(&source) {
                    Some(m) => vec![Packet::AssetManifest { source, manifest: m.clone() }],
                    None => Vec::new(), // not ours to serve — someone else may answer
                }
            }
            Packet::AssetManifest { source, manifest } => {
                if self.manifests.contains_key(&source) {
                    return Vec::new(); // already held and servable
                }
                let have: std::collections::BTreeSet<_> =
                    self.chunk_store.keys().copied().collect();
                let need = manifest.need(&have);
                if need.is_empty() {
                    // Content-addressing paying off: every chunk was already
                    // here (a re-bounce, or a shared sample) — no transfer.
                    self.finish_asset(source, manifest);
                    Vec::new()
                } else {
                    self.pending_assets.insert(source, manifest);
                    vec![Packet::ChunkRequest { ids: need }]
                }
            }
            Packet::ChunkRequest { ids } => {
                ids.iter()
                    .filter_map(|id| self.chunk_store.get(id)
                        .map(|b| Packet::Chunk { id: *id, bytes: b.clone() }))
                    .collect()
            }
            Packet::Chunk { id, bytes } => {
                // The id IS the integrity check; a mismatch is dropped on
                // the floor, and the pending manifest keeps waiting for the
                // real thing rather than assembling a lie.
                if studio_collab::assets::hash(&bytes) == id {
                    self.chunk_store.insert(id, bytes);
                    let done: Vec<_> = self.pending_assets.iter()
                        .filter(|(_, m)| {
                            let have: std::collections::BTreeSet<_> =
                                self.chunk_store.keys().copied().collect();
                            m.need(&have).is_empty()
                        })
                        .map(|(s, m)| (*s, m.clone()))
                        .collect();
                    for (s, m) in done {
                        self.finish_asset(s, m);
                    }
                }
                Vec::new()
            }
            Packet::Rack(sync) => {
                self.session.doc.integrate_rack(sync);
                // Adopt whichever value the LWW register now holds — ours,
                // if ours was newer; theirs, if not. Reading the winner back
                // (instead of keeping a local copy we believe in) is what
                // makes both peers land on the SAME rack.
                if let Some(rs) = &self.session.doc.rack {
                    if rs.rack != self.rack {
                        self.rack = rs.rack.clone();
                        self.current_pattern = self.current_pattern
                            .min(self.rack.patterns.len().saturating_sub(1));
                        self.selected_channel = self.selected_channel
                            .min(self.rack.channels.len().saturating_sub(1));
                        self.loop_dirty = true;
                        self.status = "rack updated by a peer".into();
                    }
                }
                Vec::new()
            }
            Packet::Presence(pr) => {
                // Through the SESSION, not the doc: `Session::presence` is
                // the doc merge PLUS the sign-of-life accounting the sweep
                // reads. Routing around it (as this arm originally did) is
                // exactly how "a peer who is actively scrubbing gets swept
                // for silence" — its own doc-comment's warning.
                self.session.presence(pr, now_ms);
                Vec::new()
            }
            Packet::Hello { since, .. } => {
                let ops: Vec<studio_collab::StampedOp> = self.session.doc.history()
                    .iter().filter(|s| s.stamp.lamport > since).cloned().collect();
                let mut replies = vec![Packet::Catchup { ops }];
                if let Some(rack) = self.session.rack_broadcast() {
                    replies.push(rack);
                }
                replies
            }
            // Voice is for the jitter buffer and the sink, not the Doc:
            // it goes to the call through `voice_inbox`, never into the
            // history, and earns no reply.
            Packet::Voice { site, seq, frame } => {
                self.voice_inbox.push(Packet::Voice { site, seq, frame });
                Vec::new()
            }
        }
    }

    /// Route a key through the modal map. Returns true if it was consumed.
    /// Align the SECOND selected clip to the FIRST — the vocal-double
    /// case: select the lead, shift-click the take, press A. Envelope
    /// cross-correlation (rate-agnostic, phase-blind) within ±500 ms; the
    /// move is an ordinary `MoveClip` — undoable, on the wire, refusable.
    pub fn align_selected(
        &mut self,
        audio: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) {
        let [ref_id, take_id] = self.selection[..] else {
            self.status = "align: select the reference first, then the take (2 clips)".into();
            return;
        };
        let clip = |id| self.session.doc.project.clips.get(&id).cloned();
        let (Some(r), Some(t)) = (clip(ref_id), clip(take_id)) else {
            self.status = "align refused: a selected clip vanished".into();
            return;
        };
        let window = |c: &studio_core::Clip| -> Option<Vec<f32>> {
            let a = audio.get(&c.source)?;
            let from = c.src_in.to_seconds().max(0.0) * a.sample_rate as f64;
            let len = c.len.to_seconds().max(0.0) * a.sample_rate as f64;
            let (from, to) = (from as usize, (from + len) as usize);
            let ch = a.channels.first()?;
            Some(ch.get(from..to.min(ch.len()))?.to_vec())
        };
        let (Some(rw), Some(tw)) = (window(&r), window(&t)) else {
            self.status = "align refused: a clip's media has not arrived yet".into();
            return;
        };
        let (ra, ta) = (audio[&r.source].sample_rate, audio[&t.source].sample_rate);
        let ms = studio_audio::align_offset_ms(
            &studio_audio::envelope_ms(&rw, ra),
            &studio_audio::envelope_ms(&tw, ta), 500);
        // The lag is measured WITHIN each clip's window: take[i+d]≈ref[i]
        // means the take's event sits d ms later than the reference's,
        // counted from their own starts. Aligning the material on the
        // timeline therefore lands the take's clip at r.pos − d — the
        // derivation matters, because the first draft wrote r.pos + d and
        // would have doubled every error instead of fixing it.
        let want = studio_core::Ticks(
            (r.pos.0 - studio_core::Ticks::from_seconds(ms as f64 / 1000.0).0)
                .max(0));
        if (want.0 - t.pos.0).abs() < studio_core::Ticks::from_seconds(0.0005).0 {
            self.status = "already aligned (within half a millisecond)".into();
            return;
        }
        self.edit(studio_core::Op::MoveClip { id: take_id, pos: want, track: t.track },
                  &format!("aligned: the take was {ms:+} ms off"));
    }

    /// Snap every selected clip's start to the nearest rack step. One
    /// `MoveClip` per clip, each its own undo — the multi-op precedent.
    pub fn quantize_selected(&mut self) {
        if self.selection.is_empty() {
            self.status = "quantize: nothing selected".into();
            return;
        }
        let step = self.rack.tempo().step_len().0.max(1);
        for id in self.selection.clone() {
            let Some(c) = self.session.doc.project.clips.get(&id).cloned() else { continue };
            let snapped = studio_core::Ticks(((c.pos.0 + step / 2) / step * step).max(0));
            if snapped != c.pos {
                self.edit(studio_core::Op::MoveClip { id, pos: snapped, track: c.track },
                          "quantized to the grid");
            }
        }
    }

    /// Nudge every selected clip by `delta` ticks (clamped at zero) — the
    /// keyboard's fine alignment: a step with , and . , a millisecond
    /// with < and > .
    pub fn nudge_selected(&mut self, delta: studio_core::Ticks) {
        if self.selection.is_empty() {
            self.status = "nudge: nothing selected".into();
            return;
        }
        // One delta, one undo, in collision-free order: nudging a chopped
        // take right one clip at a time used to refuse at the first clip,
        // which would land on its own unmoved neighbour.
        let ids = self.selection.clone();
        let Some(anchor) = ids.iter().find(|id| self.session.doc.project.clips.contains_key(id)).copied() else {
            self.status = "nudge: the selection no longer exists".into();
            return;
        };
        let want = studio_core::Ticks(self.session.doc.project.clips[&anchor].pos.0 + delta.0);
        match self.session.doc.project.move_together_ops(&ids, anchor, want) {
            Ok(ops) if ops.is_empty() => self.status = "nudge: already at the start".into(),
            Ok(ops) => { let n = ops.len(); let ops = self.with_room(ops); self.edit_group(ops, &format!("nudged {n}")); }
            Err(e) => self.status = format!("nudge refused: {e:?}"),
        }
    }

    /// The clips that move with `id`: every composition it belongs to,
    /// plus the selection if it is selected — or just itself when
    /// `alone`. A group is moved by grabbing any member; that is what a
    /// fold is for.
    pub fn moving_set(&self, id: studio_core::ClipId, alone: bool) -> Vec<studio_core::ClipId> {
        let mut set = vec![id];
        if alone { return set; }
        let p = &self.session.doc.project;
        for g in p.groups.values() {
            if g.clips.contains(&id) {
                set.extend(g.clips.iter().copied().filter(|c| p.clips.contains_key(c)));
            }
        }
        if self.selection.contains(&id) { set.extend(self.selection.iter().copied()); }
        set.sort(); set.dedup();
        set
    }

    /// The overlap policy for a track: its override, else the default.
    pub fn policy_for(&self, track: studio_core::TrackId) -> studio_core::Overlap {
        self.overlap_tracks.get(&track).copied().unwrap_or(self.overlap)
    }

    /// Resolve the placements in `ops` (MoveClip / AddClip) against each
    /// track's overlap policy, on a probe copy so a second placement sees
    /// what the first one trimmed. Trim: make_room_ops before the op.
    /// Lane: the op is retargeted to the parent's shadow lane (created if
    /// absent), and room is then made on the lane. Refuse: the op goes as
    /// is and the model says no. Everything else passes through.
    pub fn with_room(&self, ops: Vec<studio_core::Op>) -> Vec<studio_core::Op> {
        use studio_core::{Op, Overlap};
        let moving: Vec<studio_core::ClipId> = ops.iter().filter_map(|o| match o {
            Op::MoveClip { id, .. } => Some(*id), Op::AddClip { clip } => Some(clip.id), _ => None }).collect();
        let mut probe = self.session.doc.project.clone();
        let mut next = probe.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1
            + ops.iter().filter(|o| matches!(o, Op::AddClip { .. })).count() as u64;
        let mut next_track = probe.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1;
        let mut out = Vec::with_capacity(ops.len());
        fn push(probe: &mut studio_core::Project, out: &mut Vec<studio_core::Op>, op: studio_core::Op) {
            let _ = probe.apply(&op);
            out.push(op);
        }
        for op in ops {
            let prospective = match &op {
                Op::MoveClip { id, pos, track } => probe.clips.get(id).map(|c| {
                    let mut c = c.clone(); c.pos = *pos; c.track = *track; c }),
                Op::AddClip { clip } => Some(clip.clone()),
                _ => None,
            };
            let Some(mut clip) = prospective else { push(&mut probe, &mut out, op); continue };
            let mut op = op;
            match self.policy_for(clip.track) {
                Overlap::Refuse => {}
                Overlap::Lane if !probe.overlappers(&clip, &moving).is_empty() => {
                    let lane = match probe.shadow_lane(clip.track) {
                        Some(l) => l,
                        None => {
                            let l = studio_core::TrackId(next_track);
                            next_track += 1;
                            if let Ok(mk) = probe.shadow_lane_ops(clip.track, l) {
                                for m in mk { push(&mut probe, &mut out, m); }
                            }
                            l
                        }
                    };
                    clip.track = lane;
                    op = match op {
                        Op::MoveClip { id, pos, .. } => Op::MoveClip { id, pos, track: lane },
                        Op::AddClip { mut clip } => { clip.track = lane; Op::AddClip { clip } }
                        other => other,
                    };
                    for r in probe.make_room_ops(&clip, &moving, &mut next) { push(&mut probe, &mut out, r); }
                }
                Overlap::Lane => {}
                Overlap::Trim => {
                    for r in probe.make_room_ops(&clip, &moving, &mut next) { push(&mut probe, &mut out, r); }
                }
            }
            push(&mut probe, &mut out, op);
        }
        out
    }

    /// Set the overlap policy for one track (or the default with `None`).
    pub fn set_overlap(&mut self, track: Option<studio_core::TrackId>, policy: studio_core::Overlap) {
        match track {
            Some(t) => { self.overlap_tracks.insert(t, policy); }
            None => self.overlap = policy,
        }
        self.status = format!("overlap on {}: {}", match track {
            Some(t) => self.session.doc.project.tracks.iter().find(|x| x.id == t)
                .map(|x| x.name.clone()).unwrap_or_else(|| "?".into()),
            None => "every track".into() }, policy.name());
    }

    /// A timeline drop: the whole moving set by the anchor's delta, each
    /// on its own track; a lone clip also honours the track it was
    /// dropped on. One group, one undo, refused whole.
    pub fn drop_clip(&mut self, id: studio_core::ClipId, pos: Ticks, track: studio_core::TrackId, alone: bool) {
        let set = self.moving_set(id, alone);
        let ops = if set.len() == 1 {
            vec![studio_core::Op::MoveClip { id, pos, track }]
        } else {
            match self.session.doc.project.move_together_ops(&set, id, pos) {
                Ok(ops) => ops,
                Err(e) => { self.status = format!("move refused: {e:?}"); return; }
            }
        };
        if ops.is_empty() { self.status = "moved: nowhere to go".into(); return; }
        let moved = ops.len();
        let ops = self.with_room(ops);
        let gave_way = ops.len() - moved;
        let what = if moved == 1 { "moved".to_string() } else { format!("moved {moved} together") };
        if ops.len() == 1 {
            self.edit(ops.into_iter().next().unwrap(), &what);
        } else {
            self.edit_group(ops, &format!("{what}{}", if gave_way > 0 {
                format!(", {gave_way} op{} made room", if gave_way == 1 { "" } else { "s" }) } else { String::new() }));
        }
    }

    /// Cut every selected clip on the rack grid: `steps` per cut (4 is a
    /// beat, 16 a bar in the rack's own 16th-note grid). Pieces stay in
    /// their compositions and a clip-gated curve keeps playing what it
    /// played — `split_on_grid_ops`'s promises. Cutting by the grid rather
    /// than by ear is also the answer to editing over a Bluetooth link,
    /// where the ear is a few hundred milliseconds late.
    /// Set every delay's `time` to a note length at the working tempo:
    /// `sixteenths` sixteenth-notes (3 = the dotted eighth, 2 = an eighth,
    /// 1 = a sixteenth, 4 = a quarter). Every bus chain, and the
    /// effect chains of the selected clips. `instantiate` has a rate and
    /// no tempo, so this is where sync lives — as ops, so it undoes and
    /// travels. The tempo is the active song's map at the playhead when
    /// it has one (the ruler's own lookup), else the rack's — the clock
    /// the grid splits use.
    pub fn sync_delays_to_tempo(&mut self, sixteenths: u8) -> usize {
        let p = &self.session.doc.project;
        let bpm = self.active_song
            .and_then(|id| p.groups.get(&id))
            .and_then(|g| g.tempo.as_ref())
            .map(|m| m.bpm_at(self.playhead()))
            .unwrap_or(self.rack.tempo_bpm);
        let ms = studio_fx::space::beats_to_ms(bpm, sixteenths as f32 / 4.0);
        let retime = |chain: &[studio_core::Effect]| -> Option<Vec<studio_core::Effect>> {
            if !chain.iter().any(|e| e.kind == "delay") { return None; }
            Some(chain.iter().map(|e| {
                let mut e = e.clone();
                if e.kind == "delay" { e.params.insert("time".into(), ms); }
                e
            }).collect())
        };
        let mut ops = Vec::new();
        for b in p.busses.values() {
            if let Some(chain) = retime(&b.chain) {
                ops.push(studio_core::Op::SetBusChain { id: b.id, chain });
            }
        }
        for id in &self.selection {
            if let Some(c) = p.clips.get(id) {
                if let Some(effects) = retime(&c.effects) {
                    ops.push(studio_core::Op::SetEffects { id: *id, effects });
                }
            }
        }
        let n = ops.len();
        for op in ops { self.edit(op, "sync delay"); }
        self.status = if n == 0 {
            "sync delay: no delay on any bus or selected clip".into()
        } else {
            format!("sync delay: {n} chain(s) set to {ms:.1} ms ({} at {bpm:.0} BPM)",
                    match sixteenths { 1 => "a sixteenth", 2 => "an eighth", 3 => "a dotted eighth",
                                       4 => "a quarter", _ => "the note" })
        };
        n
    }

    /// Chord tools on the roll's channel (the same channel the rack strip
    /// selects). `degrees` scale degrees in the project's key when it has
    /// one — a third is 2, a fifth 4, an octave 7 — else the major
    /// interval in semitones. Direct rack edits, like the roll's own.
    pub fn harmonize_roll(&mut self, degrees: i32) -> usize {
        let key = self.session.doc.project.key;
        let ch = self.selected_channel;
        let Some(p) = self.rack.patterns.get_mut(self.current_pattern) else {
            self.status = "no pattern".into();
            return 0;
        };
        let n = p.harmonize(ch, degrees, key);
        self.loop_dirty |= n > 0;
        let what = match degrees { 2 => "thirds", 4 => "fifths", 7 => "octaves", -7 => "octaves below", _ => "notes" };
        self.status = if n == 0 { format!("harmonize: nothing to add ({what} already there, or an empty roll)") }
                      else { format!("harmonize: {n} {what} added{}", if key.is_some() { " in key" } else { "" }) };
        n
    }

    /// Every chord on the roll's channel becomes a run of its notes.
    pub fn arpeggiate_roll(&mut self, order: studio_live::Arp, note_len: u32) -> usize {
        let ch = self.selected_channel;
        let Some(p) = self.rack.patterns.get_mut(self.current_pattern) else {
            self.status = "no pattern".into();
            return 0;
        };
        let n = p.arpeggiate(ch, order, note_len);
        self.loop_dirty |= n > 0;
        self.status = if n == 0 { "arpeggiate: no chords on this channel (two or more notes on one step)".into() }
                      else { format!("arpeggiate: {n} chord{} broken into runs", if n == 1 { "" } else { "s" }) };
        n
    }

    /// The track a take is aimed at: the root of the selected clip's
    /// chain when one clip is selected (re-taking the phrase you are
    /// looking at), else the first audio track.
    pub fn take_target(&self) -> Option<studio_core::TrackId> {
        let p = &self.session.doc.project;
        if let Some(c) = self.selection.first().and_then(|id| p.clips.get(id)) {
            return Some(p.take_root(c.track));
        }
        p.tracks.iter().find(|t| t.kind == studio_core::TrackKind::Audio).map(|t| t.id)
    }

    /// Land a take (an already-imported source) at `at` on `track`'s
    /// chain: the first lane with room, or a new one — one undo step for
    /// the lane and the clip together. Returns where it landed.
    pub fn place_take(&mut self, source: studio_core::SourceId, len: Ticks, at: Ticks,
                      track: studio_core::TrackId) -> Option<(studio_core::TrackId, studio_core::ClipId)> {
        let p = &self.session.doc.project;
        let end = at.saturating_add(len);
        let new_track = studio_core::TrackId(p.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
        let (lane, mut ops) = match p.take_lane_ops(track, at, end, new_track) {
            Ok(x) => x,
            Err(e) => { self.status = format!("take refused: {e:?}"); return None; }
        };
        let cid = studio_core::ClipId(p.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let made_lane = !ops.is_empty();
        ops.push(studio_core::Op::AddClip { clip: studio_core::Clip {
            id: cid, source, track: lane, pos: at, src_in: Ticks::ZERO, len, gain: 1.0, effects: vec![] } });
        self.edit_group(ops, "take");
        if !self.session.doc.project.clips.contains_key(&cid) { return None; }
        let n = self.session.doc.project.take_lanes(track).len();
        self.status = format!("take {:.1}s on {}{}", len.to_seconds(),
            self.session.doc.project.tracks.iter().find(|t| t.id == lane).map(|t| t.name.as_str()).unwrap_or("?"),
            if made_lane { format!(" — lane {n} of the chain") } else { String::new() });
        Some((lane, cid))
    }

    /// The comp verb: the selected clips, each on some lane of a chain,
    /// copied onto their chain's root over their own span. A clip already
    /// on a root is left alone. One undo step.
    pub fn comp_selected_takes(&mut self) -> usize {
        let p = &self.session.doc.project;
        let mut by_root: std::collections::BTreeMap<studio_core::TrackId, Vec<(Ticks, Ticks, studio_core::TrackId)>> =
            Default::default();
        for id in &self.selection {
            let Some(c) = p.clips.get(id) else { continue };
            let root = p.take_root(c.track);
            if root == c.track { continue; }
            by_root.entry(root).or_default().push((c.pos, c.end(), c.track));
        }
        if by_root.is_empty() {
            self.status = "comp: select a take on a lane (not the track itself)".into();
            return 0;
        }
        let mut next = p.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
        let mut ops = Vec::new();
        let mut n = 0;
        for (root, choices) in &by_root {
            match p.comp_ops(*root, choices, &mut next) {
                Ok(o) => { n += choices.len(); ops.extend(o); }
                Err(e) => { self.status = format!("comp refused: {e:?}"); return 0; }
            }
        }
        self.edit_group(ops, &format!("comp {n} take{}", if n == 1 { "" } else { "s" }));
        self.status = format!("comp: {n} take{} on the track — the lanes keep the rest",
                              if n == 1 { "" } else { "s" });
        n
    }

    /// Mute (or unmute) every lane under the take target's chain, so the
    /// comp on the root is what plays.
    pub fn set_take_lanes_muted(&mut self, muted: bool) -> usize {
        let Some(root) = self.take_target() else {
            self.status = "no audio track".into();
            return 0;
        };
        let ops = self.session.doc.project.take_lanes_mute_ops(root, muted);
        let n = ops.len();
        if n == 0 {
            self.status = if muted { "no unmuted take lanes under this track" }
                          else { "no muted take lanes under this track" }.into();
            return 0;
        }
        self.edit_group(ops, if muted { "mute take lanes" } else { "unmute take lanes" });
        self.status = format!("{} {n} take lane{}", if muted { "muted" } else { "unmuted" },
                              if n == 1 { "" } else { "s" });
        n
    }

    pub fn split_selection_on_grid(&mut self, steps: i64) {
        if self.selection.is_empty() {
            self.status = "split on grid: nothing selected".into();
            return;
        }
        let grid = Ticks(self.rack.tempo().step_len().0.max(1) * steps.max(1));
        let p = &self.session.doc.project;
        let mut next = p.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
        let ids: Vec<_> = self.selection.iter().copied().filter(|c| p.clips.contains_key(c)).collect();
        match p.split_on_grid_ops(&ids, grid, Ticks::ZERO, &mut next) {
            Ok(ops) if ops.is_empty() => self.status = "split on grid: no grid line crosses the selection".into(),
            Ok(ops) => {
                let cuts = ops.iter().filter(|o| matches!(o, studio_core::Op::AddClip { .. })).count();
                let regated = ops.iter().filter(|o| matches!(o, studio_core::Op::SetBinding { .. })).count();
                let new_ids: Vec<_> = ops.iter().filter_map(|o| match o {
                    studio_core::Op::AddClip { clip } => Some(clip.id), _ => None }).collect();
                let what = format!("cut {cuts} time{}{}", if cuts == 1 { "" } else { "s" },
                    if regated > 0 { format!(" ({regated} clip-gated curve{} now gated to the original span)",
                                             if regated == 1 { "" } else { "s" }) } else { String::new() });
                self.edit_group(ops, &what);
                if new_ids.iter().all(|c| self.session.doc.project.clips.contains_key(c)) {
                    self.selection.extend(new_ids);
                }
            }
            Err(e) => self.status = format!("split on grid refused: {e:?}"),
        }
    }

    /// Give this binding its own copy of its curve. Says "already unique"
    /// rather than cloning a curve nothing else shares.
    pub fn make_unique(&mut self, id: studio_core::BindingId) {
        let p = &self.session.doc.project;
        let Some(b) = p.bindings.get(&id) else { self.status = "make unique: no such binding".into(); return };
        if p.curve_share_count(b.curve) <= 1 {
            self.status = "already unique — nothing else shares this curve".into();
            return;
        }
        let new_curve = studio_core::CurveId(p.curves.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        match p.make_curve_unique_ops(id, new_curve) {
            Ok(ops) => self.edit_group(ops, "curve made unique"),
            Err(e) => self.status = format!("make unique refused: {e:?}"),
        }
    }

    /// Add a channel that LAYERS channel `from`: same steps, same mods,
    /// in every pattern, so the authored sound plays in lockstep with the
    /// sound it was tuned from — the breakout loop's return path. The new
    /// channel's row is a copy, not a link: editing the layer afterwards
    /// diverges freely, which is what "develop the layers" means.
    pub fn layer_channel(&mut self, from: usize, channel: studio_live::Channel) -> usize {
        let ix = self.rack.add_channel(channel);
        for p in &mut self.rack.patterns {
            if let Some(row) = p.rows.get(from).copied() {
                p.rows[ix] = row;
            }
            let inherited: Vec<_> = p.mods.iter()
                .filter(|((ch, _), _)| *ch == from)
                .map(|((_, step), m)| ((ix, *step), m.clone()))
                .collect();
            p.mods.extend(inherited);
        }
        self.loop_dirty = true;
        ix
    }

    pub fn key(&mut self, k: studio_egui::keys::Key) -> bool {
        use studio_egui::keys::{Action, Resolution};
        // In Playing mode the editing keymap is inert: `d` is a note, not a
        // delete. Escape is the way back and is deliberately the ONLY editor
        // key that still works — a mode you cannot leave with the key
        // everyone reaches for is a trap. Consumed either way, so the caller
        // does not double-handle a keystroke this mode already claimed.
        if self.input_mode == studio_egui::playkeys::InputMode::Playing {
            if k.ch == '\u{1b}' {
                self.input_mode = studio_egui::playkeys::InputMode::Motions;
                self.status = "motions".into();
            }
            return true;
        }
        match self.keys.feed(k) {
            Resolution::Act(a) => {
                match a {
                    Action::PlayPause => {
                        // On the rack, space is the loop pedal; on the
                        // timeline, the tape transport. One key, the thing
                        // you are looking at — the alternative is space
                        // starting audio you cannot see.
                        if self.panel == Panel::Rack {
                            self.rack_playing = !self.rack_playing;
                            // One sink, one owner: the loop pedal takes it
                            // from the tape, not both at once.
                            if self.rack_playing { self.transport.stop(); }
                            self.status = if self.rack_playing { "loop playing".into() }
                                          else { "loop stopped".into() };
                        } else if self.transport.playing {
                            self.transport.stop(); self.status = "stopped".into();
                        } else {
                            self.rack_playing = false;
                            self.transport.play(); self.status = "playing".into();
                        }
                    }
                    Action::LeaderMenu | Action::Command => {
                        self.palette_requested = true;
                    }
                    Action::Undo => {
                        // The inverse goes back through the SAME stamping
                        // door, so peers see the undo as the edit it is. If
                        // the state has moved under it (a peer's conflicting
                        // op), the refusal is said out loud, not swallowed.
                        self.status = match self.undo_stack.pop() {
                            None => "nothing to undo".into(),
                            Some(inv) => match self.edit_inner(inv) {
                                Ok(redo) => {
                                    self.redo_stack.push(redo);
                                    self.arrangement_dirty = true;
                                    "undo".into()
                                }
                                Err(e) => format!("undo refused (state moved): {e:?}"),
                            },
                        };
                    }
                    Action::Redo => {
                        self.status = match self.redo_stack.pop() {
                            None => "nothing to redo".into(),
                            Some(op) => match self.edit_inner(op) {
                                Ok(undo) => {
                                    self.undo_stack.push(undo);
                                    self.arrangement_dirty = true;
                                    "redo".into()
                                }
                                Err(e) => format!("redo refused (state moved): {e:?}"),
                            },
                        };
                    }
                    Action::Delete => {
                        let sel = std::mem::take(&mut self.selection);
                        for id in sel { self.edit(studio_core::Op::RemoveClip { id }, "deleted"); }
                    }
                    Action::Split => self.split_at_playhead(),
                    Action::Yank => self.yank(),
                    Action::Paste => { let at = self.transport.position(); self.paste_at(at); }
                    Action::FlipTrimSide => {
                        self.trim_side = match self.trim_side {
                            studio_egui::TrimSide::Left => studio_egui::TrimSide::Right,
                            studio_egui::TrimSide::Right => studio_egui::TrimSide::Left,
                        };
                        self.status = format!("trim grabs the {} edge on a tie",
                            match self.trim_side {
                                studio_egui::TrimSide::Left => "left",
                                studio_egui::TrimSide::Right => "right",
                            });
                    }
                    Action::SelectAll => {
                        self.selection = self.session.doc.project.clips.keys().copied().collect();
                        self.status = format!("{} clips selected", self.selection.len());
                    }
                    Action::CollapseSelection => { self.selection.clear(); self.status = "cleared".into(); }
                    Action::GotoStart => self.seek(Ticks::ZERO),
                    Action::GotoEnd => {
                        let d = self.session.doc.project.duration();
                        self.seek(d);
                    }
                    // Everything below was parsed by the keymap and then
                    // dropped here by a `_ => {}` until c20 -- the HUD taught
                    // `x select` and `w/b extend` and neither did anything.
                    // The match is exhaustive now, so the next Action added
                    // to the keymap does not compile until it does something.
                    Action::NextEdit { count, extend } => self.goto_edit(count, true, extend),
                    Action::PrevEdit { count, extend } => self.goto_edit(count, false, extend),
                    Action::ClipEnd { extend } => self.goto_clip_end(extend),
                    Action::GotoPlayhead => self.reveal_playhead(),
                    Action::SelectClip { extend } => self.select_clip_at_playhead(extend),
                    Action::AddSelectionBelow => self.add_selection_beside(1),
                    Action::AddSelectionAbove => self.add_selection_beside(-1),
                    Action::SetGain { delta } => self.nudge_gain(delta),
                    Action::ToggleMute => self.toggle_track_flag(true),
                    Action::ToggleLock => self.toggle_track_flag(false),
                    Action::ToggleRecord => self.take_requested = true,
                    Action::ToggleLoop => {
                        if self.transport.loop_range.is_some() { self.loop_off() } else { self.loop_selection() }
                    }
                    Action::EnterMode(m) => self.status = format!("{} mode", m.label()),
                    // Insert mode is for text fields, which take their own
                    // input; a literal char reaching here has nowhere to go.
                    Action::Insert(_) => {}
                }
                true
            }
            Resolution::Pending(p) => { self.status = format!("…{p}"); true }
            Resolution::None => false,
        }
    }

    /// The track motions and selection work on: the last selected clip's,
    /// else the topmost track with a clip under the playhead, else the first.
    pub fn focus_track(&self) -> Option<studio_core::TrackId> {
        let p = &self.session.doc.project;
        let at = self.transport.position();
        self.selection.iter().rev().find_map(|id| p.clips.get(id)).map(|c| c.track)
            .or_else(|| p.tracks.iter()
                .find(|t| p.track_clips(t.id).iter().any(|c| c.pos <= at && at < c.end())).map(|t| t.id))
            .or_else(|| p.tracks.first().map(|t| t.id))
    }

    /// The clip on `track` under `at`, if any.
    fn clip_at(&self, track: studio_core::TrackId, at: Ticks) -> Option<studio_core::ClipId> {
        self.session.doc.project.track_clips(track).iter()
            .find(|c| c.pos <= at && at < c.end()).map(|c| c.id)
    }

    /// What a clip action applies to: the selection, else the focus track's
    /// clip under the playhead.
    fn targets(&self) -> Vec<studio_core::ClipId> {
        if !self.selection.is_empty() { return self.selection.clone(); }
        self.focus_track().and_then(|t| self.clip_at(t, self.transport.position())).into_iter().collect()
    }

    fn select_add(&mut self, id: studio_core::ClipId) {
        if !self.selection.contains(&id) { self.selection.push(id); }
    }

    /// `w` / `b`: the playhead to the next / previous cut on the focus track;
    /// in Select mode the clips passed over join the selection.
    pub fn goto_edit(&mut self, count: u32, forward: bool, extend: bool) {
        let Some(track) = self.focus_track() else { self.status = "no tracks".into(); return };
        let from = self.transport.position();
        let p = &self.session.doc.project;
        let to = studio_egui::keys::next_edit(&studio_egui::keys::edit_points(p, track), from, count, forward);
        if to == from {
            self.status = format!("no cut {} on this track", if forward { "after" } else { "before" } );
            return;
        }
        if extend {
            for id in studio_egui::keys::clips_in_span(p, track, from, to) { self.select_add(id); }
        }
        self.seek(to);
        self.status = format!("cut at {:.2}s{}", to.to_seconds(),
            if extend { format!(" · {} selected", self.selection.len()) } else { String::new() });
    }

    /// `e`: to the end of the focus track's clip under the playhead.
    pub fn goto_clip_end(&mut self, extend: bool) {
        let at = self.transport.position();
        let Some(id) = self.focus_track().and_then(|t| self.clip_at(t, at)) else {
            self.status = "no clip under the playhead".into(); return };
        let end = self.session.doc.project.clips[&id].end();
        if extend { self.select_add(id); }
        self.seek(end);
        self.status = format!("clip end {:.2}s", end.to_seconds());
    }

    /// `g c`: scroll the timeline so the playhead is in view, a little in
    /// from the left edge.
    pub fn reveal_playhead(&mut self) {
        let at = self.transport.position();
        let margin = Ticks::from_seconds(80.0 / self.view.px_per_sec.max(1.0) as f64);
        self.view.scroll = at.saturating_sub(margin);
        self.status = format!("playhead {:.2}s", at.to_seconds());
    }

    /// `x`: select the focus track's clip under the playhead. Pressed again
    /// (or in Select mode) it extends to the next clip on that track, as
    /// Helix's `x` extends line by line.
    pub fn select_clip_at_playhead(&mut self, extend: bool) {
        let Some(track) = self.focus_track() else { self.status = "no tracks".into(); return };
        let at = self.transport.position();
        let under = self.clip_at(track, at);
        let p = &self.session.doc.project;
        match under {
            Some(id) if !self.selection.contains(&id) => {
                if !extend { self.selection.clear(); }
                self.selection.push(id);
            }
            _ => {
                // repeat: the next clip on this track after the selected ones
                let after = self.selection.iter().filter_map(|id| p.clips.get(id))
                    .filter(|c| c.track == track).map(|c| c.end()).max().unwrap_or(at);
                let next = p.track_clips(track).iter().filter(|c| c.pos >= after)
                    .min_by_key(|c| c.pos).map(|c| c.id);
                match next {
                    Some(id) => self.selection.push(id),
                    None => { self.status = if under.is_none() { "no clip under the playhead".into() }
                                            else { "no further clip on this track".into() }; return; }
                }
            }
        }
        self.status = format!("{} selected", self.selection.len());
    }

    /// `C` / `alt-C`: add the clip on the track below (`dir` 1) or above
    /// (-1) that overlaps the last selected clip most -- multi-track selection
    /// from the keyboard, as Helix's `C` adds a cursor on the next line.
    pub fn add_selection_beside(&mut self, dir: i32) {
        let p = &self.session.doc.project;
        let anchor = self.selection.last().and_then(|id| p.clips.get(id)).cloned()
            .or_else(|| self.focus_track().and_then(|t| self.clip_at(t, self.transport.position()))
                .and_then(|id| p.clips.get(&id).cloned()));
        let Some(a) = anchor else { self.status = "select a clip first (x)".into(); return };
        let Some(ti) = p.tracks.iter().position(|t| t.id == a.track) else { return };
        let Some(other) = ti.checked_add_signed(dir as isize).and_then(|i| p.tracks.get(i)) else {
            self.status = format!("no track {}", if dir > 0 { "below" } else { "above" }); return };
        let overlap = |c: &studio_core::Clip| c.end().min(a.end()).0 - c.pos.max(a.pos).0;
        let best = p.track_clips(other.id).into_iter().filter(|c| overlap(c) > 0)
            .max_by_key(|c| overlap(c)).map(|c| c.id);
        match best {
            Some(id) => {
                self.selection.retain(|x| *x != id);
                self.selection.push(id); // last = the new anchor, so C repeats downward
                self.status = format!("{} selected", self.selection.len());
            }
            None => self.status = format!("nothing on {} under the selection", other.name),
        }
    }

    /// `=` / `-`: clip gain up or down, over the inspector's 0..=1.5 range.
    /// One undo for the whole selection.
    pub fn nudge_gain(&mut self, delta: f32) {
        let ids = self.targets();
        if ids.is_empty() { self.status = "gain: nothing selected or under the playhead".into(); return; }
        let p = &self.session.doc.project;
        let ops: Vec<_> = ids.iter().filter_map(|id| p.clips.get(id))
            .map(|c| studio_core::Op::SetGain { id: c.id, gain: (c.gain + delta).clamp(0.0, 1.5) }).collect();
        let shown = match &ops[..] { [studio_core::Op::SetGain { gain, .. }] => format!("gain {gain:.2}"),
                                     _ => format!("gain {:+.2} on {}", delta, ops.len()) };
        self.edit_group(ops, &shown);
    }

    /// `m` / `l`: flip mute (or lock) on the tracks of the target clips.
    pub fn toggle_track_flag(&mut self, mute: bool) {
        let p = &self.session.doc.project;
        let mut tracks: Vec<studio_core::TrackId> = self.targets().iter()
            .filter_map(|id| p.clips.get(id)).map(|c| c.track).collect();
        if tracks.is_empty() { tracks.extend(self.focus_track()); }
        tracks.dedup();
        let ops: Vec<_> = tracks.iter().filter_map(|id| p.tracks.iter().find(|t| t.id == *id)).map(|t| {
            let (muted, locked) = if mute { (!t.muted, t.locked) } else { (t.muted, !t.locked) };
            studio_core::Op::SetTrackFlags { id: t.id, muted, locked }
        }).collect();
        if ops.is_empty() { self.status = "no track".into(); return; }
        let now = match &ops[0] { studio_core::Op::SetTrackFlags { muted, locked, .. } => if mute { *muted } else { *locked }, _ => false };
        let what = format!("{} {}", if mute { if now { "muted" } else { "unmuted" } } else if now { "locked" } else { "unlocked" },
                           tracks.iter().filter_map(|id| p.tracks.iter().find(|t| t.id == *id)).map(|t| t.name.clone()).collect::<Vec<_>>().join(", "));
        self.edit_group(ops, &what);
    }

    /// `D`: a copy of the selection straight after it -- the arranging move
    /// (the next four bars are the last four, then change them). Goes through
    /// `paste_at` so compositions and automation come along; the clipboard is
    /// left as it was.
    pub fn duplicate_selected(&mut self) {
        let p = &self.session.doc.project;
        let Some(end) = self.selection.iter().filter_map(|id| p.clips.get(id)).map(|c| c.end()).max() else {
            self.status = "duplicate: nothing selected".into(); return };
        let saved = std::mem::replace(&mut self.clipboard, self.selection.clone());
        self.paste_at(end);
        self.clipboard = saved;
    }

    /// `[` / `]`: fade the target clips in up to the playhead, or out from
    /// it -- a fade drawn by standing where it should end. The other fade is
    /// kept. On a video clip over a lower track this is a dissolve.
    pub fn fade_to_playhead(&mut self, fade_in: bool) {
        let at = self.transport.position();
        let p = &self.session.doc.project;
        let ops: Vec<_> = self.targets().iter().filter_map(|id| p.clips.get(id))
            .filter(|c| c.pos < at && at < c.end())
            .map(|c| {
                let (fi, fo) = p.fades.get(&c.id).copied().unwrap_or((Ticks::ZERO, Ticks::ZERO));
                let (fi, fo) = if fade_in { (at.saturating_sub(c.pos), fo.min(c.end().saturating_sub(at))) }
                               else { (fi.min(at.saturating_sub(c.pos)), c.end().saturating_sub(at)) };
                studio_core::Op::SetClipFade { id: c.id, fade_in: fi, fade_out: fo }
            }).collect();
        if ops.is_empty() {
            self.status = format!("fade {}: put the playhead inside a selected clip", if fade_in { "in" } else { "out" });
            return;
        }
        let n = ops.len();
        self.edit_group(ops, &format!("fade {} {:.2}s on {n} clip{}", if fade_in { "in to" } else { "out from" },
                                      at.to_seconds(), if n == 1 { "" } else { "s" }));
    }

    /// `n`: set each target clip's gain so its loudest sample -- over the part
    /// of the source the clip plays -- sits at -1 dBFS, capped at the
    /// inspector's 1.5 (said out loud when the cap bites). `audio` is the
    /// shell's decoded sources; a clip whose audio is not decoded is skipped
    /// and counted.
    pub fn normalize_selected(&mut self, audio: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>) {
        const TARGET: f32 = 0.891_250_9; // -1 dBFS
        let p = &self.session.doc.project;
        let (mut ops, mut missing, mut capped) = (Vec::new(), 0, 0);
        for c in self.targets().iter().filter_map(|id| p.clips.get(id)) {
            let Some(a) = audio.get(&c.source) else { missing += 1; continue };
            let rate = a.sample_rate.max(1);
            let from = (c.src_in.to_samples(rate).max(0) as usize).min(a.frames());
            let to = (from + c.len.to_samples(rate).max(0) as usize).min(a.frames());
            let peak = a.channels.iter().flat_map(|ch| ch[from..to].iter()).fold(0.0f32, |m, v| m.max(v.abs()));
            if peak <= 1e-6 { missing += 1; continue; }
            let g = TARGET / peak;
            if g > 1.5 { capped += 1; }
            ops.push(studio_core::Op::SetGain { id: c.id, gain: g.min(1.5) });
        }
        if ops.is_empty() {
            self.status = if missing > 0 { "normalize: no decoded audio under the selection".into() }
                          else { "normalize: nothing selected or under the playhead".into() };
            return;
        }
        let n = ops.len();
        self.edit_group(ops, &format!("normalized {n} clip{} to -1 dBFS{}{}", if n == 1 { "" } else { "s" },
            if capped > 0 { format!(" ({capped} capped at gain 1.5 -- too quiet to reach it)") } else { String::new() },
            if missing > 0 { format!(" ({missing} skipped: silent or not decoded)") } else { String::new() }));
    }

    fn split_at_playhead(&mut self) {
        let at = self.transport.position();

        // A FOLDED COMP IS CUT AS A COMP, not through the clips inside it.
        //
        // `split_group_ops` exists for exactly this and nothing called it, so
        // splitting a folded composition went through `split_ops` and cut a
        // MEMBER — material spliced out of a block the user cannot see into.
        // The group split is two windows over the same member list and zero
        // clip ops, so nothing inside is touched.
        //
        // Checked BEFORE the clip search, because a folded comp has member
        // clips under the playhead too and the clip search would find one
        // first. Folded-ness is view state (`closed_groups`, per device), so
        // this is the app's call to make and not the project's: an OPEN comp
        // splits its clips, which is what opening it meant.
        if let Some(gid) = self.folded_group_at(at) {
            let next = self.session.doc.project.groups.keys().map(|g| g.0).max().unwrap_or(0) + 1;
            match self.session.doc.project
                .split_group_ops(gid, at, studio_core::GroupId(next))
            {
                Ok(ops) => {
                    // One undo step: a comp split is one gesture, and leaving
                    // it as two ops would make `u` produce a half-split comp.
                    self.edit_group(ops, "split comp");
                    return;
                }
                Err(e) => {
                    self.status = format!("split refused: {e:?}");
                    return;
                }
            }
        }

        let next = self.session.doc.project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
        let target = self.selection.first().copied().or_else(|| {
            self.session.doc.project.clips.values()
                .find(|c| c.pos < at && c.end() > at).map(|c| c.id)
        });
        let Some(id) = target else { self.status = "nothing under the playhead".into(); return };
        match self.session.doc.project.split_ops(id, at, studio_core::ClipId(next)) {
            Ok(ops) => {
                for op in ops { self.edit(op, "split"); }
            }
            Err(e) => self.status = format!("split refused: {e:?}"),
        }
    }

    /// The folded composition spanning `at`, if one does.
    ///
    /// Only FOLDED comps qualify: an open one is showing its clips, and
    /// cutting those is what the user is looking at. Strictly inside the span,
    /// matching `split_group_ops`' own refusal at the edges — splitting a comp
    /// at its own boundary is the same non-event as splitting a clip at its
    /// start.
    ///
    /// Lowest id wins when comps nest, which is arbitrary but STABLE; a split
    /// that picked a different comp depending on map iteration order would be
    /// worse than one that always picks the same wrong one.
    fn folded_group_at(&self, at: Ticks) -> Option<studio_core::GroupId> {
        self.session.doc.project.groups.keys()
            .filter(|gid| self.closed_groups.contains(gid))
            .filter(|gid| {
                self.session.doc.project.group_span(**gid)
                    .is_some_and(|(a, b)| at > a && at < b)
            })
            .min()
            .copied()
    }

    /// `y`: remember the selection for `p`. Ids, not clips — see
    /// `clipboard`'s docs.
    pub fn yank(&mut self) {
        if self.selection.is_empty() {
            self.status = "nothing selected to yank".into();
            return;
        }
        self.clipboard = self.selection.clone();
        self.status = format!("yanked {} clip{}", self.clipboard.len(),
                              if self.clipboard.len() == 1 { "" } else { "s" });
    }

    /// `p`: copy every yanked clip so the earliest lands at `at`, each on
    /// its own track, keeping their spacing — AND every curve gated to
    /// each original re-bound onto its copy, through
    /// `Project::duplicate_clip_ops`. One `edit_group`, so a paste of nine
    /// clips with their rides is one press of `u`. This is the gesture the
    /// curves module exists for: duplicating a clip no longer means
    /// redrawing its automation. Yanked clips that were deleted since are
    /// skipped and counted out loud; a refusal (no room on the track)
    /// rolls the whole paste back, `edit_group`'s rule.
    pub fn paste_at(&mut self, at: Ticks) {
        let p = &self.session.doc.project;
        let live: Vec<studio_core::Clip> = self.clipboard.iter()
            .filter_map(|id| p.clips.get(id).cloned()).collect();
        let gone = self.clipboard.len() - live.len();
        if live.is_empty() {
            self.status = if self.clipboard.is_empty() { "nothing yanked".into() }
                          else { "every yanked clip has since been deleted".into() };
            return;
        }
        let first = live.iter().map(|c| c.pos).min().unwrap_or(Ticks::ZERO);
        let mut next_clip = p.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
        let mut next_binding = p.bindings.keys().map(|b| b.0).max().unwrap_or(0) + 1;
        let mut ops = Vec::new();
        let mut new_ids = Vec::with_capacity(live.len());
        let mut bindings = 0;
        for c in &live {
            let pos = at.saturating_add(c.pos.saturating_sub(first));
            let new_id = studio_core::ClipId(next_clip);
            next_clip += 1;
            match p.duplicate_clip_ops(c.id, new_id, pos, c.track, &mut next_binding) {
                Ok(dup) => {
                    bindings += dup.len() - 1;
                    ops.extend(dup);
                    new_ids.push(new_id);
                }
                Err(e) => { self.status = format!("paste refused: {e:?}"); return; }
            }
        }
        // Compositions come along: a group whose members were (partly)
        // yanked is recreated over the copies — a chopped take pasted
        // into the next hook is still one thing there.
        let mut next_group = p.groups.keys().map(|g| g.0).max().unwrap_or(0) + 1;
        let mut groups = 0;
        for g in p.groups.values() {
            let members: Vec<studio_core::ClipId> = live.iter().zip(new_ids.iter())
                .filter(|(c, _)| g.clips.contains(&c.id)).map(|(_, n)| *n).collect();
            if members.len() >= 2 || (!members.is_empty() && members.len() == g.clips.len()) {
                ops.push(studio_core::Op::SetGroup { id: studio_core::GroupId(next_group),
                    name: format!("{} copy", g.name), clips: members });
                next_group += 1;
                groups += 1;
            }
        }
        let n = live.len();
        let ops = self.with_room(ops);
        self.edit_group(ops, &format!("pasted {n} clip{}{}{}{}",
            if groups > 0 { format!(" in {groups} composition{}", if groups == 1 { "" } else { "s" }) }
            else { String::new() },
            if n == 1 { "" } else { "s" },
            if bindings > 0 { format!(" with {bindings} automation binding{}", if bindings == 1 { "" } else { "s" }) }
            else { String::new() },
            if gone > 0 { format!(" ({gone} yanked clip{} no longer exist{})",
                                  if gone == 1 { "" } else { "s" }, if gone == 1 { "s" } else { "" }) }
            else { String::new() }));
        // A refused group leaves the selection alone; a landed one selects
        // the copies, so the next `p` or nudge acts on what just arrived.
        if self.session.doc.project.clips.contains_key(&new_ids[0]) {
            self.selection = new_ids;
        }
    }

    /// Mint a curve and bind it: gated to every selected clip, aimed at
    /// `target` — or, with no target given, at each clip's OWN track gain,
    /// the ride everyone reaches for first. With nothing selected the
    /// curve binds `Always` to `target` (a plain lane you can reuse), and
    /// with neither there is nothing to aim at, said out loud. One group,
    /// one undo; the strip switches to the new binding so the next click
    /// places its first point.
    /// A curve over a composition: the group the first selected clip is
    /// in gates the binding, so the ride spans the hull — from the first
    /// member's start to the last's end, gap included — and follows the
    /// group as its members move or are added. Target: the strip's, or
    /// the clip's own track gain. One undo step.
    pub fn new_curve_on_group(&mut self, target: Option<studio_core::AutoTarget>) {
        use studio_core::{Binding, BindingId, CurveId, Gate, Op};
        let p = &self.session.doc.project;
        let Some(clip) = self.selection.first().and_then(|id| p.clips.get(id)) else {
            self.status = "select a clip that is in a group".into();
            return;
        };
        let Some((gid, g)) = p.groups.iter().find(|(_, g)| g.clips.contains(&clip.id)) else {
            self.status = format!("clip {} is in no group — group the composition first (G)", clip.id.0);
            return;
        };
        let (gid, gname) = (*gid, g.name.clone());
        let target = target.unwrap_or(studio_core::AutoTarget::TrackGain(clip.track));
        let curve = CurveId(p.curves.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let bid = BindingId(p.bindings.keys().map(|b| b.0).max().unwrap_or(0) + 1);
        let name = format!("curve {} over {gname}", curve.0);
        self.edit_group(vec![
            Op::SetCurve { id: curve, name: name.clone() },
            Op::SetBinding { id: bid, binding: Binding { curve, target, gate: Gate::Group(gid), response: None, fit_span: false } },
        ], &format!("{name} bound"));
        if self.session.doc.project.bindings.contains_key(&bid) {
            self.auto_target = None;
            self.auto_binding = Some(bid);
            self.status = format!("{name}: rides the group's whole span, gap included");
        }
    }

    pub fn new_curve_on_selection(&mut self, target: Option<studio_core::AutoTarget>) {
        use studio_core::{Binding, BindingId, CurveId, Gate, Op};
        let p = &self.session.doc.project;
        let curve = CurveId(p.curves.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let mut next_binding = p.bindings.keys().map(|b| b.0).max().unwrap_or(0) + 1;
        let name = format!("curve {}", curve.0);
        let mut ops = vec![Op::SetCurve { id: curve, name: name.clone() }];
        let first_binding = BindingId(next_binding);
        if self.selection.is_empty() {
            let Some(target) = target else {
                self.status = "select clips to gate a curve to, or pick a lane to bind one to".into();
                return;
            };
            ops.push(Op::SetBinding { id: first_binding, binding: Binding { curve, target, gate: Gate::Always, response: None, fit_span: false } });
        } else {
            for id in &self.selection {
                let Some(c) = p.clips.get(id) else { continue };
                let target = target.unwrap_or(studio_core::AutoTarget::TrackGain(c.track));
                ops.push(Op::SetBinding { id: BindingId(next_binding),
                    binding: Binding { curve, target, gate: Gate::Clip(*id), response: None, fit_span: false } });
                next_binding += 1;
            }
        }
        let bound = ops.len() - 1;
        self.edit_group(ops, &format!("{name} bound to {bound} target{}", if bound == 1 { "" } else { "s" }));
        if self.session.doc.project.bindings.contains_key(&first_binding) {
            self.auto_target = None;
            self.auto_binding = Some(first_binding);
        }
    }

    /// Drop one binding. The curve stays — it may drive other clips, and
    /// a curve nothing drives is inert, not garbage.
    pub fn unbind(&mut self, id: studio_core::BindingId) {
        self.edit(studio_core::Op::RemoveBinding { id }, "unbound");
        if self.auto_binding == Some(id) { self.auto_binding = None; }
    }

    /// The playhead to render, honouring theatre mode.
    pub fn playhead(&self) -> Ticks {
        self.mode.effective_playhead(self.transport.position(), None)
    }

    /// The camera the timeline should draw with right now: `active_song`'s
    /// own remembered zoom/scroll, or the single project-wide `view` when
    /// nothing is selected (today's only behaviour, for every project with
    /// no groups or with no song picked yet).
    pub fn current_view(&self) -> &studio_egui::View {
        match self.active_song {
            Some(id) => self.song_views.get(&id).unwrap_or(&self.view),
            None => &self.view,
        }
    }

    /// Mutable version, for the timeline's own scroll/zoom handling.
    /// Creates the song's entry on first use rather than requiring one to
    /// already exist — a song opened for the first time this session
    /// starts from a fresh default camera, not a missing one.
    pub fn current_view_mut(&mut self) -> &mut studio_egui::View {
        match self.active_song {
            Some(id) => self.song_views.entry(id).or_default(),
            None => &mut self.view,
        }
    }

    /// Switch which song the timeline shows. A no-op guard against
    /// clobbering: switching to the song already active must not reset
    /// anything, since `current_view_mut` would otherwise be free to
    /// insert a fresh default over a camera that already existed the
    /// instant this ran with the SAME id — it doesn't today (`entry`
    /// preserves an existing value), but the guard also means this
    /// function is the one place "switch song" logic lives if that ever
    /// needs to do more than assign a field (recentring the playhead on
    /// the song's own start, for instance).
    pub fn switch_song(&mut self, to: Option<studio_core::GroupId>) {
        self.active_song = to;
    }

    /// Write the arrangement to `dir` as a file worth sending: 16-bit
    /// dithered WAV or 128kbps Ogg Opus — the format this project's own
    /// sample libraries arrive in, at roughly a tenth of the WAV's size.
    /// Renders at the PROJECT rate (48k), not the device's: an export must
    /// not change length because of the sound card. Returns the path so the
    /// status line can say where the file went — an export whose location
    /// you have to guess is half an export.
    pub fn export_arrangement(
        &mut self,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
        dir: &std::path::Path,
        format: ExportFormat,
        rate: u32,
        layout: studio_audio::format::ChannelFormat,
    ) -> Result<std::path::PathBuf, String> {
        // Opus is a 48k codec by contract (studio-media refuses anything
        // else, loudly); WAV renders at whatever was asked — mixdown is
        // rate-parametric end to end, and since the source-rate fix it
        // resamples sources correctly at ANY render rate rather than only
        // happening to be right at 48k.
        let rate = match format { ExportFormat::Opus => 48_000, _ => rate };
        // Opus's mapping family 0 is mono/stereo only (studio-media refuses
        // wider, loudly), so a surround deliverable is a WAV — said here
        // rather than discovered as a confusing encoder error. The
        // phone's formats (mp3, m4a) are stereo for the same reason.
        let layout = if format.stereo_only() { studio_audio::format::ChannelFormat::Stereo } else { layout };
        let stereo = self.render_arrangement(rate, sources);
        // The render is stereo (see studio_audio::format's scope note); the
        // deliverable is whatever was asked for.
        let samples = studio_audio::format::expand_from_stereo(&stereo, layout, rate);
        if samples.is_empty() {
            self.status = "export refused: the arrangement is empty".into();
            return Err(self.status.clone());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
        let name: String = self.session.doc.project.name.chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        // The layout is in the NAME for anything but plain stereo: a folder
        // of exports where the 5.1 and the 2.0 are told apart only by their
        // byte size is a folder someone ships the wrong file from.
        let suffix = if layout == studio_audio::format::ChannelFormat::Stereo {
            String::new()
        } else {
            format!("-{}", layout.label())
        };
        // Milliseconds, AND an exists check: second-granularity stamps let
        // two exports in one second silently overwrite each other — found
        // when the 48k and 96k test exports turned out to be one file.
        let mut path = dir.join(format!("{name}-{stamp}{suffix}.{}", format.ext()));
        let mut n = 1u32;
        while path.exists() {
            path = dir.join(format!("{name}-{stamp}{suffix}-{n}.{}", format.ext()));
            n += 1;
        }
        match format {
            ExportFormat::Wav => {
                studio_audio::export::export_wav_dithered(
                    &samples, layout.channels() as u16, rate, stamp, &path)
                    .map_err(|e| format!("wav write: {e}"))?;
            }
            ExportFormat::Opus => {
                // De-interleave: the encoder speaks the same planar
                // SourceAudio every other seam here does.
                let mut planes = vec![Vec::with_capacity(samples.len() / 2); 2];
                for fr in samples.chunks(2) {
                    planes[0].push(fr[0]);
                    planes[1].push(fr[1]);
                }
                let audio = studio_audio::SourceAudio { sample_rate: rate, channels: planes };
                let bytes = studio_media::encode_opus(&audio, 128_000)
                    .map_err(|e| e.to_string())?;
                std::fs::write(&path, bytes).map_err(|e| format!("write {path:?}: {e}"))?;
            }
            ExportFormat::Mp3 | ExportFormat::M4a => {
                // A WAV beside it, transcoded by ffmpeg, then removed.
                let wav = path.with_extension("tmp.wav");
                studio_audio::export::export_wav_dithered(&samples, 2, rate, stamp, &wav)
                    .map_err(|e| format!("wav write: {e}"))?;
                let r = studio_video::transcode_audio(&wav, &path, format.ffmpeg_codec().expect("lossy"), 192_000)
                    .map_err(|e| format!("{}: {e} — ffmpeg with that encoder is needed for .{}", format.ext(), format.ext()));
                let _ = std::fs::remove_file(&wav);
                r?;
            }
        }
        self.status = format!("exported {} ({})", path.display(), layout.label());
        Ok(path)
    }

    /// Render the whole arrangement at `rate` as interleaved stereo — what
    /// the sink plays when the TIMELINE transport rolls. `mixdown` is the
    /// tested pure function; this is a one-shot offline render of it, traded
    /// deliberately against `studio-engine`'s streaming path: at the sizes a
    /// beat project has, re-rendering the whole thing on an edit is cheap,
    /// keeps zero state, and cannot glitch — the streaming DeviceEngine
    /// remains the named path for when projects outgrow that trade.
    pub fn render_arrangement(
        &self,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Vec<f32> {
        // Where the SOUND ends, not the last clip: a ring-out on the final
        // clip must not be cut off by the render that plays it.
        let end = studio_audio::audible_end(&self.session.doc.project);
        if end <= Ticks::ZERO { return Vec::new(); }
        studio_audio::mixdown(
            &self.session.doc.project, sources, &std::collections::BTreeMap::new(),
            Ticks::ZERO, end, rate, &self.solo, &[],
        )
    }

    /// `[from, to)` of the arrangement, for the loop window.
    pub fn render_arrangement_range(
        &self,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
        from: Ticks,
        to: Ticks,
    ) -> Vec<f32> {
        if to <= from { return Vec::new(); }
        studio_audio::mixdown(
            &self.session.doc.project, sources, &std::collections::BTreeMap::new(),
            from, to, rate, &self.solo, &[],
        )
    }

    /// Set (or clear) the loop range from the selection's span, as the
    /// transport's `loop_range`. The playback block renders the range
    /// alone and loops it; recording inside it is loop-record.
    pub fn loop_selection(&mut self) {
        let p = &self.session.doc.project;
        let clips: Vec<&studio_core::Clip> = self.selection.iter().filter_map(|id| p.clips.get(id)).collect();
        if clips.is_empty() {
            self.status = "loop: select the clips that span the bars to loop".into();
            return;
        }
        let start = clips.iter().map(|c| c.pos).min().unwrap();
        let end = clips.iter().map(|c| c.end()).max().unwrap();
        self.transport.loop_range = Some((start, end));
        self.arrangement_dirty = true;
        self.status = format!("looping {:.2}s – {:.2}s (record inside it for a take per pass)",
                              start.to_seconds(), end.to_seconds());
    }

    pub fn loop_off(&mut self) {
        self.transport.loop_range = None;
        self.arrangement_dirty = true;
        self.status = "loop off".into();
    }

    /// A folder dropped on the app is a KIT: every audio file in it, in
    /// name order, becomes a sampler channel — kick, clap, hat, snare,
    /// the way a drum pack unzips. Non-audio files and files that fail
    /// to decode are skipped and counted; more than [`Self::KIT_MAX`]
    /// files takes the first that many, and says so. Returns what was
    /// decoded so the shell can keep the audio and the raw bytes (the
    /// peers' copy) beside the sources it registered.
    pub fn import_kit(&mut self, dir: &std::path::Path)
        -> Result<Vec<(studio_core::SourceId, studio_audio::SourceAudio, Vec<u8>)>, String>
    {
        const AUDIO_EXT: &[&str] = &["wav", "flac", "mp3", "ogg", "opus", "aif", "aiff", "m4a"];
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str())
                .map(|x| AUDIO_EXT.contains(&x.to_ascii_lowercase().as_str())).unwrap_or(false))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(format!("no audio files in {}", dir.display()));
        }
        let total = files.len();
        files.truncate(Self::KIT_MAX);
        let mut out = Vec::new();
        let mut failed = 0;
        for path in &files {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("hit").to_string();
            let raw = match std::fs::read(path) { Ok(b) => b, Err(_) => { failed += 1; continue } };
            let hint = path.extension().and_then(|e| e.to_str());
            let audio = match studio_media::decode_bytes(raw.clone(), hint) {
                Ok(a) => a,
                Err(_) => { failed += 1; continue }
            };
            let id = self.import_audio(&name, &path.display().to_string(), &audio);
            out.push((id, audio, raw));
        }
        let kit = dir.file_name().and_then(|s| s.to_str()).unwrap_or("kit");
        self.status = format!("kit '{kit}': {} channel{} added{}{}", out.len(),
            if out.len() == 1 { "" } else { "s" },
            if failed > 0 { format!(", {failed} file(s) could not be decoded") } else { String::new() },
            if total > Self::KIT_MAX { format!(", first {} of {total} taken", Self::KIT_MAX) } else { String::new() });
        Ok(out)
    }

    /// Rack channels one folder drop may add: a 64-step bitmask per
    /// channel is the pattern's row, and a kit past this is a library,
    /// not a kit.
    pub const KIT_MAX: usize = 32;

    /// Land decoded audio on the timeline as a clip at the playhead — the
    /// import a melody loop wants, where the rack sampler is the import a
    /// drum hit wants. Same op shape as a bounce and a take, so it is
    /// undoable and shareable for free.
    pub fn import_clip(
        &mut self,
        name: &str,
        path: &str,
        audio: &studio_audio::SourceAudio,
    ) -> Option<studio_core::SourceId> {
        let len = Ticks::from_samples(audio.frames() as i64, audio.sample_rate);
        let at = self.playhead();
        // First audio track WITH ROOM at the playhead, not just the first
        // audio track: check_room would (rightly) refuse an overlap, and a
        // loop dropped where A1 is busy should land on A2, the way a person
        // placing it by hand would — not bounce off with an error. This
        // pre-flight mirrors check_room's rule; the op's own validation
        // still has the final word.
        let project = &self.session.doc.project;
        let end = Ticks(at.0 + len.0);
        let track = project.tracks.iter()
            .filter(|t| t.kind == studio_core::TrackKind::Audio)
            .find(|t| !project.clips.values()
                .any(|c| c.track == t.id && c.pos < end && c.end() > at))
            .map(|t| t.id);
        let Some(track) = track else {
            self.status = "import refused: no audio track has room at the playhead".into();
            return None;
        };
        let id = studio_core::SourceId(
            project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let clip = studio_core::ClipId(
            project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        self.edit(studio_core::Op::AddSource {
            source: studio_core::Source {
                id, path: path.into(), len,
                sample_rate: audio.sample_rate,
                channels: audio.channels.len() as u16,
                has_video: false, kind: studio_core::SourceKind::File,
            },
        }, "imported");
        self.edit(studio_core::Op::AddClip {
            clip: studio_core::Clip {
                id: clip, source: id, track,
                pos: at, src_in: Ticks::ZERO, len,
                gain: 1.0, effects: Vec::new(),
            },
        }, "imported");
        self.status = format!("imported '{name}' to the timeline at the playhead");
        Some(id)
    }

    /// Land a VIDEO on the timeline: the C12-6a join, finally legal to
    /// make because `studio_video::frame_rgb` + the viewport can decode
    /// and display a frame (the gate that section set). The source is the
    /// video FILE — `has_video: true`, kind: studio_core::SourceKind::File, length from the probe (a video may
    /// outlast its own audio) — and the clip lands on a video track at the
    /// playhead. Where `import_clip` REFUSES when no audio track has room,
    /// this CREATES a video track when none has room: §6's own words are
    /// "give the timeline a video track to put it on", video tracks are
    /// scarce where audio tracks are plentiful, and the AddTrack is an
    /// ordinary undoable op either way. The audio half, when the container
    /// has one, is the caller's to decode (ffmpeg lives in the shell's
    /// world); passing `audio_rate`/`audio_channels` here keeps this
    /// method headless and testable without a subprocess.
    pub fn import_video_clip(
        &mut self,
        name: &str,
        path: &str,
        len: Ticks,
        audio: Option<(u32, u16)>,
    ) -> Option<studio_core::SourceId> {
        if len <= Ticks::ZERO {
            self.status = format!("'{name}' has no duration — probe failed?");
            return None;
        }
        let at = self.playhead();
        let end = Ticks(at.0 + len.0);
        let project = &self.session.doc.project;
        let track = project.tracks.iter()
            .filter(|t| t.kind == studio_core::TrackKind::Video)
            .find(|t| !project.clips.values()
                .any(|c| c.track == t.id && c.pos < end && c.end() > at))
            .map(|t| t.id);
        let track = match track {
            Some(t) => t,
            None => {
                let tid = studio_core::TrackId(
                    project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
                let n = project.tracks.iter()
                    .filter(|t| t.kind == studio_core::TrackKind::Video).count() + 1;
                self.edit(studio_core::Op::AddTrack {
                    track: studio_core::Track {
                        id: tid, kind: studio_core::TrackKind::Video,
                        name: format!("V{n}"), muted: false, locked: false,
                        gain: 1.0, route: None },
                    at: 0, // video on top: the timeline's own drawing convention
                }, "video track");
                tid
            }
        };
        let (sample_rate, channels) = audio.unwrap_or((48_000, 0));
        let project = &self.session.doc.project;
        let id = studio_core::SourceId(
            project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let clip = studio_core::ClipId(
            project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        self.edit(studio_core::Op::AddSource {
            source: studio_core::Source {
                id, path: path.into(), len,
                sample_rate, channels,
                has_video: true, kind: studio_core::SourceKind::File,
            },
        }, "imported");
        self.edit(studio_core::Op::AddClip {
            clip: studio_core::Clip {
                id: clip, source: id, track,
                pos: at, src_in: Ticks::ZERO, len,
                gain: 1.0, effects: Vec::new(),
            },
        }, "imported");
        self.status = format!("imported '{name}' — picture on the video track");
        Some(id)
    }


    /// Pack one rack pattern for sharing: the pattern, the channels it
    /// fires, and — for `Sampler` channels — the sample FILES read back
    /// from each source's path (the compact representation IS the file,
    /// `import_audio`'s own rule). A missing file refuses the whole pack:
    /// a loop that ships without its kick is worse than no loop.
    ///
    /// A GENERATOR source refuses the pack too, for the same reason and with
    /// its own message: it has no file, so there is nothing to embed. See the
    /// check below for why that is caught by type rather than left to the
    /// read.
    pub fn export_pattern_pack(&self, pattern_ix: usize)
        -> Result<pattern_pack::PatternPack, String>
    {
        let p = self.rack.patterns.get(pattern_ix)
            .ok_or_else(|| "no such pattern".to_string())?;
        let mut media_meta = Vec::new();
        let mut media = Vec::new();
        for ch in &self.rack.channels {
            if let studio_live::SynthKind::Sampler { source } = ch.kind {
                if media_meta.iter().any(|(id, _)| *id == source.0) { continue; }
                let src = self.session.doc.project.sources.get(&source)
                    .ok_or_else(|| format!("channel '{}' references a source the \
                                            project no longer has", ch.name))?;
                // A generator has no file to pack. Refused HERE, by type,
                // rather than letting the read below fail: a generator's path
                // is empty, so `fs::read("")` reports "No such file or
                // directory" against a blank filename and blames the file
                // system for what is a category error. The pack format's rule
                // is that the compact representation IS the file, so a source
                // with no file cannot participate -- and saying so names the
                // fix (bounce it to a sample first) instead of sending the
                // user looking for a missing wav.
                if src.is_generator() {
                    return Err(format!(
                        "channel '{}' plays the generator '{}', which has no sample file to \
                         pack — bounce it to audio first",
                        ch.name, src.display_name()));
                }
                let bytes = std::fs::read(&src.path)
                    .map_err(|e| format!("'{}': {e} — the pack would ship without \
                                          its sample", src.path))?;
                // The FILE NAME here, deliberately not display_name(): this
                // hint names a file inside the pack, so it wants the
                // extension. display_name() is for labelling a clip in the
                // UI. The generator case that made the two differ is refused
                // above, so by here a path always exists.
                let hint = std::path::Path::new(&src.path).file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "sample".into());
                media_meta.push((source.0, hint));
                media.push(bytes);
            }
        }
        Ok(pattern_pack::PatternPack {
            meta: pattern_pack::PackMeta {
                name: if p.name.is_empty() {
                    format!("pattern {}", pattern_ix + 1)
                } else { p.name.clone() },
                tempo_bpm: self.rack.tempo_bpm,
                swing: self.rack.swing,
                channels: self.rack.channels.clone(),
                pattern: pattern_pack::PatternFlat::from_pattern(p),
                media: media_meta,
            },
            media,
        })
    }

    /// Drop a pack's loop into THIS project: fresh sources minted through
    /// ordinary `AddSource` ops (so peers converge and undo works),
    /// `Sampler` channels rewritten to them, channels appended, and the
    /// pattern's rows/mods/rolls SHIFTED by the existing channel count —
    /// they are indexed by position, and an off-by-one here plays the hat
    /// pattern on the kick. Returns the decoded audio + raw bytes per new
    /// source for the shell to land in peaks/audio and register as assets.
    /// Tempo/swing are deliberately NOT adopted: an import is "this loop,
    /// in my session", not "my session, at their tempo".
    pub fn import_pattern_pack(&mut self, pack: pattern_pack::PatternPack)
        -> Result<Vec<(studio_core::SourceId, studio_audio::SourceAudio, Vec<u8>)>, String>
    {
        let mut remap: std::collections::BTreeMap<u64, studio_core::SourceId> =
            Default::default();
        let mut landed = Vec::new();
        for ((old, hint), raw) in pack.meta.media.iter().zip(&pack.media) {
            let ext = std::path::Path::new(hint).extension()
                .and_then(|e| e.to_str());
            let audio = studio_media::decode_bytes(raw.clone(), ext)
                .map_err(|e| format!("'{hint}': {e}"))?;
            let id = studio_core::SourceId(
                self.session.doc.project.sources.keys()
                    .map(|s| s.0).max().unwrap_or(0) + 1);
            self.edit(studio_core::Op::AddSource {
                source: studio_core::Source {
                    id, path: hint.clone(),
                    len: Ticks::from_samples(audio.frames() as i64, audio.sample_rate),
                    sample_rate: audio.sample_rate,
                    channels: audio.channels.len() as u16,
                    has_video: false, kind: studio_core::SourceKind::File,
                },
            }, "pack sample");
            remap.insert(*old, id);
            landed.push((id, audio, raw.clone()));
        }
        let base = self.rack.channels.len();
        for ch in &pack.meta.channels {
            let mut ch = ch.clone();
            if let studio_live::SynthKind::Sampler { source } = &mut ch.kind {
                *source = *remap.get(&source.0)
                    .ok_or_else(|| format!("channel '{}' references media the \
                                            pack does not carry", ch.name))?;
            }
            self.rack.channels.push(ch);
        }
        let mut p = pack.meta.pattern.into_pattern();
        p.rows = {
            let mut rows = vec![0u64; base];
            rows.extend(p.rows);
            rows
        };
        p.mods = p.mods.into_iter()
            .map(|((ch, st), m)| ((ch + base, st), m)).collect();
        p.rolls = p.rolls.into_iter()
            .map(|(ch, n)| (ch + base, n)).collect();
        let name = p.name.clone();
        self.rack.patterns.push(p);
        self.current_pattern = self.rack.patterns.len() - 1;
        self.loop_dirty = true;
        self.status = format!("imported '{name}' — {} new channels", pack.meta.channels.len());
        Ok(landed)
    }

    /// Render the current pattern's loop at `rate` — what the audition sink
    /// plays and what a bounce stores. One function so they cannot drift.
    pub fn render_loop(
        &self,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Vec<f32> {
        let Some(p) = self.rack.patterns.get(self.current_pattern) else { return Vec::new() };
        let mut synth = studio_live::BuiltinSynth { sources };
        studio_live::render_pattern(p, &self.rack, &mut synth, &self.rack.tempo(), rate, 0)
    }

    /// Land decoded audio in the project and give it a rack channel, in one
    /// gesture. The source reaches the project as an `AddSource` op — the
    /// same undoable, shareable shape a take and a bounce use — with `path`
    /// kept as the file's real location, because the compact representation
    /// of a sample IS the compressed file: the project references (or, via
    /// `studio-collab`'s chunked assets, carries the bytes of) the .opus
    /// itself, and PCM only ever exists in memory after decode. That is the
    /// difference between this and an editor that inflates every import to
    /// WAV inside the project.
    pub fn import_audio(
        &mut self,
        name: &str,
        path: &str,
        audio: &studio_audio::SourceAudio,
    ) -> studio_core::SourceId {
        let id = studio_core::SourceId(
            self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        self.edit(studio_core::Op::AddSource {
            source: studio_core::Source {
                id, path: path.into(),
                len: studio_core::Ticks::from_samples(audio.frames() as i64, audio.sample_rate),
                sample_rate: audio.sample_rate,
                channels: audio.channels.len() as u16,
                has_video: false, kind: studio_core::SourceKind::File,
            },
        }, "imported");
        let ch = self.rack.add_channel(studio_live::Channel {
            name: name.into(),
            kind: studio_live::SynthKind::Sampler { source: id },
            gain: 0.9, pan: 0.0, muted: false, choke_group: None,
        });
        self.selected_channel = ch;
        self.loop_dirty = true;
        self.status = if audio.sample_rate == 48_000 {
            format!("imported '{name}' → rack channel {}", ch + 1)
        } else {
            // Playable at pitch either way (the sampler resamples at render
            // time), but worth a sentence: mixdown quality is best at the
            // project rate, and the person can re-export their library once
            // instead of wondering.
            format!("imported '{name}' at {} Hz (project is 48000; resampled live)",
                    audio.sample_rate)
        };
        id
    }

    /// Bounce the current pattern onto the first audio track at the playhead.
    ///
    /// Returns the new source's id plus the samples, so the caller can stash
    /// the audio wherever it keeps decoded sources (and build a peak pyramid
    /// for the timeline). The ops go through [`Studio::edit`], so the bounce
    /// is undoable and — because it is `AddSource`+`AddClip`, the exact shape
    /// a recorded take uses — shareable by the same mechanism a drag is.
    pub fn bounce_current_pattern(
        &mut self,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Option<(studio_core::SourceId, studio_audio::SourceAudio)> {
        let track = self.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio)
            .map(|t| t.id);
        let Some(track) = track else {
            self.status = "bounce refused: no audio track".into();
            return None;
        };
        let at = self.playhead();
        self.place_pattern(self.current_pattern, at, track, rate, sources)
    }

    /// Render pattern `index` at the rack tempo and land it at `at` on `track` as an
    /// ordinary AddSource + AddClip (undoable, on the wire); the source is LINKED to the
    /// pattern by name so later rack edits re-render it (`rerender_pattern_links`). The
    /// library drop, ⇥ and the rack's bounce all come through here.
    pub fn place_pattern(
        &mut self,
        index: usize,
        at: studio_core::Ticks,
        track: studio_core::TrackId,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Option<(studio_core::SourceId, studio_audio::SourceAudio)> {
        let pattern = match self.rack.patterns.get(index) {
            Some(p) => p.clone(),
            None => { self.status = "that pattern no longer exists".into(); return None; }
        };
        if !self.session.doc.project.tracks.iter().any(|t| t.id == track && t.kind == studio_core::TrackKind::Audio) {
            self.status = "placing refused: a pattern needs an audio track".into();
            return None;
        }
        let source = studio_core::SourceId(
            self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let clip = studio_core::ClipId(
            self.session.doc.project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let path = format!("bounces/{}-{}.wav", pattern.name, source.0);
        let ops = studio_live::bounce_ops(&pattern, &self.rack.tempo(), rate,
                                          source, clip, track, at, &path);
        // Room is checked BEFORE the source lands: an AddClip the model refuses (overlap)
        // would otherwise leave a source with no clip and a status line about a bounce
        // that is not on the timeline. The refusal names the clip in the way.
        if let Some(studio_core::Op::AddClip { clip: c }) = ops.get(1) {
            if let Some(other) = self.session.doc.project.clips.values().find(|o| c.overlaps(o)) {
                self.status = format!("placing refused: '{}' would overlap clip {} on that track at {:.2}s",
                                      pattern.name, other.id.0, at.0 as f64 / studio_core::TICKS_PER_SECOND as f64);
                return None;
            }
        }
        let samples = self.render_pattern_index(index, rate, sources);
        for op in ops {
            self.edit(op, "bounced");
        }
        self.pattern_links.insert(source, pattern.name.clone());
        self.status = format!("placed '{}' on the timeline at {:.2}s (live: rack edits re-render it)",
                              pattern.name, at.0 as f64 / studio_core::TICKS_PER_SECOND as f64);
        Some((source, studio_audio::SourceAudio { sample_rate: rate, channels: vec![samples] }))
    }

    /// `render_loop` for a chosen pattern rather than the current one.
    pub fn render_pattern_index(
        &self,
        index: usize,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Vec<f32> {
        let Some(p) = self.rack.patterns.get(index) else { return Vec::new() };
        let mut synth = studio_live::BuiltinSynth { sources };
        studio_live::render_pattern(p, &self.rack, &mut synth, &self.rack.tempo(), rate, 0)
    }

    /// The live half of a placed pattern: every linked source whose pattern still exists is
    /// re-rendered from the rack as it is now. Returns the sources that changed, so the
    /// caller can replace their audio and peaks and re-mix the arrangement. A source whose
    /// pattern was deleted keeps its last render and is unlinked — it is a bounce now, and
    /// the status line says so.
    pub fn rerender_pattern_links(
        &mut self,
        rate: u32,
        sources: &std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    ) -> Vec<(studio_core::SourceId, studio_audio::SourceAudio)> {
        let mut out = Vec::new();
        let mut orphaned = Vec::new();
        for (source, name) in self.pattern_links.clone() {
            if !self.session.doc.project.sources.contains_key(&source) { orphaned.push(source); continue; }
            match self.rack.patterns.iter().position(|p| p.name == name) {
                Some(i) => {
                    let samples = self.render_pattern_index(i, rate, sources);
                    out.push((source, studio_audio::SourceAudio { sample_rate: rate, channels: vec![samples] }));
                }
                None => {
                    orphaned.push(source);
                    self.status = format!("pattern '{name}' is gone; its clip keeps the last render");
                }
            }
        }
        for s in orphaned { self.pattern_links.remove(&s); }
        out
    }
}


/// The "join session" window's state.
#[cfg(feature = "app")]
#[derive(Debug, Default, Clone)]
pub struct JoinDialog {
    pub text: String,
    pub note: String,
}

#[cfg(feature = "app")]
fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(feature = "app")]
impl App {
    /// Spawn the background hash + fingerprint job for a freshly-imported
    /// source. `is_video` decides whether fingerprinting is attempted at
    /// all -- hashing runs either way (any source can have an exact-match
    /// identity; only video gets a temporal perceptual one).
    fn spawn_provenance_job(&self, source: studio_core::SourceId, path: std::path::PathBuf, is_video: bool) {
        let tx = self.provenance_tx.clone();
        std::thread::Builder::new().name("studio-provenance".into()).spawn(move || {
            let hash = studio_video::provenance::sha256_file(&path).ok()
                .and_then(|h| std::fs::metadata(&path).ok().map(|m| (h, m.len())));
            let fingerprint = if is_video {
                studio_video::fingerprint::fingerprint_clip(&path, 1.0).ok()
                    .map(|fp| studio_core::FingerprintData { hashes: fp.hashes, interval: fp.interval })
            } else {
                None
            };
            let _ = tx.send(ProvenanceJobResult { source, path, hash, fingerprint });
        }).ok();
    }

    /// Once per frame: turn any finished provenance/fingerprint jobs into
    /// real, undoable edits. Silent on failure beyond a status line -- a
    /// clip a person can already see and use on the timeline should not
    /// be blocked or rolled back just because its background identity
    /// computation didn't finish or errored.
    fn drain_provenance(&mut self) {
        while let Ok(result) = self.provenance_rx.try_recv() {
            if let Some((hash, size)) = result.hash {
                self.studio.edit(studio_core::Op::SetProvenance {
                    source: result.source,
                    provenance: studio_core::Provenance {
                        content_hash: hash, file_size: size,
                        last_verified_path: Some(result.path.display().to_string()),
                        verified_at: None, proxy_of: None,
                    },
                }, "computed provenance");
            }
            if let Some(fp) = result.fingerprint {
                self.studio.edit(studio_core::Op::SetFingerprint { source: result.source, fingerprint: fp }, "computed fingerprint");
            }
        }
    }

    /// Once per frame: say our session's name on the LAN, hear the others.
    fn lan_tick(&mut self) {
        let now = now_ms();
        let beacon = studio_collab::lan::Beacon {
            key: self.studio.carrier_key(),
            project: self.studio.session.doc.project.name.clone(),
            host: if self.studio.display_name.is_empty() { format!("site {}", self.studio.session.doc.site.0) } else { self.studio.display_name.clone() },
            site: self.studio.session.doc.site.0,
        };
        self.lan.announce(&beacon, now);
        self.lan.poll(now);
    }

    /// Type an id, or pick a session the LAN is showing. Joining sets the
    /// key the carrier uses (`Studio::joined_key`); it does not replace the
    /// document, which arrives through the carrier's catch-up like any
    /// other late joiner's.
    fn join_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.join.take() else { return };
        let mut open = true;
        let mut done = false;
        let width = Self::dialog_width(ctx.content_rect().width(), 440.0);
        let own = studio_collab::lan::SessionId(self.studio.carrier_key());
        let site = self.studio.session.doc.site.0;
        egui::Window::new("Join session").open(&mut open).resizable(false).collapsible(false)
            .default_width(width).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("this session:");
                    ui.monospace(own.render());
                    if ui.small_button("copy").clicked() { ctx.copy_text(own.render()); }
                });
                ui.separator();
                ui.label("On this network:");
                let found: Vec<(String, String, [u8; 20])> = self.lan.sessions(site).iter()
                    .map(|s| (s.beacon.project.clone(), s.beacon.host.clone(), s.beacon.key)).collect();
                if found.is_empty() {
                    match &self.lan.error {
                        Some(e) => ui.colored_label(egui::Color32::GRAY, format!("LAN discovery unavailable: {e}")),
                        None => ui.colored_label(egui::Color32::GRAY, "nothing yet — sessions announce once a second"),
                    };
                }
                for (project, host, key) in found {
                    if ui.button(format!("join {host}'s \"{project}\"  ·  {}", studio_collab::lan::SessionId(key).short())).clicked() {
                        self.studio.joined_key = Some(key);
                        self.studio.status = format!("joined {host}'s \"{project}\"");
                        done = true;
                    }
                }
                ui.separator();
                ui.label("Or by id:");
                let edit = ui.add(egui::TextEdit::singleline(&mut d.text).id_salt("studio-join-by-id").hint_text("xxxx-xxxx-xxxx-…").desired_width(f32::INFINITY));
                let go = ui.button("join").clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go {
                    match studio_collab::lan::SessionId::parse(&d.text) {
                        Some(id) if id == own => d.note = "that is this session".into(),
                        Some(id) => {
                            self.studio.joined_key = Some(id.0);
                            self.studio.status = format!("joined session {}", id.short());
                            done = true;
                        }
                        None => d.note = "not a session id: 32 letters/digits, dashes optional, no 0/1/i/l/o".into(),
                    }
                }
                if !d.note.is_empty() { ui.colored_label(egui::Color32::LIGHT_RED, &d.note); }
            });
        if open && !done { self.join = Some(d); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_egui::keys::Key;


    /// The presence beat: the SENDING half. The receive path (integrate ->
    /// session -> doc -> timeline ghost playheads) existed for a long time
    /// with nothing on the other end — this pins down the contract that
    /// makes it live: first call sends, silence is cheap, change is fast
    /// (but rate-limited), and the heartbeat always lands inside the sweep
    /// window even when nothing moves.
    #[test]
    fn presence_beat_sends_changes_and_heartbeats_but_not_every_frame() {
        let mut s = Studio::demo();
        // First call: always sends — peers know we exist.
        let first = s.presence_beat(10_000, None, studio_collab::ViewMode::Plain).expect("first beat sends");
        let pr = match first {
            studio_collab::wire::Packet::Presence(pr) => pr,
            other => panic!("expected Presence, got {other:?}"),
        };
        assert_eq!(pr.site, s.session.doc.site);
        assert_eq!(pr.name, format!("site {}", s.session.doc.site.0),
                   "empty display_name falls back, never sends an empty string");
        assert_eq!(pr.seq, 1);

        // Nothing changed, heartbeat not due: an idle shell is near-silent.
        assert!(s.presence_beat(10_016, None, studio_collab::ViewMode::Plain).is_none(), "16ms later, idle: no packet");

        // The playhead moved right after a send: news, but rate-limited —
        // the gap counts from the LAST SEND, so a scrub is ~10 pkt/s, not 60.
        s.transport.set_position(studio_core::Ticks::from_seconds(3.0));
        assert!(s.presence_beat(10_050, None, studio_collab::ViewMode::Plain).is_none(),
                "changed but only 50ms since the last send: held");
        let moved = s.presence_beat(10_120, None, studio_collab::ViewMode::Plain).expect("past the gap: the move ships");
        match moved {
            studio_collab::wire::Packet::Presence(pr) => {
                assert_eq!(pr.playhead, studio_core::Ticks::from_seconds(3.0));
                assert_eq!(pr.seq, 2, "seq is monotonic");
            }
            other => panic!("expected Presence, got {other:?}"),
        }

        // Idle again — but the heartbeat must still beat inside LIVENESS_MS,
        // or every peer sweeps us as a ghost while we sit and listen.
        let hb = studio_collab::session::LIVENESS_MS / 3;
        assert!(s.presence_beat(10_120 + hb - 1, None, studio_collab::ViewMode::Plain).is_none());
        assert!(s.presence_beat(10_120 + hb, None, studio_collab::ViewMode::Plain).is_some(), "heartbeat due: sends unchanged state");
        assert!(hb * 2 < studio_collab::session::LIVENESS_MS,
                "two consecutive lost heartbeats still leave a beat inside the window");
    }

    /// Selection changes are presence too — the warning edge peers draw
    /// around a clip someone else is inside is only as fresh as this.
    #[test]
    fn presence_beat_announces_selection_and_derived_track() {
        let mut s = Studio::demo();
        s.presence_beat(0, None, studio_collab::ViewMode::Plain);
        let (clip, track) = {
            let (id, c) = s.session.doc.project.clips.iter().next()
                .expect("demo project has clips");
            (*id, c.track)
        };
        s.selection = vec![clip];
        match s.presence_beat(1_000, None, studio_collab::ViewMode::Plain) {
            Some(studio_collab::wire::Packet::Presence(pr)) => {
                assert_eq!(pr.selection, vec![clip]);
                assert_eq!(pr.active_track, Some(track),
                           "active track derives from the first selected clip");
            }
            other => panic!("selection change must ship: {other:?}"),
        }
    }


    /// C12-6a's join, headless: a video import carries has_video: true and, kind: studio_core::SourceKind::File
    /// lands on a video track — creating one when none has room, because
    /// §6's words are "give the timeline a video track to put it on" and
    /// AddTrack is an ordinary undoable op.

    /// The pack round trip through a real Studio: export from one, import
    /// into another with a busy rack, and the loop plays on ITS channels —
    /// rows, mods and rolls all shifted by the importing rack's channel
    /// count, sources minted fresh through ops.
    #[test]
    fn a_pattern_pack_survives_the_project_border_with_indices_intact() {
        // Exporter: trap kit + a sampler channel whose file exists on disk.
        let dir = std::env::temp_dir()
            .join(format!("stpack-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("clap.wav");
        // A tiny valid WAV: studio_media must decode it on import.
        let mut w = Vec::new();
        let (rate, frames): (u32, u32) = (48_000, 8);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());   // PCM
        w.extend_from_slice(&1u16.to_le_bytes());   // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(frames * 2).to_le_bytes());
        for i in 0..frames { w.extend_from_slice(&((i as i16) * 100).to_le_bytes()); }
        std::fs::write(&wav, &w).unwrap();

        let mut a = Studio::demo();
        let sid = studio_core::SourceId(
            a.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        a.edit(studio_core::Op::AddSource { source: studio_core::Source {
            id: sid, path: wav.display().to_string(),
            len: studio_core::Ticks::from_samples(frames as i64, rate),
            sample_rate: rate, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }, "s");
        a.rack.channels.push(studio_live::Channel {
            name: "clap".into(),
            kind: studio_live::SynthKind::Sampler { source: sid },
            gain: 1.0, pan: 0.0, muted: false, choke_group: None });
        let clap_ix = a.rack.channels.len() - 1;
        let pat_ix = a.rack.patterns.len() - 1;
        {
            let p = &mut a.rack.patterns[pat_ix];
            while p.rows.len() <= clap_ix { p.rows.push(0); }
            p.rows[clap_ix] = 0b1010;
            p.mods.insert((clap_ix, 1), studio_live::StepMod {
                vel: 0.4, ..Default::default() });
        }
        let pack = a.export_pattern_pack(pat_ix).expect("exports");
        assert_eq!(pack.media.len(), 1, "one sampler, one media entry");

        // Importer: a different project whose rack already has channels —
        // the shift everything below must survive.
        let mut b = Studio::demo();
        let base = b.rack.channels.len();
        let landed = b.import_pattern_pack(
            pattern_pack::PatternPack::decode(&pack.encode()).unwrap())
            .expect("imports");
        assert_eq!(landed.len(), 1);
        let new_sid = landed[0].0;
        assert!(b.session.doc.project.sources.contains_key(&new_sid),
                "the source arrived as an ordinary op");
        let p = b.rack.patterns.last().unwrap();
        assert_eq!(p.rows.len(), base + pack.meta.channels.len());
        assert!(p.rows[..base].iter().all(|r| *r == 0),
                "the importing rack's own channels stay silent in the new pattern");
        assert_eq!(p.rows[base + clap_ix], 0b1010,
                   "the clap fires on the CLAP's shifted row");
        assert_eq!(p.mods.get(&(base + clap_ix, 1)).map(|m| m.vel), Some(0.4),
                   "mods shifted with their channel");
        match b.rack.channels[base + clap_ix].kind {
            studio_live::SynthKind::Sampler { source } =>
                assert_eq!(source, new_sid, "the sampler points at the MINTED source"),
            ref k => panic!("expected a sampler, got {k:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_video_import_keeps_its_picture_and_finds_or_makes_a_video_track() {
        // Studio::demo() may or may not ship a video track; build bare.
        let mut s = Studio::new(
            studio_core::Project { name: "mv".into(), ..Default::default() },
            SiteId(1), Team::from_contacts("solo", &["sc-local".into()]));
        let len = studio_core::Ticks::from_seconds(12.0);
        let id = s.import_video_clip("clip", "shoot/a.mp4", len, Some((48_000, 2)))
            .expect("imports");
        let (src_has_video, src_len, clip_track, video_tracks) = {
            let p = &s.session.doc.project;
            let src = &p.sources[&id];
            let clip = p.clips.values().find(|c| c.source == id).expect("a clip landed");
            (src.has_video, src.len, clip.track,
             p.tracks.iter().filter(|t| t.kind == studio_core::TrackKind::Video).count())
        };
        assert!(src_has_video, "the whole point of C12-6a");
        assert_eq!(src_len, len, "length from the probe, not the audio");
        assert_eq!(s.session.doc.project.tracks.iter()
                   .find(|t| t.id == clip_track).unwrap().kind,
                   studio_core::TrackKind::Video,
                   "picture goes on a video track, not an audio one");
        assert_eq!(video_tracks, 1, "no video track existed, so exactly one was made");

        // A second import at the same playhead: V1 is busy, so a SECOND
        // video track appears rather than a refusal — video tracks are
        // scarce where audio tracks are plentiful, and a music video with
        // an overlay is two pictures at once on purpose.
        let id2 = s.import_video_clip("overlay", "shoot/b.mp4",
            studio_core::Ticks::from_seconds(3.0), None).expect("imports");
        let clip2_track = s.session.doc.project.clips.values()
            .find(|c| c.source == id2).unwrap().track;
        assert_ne!(clip2_track, clip_track, "V1 had no room at the playhead");
        assert_eq!(s.session.doc.project.tracks.iter()
                   .filter(|t| t.kind == studio_core::TrackKind::Video).count(), 2);
        // A soundless container (audio: None) still imports — its audio
        // half is silence, its picture is the point.
        assert_eq!(s.session.doc.project.sources[&id2].channels, 0);
        // And the whole thing is ops: undo (through the keymap, the same
        // door a person uses) removes the overlay clip again.
        s.key(studio_egui::keys::Key::c('u'));
        assert!(!s.session.doc.project.clips.values().any(|c| c.source == id2));
    }

    #[test]
    fn each_song_remembers_its_own_camera() {
        // The whole point: zooming into song A must not move where song B
        // is scrolled to, and switching back to A must restore exactly
        // where it was, not reset to a fresh default.
        let mut s = Studio::demo();
        assert_eq!(s.active_song, None, "no groups yet: the single legacy view");
        let (a, b) = (studio_core::GroupId(1), studio_core::GroupId(2));

        s.switch_song(Some(a));
        s.current_view_mut().px_per_sec = 40.0;
        s.current_view_mut().scroll = Ticks(1000);

        s.switch_song(Some(b));
        // A fresh song starts from a fresh default, not a copy of A's camera.
        assert_eq!(s.current_view().px_per_sec, studio_egui::View::default().px_per_sec);
        s.current_view_mut().px_per_sec = 200.0;
        s.current_view_mut().scroll = Ticks(50_000);

        s.switch_song(Some(a));
        assert_eq!(s.current_view().px_per_sec, 40.0, "A's zoom must survive visiting B");
        assert_eq!(s.current_view().scroll, Ticks(1000));

        s.switch_song(Some(b));
        assert_eq!(s.current_view().px_per_sec, 200.0, "B's zoom must survive visiting A");
        assert_eq!(s.current_view().scroll, Ticks(50_000));

        // Leaving song mode entirely falls back to the ORIGINAL single
        // view, untouched by either song's camera — the exact behaviour
        // every pre-existing (groupless) project still gets.
        s.switch_song(None);
        assert_eq!(s.current_view().px_per_sec, studio_egui::View::default().px_per_sec);
    }

    #[test]
    fn the_demo_project_is_not_empty() {
        // An editor that opens empty gives a new user nothing to click, and an
        // empty timeline is indistinguishable from a broken one.
        let s = Studio::demo();
        assert_eq!(s.session.doc.project.tracks.len(), 3);
        assert!(s.session.doc.project.clips.len() >= 5);
        assert!(s.session.doc.project.duration() > Ticks::ZERO);
    }

    #[test]
    fn opening_a_saved_project_hears_its_video_sources() {
        // load_from went straight to symphonia, which cannot open an mkv, so
        // every video source of a reopened project was "missing" and silent.
        let ok = std::process::Command::new("ffmpeg").arg("-version").output()
            .map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            assert!(std::env::var("STUDIO_REQUIRE_FFMPEG").as_deref() != Ok("1"), "ffmpeg required");
            return;
        }
        let dir = std::env::temp_dir().join(format!("open-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("take.mkv");
        let out = std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=gray:s=64x36:r=12:d=2",
            "-f", "lavfi", "-i", "sine=f=440:d=2", "-shortest",
            "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "pcm_s16le"]).arg(&clip).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        let mut p = studio_core::Project { name: "reopened".into(), ..Default::default() };
        p.apply(&studio_core::Op::AddSource { source: studio_core::Source::file(studio_core::SourceId(1),
            "take.mkv", studio_core::Ticks::from_seconds(2.0), 48_000, 1, true) }).unwrap();
        let file = studio_collab::file::SaveFile::from_project(&p, Vec::new(), None);
        let path = dir.join("reopened.studio");
        std::fs::write(&path, file.encode()).unwrap();

        let t = std::time::Instant::now();
        let s = Studio::load_from(&path, SiteId(1), Team::from_contacts("solo", &["sc-local".into()]))
            .expect("opens");
        assert!(!s.status.contains("missing"), "{}", s.status);
        // Opening must not extract: that is ffmpeg per file, and inline it
        // held a 77-clip project's window closed for over a minute.
        assert!(s.incoming_audio.is_empty(), "nothing is extracted on the opening path");
        assert_eq!(s.pending_video_audio.len(), 1, "the video is queued for the worker");
        assert!(t.elapsed() < std::time::Duration::from_millis(500), "opening stays fast: {:?}", t.elapsed());
        // and the worker delivers it
        let (tx, rx) = std::sync::mpsc::channel();
        let woken = std::sync::atomic::AtomicUsize::new(0);
        Studio::extract_video_audio(s.pending_video_audio.clone(), tx, || { woken.fetch_add(1, std::sync::atomic::Ordering::SeqCst); });
        assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 1, "the window is woken once per delivery");
        let (id, audio) = rx.recv().expect("one delivery");
        assert_eq!(id, studio_core::SourceId(1));
        let audio = audio.expect("the video's audio came through");
        assert!(audio.frames() > 40_000, "about two seconds of it: {}", audio.frames());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ring_out_on_the_last_clip_is_not_cut_off_by_the_render() {
        // Project::duration is the end of the last CLIP; a tail on that clip
        // would be truncated by the very render meant to carry it.
        let mut s = Studio::demo();
        let last = s.session.doc.project.clips.values().max_by_key(|c| c.end()).unwrap().clone();
        let before = s.session.doc.project.duration();
        s.edit(studio_core::Op::SetEffects { id: last.id, effects: vec![
            studio_audio::ClipTail { ring: 2.0, fade: 1.0 }.effect()] }, "tail");
        assert_eq!(s.session.doc.project.duration(), before, "the timeline's extent is unchanged");
        let end = before.saturating_add(studio_core::Ticks::from_seconds(2.0));
        assert_eq!(studio_audio::audible_end(&s.session.doc.project), end,
                   "but the sound ends two seconds later");
        // And the render the transport plays actually goes that far — the
        // wiring, not just the helper (a test of the helper alone passed with
        // render_arrangement still stopping at the last clip).
        let buf = s.render_arrangement(48_000, &Default::default());
        assert_eq!(buf.len(), end.to_samples(48_000) as usize * 2, "stereo frames to the audible end");
    }

    #[test]
    #[cfg(feature = "app")]   // App is the window: no window, no test
    fn the_status_bar_names_which_clock_is_running() {
        use crate::App;
        assert_eq!(App::clock_note(false, false, None, true), "", "stopped says nothing");
        assert_eq!(App::clock_note(true, false, Some(4800), true), "audio clock");
        // The reported bug's signature: playing, something loaded, and no
        // clock coming from the device.
        assert_eq!(App::clock_note(true, false, None, true), "wall clock — nothing rendered to play");
        assert_eq!(App::clock_note(true, false, None, false), "no clock — nothing loaded");
        assert_eq!(App::clock_note(false, true, Some(1), true), "audio clock", "the rack loop counts too");
    }

    #[test]
    fn the_menu_keys_the_hint_advertises_actually_open_the_palette() {
        // The corner hint has said "S-space menu" all along; the keymap
        // resolved Shift+Space and ':' but no arm handled either, so both
        // did nothing.
        use studio_egui::keys::Key;
        let mut s = Studio::demo();
        s.key(Key { ch: ' ', shift: true, ctrl: false, alt: false });
        assert!(s.palette_requested, "Shift+Space asks for the palette");
        s.palette_requested = false;
        s.key(Key::c(':'));
        assert!(s.palette_requested, "':' asks for the palette, as in Helix");
        s.palette_requested = false;
        s.key(Key::c(' '));
        assert!(!s.palette_requested && s.transport.playing, "plain space is still play");
    }

    #[test]
    fn space_toggles_the_transport_and_says_so() {
        let mut s = Studio::demo();
        assert!(!s.transport.playing);
        assert!(s.key(Key::c(' ')));
        assert!(s.transport.playing);
        assert_eq!(s.status, "playing");
        s.key(Key::c(' '));
        assert!(!s.transport.playing);
    }

    #[test]
    fn undo_reaches_the_project_through_the_shell() {
        let mut s = Studio::demo();
        let before = s.session.doc.project.clips.len();
        s.selection = vec![*s.session.doc.project.clips.keys().next().unwrap()];
        s.key(Key::c('d'));
        assert_eq!(s.session.doc.project.clips.len(), before - 1, "the delete landed");
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips.len(), before, "and undo put it back");
    }

    #[test]
    fn a_refused_edit_reports_rather_than_vanishing() {
        // The worst failure an editor has is a silently dropped edit: the user
        // believes it worked.
        let mut s = Studio::demo();
        s.edit(studio_core::Op::RemoveClip { id: studio_core::ClipId(9999) }, "delete");
        assert!(s.status.contains("refused"), "status was {:?}", s.status);
    }

    #[test]
    fn splitting_needs_something_under_the_playhead() {
        let mut s = Studio::demo();
        s.transport.set_position(Ticks::from_seconds(999.0));
        s.key(Key::c('s'));
        assert!(s.status.contains("nothing"), "status was {:?}", s.status);
        // and over a clip it works
        s.selection.clear();
        s.transport.set_position(Ticks::from_seconds(1.0));
        let before = s.session.doc.project.clips.len();
        s.key(Key::c('s'));
        assert_eq!(s.session.doc.project.clips.len(), before + 1, "split made a second clip");
    }

    #[test]
    fn select_all_then_clear() {
        let mut s = Studio::demo();
        s.key(Key::c('%'));
        assert_eq!(s.selection.len(), s.session.doc.project.clips.len());
        s.key(Key::c(';'));
        assert!(s.selection.is_empty());
    }
}


// ── the shell ───────────────────────────────────────────────────────────────

/// The eframe application. Thin by design: it draws chrome and hands the
/// timeline to `studio-egui`. Anything decidable lives above, where it can be
/// tested without a window.
#[cfg(feature = "app")]
/// What a background provenance/fingerprint job reports back. Gated the
/// same as `App` itself -- meaningless without it, since its only use is
/// inside `impl App`.
struct ProvenanceJobResult {
    source: studio_core::SourceId,
    /// The path this job actually hashed -- carried through so a
    /// successful hash can record WHERE it was verified, not just that it
    /// was.
    path: std::path::PathBuf,
    /// `None` if hashing failed (file vanished mid-job, permissions,
    /// etc.) -- reported as a status line, not silently dropped.
    hash: Option<([u8; 32], u64)>,
    /// `None` if fingerprinting failed OR was never attempted (only
    /// attempted for paths `library::is_video_path` recognizes -- hashing
    /// a plain audio file is still worth doing, fingerprinting it against
    /// a temporal-frame algorithm is not).
    fingerprint: Option<studio_core::FingerprintData>,
}

#[cfg(feature = "app")]
pub struct App {
    /// A Tab taken out of the input by `raw_input_hook`, for `ui()` to act on.
    tab_flip: bool,
    pub studio: Studio,
    peaks: std::collections::BTreeMap<studio_core::SourceId, studio_audio::Peaks>,
    /// Background provenance/fingerprint jobs for freshly-imported video
    /// clips: hashing a multi-gigabyte file and fingerprinting it (real
    /// `ffmpeg` calls per sample -- seconds, not milliseconds, on real
    /// footage, per this project's own measurements) must never run on the
    /// UI thread. `provenance_tx` is cloned into each spawned job;
    /// `provenance_rx` is drained once per frame in `drain_provenance`.
    provenance_tx: std::sync::mpsc::Sender<ProvenanceJobResult>,
    provenance_rx: std::sync::mpsc::Receiver<ProvenanceJobResult>,
    /// Decoded / synthesised audio per source — bounces and imports land
    /// here; the mixdown and the sampler both read it.
    audio: std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    sink: Box<dyn audition::PlaybackSink>,
    /// Wall-clock epoch used ONLY while the arrangement has no audio to
    /// report a position from. `None` whenever the sink is driving the
    /// playhead, which is the normal case; see `sync_sink`.
    silent_clock: Option<std::time::Instant>,
    /// The wire, when one was asked for: STUDIO_LISTEN=addr:port waits for a
    /// peer, STUDIO_CONNECT=addr:port dials one. Env vars, not a dialog —
    /// the same predictable-beats-configurable trade the export path makes,
    /// and this shell's whole collab stack works identically without it.
    carrier: Option<studio_collab::stream::StreamPeer>,
    /// Where Ctrl+S writes: STUDIO_PROJECT if set, else a project
    /// directory under the projects root.
    project_path: std::path::PathBuf,
    /// The MIDI controller, if one was found at startup. `None` is normal
    /// and silent: most sessions have no controller, and an editor that
    /// complains about it every launch trains people to ignore its status
    /// line.
    midi: Option<midi::MidiIn>,
    /// Notes currently held on the controller, so a note-off can end the
    /// right one — the rack's voices self-retire, but a sustaining synth
    /// (and MIDI-to-pattern recording) needs the pairing.
    midi_held: std::collections::BTreeMap<u8, ()>,
    /// The sound library root and its scan — rescanned only on demand (the
    /// refresh button), not per frame: a directory walk per frame on a
    /// phone's flash is how battery dies.
    library_root: std::path::PathBuf,
    library_roots: library::Roots,
    library_tree: library::TreeState,
    /// Per-project sample triage — pinned floats, demoted sinks. Lives in
    /// the workspace sidecar, not the op log: a pin is how one person
    /// keeps their head straight, not a fact about the music.
    library_prefs: library::Prefs,
    morph: morph_pad::PadState,
    bounce_format: morph_pad::BounceFormat,
    /// The mastering pass's live state: what was measured, what is
    /// proposed, and at what strength — held between frames so the
    /// findings can be READ and the dial moved before anything applies.
    mix_pass: Option<MixPassUi>,
    /// The command palette, open when `Some`. C-p opens it; it holds the
    /// wizard's position and the current query between frames.
    palette: Option<palette::Palette>,
    /// "join session": the id being typed, plus what the LAN is showing.
    join: Option<JoinDialog>,
    /// Text to hand the clipboard on the next frame (acts run without a ctx).
    clipboard_pending: Option<String>,
    /// The LAN beacon: announces our session, collects everyone else's.
    lan: studio_collab::lan::Discovery,
    /// The voice dialog. Present when voice is engaged; the popup renders
    /// from its state, and utterances run through the SAME palette acts
    /// the keyboard uses — no second command system.
    voice: Option<voice::Dialog>,
    /// The vocabularies the fastpath listens with — kept on the App (not
    /// in the Dialog) so a dialog can be opened and closed without
    /// forgetting which genres this device works in.
    lexicon: voice::Lexicon,
    /// The actual microphone — everything in `voice.rs`/`voice_utterance`
    /// is capture-agnostic by design ("the seam a speech backend calls,"
    /// per that function's own doc), and until now nothing called it with
    /// real audio: the Dialog state machine, the palette-act dispatch, and
    /// `feedback_popup`'s drawing were all complete and tested headless,
    /// but Ctrl+Space only ever opened an empty `Dialog` sitting at
    /// `State::Listening` forever. This is that missing seam, built on
    /// `voice-engine-kit` the same way voiceterm-egui's popup is —
    /// spawned on a background thread on open, polled here, fed into
    /// `voice_utterance` once text is ready.
    voice_capture: std::sync::Arc<std::sync::Mutex<voice::VoiceCapturePending>>,
    /// The find panel: how to search, what was typed, and what came back.
    find_ui: Option<FindUi>,
    /// Whose viewport this device is following, by collaborator name.
    ///
    /// LOCAL, and not in the project or the op log: what I am looking at
    /// is not part of the document, and syncing it would let one person
    /// drag everybody else's screen around.
    follow: Option<String>,
    mic: Box<dyn capture::Capture>,
    /// The in-house voice link: off until joined, pumped every frame the
    /// wire is polled. See `call`.
    call: call::Call,
    /// Loop-record's pass detector, armed while recording with a loop
    /// range. See `looping`.
    passes: looping::Passes,
    /// Where the take started, so it lands where it was sung.
    take_from: studio_core::Ticks,
    settings_open: bool,
    /// The grouping graph debug panel (`ShellVerb::ShowGroupGraph`).
    group_graph_open: bool,
    library_open: bool,
    /// Pressed→effective key translations, applied to raw typed input
    /// only (see the module docs for why the palette's keys bypass it).
    rebinds: rebinds::Rebinds,
    /// The settings popup is waiting for the next typed key to bind to
    /// this action's default char. Consumes exactly one keypress.
    rebind_capture: Option<char>,
    /// Calibrated piano zones for playing mode; empty = tracker layout.
    zones: studio_egui::playkeys::Zones,
    /// The zone calibration walk, when running. Typed keys land on the
    /// current note; Enter advances; closing Settings saves and stops.
    piano_cal: Option<studio_egui::playkeys::Calibration>,
    /// Pages-mode over-scroll navigation — the detent between panels on a
    /// screen too narrow for tabs. Fed only from the CENTRAL panel's
    /// scroll report, only when the nav actually chose Pages.
    panel_nav: studio_egui::touch::PanelNav,
    /// The export dialog's plan while it is open, plus its last refusal —
    /// shown inline so a bad plan is a message, not a closed window.
    export_dialog: Option<export::Plan>,
    export_note: String,
    /// A render in flight, drained once per frame into the tracker.
    export_run: Option<export::Run>,
    /// Which band every waveform is drawn from. Per device and per session:
    /// it is a way of LOOKING at the audio, like a zoom level, not a fact
    /// about the project.
    peak_band: studio_audio::Band,
    /// Audio arriving from the background extraction of video sources, and
    /// how many are still to come — for the status line.
    video_audio_rx: Option<std::sync::mpsc::Receiver<(studio_core::SourceId, Option<studio_audio::SourceAudio>)>>,
    video_audio_left: usize,
    /// Where background workers find the egui context to wake the window.
    ui_waker: std::sync::Arc<std::sync::Mutex<Option<egui::Context>>>,
    /// What was said in each source — whisper transcripts, loaded from a
    /// folder of `.srt` files beside the media or picked in the palette.
    /// Session state: a transcript is a reading of the media, rebuildable,
    /// and not a fact the project should carry or collaborators should sync.
    words: std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>,
    /// The transcripts as loaded; `words` is this through `word_filter`.
    /// Kept apart so switching filters never re-reads a file and never
    /// loses what the previous filter hid.
    words_raw: std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>,
    word_filter: studio_voice::WordFilter,
    /// What each pattern clip's cache was rendered from (pattern_clips).
    pattern_stamps: std::collections::BTreeMap<studio_core::SourceId, pattern_clips::Stamp>,
    /// The filmstrip's decoded frames and the worker that decodes them.
    thumbs: thumbs::Cache,
    /// The project as it was last opened or saved. "Unsaved changes" means
    /// the live project differs from it -- the question asked before
    /// switching to another project, so a switch never loses work and never
    /// rewrites a file (a demo fixture, say) that was only looked at.
    saved_project: Option<studio_core::Project>,
    /// A project waiting to be opened while the unsaved-changes question is asked.
    open_pending: Option<std::path::PathBuf>,
    /// The track-points window, open over the selected clip's first frame.
    track_points: Option<track_points::Window>,
    /// A finger has touched the screen this session: tool windows grow their
    /// targets to fingertip size even on a wide screen (a touch laptop).
    touch_seen: bool,
    /// The comic setup window, open over a camera group.
    comic_setup: Option<comic_job::SetupWindow>,
    /// A comic render in flight.
    comic_run: Option<comic_job::Job>,
    /// The signals window, open over one camera group.
    signals_panel: Option<signals_panel::Panel>,
    /// The look copied by "style: copy", waiting to be pasted.
    style_ref: Option<studio_video::style::Stats>,
    /// The kaleidoscope gallery, open over the selected clip's frame.
    kaleido_gallery: Option<kaleido_gallery::Gallery>,
    /// Structure view: sections across, lanes down, instead of one row per
    /// track. View state, per session — a way of looking, like zoom.
    structure_view: bool,
    /// The camera group the program monitor is showing as a grid, if any.
    multiview: Option<studio_core::GroupId>,
    /// A stabilize/clean-plate render in flight, and the clip it belongs to.
    touchup_run: Option<touchup_job::Job>,
    export_done: Vec<std::path::PathBuf>,
    export_progress: (usize, usize, f32),
    /// The clip a bscale bounce belongs to while its run is in flight;
    /// landed from the tracker's Done.
    upscale_clip: Option<studio_core::ClipId>,
    upscale_settings: upscale::Settings,
    /// The archive popup's working state while open.
    archive_ui: Option<ArchiveUi>,
    /// The workbench's right slot: the viewport (default) or another tool,
    /// or nothing. The music-video loop is the point: the frame under the
    /// playhead beside the rack, instead of export → import → watch.
    side_slot: SideSlot,
    frame_cache: viewport::FrameCache,
    /// The program monitor above the timeline: the frame under the playhead from the
    /// topmost video track — the same clip choice the export makes (`viewport::video_at`)
    /// — with the arrangement's audio already coming through the mixdown. Toggled from
    /// the bar; the workbench's viewport side slot is the same picture in another place.
    preview_open: bool,
    /// `None` until the person picks a key — autotune has no sensible
    /// default scale, and silently tuning to C minor because nobody chose
    /// anything is a worse failure than making "tune take" wait.
    tune_scale: Option<(u8, studio_fx::retune::Mode)>,
    /// Velocity source for playing mode. `NullAnalog` today; a Wooting
    /// binding drops in here (see playkeys' docs).
    analog: Box<dyn studio_egui::playkeys::AnalogSource>,
    /// What the sink's buffer currently holds, so a mode switch (loop ↔
    /// arrangement) knows to reload even when nothing is dirty.
    loaded: SinkContent,
}

/// What should move the playhead when the sink has no position to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SilentFallback {
    /// The sink is reporting a position; audio wins, as it normally does.
    SinkDrives,
    /// A buffer is loaded but has no audible content. Run a wall clock.
    WallClock,
    /// Nothing is loaded at all -- genuinely the end.
    Stop,
}

#[derive(PartialEq, Clone, Copy)]
enum SinkContent {
    Nothing,
    RackLoop,
    Arrangement,
    /// The arrangement's loop range alone, rendered and set looped — see
    /// `looping::Window`. Carries the range so a changed loop reloads.
    ArrangementLoop(looping::Window),
}

#[cfg(feature = "app")]
impl Default for App {
    fn default() -> Self { App::with_project(default_project_path()) }
}

/// Never `./project.studio` any more: an unnamed project gets its OWN
/// directory under the projects root (see project_dir's containment rule and
/// the accident that motivated it). The root is $STUDIO_PROJECTS, else the
/// last place an archive was written -- a person who archives to an SD card
/// wants the working copy beside it -- else the freedesktop data directory.
#[cfg(feature = "app")]
fn default_project_path() -> std::path::PathBuf {
    // a project named on the command line wins over STUDIO_PROJECT
    // (`studio film.studio`, or `studio fixtures/cartoon-fragile`); `run`
    // has already handled --help and anything it refused
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let open_project::Launch::Project(p) = open_project::from_args(&args) { return p; }
    match std::env::var("STUDIO_PROJECT") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => {
            let root = last_archive_root().unwrap_or_else(project_dir::projects_root);
            project_dir::project_dir(&root, "untitled").join("project.studio")
        }
    }
}

#[cfg(feature = "app")]
impl App {
    /// The shell, opened on `project_path`: the same path a launch with
    /// STUDIO_PROJECT takes, and what "open project" switches to -- so a
    /// project opened from the palette is built exactly like one opened at
    /// start-up, with nothing of the previous project left behind.
    pub fn with_project(project_path: std::path::PathBuf) -> App {
        let sink = audition::open_sink();
        // Open the named project when it exists; a demo otherwise. A load
        // FAILURE is neither: overwriting a damaged-but-real file with a
        // demo on the next Ctrl+S would finish what the damage started, so
        // the shell starts demo'd but says exactly why.
        let mut load_note = None;
        let is_archive = project_path.extension()
            .and_then(|e| e.to_str()) == Some("studioarch");
        let mut studio = if project_path.exists() {
            let team = Team::from_contacts("solo", &["sc-local".into()]);
            match if is_archive { Studio::open_archive(&project_path, SiteId(1), team) }
                  else { Studio::load_from(&project_path, SiteId(1), team) } {
                Ok(s) => s,
                Err(e) => { load_note = Some(e); Studio::demo() }
            }
        } else {
            Studio::demo()
        };
        // The per-device layout sidecar: applied after the project so a
        // failed project load still gets a sane default layout, and view
        // state is never the reason a project fails to open.
        let ws = workspace::load(&project_path);
        studio.combined = ws.combined;
        if let Some(p) = studio_egui::touch::Panel::ALL.iter()
            .find(|p| p.label() == ws.panel) { studio.panel = *p; }
        studio.view.px_per_sec = ws.px_per_sec;
        studio.view.scroll = studio_core::Ticks(ws.scroll_ticks);
        studio.overlap = studio_core::Overlap::parse(&ws.overlap).unwrap_or_default();
        studio.overlap_tracks = ws.overlap_tracks.split(',')
            .filter_map(|kv| { let (t, p) = kv.split_once(':')?;
                Some((studio_core::TrackId(t.trim().parse().ok()?), studio_core::Overlap::parse(p)?)) })
            .collect();
        studio.closed_groups = ws.closed_groups.iter()
            .map(|g| studio_core::GroupId(*g)).collect();
        let bounce_format = morph_pad::BounceFormat::from_token(&ws.bounce_format);
        let lexicon = voice::Lexicon::from_tokens(&ws.voice_domains);
        let library_prefs = library::Prefs {
            pinned: ws.pinned.iter().map(std::path::PathBuf::from).collect(),
            demoted: ws.demoted.iter().map(std::path::PathBuf::from).collect(),
        };
        studio.display_name = ws.display_name.clone();
        studio.status = match load_note {
            Some(e) => format!("{e} — started fresh (Ctrl+S will overwrite!)"),
            None => format!("{} — audition: {}", studio.status, sink.describe()),
        };
        let carrier = if let Ok(addr) = std::env::var("STUDIO_LISTEN") {
            match studio_collab::stream::StreamPeer::listen(&addr) {
                Ok(p) => {
                    studio.status = format!("{} — listening on {}",
                        studio.status, p.local.map(|a| a.to_string()).unwrap_or(addr));
                    Some(p)
                }
                Err(e) => { studio.status = format!("listen {addr} failed: {e}"); None }
            }
        } else if let Ok(addr) = std::env::var("STUDIO_CONNECT") {
            match studio_collab::stream::StreamPeer::connect(&addr) {
                Ok(p) => { studio.status = format!("{} — dialing {addr}", studio.status); Some(p) }
                Err(e) => { studio.status = format!("connect {addr} failed: {e}"); None }
            }
        } else {
            None
        };
        let library_root = project_dir::library_root();
        let library_tree = library::TreeState::default();
        let midi = midi::MidiIn::open(std::env::var("STUDIO_MIDI").ok().as_deref()).ok();
        let analog = WootingAnalog::default();
        studio.status = format!("{} — {}", studio.status, analog.0.describe());
        if let Some(m) = &midi {
            studio.status = format!("{} — midi: {}", studio.status, m.port_name());
        }
        let (provenance_tx, provenance_rx) = std::sync::mpsc::channel();
        let beside = transcripts::beside_media(&studio.session.doc.project,
            project_path.parent().unwrap_or(std::path::Path::new(".")));
        let pending_video = std::mem::take(&mut studio.pending_video_audio);
        let video_audio_left = pending_video.len();
        // The worker wakes the window through this slot; the first frame fills it.
        let ui_waker: std::sync::Arc<std::sync::Mutex<Option<egui::Context>>> = Default::default();
        {
            let w = ui_waker.clone();
            studio.waker = std::sync::Arc::new(move || { if let Some(c) = w.lock().ok().and_then(|g| g.clone()) { c.request_repaint(); } });
        }
        let video_audio_rx = (!pending_video.is_empty()).then(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            let w = ui_waker.clone();
            std::thread::spawn(move || Studio::extract_video_audio(pending_video, tx, move || {
                if let Some(c) = w.lock().ok().and_then(|g| g.clone()) { c.request_repaint(); }
            }));
            rx
        });
        let thumb_cache = {
            let w = ui_waker.clone();
            thumbs::Cache::new(std::sync::Arc::new(move || { if let Some(c) = w.lock().ok().and_then(|g| g.clone()) { c.request_repaint(); } }))
        };
        let mut app = App { studio, midi, midi_held: Default::default(), peaks: Default::default(), audio: Default::default(), sink,
            provenance_tx, provenance_rx,
            follow: None,
              carrier, project_path,
              // The sidecar wins over the environment when it has an answer:
              // a person who set the libraries in Settings expects them to
              // still be set next launch, and a launcher script's env var is
              // the DEFAULT, not an override of what they chose.
              library_roots: {
                  let mut r = library::Roots::from_env(library_root.clone());
                  if !ws.library_audio.is_empty() { r.audio = ws.library_audio.clone().into(); }
                  if !ws.library_video.is_empty() { r.video = Some(ws.library_video.clone().into()); }
                  r
              },
              library_root, library_tree, library_prefs,
              morph: morph_pad::PadState::default(),
              bounce_format,
              mix_pass: None,
              palette: None,
              join: None,
              clipboard_pending: None,
              lan: studio_collab::lan::Discovery::open(),
              voice: None,
              voice_capture: Default::default(),
              lexicon,
              find_ui: None,
              mic: capture::open_capture(),
              call: call::Call::new(),
              passes: looping::Passes::default(),
              take_from: studio_core::Ticks::ZERO,
              settings_open: false, group_graph_open: false, library_open: ws.library_open,
              rebinds: rebinds::Rebinds::decode(&ws.rebinds), rebind_capture: None, tab_flip: false,
              panel_nav: Default::default(),
              zones: studio_egui::playkeys::Zones::decode(&ws.piano_zones),
              piano_cal: None,
              archive_ui: None,
              side_slot: match ws.side_slot.as_str() {
                  "off" => SideSlot::Off,
                  "mixer" => SideSlot::Mixer,
                  "rack" => SideSlot::Rack,
                  "program" => SideSlot::Program,
                  _ => SideSlot::Viewport,
              },
              frame_cache: viewport::FrameCache::default(),
              preview_open: true,
              export_dialog: None,
              export_note: String::new(),
              export_run: None,
              touchup_run: None,
              multiview: None,
              peak_band: studio_audio::Band::Full,
              video_audio_rx, video_audio_left, ui_waker,
              // Whisper writes a transcript beside its media by default, so
              // look there first — no command needed for the common case.
              words: beside.words.clone(),
              words_raw: beside.words,
              word_filter: studio_voice::WordFilter::All,
              structure_view: false,
              kaleido_gallery: None,
              style_ref: None,
              signals_panel: None,
              comic_run: None,
              comic_setup: None,
              touch_seen: false,
              track_points: None,
              export_done: Vec::new(),
              export_progress: (0, 0, 0.0),
              upscale_clip: None,
              upscale_settings: upscale::Settings::default(),
              tune_scale: None,
              analog: Box::new(analog),
              loaded: SinkContent::Nothing, silent_clock: None, saved_project: None, open_pending: None,
              thumbs: thumb_cache, pattern_stamps: Default::default() };
        // the project as opened: "unsaved changes" means different from this
        app.saved_project = Some(app.studio.session.doc.project.clone());
        app
    }
}

#[cfg(feature = "app")]
impl eframe::App for App {
    /// Tab on the two views of a pattern flips between them. Taken here,
    /// before egui begins the frame, because egui moves keyboard focus on Tab
    /// at that point (see `seq_header::take_tab`). Only there, and only with
    /// nothing focused: in a text field, or on any other tab, Tab keeps its
    /// meaning of moving focus.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if seq_header::other_view(self.studio.panel).is_some() && ctx.memory(|m| m.focused().is_none()) {
            self.tab_flip |= seq_header::take_tab(&mut raw_input.events);
        }
    }

    // eframe 0.34's App trait is `ui(&mut self, ui, frame)`, not the `update(ctx, frame)`
    // of earlier versions — the panels come from the Ui's context.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        // Keys first, so a shortcut is never eaten by a widget holding
        // focus — EXCEPT a text field, which is the one widget whose
        // whole job is to eat them.
        //
        // Reading `Event::Text` unconditionally is why space typed into a
        // track name also started the transport: the field inserted the
        // character AND this loop turned it into a shortcut. The old
        // comment describes the intent exactly, and the intent is wrong
        // for text — "never eaten" and "typing works" cannot both hold.
        //
        // `Event::Text` is BY DEFINITION unmodified text (a chord never
        // arrives this way, which the C-p handler below already relies
        // on), so when a field is taking input this stream belongs
        // entirely to it. The modified-chord path underneath is
        // unaffected and still fires while typing, which is what makes
        // Ctrl-P keep working in a rename box.
        let typed: Vec<char> = if uikit_egui::typing_guard::text_has_focus(ctx) {
            Vec::new()
        } else {
            ctx.input(|i| i.events.iter().flat_map(|e| match e {
                egui::Event::Text(t) => t.chars().collect::<Vec<_>>(),
                _ => Vec::new(),
            }).collect())
        };
        // C-p, read from the key events rather than the text stream: a
        // modified chord never arrives as Event::Text. Consumed here so no
        // widget with focus can eat it.
        // C-r records. NOT plain 'r': the keymap already owns that, and
        // the palette's drift test said so out loud the moment this tried
        // to take it — exactly the collision that guard exists to catch.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::R)) {
            self.toggle_take();
        }
        // Tab flips a pattern between its two views (Steps / Notes); the key
        // was taken out of the input in `raw_input_hook`, before egui could
        // move focus with it.
        if std::mem::take(&mut self.tab_flip) {
            if let Some(other) = seq_header::other_view(self.studio.panel) {
                self.studio.panel = other;
                self.studio.status = format!("{} view", seq_header::view_name(other));
            }
        }
        if std::mem::take(&mut self.studio.palette_requested) && self.palette.is_none() {
            self.palette = Some(palette::Palette::open_with(self.words_raw.clone()));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            self.palette = match self.palette {
                Some(_) => None, // C-p again closes: a toggle, not a stack
                None => Some(palette::Palette::open_with(self.words_raw.clone())),
            };
        }
        // C-1..C-4: the workspaces. One keypress per room, the kdenlive
        // habit; the palette lists them with these hints so the shortcut
        // teaches itself.
        for (i, key) in [egui::Key::Num1, egui::Key::Num2,
                         egui::Key::Num3, egui::Key::Num4].iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::COMMAND, *key)) {
                self.apply_layout(i);
            }
        }
        // While the palette is open it owns the keyboard — typing 'd' is a
        // search, not a delete.
        let typed: Vec<char> = if self.palette.is_some() { Vec::new() } else { typed };
        // A running piano calibration eats every typed key: each one joins
        // the current note's region. Enter advances the note; closing
        // Settings (below) saves and stops.
        let typed: Vec<char> = if let Some(cal) = &mut self.piano_cal {
            for ch in &typed { cal.press(*ch); }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                cal.advance();
            }
            self.studio.status = format!("calibrating: press key(s) for {}, \
                                          Enter for the next note", cal.note_name());
            Vec::new()
        } else { typed };
        // A pending rebind capture eats exactly one key — the next thing
        // typed IS the answer to "press the new key", not an edit.
        let typed: Vec<char> = if let Some(target) = self.rebind_capture {
            if let Some(ch) = typed.first() {
                self.rebinds.set(*ch, target);
                self.rebind_capture = None;
                self.studio.status = if *ch == target {
                    format!("{target:?} back to default")
                } else {
                    format!("{ch:?} now does what {target:?} did")
                };
            }
            Vec::new()
        } else { typed };
        for ch in typed {
            // Rebinds translate RAW input only, before the intercepted keys
            // and the keymap alike see it — one table, both consumers.
            let ch = self.rebinds.resolve(ch);
            // The alignment keys, intercepted before the Helix keymap —
            // they need `self.audio` (the keymap cannot) and their chars
            // are pinned unbound in the keymap by test. Every one of them
            // appears in `studio_egui::CLIP_HINTS`, also by test: a
            // shortcut the timeline does not show is one nobody learns.
            let step = studio_core::Ticks(self.studio.rack.tempo().step_len().0);
            let ms = studio_core::Ticks::from_seconds(0.001);
            match ch {
                'A' => { self.studio.align_selected(&self.audio); continue; }
                'n' => { self.studio.normalize_selected(&self.audio); continue; }
                'D' => { self.studio.duplicate_selected(); continue; }
                '[' => { self.studio.fade_to_playhead(true); continue; }
                ']' => { self.studio.fade_to_playhead(false); continue; }
                'Q' => { self.studio.quantize_selected(); continue; }
                ',' => { self.studio.nudge_selected(studio_core::Ticks(-step.0)); continue; }
                '.' => { self.studio.nudge_selected(step); continue; }
                '<' => { self.studio.nudge_selected(studio_core::Ticks(-ms.0)); continue; }
                '>' => { self.studio.nudge_selected(ms); continue; }
                'g' if self.studio.selection.len() >= 2 => {
                    let id = studio_core::GroupId(
                        self.studio.session.doc.project.groups.keys()
                            .map(|x| x.0).max().unwrap_or(0) + 1);
                    let n = self.studio.session.doc.project.groups.len() + 1;
                    self.studio.edit(studio_core::Op::SetGroup {
                        id, name: format!("comp {n}"),
                        clips: self.studio.selection.clone() }, "grouped");
                    // Born CLOSED: the point of grouping is the fold.
                    self.studio.closed_groups.insert(id);
                    continue;
                }
                'G' => {
                    let selected: Vec<_> = self.studio.session.doc.project.groups.iter()
                        .filter(|(_, g)| g.clips.iter()
                            .any(|c| self.studio.selection.contains(c)))
                        .map(|(id, _)| *id).collect();
                    if selected.is_empty() {
                        self.studio.status = "ungroup: select a composition's clips first".into();
                    }
                    for id in selected {
                        self.studio.edit(studio_core::Op::RemoveGroup { id }, "ungrouped");
                        self.studio.closed_groups.remove(&id);
                    }
                    continue;
                }
                _ => {}
            }
            self.studio.key(studio_egui::keys::Key::c(ch));
        }
        // `r` in the keymap: the take lives here, with the microphone.
        if std::mem::take(&mut self.studio.take_requested) {
            self.toggle_take();
        }

        // ctx variant: same colours, plus the symbol fonts. studio draws a
        // lot of glyphs and had none of them — nine files here use
        // PaletteColors and nothing installed a font. See
        // maintenance/tools/font_install_check.py.
        let colors = uikit_egui::palette_colors_ctx(ctx, "midnight");
        // Whether the top bar chose Pages this frame — read by the central
        // panel below to gate over-scroll navigation. A local, not a field:
        // it is one frame's fact, recomputed from real width every time.
        let mut pages_mode = false;
        egui::Panel::top("bar").show(ctx, |ui| {
            // WRAPPED, not one row: on a phone-width window the single row ran
            // off the right edge and took `film`, `save`, export and every
            // tool with it -- and the palette's shortcut needs a keyboard a
            // phone does not have. Wrapping keeps every control reachable.
            ui.horizontal_wrapped(|ui| {
                // Tabs when the labels actually fit, a compact picker when
                // they don't — decided from a WIDTH FIT via responsive_nav,
                // not a device-class guess (its module doc explains why the
                // guess is wrong in both directions). The reserve keeps the
                // fit honest: the transport half of this bar needs room too,
                // and "the tabs fit" while play/timecode wrap would be a
                // lie about the same width.
                const BAR_RESERVE: f32 = 260.0;
                let fit = uikit_egui::responsive_nav::choose(
                    (ui.available_width() - BAR_RESERVE).max(0.0),
                    Panel::ALL.len(),
                    uikit_egui::responsive_nav::DEFAULT_MIN_TAB_WIDTH);
                pages_mode = fit == uikit_egui::responsive_nav::NavLayout::Pages;
                match fit {
                    uikit_egui::responsive_nav::NavLayout::Tabs => {
                        for p in Panel::ALL {
                            if ui.add(egui::Button::selectable(self.studio.panel == p, p.label())).clicked() {
                                self.studio.panel = p;
                            }
                        }
                    }
                    uikit_egui::responsive_nav::NavLayout::Pages => {
                        // One panel at a time on a screen this narrow; the
                        // picker is the route between them. (The swipe layer
                        // — snapclone's over-scroll detents — is the named
                        // next step; the picker is the floor, so no panel is
                        // ever unreachable. ambient-learner's lesson.)
                        egui::ComboBox::from_id_salt("panel-pick")
                            .selected_text(self.studio.panel.label())
                            .show_ui(ui, |ui| {
                                for p in Panel::ALL {
                                    ui.selectable_value(&mut self.studio.panel, p, p.label());
                                }
                            });
                    }
                }
                ui.separator();
                if ui.button(if self.studio.transport.playing { "pause" } else { "play" }).clicked() {
                    self.studio.key(studio_egui::keys::Key::c(' '));
                }
                ui.label(studio_egui::format_timecode(self.studio.playhead()));
                ui.separator();
                ui.label(self.studio.mode.label());
                ui.separator();
                ui.separator();
                // Exports go to ./exports, named after the project; the
                // status line prints the exact path. No dialog on purpose:
                // predictable beats configurable for the ship-a-beat loop,
                // and a picker can arrive later without moving this seam.
                let m = self.studio.input_mode;
                if ui.add(egui::Button::selectable(
                        m == studio_egui::playkeys::InputMode::Playing, m.label()))
                    .on_hover_text("motions ⇄ playing (Esc leaves playing)").clicked()
                {
                    self.studio.input_mode = m.toggled();
                }
                if m == studio_egui::playkeys::InputMode::Playing {
                    ui.label("oct");
                    ui.add(egui::DragValue::new(&mut self.studio.octave).range(-2..=2));
                }
                // Print-side autotune on the selected clip. Key and mode
                // live here rather than in a dialog: two clicks from
                // noticing a flat take to hearing a tuned one.
                if !self.studio.selection.is_empty() {
                    egui::ComboBox::from_id_salt("tune-key")
                        .selected_text(match self.tune_scale {
                            None => "no key".to_string(),
                            Some((root, mode)) => format!("{} {}",
                                ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"]
                                    [root as usize % 12],
                                match mode {
                                    studio_fx::retune::Mode::Minor => "min",
                                    studio_fx::retune::Mode::HarmonicMinor => "harm",
                                    studio_fx::retune::Mode::Chromatic => "chrom",
                                }),
                        })
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            // No default scale: the picker opens on "no
                            // key" rather than pre-selecting one, so
                            // choosing IS the action, not un-choosing
                            // whatever happened to be selected already.
                            ui.selectable_value(&mut self.tune_scale, None, "no key");
                            ui.separator();
                            let mode = self.tune_scale.map(|(_, m)| m)
                                .unwrap_or(studio_fx::retune::Mode::Minor);
                            for (i, n) in ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"]
                                .iter().enumerate() {
                                let picked = self.tune_scale.is_some_and(|(r, _)| r == i as u8);
                                if ui.selectable_label(picked, *n).clicked() {
                                    self.tune_scale = Some((i as u8, mode));
                                }
                            }
                            ui.separator();
                            for (m, n) in [(studio_fx::retune::Mode::Minor, "minor"),
                                           (studio_fx::retune::Mode::HarmonicMinor, "harmonic"),
                                           (studio_fx::retune::Mode::Chromatic, "chromatic")] {
                                let picked = self.tune_scale.is_some_and(|(_, sm)| sm == m);
                                if ui.selectable_label(picked, n).clicked() {
                                    // Picking a mode with no root yet reads
                                    // as "chromatic" defaulting to C, which
                                    // is the one case a mode choice alone
                                    // can mean something (chromatic has no
                                    // key); any other mode still needs a
                                    // root before "tune take" enables.
                                    let root = self.tune_scale.map(|(r, _)| r).unwrap_or(0);
                                    self.tune_scale = Some((root, m));
                                }
                            }
                        });
                    let scale = self.tune_scale;
                    if ui.add_enabled(scale.is_some(), egui::Button::new("tune take"))
                        .on_hover_text(if scale.is_some() {
                            "print-side autotune → a new clip beside the dry one".to_string()
                        } else {
                            "pick a key first — there is no default scale".to_string()
                        })
                        .clicked()
                    {
                        if let Some((root, mode)) = scale {
                            let scale = studio_fx::retune::Scale { root, mode };
                            if let Some((id, audio)) = self.studio.autotune_selected(&self.audio, scale) {
                                self.peaks.insert(id, self.build_peaks(&audio));
                                if let Ok(bytes) = studio_audio::export::export_wav_dithered_bytes(
                                    &audio.channels[0], audio.channels.len() as u16,
                                    audio.sample_rate, id.0)
                                {
                                    self.studio.register_asset(id, &bytes);
                                }
                                self.audio.insert(id, audio);
                            }
                        }
                    }
                }
                // bscale clip bounce: same shape as tune take (a whole-
                // clip pass landing a new clip), through the export tracker.
                if !self.studio.selection.is_empty() && self.export_run.is_none() {
                    let busy = self.export_run.is_some();
                    if ui.add_enabled(!busy, egui::Button::new("upscale clip"))
                        .on_hover_text("bscale: block-matched Real-ESRGAN upscale of the selected clip's range, to a new clip beside it (D2X_MODEL selects the model; default is the fast anime-video net)")
                        .clicked()
                    {
                        let settings = self.upscale_settings.clone();
                        if let Some((clip, run)) = self.studio.upscale_selected(&settings) {
                            self.export_done.clear();
                            self.export_progress = (0, 1, 0.0);
                            self.upscale_clip = Some(clip);
                            self.export_run = Some(run);
                        }
                    }
                }
                // Film: the ingest and touch-up verbs the palette already
                // carries, as buttons, because a person who does not know
                // the palette's words cannot search for a command they have
                // never heard of. Each one is the SAME shell verb the
                // palette raises, so the two can never drift.
                let film_layout = studio_egui::touch::ToolLayout::for_screen(ui.ctx().content_rect().width(), self.touch_seen);
                ui.menu_button("film", |ui| {
                    // fingertip rows on a phone or wherever a finger has touched
                    film_layout.apply(ui);
                    let selected = !self.studio.selection.is_empty();
                    let busy = self.touchup_run.is_some();
                    if ui.add_enabled(selected && !busy, egui::Button::new("stabilize — smooth-cam"))
                        .on_hover_text("keep the camera move, lose the shake")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(
                            palette::ShellVerb::TouchUpStabilize(palette::StabilizeKind::Smooth)));
                        ui.close();
                    }
                    if ui.add_enabled(selected && !busy, egui::Button::new("stabilize — still-cam"))
                        .on_hover_text("lock off a shot that was handheld by accident")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(
                            palette::ShellVerb::TouchUpStabilize(palette::StabilizeKind::Still)));
                        ui.close();
                    }
                    if ui.add_enabled(selected && !busy, egui::Button::new("stabilize — punch-in and de-jerk"))
                        .on_hover_text("a deliberate pan with a bump in it; the least crop")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(
                            palette::ShellVerb::TouchUpStabilize(palette::StabilizeKind::Dejerk)));
                        ui.close();
                    }
                    if ui.add_enabled(selected, egui::Button::new("kaleidoscope gallery…"))
                        .on_hover_text("every option drawn on this clip's frame, arranged by wedges, turn and centre")
                        .clicked() {
                        self.open_kaleido_gallery();
                        ui.close();
                    }
                    if ui.add_enabled(selected && !busy, egui::Button::new("rescue: follow the motion (1.6x)"))
                        .on_hover_text("punch in on what moves and follow it — covers a weak take with the gesture")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(palette::ShellVerb::TouchUpFollow { zoom_pct: 160 }));
                        ui.close();
                    }
                    if ui.add_enabled(selected && !busy, egui::Button::new("clean plate (±6 frames)"))
                        .on_hover_text("remove something crossing the shot, using the frames around it")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(
                            palette::ShellVerb::TouchUpCleanPlate { radius: 6, moving: false }));
                        ui.close();
                    }
                    ui.separator();
                    let groups: Vec<(studio_core::GroupId, String)> = self.studio.session.doc.project
                        .groups.iter()
                        .filter(|(id, _)| studio_video::multicam::from_group(
                            &self.studio.session.doc.project, **id).is_ok())
                        .map(|(id, g)| (*id, g.name.clone())).collect();
                    ui.add_enabled_ui(!groups.is_empty(), |ui| {
                        ui.menu_button("multiview a camera group", |ui| {
                            for (id, name) in &groups {
                                let on = self.multiview == Some(*id);
                                if ui.add(egui::Button::selectable(on, name)).clicked() {
                                    self.run_palette_act(palette::Act::Shell(
                                        palette::ShellVerb::Multiview(if on { None } else { Some(*id) })));
                                    ui.close();
                                }
                            }
                        });
                    });
                    if groups.is_empty() {
                        ui.colored_label(egui::Color32::GRAY,
                            "no camera group yet — group the angles of one moment first");
                    }
                    ui.separator();
                    if ui.add(egui::Button::selectable(self.structure_view, "structure view (whole project)"))
                        .on_hover_text("sections across, lanes down — for projects too big to read track by track")
                        .clicked() {
                        self.structure_view = !self.structure_view;
                        ui.close();
                    }
                    if ui.add_enabled(!groups.is_empty() && self.comic_run.is_none(), egui::Button::new("comic: auto layout a camera group…"))
                        .on_hover_text("wide, page, inset and split from the dialogue; sides from who each mic hears; renders then lands on a new track")
                        .clicked() {
                        if let Some((g, _)) = groups.first() { self.open_comic_setup(*g); }
                        ui.close();
                    }
                    if ui.add_enabled(!groups.is_empty(), egui::Button::new("cut a camera group by signals…"))
                        .on_hover_text("see the lanes — kick, control channels, line starts — weight them, and plan a cut that says why")
                        .clicked() {
                        if let Some((g, _)) = groups.first() { self.open_signals(*g); }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("show transcripts under clips…")
                        .on_hover_text("load a folder of whisper .srt files; matched to clips by name")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(palette::ShellVerb::LoadTranscripts));
                        ui.close();
                    }
                    if ui.button("relink media to a folder…")
                        .on_hover_text("proxies → camera originals; only the paths move")
                        .clicked() {
                        self.run_palette_act(palette::Act::Shell(palette::ShellVerb::RelinkToFolder));
                        ui.close();
                    }
                });
                if ui.button("save").clicked()
                    || ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
                {
                    let _ = self.save_project();
                }
                let mut combined = self.studio.combined;
                if ui.add(egui::Button::selectable(combined, "workbench"))
                    .on_hover_text("timeline above, rack/program/mixer below — instead of tabs")
                    .clicked() { combined = !combined; }
                self.studio.combined = combined;
                if ui.add(egui::Button::selectable(self.preview_open, "preview"))
                    .on_hover_text("program monitor above the timeline: the frame under the playhead \
                                    from the topmost video track, the clip the export takes; audio is the mixdown")
                    .clicked() { self.preview_open = !self.preview_open; }
                // The playhead's visible controls. `g g` and `g e` have
                // always done this; nothing on screen said so, which
                // meant a new user had no way back to the start at all.
                // The take button. Red while live, with the elapsed time
                // on it — a recording UI that does not show you it is
                // recording is how takes get lost.
                let live = self.mic.recording();
                self.studio.recording = live;
                let label = if live { format!("⏺ {:.1}s", self.mic.elapsed()) }
                            else { "⏺".to_string() };
                let btn = egui::Button::new(label)
                    .fill(if live { egui::Color32::from_rgb(150, 40, 40) }
                          else { ui.visuals().widgets.inactive.bg_fill });
                if ui.add(btn)
                    .on_hover_text(format!("record a take from {} (C-r)", self.mic.device_name()))
                    .clicked() {
                    self.toggle_take();
                }
                if live { ctx.request_repaint(); } // the counter must tick
                // The call. One button to join or leave, and while on it,
                // mute and a HOLD-to-talk control — a button you hold is
                // the one push-to-talk that works on a phone, where there
                // is no key to hold.
                let on_call = self.call.on();
                let call_btn = egui::Button::new(if on_call { "☎ on call" } else { "☎" })
                    .fill(if on_call { egui::Color32::from_rgb(40, 110, 60) }
                          else { ui.visuals().widgets.inactive.bg_fill });
                if ui.add(call_btn)
                    .on_hover_text(if on_call { "leave the call" }
                                   else { "join the session's voice call" })
                    .clicked() {
                    self.toggle_call();
                }
                if on_call && self.call.listen_only().is_none() {
                    let muted = self.call.muted();
                    if ui.add(egui::Button::selectable(muted, "mute"))
                        .on_hover_text("send silence (the far side's meter reads no loss)")
                        .clicked() {
                        self.call.set_muted(!muted);
                    }
                    if self.call.push_to_talk() {
                        let held = ui.add(egui::Button::new("hold to talk"))
                            .on_hover_text("push-to-talk: heard while this is held")
                            .is_pointer_button_down_on();
                        self.call.set_talking(held);
                    }
                }
                if ui.button("⏮").on_hover_text("playhead to start (g g)").clicked() {
                    self.studio.seek(studio_core::Ticks::ZERO);
                    self.studio.status = "playhead at start".into();
                }
                if ui.button("⏭").on_hover_text("playhead to end (g e)").clicked() {
                    let d = self.studio.session.doc.project.duration();
                    self.studio.seek(d);
                    self.studio.status = "playhead at end".into();
                }
                if ui.button("settings…").clicked() {
                    self.settings_open = !self.settings_open;
                }
                if ui.button("archive…")
                    .on_hover_text("the whole session — state and audio — as ONE file")
                    .clicked() {
                    self.archive_ui = Some(ArchiveUi::default());
                }
                if ui.button("export…").clicked() {
                    self.export_dialog = Some(export::Plan::default());
                    self.export_note.clear();
                }
            });
        });
        egui::Panel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.studio.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} clips", self.studio.session.doc.project.clips.len()));
                    let note = Self::clock_note(self.studio.transport.playing,
                                                self.studio.rack_playing,
                                                self.sink.position(),
                                                self.loaded != SinkContent::Nothing);
                    if !note.is_empty() { ui.separator(); ui.label(note); }
                    let call = self.call.describe(&*self.mic);
                    if !call.is_empty() { ui.separator(); ui.label(call); }
                });
            });
        });
        if self.settings_open {
            let mut open = true;
            egui::Window::new("Settings").open(&mut open).resizable(false)
                .collapsible(false).show(ctx, |ui| {
                    ui.label("keys — press rebind, then the new key:");
                    // The actions worth offering: the everyday editing verbs.
                    // Each row names the DEFAULT char; what currently reaches
                    // it is read from the table, so a swap shows truthfully
                    // on both rows. Not every keymap action — the point is
                    // the keys people's hands disagree about, not a full
                    // keymap editor (that stays a non-goal; see rebinds.rs).
                    const REBINDABLE: [(&str, char); 8] = [
                        ("delete selection", 'd'), ("split at playhead", 's'),
                        ("yank", 'y'), ("paste", 'p'),
                        ("undo", 'u'), ("redo", 'U'),
                        ("align clips", 'A'), ("quantize clips", 'Q'),
                    ];
                    for (label, def) in REBINDABLE {
                        ui.horizontal(|ui| {
                            let now = self.rebinds.keys_for(def);
                            let shown = if now.is_empty() { "(unbound!)".to_string() }
                                else { now.iter().map(|c| format!("{c}"))
                                          .collect::<Vec<_>>().join(" ") };
                            let capturing = self.rebind_capture == Some(def);
                            if ui.add(egui::Button::selectable(capturing,
                                    if capturing { "press a key…" } else { "rebind" }))
                                .clicked() {
                                self.rebind_capture =
                                    if capturing { None } else { Some(def) };
                            }
                            ui.monospace(shown);
                            ui.label(label);
                        });
                    }
                    if !self.rebinds.is_empty() && ui.button("reset all keys").clicked() {
                        self.rebinds = rebinds::Rebinds::default();
                        self.rebind_capture = None;
                    }
                    ui.colored_label(egui::Color32::GRAY,
                        "raw keys only — the palette and its hints keep the defaults. \
                         Saved per device with the workspace.");
                    ui.separator();
                    ui.label("piano keys — calibrate playing mode to YOUR hands:");
                    ui.horizontal(|ui| {
                        match &self.piano_cal {
                            None => {
                                if ui.button("calibrate").clicked() {
                                    self.piano_cal = Some(Default::default());
                                }
                                ui.label(if self.zones.is_empty() {
                                    "tracker layout (uncalibrated)".to_string()
                                } else { "calibrated".to_string() });
                                if !self.zones.is_empty() && ui.button("reset").clicked() {
                                    self.zones = Default::default();
                                }
                            }
                            Some(cal) => {
                                ui.label(format!("press every key that should mean \
                                                  {}, then Enter", cal.note_name()));
                                if ui.button("done").clicked() {
                                    if let Some(cal) = self.piano_cal.take() {
                                        self.zones = cal.zones;
                                    }
                                }
                            }
                        }
                    });
                    ui.colored_label(egui::Color32::GRAY,
                        "several keys can share one note — wide regions like real \
                         piano-key spacing. Uncalibrated keys keep the tracker rows.");
                    ui.separator();
                    // Libraries: until now these were STUDIO_LIBRARY and
                    // STUDIO_VIDEO_LIB only, which means "edit your launcher
                    // script" — fine for the person who wrote it, invisible
                    // to everyone else. The field shows what is in use and
                    // what is actually there, because a typo and a real
                    // folder look identical in a text box.
                    ui.label("libraries — where sound and picture are browsed from:");
                    let mut audio = self.library_roots.audio.display().to_string();
                    let mut video = self.library_roots.video.as_ref()
                        .map(|p| p.display().to_string()).unwrap_or_default();
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("sound");
                        if ui.add(egui::TextEdit::singleline(&mut audio).desired_width(260.0)
                            .id_salt("lib-root-audio")).changed() { changed = true; }
                        if ui.button("browse…").clicked() {
                            if let Some(d) = export::pick_folder() {
                                audio = d.display().to_string();
                                changed = true;
                            }
                        }
                        let state = library::inspect(std::path::Path::new(&audio), false);
                        ui.colored_label(if state.usable() { colors.text_weak } else { colors.danger },
                                         state.describe());
                    });
                    ui.horizontal(|ui| {
                        ui.label("picture");
                        if ui.add(egui::TextEdit::singleline(&mut video).desired_width(260.0)
                            .hint_text("optional — proxies, takes").id_salt("lib-root-video")).changed() {
                            changed = true;
                        }
                        if ui.button("browse…").clicked() {
                            if let Some(d) = export::pick_folder() {
                                video = d.display().to_string();
                                changed = true;
                            }
                        }
                        if !video.is_empty() {
                            let state = library::inspect(std::path::Path::new(&video), true);
                            ui.colored_label(if state.usable() { colors.text_weak } else { colors.danger },
                                             state.describe());
                        }
                    });
                    if changed {
                        self.library_roots.audio = audio.clone().into();
                        self.library_roots.video =
                            (!video.trim().is_empty()).then(|| std::path::PathBuf::from(&video));
                        self.library_root = self.library_roots.audio.clone();
                        // The sidebar walks from the root each frame, so resetting its
                        // open-folder state is all a root change needs.
                        self.library_tree = library::TreeState::default();
                        self.studio.status = "library — saved with the workspace on next save".into();
                    }
                    ui.colored_label(egui::Color32::GRAY,
                        "set here, these override STUDIO_LIBRARY / STUDIO_VIDEO_LIB and are \
                         kept per device with the workspace — a sample drive is not mounted \
                         in the same place on two machines.");
                    ui.separator();
                    ui.label("your name, as peers see it beside your playhead:");
                    let hint = format!("site {}", self.studio.session.doc.site.0);
                    ui.add(egui::TextEdit::singleline(&mut self.studio.display_name).id_salt("lib-self_studio_display_name")
                        .hint_text(hint));
                    ui.separator();
                    ui.label("bounce fidelity — what library-bound renders cost on THIS device:");
                    let all = morph_pad::BounceFormat::ALL;
                    let mut ix = all.iter().position(|f| *f == self.bounce_format).unwrap_or(1);
                    ui.add(egui::Slider::new(&mut ix, 0..=all.len() - 1).show_value(false));
                    self.bounce_format = all[ix];
                    ui.label(self.bounce_format.label());
                    ui.colored_label(egui::Color32::GRAY,
                        "opus 192k is transparent on most systems at a tenth the size; \
                         flac is the lossless rung; wav is for whoever insists. \
                         Saved per device with the workspace — a phone and a desktop \
                         answer this differently on purpose.");
                });
            if !open {
                self.settings_open = false;
                self.rebind_capture = None;
                // Closing mid-calibration keeps what was walked so far —
                // a half-calibration plays (zones first, layout fallback).
                if let Some(cal) = self.piano_cal.take() { self.zones = cal.zones; }
            }
        }
        if self.group_graph_open {
            let mut open = true;
            egui::Window::new("Grouping graph").open(&mut open).resizable(true)
                .default_size([420.0, 320.0])
                .show(ctx, |ui| {
                    let colors = uikit_egui::palette_colors_ctx(ctx, "midnight");
                    studio_egui::group_graph::paint(ui, &self.studio.session.doc.project, &colors);
                });
            if !open { self.group_graph_open = false; }
        }
        if let Some((bus, bus_name)) = self.studio.mix_pass_request.take() {
            // Measure the ACTUAL mixdown — the thing being mastered, not a
            // guess from the project's shape.
            let mixed = self.studio.render_arrangement(48_000, &self.audio);
            match mix_assistant::measure(&mixed, 48_000) {
                Some((spec, measured)) => {
                    let strength = studio_fx::assistant::DEFAULT_STRENGTH;
                    let gain = self.studio.session.doc.project.busses.get(&bus)
                        .map(|b| b.gain).unwrap_or(1.0);
                    let proposal = studio_copilot::mix_pass(
                        &spec, measured.crest_db, measured.correlation,
                        measured.low_width, bus, gain, strength);
                    self.mix_pass = Some(MixPassUi { bus, bus_name, spec, measured,
                                                     strength, proposal });
                }
                None => self.studio.status =
                    "nothing to analyse — the arrangement is silent".into(),
            }
        }
        // Voice is engaged with C-space and shows voiceterm's popup. The
        // recognizer itself is not in this repo; `voice_utterance` is the
        // seam it feeds, and the palette walk behind it is fully tested
        // headless, so wiring a backend is one call and no new verbs.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Space)) {
            self.voice = match self.voice {
                Some(_) => {
                    voice_engine_kit::cancel_recording();
                    None
                }
                None => {
                    voice::spawn_listen(self.voice_capture.clone());
                    Some(voice::Dialog { lexicon: self.lexicon.clone(),
                                         ..Default::default() })
                }
            };
        }
        if let Some(text) = voice::poll_capture(&self.voice_capture) {
            if text.is_empty() {
                // Cancelled (voice_engine_kit's own convention -- see
                // poll_capture's doc for why this is classified HERE,
                // not inside hear()). Reset to the resting frame rather
                // than feed hear() an empty string, which would show a
                // spurious "nothing but filler" for a deliberate cancel.
                if let Some(d) = &mut self.voice {
                    d.state = voice::State::Listening;
                }
            } else if text.starts_with('[') {
                // voice_engine_kit's embedded-error convention (no model,
                // no input device, etc.) -- a real system problem, not a
                // misheard command. Shown through Failed's own `why`
                // field rather than invented as a new State variant, but
                // with an empty `heard` so the popup does not print the
                // bracketed string twice.
                if let Some(d) = &mut self.voice {
                    d.state = voice::State::Failed { heard: String::new(), why: text };
                }
            } else {
                self.voice_utterance(&text);
            }
        }
        if let Some(d) = &self.voice {
            voice::feedback_popup(ctx, d, &colors);
        }
        self.palette_window(ctx);
        if let Some(t) = self.clipboard_pending.take() { ctx.copy_text(t); }
        fx_editor::window(ctx, &mut self.studio, &colors);
        self.lan_tick();
        self.drain_provenance();
        self.join_window(ctx);
        self.find_window(ctx);
        self.mix_pass_window(ctx);
        self.archive_window(ctx);
        self.export_dialog_window(ctx);
        self.export_tracker_window(ctx);
        self.touchup_tracker(ctx);
        self.kaleido_gallery_window(ctx);
        self.signals_window(ctx);
        self.comic_tracker();
        // the filmstrip: take in decoded frames, ask for what the timeline lacked
        // (paths against the project folder -- relative sources are beside it)
        {
            let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let paths: std::collections::HashMap<studio_core::SourceId, std::path::PathBuf> = self.studio.session.doc.project.sources
                .iter().filter(|(_, s)| s.has_video).map(|(id, s)| (*id, base.join(&s.path))).collect();
            self.thumbs.pump(ctx, |id| paths.get(&id).cloned());
        }
        self.comic_setup_window(ctx);
        self.track_points_window(ctx);

        // ── files dropped on the window ──────────────────────────────────
        //
        // The zone decides, not the file type. `drop_plan` is the table; this
        // is the only caller. Before it, this loop did one thing with every
        // drop — decode as audio and make a channel or a clip — which meant a
        // VIDEO dropped here went into the audio decoder and came back as
        // "import failed", and an unsupported file said the same. Both are now
        // answered before the decode, with the reason.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            // The panel under the pointer IS the zone. The existing comment
            // below already reasoned this way ("the panel you're looking at
            // names the import you meant"); `drop_plan` makes it a value
            // instead of an `if` buried after the decode.
            let zone = if self.studio.panel == Panel::Rack {
                crate::drop_plan::DropZone::Rack
            } else {
                crate::drop_plan::DropZone::Unspecified
            };
            if let Some(path) = f.path.as_ref() {
                let media = crate::drop_plan::media_of(
                    path,
                    path.is_dir(),
                    // EXTENSION, not a probe. `studio_video::probe` shells out
                    // to ffmpeg, and this runs per dropped file on the UI
                    // thread -- a folder of ninety samples would spawn ninety
                    // processes to answer a question the filename settles.
                    // `is_video_path` is the same classification the library
                    // uses, so the two cannot disagree about what a video is.
                    library::is_video_path,
                );
                let plan = crate::drop_plan::plan(path, media, zone, None);
                if let Some(note) = &plan.caution {
                    self.studio.status = note.clone();
                }
                // REFUSED BY NAME, before any decode. An unsupported file used
                // to reach the audio decoder and come back with whatever
                // studio-media said about its bytes, which describes the
                // failure rather than the cause.
                if let crate::drop_plan::Intent::Refuse { why, .. } = &plan.intent {
                    self.studio.status = why.clone();
                    continue;
                }
                // Video is NOT intercepted here. `drop_plan` sends it to a
                // rack channel (its audio) or a timeline clip (its picture),
                // and the block further down already implements exactly that
                // split, with the better-argued version of the rule. Adding a
                // second video path here would be a fork of a decision that is
                // already made correctly one screen away.
            }
            // A folder is a kit — see `Studio::import_kit`.
            if let Some(dir) = f.path.as_ref().filter(|p| p.is_dir()) {
                match self.studio.import_kit(dir) {
                    Ok(hits) => for (id, audio, raw) in hits {
                        self.peaks.insert(id, self.build_peaks(&audio));
                        self.audio.insert(id, audio);
                        self.studio.register_asset(id, &raw);
                    },
                    Err(e) => self.studio.status = format!("kit import failed: {e}"),
                }
                continue;
            }
            let name = f.path.as_ref()
                .and_then(|p| p.file_stem()).and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| f.name.clone());
            // Keep the raw file bytes alongside the decode: they are what
            // peers fetch (the compressed original — a .opus stays .opus on
            // the wire, which is the whole size story).
            let decoded = match (&f.path, &f.bytes) {
                (Some(p), _) => std::fs::read(p)
                    .map_err(|e| studio_media::DecodeError::Io(e))
                    .and_then(|raw| {
                        let hint = p.extension().and_then(|e| e.to_str());
                        studio_media::decode_bytes(raw.clone(), hint)
                            .map(|a| (a, p.display().to_string(), raw))
                    }),
                (None, Some(b)) => {
                    let hint = f.name.rsplit('.').next();
                    studio_media::decode_bytes(b.to_vec(), hint)
                        .map(|a| (a, f.name.clone(), b.to_vec()))
                }
                (None, None) => continue,
            };
            match decoded {
                Ok((audio, path, raw)) => {
                    // ALREADY HAVE THIS FILE? Reuse the source instead of
                    // minting a second one. By content, so a sample renamed or
                    // copied into a second folder is recognised as the take it
                    // is -- a path check would miss both, and the same loop
                    // living in `bounces/` and the project folder is the
                    // common case rather than the odd one.
                    //
                    // The decode above is not wasted work worth avoiding: the
                    // bytes have to be read to be hashed at all, and skipping
                    // the decode would mean hashing separately and reading the
                    // file twice.
                    if let Some(existing) = self.studio.source_for_bytes(&raw) {
                        // SKIPS the second import; does not yet place a second
                        // clip from the existing source. Stated plainly because
                        // the two are different gestures and only one is done:
                        // dropping a file you already have no longer costs a
                        // duplicate source and a second copy of the audio in
                        // memory, but if you dropped it meaning "give me
                        // another instance of this", that still needs the
                        // library. Saying "reusing it" would have claimed the
                        // second.
                        let short = crate::media_map::AssetKey(
                            studio_collab::assets::hash(&raw)).short();
                        self.studio.status = format!(
                            "'{name}' is already imported as source {} [{short}]",
                            existing.0);
                        continue;
                    }
                    // The panel you're looking at names the import you meant:
                    // a file dropped on the Rack is a drum hit for a sampler
                    // channel; dropped anywhere else it's arrangement
                    // material — a melody loop — and lands as a clip at the
                    // playhead. (A long loop as a rack one-shot would be cut
                    // at the voice window, which is never what a melody
                    // loop means.)
                    let id = if self.studio.panel == Panel::Rack {
                        Some(self.studio.import_audio(&name, &path, &audio))
                    } else {
                        self.studio.import_clip(&name, &path, &audio)
                    };
                    if let Some(id) = id {
                        self.peaks.insert(id, self.build_peaks(&audio));
                        self.audio.insert(id, audio);
                        self.studio.register_asset(id, &raw);
                    }
                }
                // A decode error is not a missing-plugin error (studio-media's
                // own rule): say exactly what failed, keep editing.
                Err(e) => self.studio.status = format!("import '{name}' failed: {e}"),
            }
        }

        // ── playing mode: the keyboard is a piano ────────────────────────
        // Physical keys, not logical ones: see playkeys' module docs on why
        // AZERTY otherwise scatters the layout. egui hands both to us.
        if self.studio.input_mode == studio_egui::playkeys::InputMode::Playing {
            let pressed: Vec<char> = ctx.input(|i| i.events.iter().filter_map(|e| {
                let egui::Event::Key { key, physical_key, pressed: true, .. } = e else { return None };
                let k = physical_key.unwrap_or(*key);
                k.name().chars().next().map(|c| c.to_ascii_lowercase())
            }).collect());
            // The sink's column when the loop runs, else step 0 — playing
            // into a stopped rack writes the downbeat, which is what a
            // person tapping a stopped grid expects.
            let column = self.sink.position().and_then(|pos| {
                let p = self.studio.rack.patterns.get(self.studio.current_pattern)?;
                let step = self.studio.rack.tempo().step_len()
                    .to_samples(self.sink.rate()).max(1) as usize;
                (p.steps > 0).then(|| (pos / step) as u32 % p.steps)
            }).unwrap_or(0);
            for c in pressed {
                if let Some(note) = studio_egui::playkeys::play_note(
                    c, self.studio.octave, &studio_egui::keys::Key::c(c), &*self.analog,
                    &self.zones)
                {
                    self.studio.play_note_at(note, column);
                }
            }
        }

        // ── the wire, if one exists ───────────────────────────────────────
        if let Some(carrier) = &self.carrier {
            for ev in carrier.events.try_iter() {
                match ev {
                    studio_collab::stream::PeerEvent::Connected(addr) => {
                        self.studio.status = format!("peer connected: {addr}");
                        // since: 0 EXPLICITLY, not Doc::hello() — that
                        // helper sends our own max lamport, which is right
                        // for resuming a one-way catch-up but under-asks on
                        // first contact between peers who both edited
                        // offline: their concurrent ops stamped below our
                        // lamport would be filtered out. Lost edits. The
                        // seen-set makes over-fetching free; asking for
                        // everything is the only safe opening line.
                        carrier.send(studio_collab::wire::Packet::Hello {
                            site: self.studio.session.doc.site, since: 0,
                        });
                        if let Some(rack) = self.studio.session.rack_broadcast() {
                            carrier.send(rack);
                        }
                    }
                    studio_collab::stream::PeerEvent::Disconnected(why) => {
                        self.studio.status = format!("peer disconnected: {why}");
                    }
                }
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64).unwrap_or(0);
            for p in carrier.poll() {
                for reply in self.studio.integrate_packet(p, now_ms) {
                    carrier.send(reply);
                }
            }
            for p in self.studio.drain_outbox() {
                carrier.send(p);
            }
            // The call: frames that arrived go to the link, the tap's
            // audio goes out, and the sink's stream is topped up. Only
            // here, inside the wire block — a call with no carrier has
            // nobody on it, and the verb says so instead of opening a
            // microphone for no one.
            for p in self.studio.voice_inbox.drain(..) {
                self.call.receive(&p);
            }
            let site = self.studio.session.doc.site;
            for p in self.call.pump(&mut *self.mic, &mut *self.sink, site) {
                carrier.send(p);
            }
            // Our own presence: the sending half of what integrate_packet
            // merges. Rate-limited and heartbeat'd inside; called every
            // frame, sends when it should.
            //
            // `follow` is local, by NAME (see its own doc); resolved to a
            // SiteId here, at the boundary, because `Studio` deliberately
            // doesn't know about this UI-only concept -- `follow_graph`'s
            // chain resolution needs a real SiteId to walk, not a name.
            // `view_mode` has no real per-device setting yet (the
            // "terrain toggle" UI hasn't been built) -- `Plain` until it
            // does, which is also the correct value for every peer who
            // never touches this feature at all.
            let roster = self.studio.session.roster();
            let following = self.follow.as_deref()
                .and_then(|name| roster.iter().find(|m| m.name == name))
                .map(|m| m.site);
            if let Some(p) = self.studio.presence_beat(now_ms, following, studio_collab::ViewMode::Plain) {
                carrier.send(p);
            }
            // And the silence sweep — members time out with an announcement,
            // presence-only ghosts are pruned quietly.
            for ev in self.studio.session.sweep(now_ms) {
                if let studio_collab::session::Event::TimedOut(site) = ev {
                    self.studio.status = format!("site {} timed out", site.0);
                    // Their last words must not replay when they are back
                    // on a fresh sequence.
                    self.call.forget(site);
                }
            }
            // Packets arrive between frames; a connected shell keeps frames
            // coming so a peer's edit shows without waiting for a local one.
            // A call needs the pump more often than that: its stream depth
            // is the stall budget, and 50 ms is half of it.
            ctx.request_repaint_after(std::time::Duration::from_millis(
                if self.call.on() { 20 } else { 50 }));
        }

        // ── the controller ────────────────────────────────────────────────
        // MIDI notes take the SAME path the computer keyboard's do: voice
        // them now, and land them in the pattern when the transport is
        // recording. One note path, so velocity and channel selection cannot
        // drift between the two kinds of keyboard.
        let mut midi_notes: Vec<studio_egui::playkeys::PlayNote> = Vec::new();
        if let Some(m) = &self.midi {
            for msg in m.poll() {
                match msg {
                    midi::MidiMessage::NoteOn { note, velocity, .. } => {
                        self.midi_held.insert(note, ());
                        midi_notes.push(midi::play_note(note, velocity));
                    }
                    midi::MidiMessage::NoteOff { note, .. } => {
                        self.midi_held.remove(&note);
                    }
                    // CC and bend are parsed and deliberately unrouted: a
                    // mod wheel that silently moved an unrelated fader would
                    // be worse than one that does nothing. Mapping needs a
                    // learn-and-bind UI, named rather than guessed.
                    _ => {}
                }
            }
        }
        for n in midi_notes {
            let samples = self.studio.render_play_note(&n, self.sink.rate());
            if !samples.is_empty() {
                self.sink.trigger(std::sync::Arc::new(samples));
            }
            if self.studio.transport.recording {
                let step = self.studio.recording_step();
                self.studio.play_note_at(n, step);
            }
        }

        if let Ok(mut w) = self.ui_waker.lock() { if w.is_none() { *w = Some(ctx.clone()); } }
        if !self.touch_seen && ctx.input(|i| i.any_touches()) { self.touch_seen = true; }
        // ── video sources' audio, arriving from the extraction worker ────
        if let Some(rx) = &self.video_audio_rx {
            let mut got = Vec::new();
            while let Ok(item) = rx.try_recv() { got.push(item); }
            for (id, audio) in got {
                self.video_audio_left = self.video_audio_left.saturating_sub(1);
                if let Some(a) = audio {
                    self.peaks.insert(id, self.build_peaks(&a));
                    self.audio.insert(id, a);
                    self.studio.arrangement_dirty = true;
                }
            }
            if self.video_audio_left == 0 {
                self.video_audio_rx = None;
                self.studio.status = "video audio extracted — waveforms and playback complete".into();
            } else {
                self.studio.status = format!("extracting audio from video… {} to go", self.video_audio_left);
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }

        // ── media that finished arriving from peers ──────────────────────
        for (id, audio) in std::mem::take(&mut self.studio.incoming_audio) {
            self.peaks.insert(id, self.build_peaks(&audio));
            self.audio.insert(id, audio);
            // Clips referencing this source were silent until now; racks
            // sampling it were too.
            self.studio.arrangement_dirty = true;
            self.studio.loop_dirty = true;
        }

        // ── keep the Doc's rack register current ──────────────────────────
        // Every panel mutates `studio.rack` directly; this is the one choke
        // point that turns those mutations into LWW syncs in the outbox. No
        // carrier drains the outbox in this shell yet — named plainly: the
        // packets are real and tested, the socket is not built.
        self.studio.sync_rack_if_changed();

        // ── one sink, one owner ───────────────────────────────────────────
        self.sync_sink();
        // LIVE SUBPATTERNS: the rack changed (loop_dirty is the rack's own dirty bit) — every
        // source placed from a pattern is re-rendered, its peaks rebuilt, and the arrangement
        // re-mixed, so the tape plays the pattern as it is now through the mixer.
        if self.studio.loop_dirty && !self.studio.pattern_links.is_empty() {
            let rate = self.sink.rate();
            let fresh = self.studio.rerender_pattern_links(rate, &self.audio);
            if !fresh.is_empty() {
                for (id, audio) in fresh {
                    self.peaks.insert(id, self.build_peaks(&audio));
                    self.audio.insert(id, audio);
                }
                self.studio.arrangement_dirty = true;
            }
        }
        // LINKED PATTERN CLIPS: any pattern source whose rack changed, or whose
        // clips now need more than was rendered, is rendered again -- the link is
        // the source kind itself, saved and synced with the project.
        let redone = pattern_clips::refresh(&self.studio.session.doc.project, &self.studio.rack,
                                            &mut self.audio, &mut self.pattern_stamps);
        if !redone.is_empty() {
            for sid in redone {
                match self.audio.get(&sid) {
                    Some(a) => { let pk = self.build_peaks(a); self.peaks.insert(sid, pk); }
                    None => { self.peaks.remove(&sid); }
                }
            }
            self.studio.arrangement_dirty = true;
        }
        if self.studio.pending_bounce {
            self.studio.pending_bounce = false;
            let first_audio = self.studio.session.doc.project.tracks.iter()
                .find(|t| t.kind == studio_core::TrackKind::Audio).map(|t| t.id);
            match first_audio {
                Some(track) => self.studio.pending_place = Some((self.studio.current_pattern, self.studio.playhead(), track)),
                None => self.studio.status = "bounce refused: no audio track".into(),
            }
        }
        if let Some((index, at, track)) = self.studio.pending_place.take() {
            // The timeline's rate, not the sink's: a bounce is project
            // material and must not change length because of the sound card.
            if let Some((id, audio)) = self.studio.place_pattern(index, at, track, 48_000, &self.audio) {
                self.peaks.insert(id, self.build_peaks(&audio));
                // The op recorded a path; make it true on disk, so the
                // project's reference points at a real file rather than a
                // label. A failed write is a status note, not a lost bounce
                // — the audio is already registered in memory either way.
                if let Some(src) = self.studio.session.doc.project.sources.get(&id) {
                    let path = std::path::PathBuf::from(&src.path);
                    match studio_audio::export::export_wav_dithered_bytes(
                        &audio.channels[0], 1, 48_000, id.0)
                    {
                        Ok(bytes) => {
                            // One encode, two destinations: the file the op's
                            // path promises, and the chunk store peers fetch
                            // from.
                            self.studio.register_asset(id, &bytes);
                            let write = path.parent()
                                .map(|d| std::fs::create_dir_all(d)).unwrap_or(Ok(()))
                                .and_then(|_| std::fs::write(&path, &bytes));
                            if let Err(e) = write {
                                self.studio.status = format!("{} (file not written: {e})",
                                                             self.studio.status);
                            }
                        }
                        Err(e) => self.studio.status =
                            format!("{} (wav encode failed: {e})", self.studio.status),
                    }
                }
                self.audio.insert(id, audio);
            }
        }
        let playhead_col = self.sink.position().and_then(|pos| {
            let p = self.studio.rack.patterns.get(self.studio.current_pattern)?;
            if p.steps == 0 { return None; }
            let step = self.studio.rack.tempo().step_len()
                .to_samples(self.sink.rate()).max(1) as usize;
            Some((pos / step) as u32 % p.steps)
        });
        if self.studio.rack_playing || self.studio.transport.playing {
            // The playhead column is a read recomputed each frame (grid's own
            // rule), so the frames have to keep coming while anything runs.
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        let mut lib_action = None;
        if self.library_open {
            // `resizable(true)` + an explicit `size_range` is what stops the
            // reported "sidebar fights to grow, and snaps back wide after a
            // resize whenever I scroll or tap". Without `resizable`, egui
            // re-derives this panel's width from its CONTENT every frame, and
            // the content includes a full library path (`library — /home/…`)
            // plus the footage-root line — so any interaction that repainted
            // shoved it back out to whatever those strings measure, discarding
            // the drag. Marking it resizable makes egui persist the user's
            // width in panel state instead; the range then bounds it at both
            // ends so neither a long path nor an over-enthusiastic drag can
            // take the whole window. The labels inside are truncated to match
            // (see library.rs) so they wrap into the chosen width rather than
            // pushing against it.
            // The cap is RELATIVE to the surface, not a fixed 420. On a phone
            // 420pt is most of the screen, so a sidebar allowed to reach it
            // leaves no timeline — the panel was obeying its range and the
            // range was wrong for the device. Measured from content_rect (the
            // usable area, excluding what sits under the system bars), and
            // floored so it cannot collapse to nothing on a very narrow one.
            let surface = ctx.content_rect().width();
            let side_max = (surface * 0.42).clamp(180.0, 420.0);
            let side_default = side_max.min(210.0);
            egui::Panel::left("library")
                .resizable(true)
                .default_size(side_default)
                .size_range(160.0_f32.min(side_max)..=side_max)
                .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("⟳").on_hover_text("rescan").clicked() {
                        self.library_tree.refresh();
                    }
                    if ui.small_button("hide").clicked() {
                        self.library_open = false;
                    }
                });
                let pad_open = !self.morph.anchors.is_empty();
                egui::CollapsingHeader::new("morph pad").default_open(pad_open)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Breakout FROM the pattern: the selected rack
                            // channel's sound onto the pad.
                            if ui.small_button("⇐ channel")
                                .on_hover_text("load the selected rack channel's sample")
                                .clicked() {
                                let sel = self.studio.selected_channel;
                                let src = self.studio.rack.channels.get(sel).and_then(|ch| {
                                    if let studio_live::SynthKind::Sampler { source } = ch.kind {
                                        self.audio.get(&source).cloned()
                                            .map(|a| (ch.name.clone(), a))
                                    } else { None }
                                });
                                match src {
                                    Some((name, a)) => self.morph.push(name, a),
                                    None => self.studio.status =
                                        "the selected channel is not a sampler (or its audio is absent)".into(),
                                }
                            }
                            // The return path: the tuned sound back INTO
                            // the pattern — replacing the sound it came
                            // from, or layered beside it with the same
                            // steps. Both bounce to the library first, so
                            // every authored sound is also a FILE.
                            let ready = !self.morph.anchors.is_empty();
                            if ui.add_enabled(ready, egui::Button::small(
                                    egui::Button::new("⇒ replace")))
                                .on_hover_text("bounce, then swap the selected channel to it")
                                .clicked() {
                                self.morph_return(false);
                            }
                            if ui.add_enabled(ready, egui::Button::small(
                                    egui::Button::new("⇒ layer")))
                                .on_hover_text("bounce, then add it as a channel with the same steps")
                                .clicked() {
                                self.morph_return(true);
                            }
                        });
                        match morph_pad::show(ui, &mut self.morph, &colors) {
                            Some(morph_pad::PadAction::Preview) => {
                                let a = self.morph.blend();
                                let n = a.frames().min(self.sink.rate() as usize * 4);
                                if n > 0 {
                                    let mono: Vec<f32> = (0..n).map(|i| a.mono(i)).collect();
                                    self.sink.trigger(std::sync::Arc::new(mono));
                                }
                            }
                            Some(morph_pad::PadAction::Bounce) => {
                                match morph_pad::bounce(&self.library_root, &self.morph,
                                                        self.bounce_format) {
                                    Ok(p) => {
                                        self.studio.status = format!("bounced {}", p.display());
                                        self.morph.last_bounce = Some(p);
                                        self.library_tree.refresh();
                                    }
                                    Err(e) => self.studio.status = e,
                                }
                            }
                            Some(morph_pad::PadAction::Reset) => {
                                (self.morph.x, self.morph.y) = morph_pad::PadState::neutral();
                                // The double's first click bounced a frame
                                // ago; a reset that leaves that junk behind
                                // teaches people to fear the reset.
                                if let Some(p) = self.morph.last_bounce.take() {
                                    let _ = std::fs::remove_file(&p);
                                    self.library_tree.refresh();
                                    self.studio.status = "morph pad reset (and un-bounced)".into();
                                } else {
                                    self.studio.status = "morph pad reset".into();
                                }
                            }
                            Some(morph_pad::PadAction::Remove(i)) => {
                                if i < self.morph.anchors.len() {
                                    self.morph.anchors.remove(i);
                                }
                            }
                            None => {}
                        }
                    });
                ui.separator();
                // What the sidebar searches follows what you are looking
                // at — the timeline wants footage among its results, the
                // rack and program tabs want samples first.
                let focus = match self.studio.panel {
                    studio_egui::touch::Panel::Rack => library::Focus::Rack,
                    studio_egui::touch::Panel::Program => library::Focus::Program,
                    _ => library::Focus::Timeline,
                };
                lib_action = library::show_tree(
                    ui, &self.library_roots, &mut self.library_tree,
                    &self.studio.rack, &self.studio.session.doc.project,
                    &self.library_prefs, focus, &colors);
            });
        }
        if let Some(action) = lib_action {
            // The sidebar's two buttons are the two import doors drops
            // already use, minus the panel-guessing: the button says which.
            let (path, to_rack) = match action {
                library::LibraryAction::ToRack(p) => (p, true),
                library::LibraryAction::ToTimeline(p) => (p, false),
                library::LibraryAction::Pin(p) => {
                    self.library_prefs.pin(&p);
                    self.studio.status = format!("{} {}",
                        if self.library_prefs.rank(&p) < 0 { "pinned" } else { "unpinned" },
                        p.file_name().map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default());
                    return;
                }
                library::LibraryAction::Demote(p) => {
                    self.library_prefs.demote(&p);
                    self.studio.status = format!("{} {}",
                        if self.library_prefs.rank(&p) > 0 { "demoted" } else { "restored" },
                        p.file_name().map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default());
                    return;
                }
                library::LibraryAction::ToMorphSet(paths) => {
                    self.morph.anchors.clear();
                    let mut loaded = 0;
                    for p in paths.iter().take(4) {
                        if let Some(audio) = std::fs::read(p).ok().and_then(|raw| {
                            let hint = p.extension().and_then(|e| e.to_str())
                                .map(str::to_owned);
                            studio_media::decode_bytes(raw, hint.as_deref()).ok()
                        }) {
                            let name = p.file_stem().and_then(|x| x.to_str())
                                .unwrap_or("sound").to_owned();
                            self.morph.push(name, audio);
                            loaded += 1;
                        }
                    }
                    self.studio.status = format!("morph pad loaded: {loaded} of a family");
                    return;
                }
                library::LibraryAction::ToMorph(p) => {
                    match std::fs::read(&p).ok().and_then(|raw| {
                        let hint = p.extension().and_then(|e| e.to_str()).map(str::to_owned);
                        studio_media::decode_bytes(raw, hint.as_deref()).ok()
                    }) {
                        Some(audio) => {
                            let name = p.file_stem().and_then(|x| x.to_str())
                                .unwrap_or("sound").to_owned();
                            self.morph.push(name.clone(), audio);
                            self.studio.status = format!(
                                "'{name}' on the morph pad ({} of 4)", self.morph.anchors.len());
                        }
                        None => self.studio.status = format!("could not decode {}", p.display()),
                    }
                    return;
                }
                library::LibraryAction::ReplaceChannel(i, p) => {
                    // Swap the sample under an existing channel, keeping
                    // its steps and rolls — "try this snare in the pattern
                    // I already have", the offer the classifier makes.
                    // import_audio adds a fresh channel (its contract for
                    // the plain rack door); replacement moves that new
                    // kind onto the old channel and drops the extra.
                    match std::fs::read(&p).ok().and_then(|raw| {
                        let hint = p.extension().and_then(|e| e.to_str())
                            .map(str::to_owned);
                        studio_media::decode_bytes(raw.clone(), hint.as_deref())
                            .ok().map(|a| (raw, a))
                    }) {
                        Some((raw, audio)) => {
                            let name = p.file_stem().and_then(|x| x.to_str())
                                .unwrap_or("sample").to_owned();
                            let id = self.studio.import_audio(
                                &name, &p.display().to_string(), &audio);
                            self.peaks.insert(id, self.build_peaks(&audio));
                            self.audio.insert(id, audio);
                            self.studio.register_asset(id, &raw);
                            let added = self.studio.rack.channels.len().saturating_sub(1);
                            if i < added {
                                self.studio.rack.channels[i].kind =
                                    studio_live::SynthKind::Sampler { source: id };
                                self.studio.rack.channels[i].name = name.clone();
                                self.studio.rack.channels.truncate(added);
                                self.studio.selected_channel = i;
                                self.studio.loop_dirty = true;
                                self.studio.status = format!("channel {} is now '{name}'", i + 1);
                            }
                        }
                        None => self.studio.status =
                            format!("could not decode {}", p.display()),
                    }
                    return;
                }
                library::LibraryAction::ToNewTrackBus(p, bus) => {
                    // The smart bus choice: an existing drum bus by name,
                    // or a new 'Drums' bus made on the spot; then a new
                    // audio track routed into it, and the clip at the
                    // playhead on that track — every step an ordinary op.
                    let bus_id = match bus {
                        Some(b) => b,
                        None => {
                            let b = studio_core::BusId(
                                self.studio.session.doc.project.busses.keys()
                                    .map(|x| x.0).max().unwrap_or(0) + 1);
                            self.studio.edit(studio_core::Op::AddBus {
                                bus: studio_core::Bus { id: b, name: "Drums".into(),
                                    parent: None, gain: 1.0, chain: vec![],
                                    sends: vec![], muted: false, silent: false } }, "drum bus");
                            b
                        }
                    };
                    let name = p.file_stem().and_then(|x| x.to_str())
                        .unwrap_or("drum").to_owned();
                    let track = studio_core::TrackId(
                        self.studio.session.doc.project.tracks.iter()
                            .map(|t| t.id.0).max().unwrap_or(0) + 1);
                    let at = self.studio.session.doc.project.tracks.len();
                    self.studio.edit(studio_core::Op::AddTrack {
                        track: studio_core::Track { id: track,
                            kind: studio_core::TrackKind::Audio, name: name.clone(),
                            muted: false, locked: false, gain: 1.0,
                            route: Some(bus_id) }, at }, "drum track");
                    // import_clip lands at the playhead on the FIRST audio
                    // track with room; a purpose-built track wants the clip
                    // on ITSELF, so place it directly.
                    match std::fs::read(&p).ok().and_then(|raw| {
                        let hint = p.extension().and_then(|e| e.to_str())
                            .map(str::to_owned);
                        studio_media::decode_bytes(raw.clone(), hint.as_deref())
                            .ok().map(|a| (raw, a))
                    }) {
                        Some((raw, audio)) => {
                            let sid = studio_core::SourceId(
                                self.studio.session.doc.project.sources.keys()
                                    .map(|x| x.0).max().unwrap_or(0) + 1);
                            let len = studio_core::Ticks::from_samples(
                                audio.frames() as i64, audio.sample_rate);
                            self.studio.edit(studio_core::Op::AddSource {
                                source: studio_core::Source { id: sid,
                                    path: p.display().to_string(), len,
                                    sample_rate: audio.sample_rate,
                                    channels: audio.channels.len() as u16,
                                    has_video: false, kind: studio_core::SourceKind::File } }, "sample");
                            let cid = studio_core::ClipId(
                                self.studio.session.doc.project.clips.keys()
                                    .map(|x| x.0).max().unwrap_or(0) + 1);
                            self.studio.edit(studio_core::Op::AddClip {
                                clip: studio_core::Clip { id: cid, source: sid,
                                    track, pos: self.studio.playhead(),
                                    src_in: studio_core::Ticks::ZERO, len,
                                    gain: 1.0, effects: vec![] } }, "placed");
                            self.peaks.insert(sid, self.build_peaks(&audio));
                            self.audio.insert(sid, audio);
                            self.studio.register_asset(sid, &raw);
                            self.studio.status = format!(
                                "'{name}' on its own track into the drum bus");
                        }
                        None => self.studio.status =
                            format!("could not decode {}", p.display()),
                    }
                    return;
                }
                library::LibraryAction::PatternToTimeline(i) => {
                    // A LINKED clip: the pattern source names the rack pattern, and
                    // the audio is a cache kept current from the rack. (Rendering
                    // to a file stays the bounce command's job.)
                    let at = self.studio.playhead();
                    self.pattern_to_timeline(i, at, None);
                    return;
                }
                library::LibraryAction::Preview(p) => {
                    match studio_media::decode_file(&p) {
                        Ok(a) => {
                            // Mono-summed, because a voice is mono; loud
                            // samples are left alone rather than normalised,
                            // since a preview that lies about level is worse
                            // than a quiet one.
                            let n = a.frames().min(self.sink.rate() as usize * 4);
                            let mut mono = Vec::with_capacity(n);
                            for i in 0..n { mono.push(a.mono(i)); }
                            self.sink.trigger(std::sync::Arc::new(mono));
                            self.studio.status = format!("previewing {}", p.display());
                        }
                        Err(e) => self.studio.status = format!("preview failed: {e}"),
                    }
                    return;
                }
            };
            let name = path.file_stem().and_then(|s| s.to_str())
                .unwrap_or("sample").to_owned();
            // Video takes the ffmpeg door, BEFORE decode_bytes — symphonia
            // cannot open an mp4, and the old flow's silent has_video: false, kind: studio_core::SourceKind::File
            // (C12-6a's finding) began exactly here. To the rack, a video
            // is its audio (refusing a video's audio to a sampler would be
            // the tool overruling the musician — library.rs's own words);
            // to the timeline it is a picture on the video track.
            if !to_rack && library::is_video_path(&path) {
                match studio_video::probe(&path) {
                    Ok(pr) => {
                        let audio = studio_video::extract_audio(&path).ok().flatten();
                        let meta = audio.as_ref()
                            .map(|a| (a.sample_rate, a.channels.len() as u16));
                        if let Some(id) = self.studio.import_video_clip(
                            &name, &path.display().to_string(), pr.duration, meta) {
                            if let Some(a) = audio {
                                self.peaks.insert(id, self.build_peaks(&a));
                                self.audio.insert(id, a);
                            }
                            if let Ok(raw) = std::fs::read(&path) {
                                self.studio.register_asset(id, &raw);
                            }
                            self.spawn_provenance_job(id, path.clone(), true);
                        }
                    }
                    Err(e) => self.studio.status =
                        format!("'{name}': {e} (is ffmpeg installed?)"),
                }
                return;
            }
            match std::fs::read(&path) {
                Ok(raw) => {
                    let hint = path.extension().and_then(|e| e.to_str());
                    match studio_media::decode_bytes(raw.clone(), hint) {
                        Ok(audio) => {
                            let label = path.display().to_string();
                            let id = if to_rack {
                                Some(self.studio.import_audio(&name, &label, &audio))
                            } else {
                                self.studio.import_clip(&name, &label, &audio)
                            };
                            if let Some(id) = id {
                                self.peaks.insert(id, self.build_peaks(&audio));
                                self.audio.insert(id, audio);
                                self.studio.register_asset(id, &raw);
                            }
                        }
                        Err(e) => self.studio.status = format!("'{name}' failed: {e}"),
                    }
                }
                Err(e) => self.studio.status = format!("read {}: {e}", path.display()),
            }
        }

        let has_video = self.studio.session.doc.project.clips.values().any(|c| {
            self.studio.session.doc.project.tracks.iter()
                .any(|t| t.id == c.track && t.kind == studio_core::TrackKind::Video)
        });
        // The viewfinder collapses when there is no video to find: a music
        // project gets the full width back instead of a panel explaining
        // its own emptiness.
        let slot_visible = self.side_slot != SideSlot::Off
            && !(self.side_slot == SideSlot::Viewport && !has_video);
        if self.studio.combined && slot_visible {
            egui::Panel::right("side-slot").resizable(true)
                .default_size(360.0).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        for (s, l) in [(SideSlot::Viewport, "viewport"),
                                       (SideSlot::Mixer, "mixer"),
                                       (SideSlot::Rack, "rack"),
                                       (SideSlot::Program, "program"),
                                       (SideSlot::Off, "off")] {
                            if ui.add(egui::Button::selectable(self.side_slot == s, l))
                                .clicked() { self.side_slot = s; }
                        }
                    });
                    ui.separator();
                    let base = self.project_path.parent()
                        .map(|p| p.to_path_buf()).unwrap_or_default();
                    match self.side_slot {
                        SideSlot::Viewport => viewport::show(
                            ui, &self.studio.session.doc.project, self.studio.playhead(),
                            &base, &mut self.frame_cache, &colors),
                        SideSlot::Mixer => { mixer_panel::show(ui, &mut self.studio, &colors); }
                        SideSlot::Rack => { rack_panel::show(
                            ui, &mut self.studio, &colors, playhead_col); }
                        SideSlot::Program => { program_panel::show(
                            ui, &mut self.studio, &colors, playhead_col); }
                        SideSlot::Off => {}
                    }
                });
        }
        if self.studio.combined {
            egui::Panel::bottom("workbench").resizable(true)
                .default_size(300.0).show(ctx, |ui| {
                    // Which tool sits in the strip is the SAME panel state
                    // the tab layout uses, so switching layouts never loses
                    // your place; Timeline just isn't a strip tool — the
                    // timeline is the whole point of the top half.
                    let mut tab = match self.studio.panel {
                        Panel::Timeline => Panel::Rack,
                        p => p,
                    };
                    ui.horizontal(|ui| {
                        for p in [Panel::Rack, Panel::Program, Panel::Mixer] {
                            if ui.add(egui::Button::selectable(tab == p, p.label())).clicked() {
                                tab = p;
                            }
                        }
                    });
                    self.studio.panel = tab;
                    match tab {
                        Panel::Rack => { rack_panel::show(ui, &mut self.studio, &colors, playhead_col); }
                        Panel::Program => { program_panel::show(ui, &mut self.studio, &colors, playhead_col); }
                        Panel::Mixer => { mixer_panel::show(ui, &mut self.studio, &colors); }
                        Panel::Timeline => {}
                    }
                });
        }
        let timeline_showing = matches!(if self.studio.combined { Panel::Timeline } else { self.studio.panel }, Panel::Timeline);
        if timeline_showing && self.preview_open {
            egui::Panel::top("program").resizable(true).default_size(220.0).show(ctx, |ui| {
                let project = &self.studio.session.doc.project;
                let ph = self.studio.playhead();
                let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("program").strong());
                    let secs = ph.0 as f64 / studio_core::TICKS_PER_SECOND as f64;
                    ui.label(egui::RichText::new(format!("{:02}:{:06.3}", (secs / 60.0) as u64, secs % 60.0)).monospace());
                    match viewport::video_at(project, ph) {
                        Some((path, _)) => { ui.label(egui::RichText::new(format!("← {}", std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).weak().small()); }
                        None => { ui.label(egui::RichText::new("no video under the playhead").weak().small()); }
                    }
                });
                match self.multiview {
                    Some(g) => viewport::show_multiview(ui, project, ph, g, &base, &mut self.frame_cache, &colors),
                    None => viewport::show(ui, project, ph, &base, &mut self.frame_cache, &colors),
                }
            });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            // The central panel's scroll report, when the panel has one.
            // Pages-mode over-scroll navigation reads ONLY this — the
            // side-slot and workbench copies of the same panels discard
            // theirs, because paging away from a slot would be paging away
            // from the wrong thing.
            let mut edge: Option<scroll_edge::ScrollReport> = None;
            match if self.studio.combined { Panel::Timeline } else { self.studio.panel } {
                Panel::Rack => {
                    edge = Some(rack_panel::show(ui, &mut self.studio, &colors, playhead_col));
                }
                Panel::Mixer => {
                    edge = Some(mixer_panel::show(ui, &mut self.studio, &colors));
                }
                Panel::Program => {
                    edge = Some(program_panel::show(ui, &mut self.studio, &colors, playhead_col));
                }
                Panel::Timeline => 'timeline: {
                    // The inspector sits above the arrangement rather than
                    // in a side panel: it is about the thing you just
                    // clicked, so it belongs where your eyes already are,
                    // and it collapses to nothing when nothing is selected.
                    if self.structure_view {
                        // The megastructure: the whole project as sections ×
                        // lanes. Clicking a section drops back to the fine
                        // timeline at its start — the zoom is a way to find
                        // where to work, and the work happens in the fine view.
                        let ov = overview::summarize(&self.studio.session.doc.project, 12);
                        ui.horizontal(|ui| {
                            ui.label(format!("structure — {} sections × {} lanes, {} tracks",
                                ov.sections.len(), ov.lanes.len(), self.studio.session.doc.project.tracks.len()));
                            if ui.button("back to the timeline").clicked() { self.structure_view = false; }
                        });
                        if let Some(t) = overview::show(ui, &ov, self.studio.playhead(), &colors) {
                            self.studio.seek(t);
                            self.structure_view = false;
                        }
                        // not `return`: that would leave the whole panel closure and
                        // skip the scroll-edge handling and the workbench below it
                        break 'timeline;
                    }
                    clip_inspector::show(ui, &mut self.studio, &colors);
                    let ph = self.studio.playhead();
                    let sel = self.studio.selection.clone();
                    let mode = self.studio.keys.mode();
                    let input_mode = self.studio.input_mode;
                    let pending = self.studio.keys.pending();
                    // One destructure, several fields at once: Rust treats
                    // fields named together in a `let Struct { .. } = &mut
                    // x` pattern as independently borrowed, which a method
                    // call like `current_view_mut()` cannot express (it
                    // takes `&mut self.studio` as a whole, opaque to the
                    // borrow checker) — same shape as the identical fix in
                    // rusty-board-egui and uikit-egui's `timeline()` call
                    // this session.
                    let Studio { view, song_views, active_song, session, trim_side,
                                 closed_groups, .. } = &mut self.studio;
                    let view = match *active_song {
                        Some(id) => song_views.entry(id).or_default(),
                        None => view,
                    };
                    // The musical ruler follows the SONG being shown; the
                    // whole-project view keeps the clock (see timeline()'s
                    // param docs for why that stays undecided).
                    let song_tempo = active_song
                        .and_then(|id| session.doc.project.groups.get(&id))
                        .and_then(|g| g.tempo.as_ref());
                    let tr = studio_egui::timeline(ui, &session.doc,
                                                   view,
                                                   &colors, ph, &sel, &self.peaks, &self.words,
                                                   &self.thumbs.thumbs,
                                                   *trim_side,
                                                   closed_groups,
                                                   mode, input_mode, &pending,
                                                   song_tempo);
                    if let Some(t) = tr.sought {
                        // Scrub. The transport owns position, so this is a
                        // seek and not a model edit — moving the playhead
                        // is not something a collaborator should undo.
                        self.studio.seek(t);
                    }
                    if let Some((index, at, track)) = tr.pattern_dropped {
                        // linked, not bounced: a pattern clip follows the rack (pattern_clips)
                        self.pattern_to_timeline(index, at, Some(track));
                    }
                    if let Some(g) = tr.toggled_group {
                        if !self.studio.closed_groups.remove(&g) {
                            self.studio.closed_groups.insert(g);
                        }
                    }
                    if let Some(g) = tr.clicked_group {
                        // Selecting the fold selects its members: nudge,
                        // quantize, align and delete then act on the
                        // composition as one thing — the keyboard already
                        // works on selections, so the fold needs no verbs
                        // of its own.
                        if let Some(grp) = self.studio.session.doc.project.groups.get(&g) {
                            self.studio.selection = grp.clips.iter().copied()
                                .filter(|c| self.studio.session.doc.project.clips.contains_key(c))
                                .collect();
                        }
                    }
                    if let Some(id) = tr.clicked {
                        // Click selects; shift-click toggles membership — the
                        // return the shell used to throw away, which made
                        // every clip mouse-dead.
                        if ui.input(|i| i.modifiers.shift) {
                            if let Some(at) = self.studio.selection.iter().position(|c| *c == id) {
                                self.studio.selection.remove(at);
                            } else {
                                self.studio.selection.push(id);
                            }
                        } else {
                            self.studio.selection = vec![id];
                        }
                    }
                    if let Some((id, pos, src_in, len)) = tr.trimmed {
                        // A right trim is one op. A left trim genuinely IS
                        // two facts — new material window AND new position —
                        // and lands as two ops / two undo steps, the same
                        // precedent a bounce set. Trim first: shrinking
                        // can't collide, and the subsequent move slides
                        // into space the shrink just vacated.
                        let old_pos = self.studio.session.doc.project.clips
                            .get(&id).map(|c| c.pos);
                        self.studio.edit(
                            studio_core::Op::TrimClip { id, src_in, len }, "trimmed");
                        if old_pos.is_some_and(|p| p != pos) {
                            let track = self.studio.session.doc.project.clips
                                .get(&id).map(|c| c.track);
                            if let Some(track) = track {
                                self.studio.edit(
                                    studio_core::Op::MoveClip { id, pos, track }, "trimmed");
                            }
                        }
                    }
                    if let Some((id, pos, track)) = tr.moved {
                        // The drop is an op like any other: undoable, on the
                        // wire, and REFUSABLE — check_room may say no, and
                        // the status says so instead of the clip snapping
                        // back wordlessly. A clip that belongs to a
                        // composition, or to a multi-selection, takes the
                        // others with it (alt-drag moves just this one —
                        // kdenlive's "select inside the group" key).
                        let alone = ui.input(|i| i.modifiers.alt);
                        self.studio.drop_clip(id, pos, track, alone);
                    }

                    // ── the automation strip ─────────────────────────────
                    // A target whose track or bus has been removed clears
                    // itself rather than offering a strip every edit on
                    // which would be refused. A binding that was removed
                    // (by undo, or by a peer) likewise.
                    if let Some(t) = self.studio.auto_target {
                        let p = &self.studio.session.doc.project;
                        let alive = match t {
                            studio_core::AutoTarget::TrackGain(id) =>
                                p.tracks.iter().any(|tr| tr.id == id),
                            studio_core::AutoTarget::BusGain(id)
                            | studio_core::AutoTarget::BusParam { bus: id, .. } =>
                                p.busses.contains_key(&id),
                            studio_core::AutoTarget::ClipParam { clip, .. } =>
                                p.clips.contains_key(&clip),
                        };
                        if !alive { self.studio.auto_target = None; }
                    }
                    if let Some(b) = self.studio.auto_binding {
                        if !self.studio.session.doc.project.bindings.contains_key(&b) {
                            self.studio.auto_binding = None;
                        }
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("automate:");
                        let mut target = self.studio.auto_target;
                        let mut binding = self.studio.auto_binding;
                        if ui.add(egui::Button::selectable(target.is_none() && binding.is_none(), "off")).clicked() {
                            target = None; binding = None;
                        }
                        for t in &self.studio.session.doc.project.tracks {
                            let this = Some(studio_core::AutoTarget::TrackGain(t.id));
                            if ui.add(egui::Button::selectable(target == this, &t.name)).clicked() {
                                target = this; binding = None;
                            }
                        }
                        for b in self.studio.session.doc.project.busses.values() {
                            let this = Some(studio_core::AutoTarget::BusGain(b.id));
                            if ui.add(egui::Button::selectable(target == this,
                                                               format!("bus: {}", b.name))).clicked() {
                                target = this; binding = None;
                            }
                        }
                        self.studio.auto_target = target;
                        self.studio.auto_binding = binding;
                    });
                    // The curves row: every binding, and the one button
                    // that makes a new curve. A lane is the clock's; a
                    // binding is a clip's — the strip shows one or the other.
                    let mut new_curve = false;
                    let mut unbind: Option<studio_core::BindingId> = None;
                    let mut uniq: Option<studio_core::BindingId> = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("curves:");
                        let sel = self.studio.selection.len();
                        let hint = if sel > 0 {
                            format!("new curve gated to the {sel} selected clip{}, riding each clip's track gain (y/p paste it along)",
                                    if sel == 1 { "" } else { "s" })
                        } else if self.studio.auto_target.is_some() {
                            "new curve bound always to the chosen lane".into()
                        } else {
                            "select clips (or pick a lane) first".into()
                        };
                        if ui.button("+ curve").on_hover_text(hint).clicked() { new_curve = true; }
                        let p = &self.studio.session.doc.project;
                        let target_name = |t: studio_core::AutoTarget| match t {
                            studio_core::AutoTarget::TrackGain(id) =>
                                p.tracks.iter().find(|x| x.id == id).map(|x| x.name.clone())
                                    .unwrap_or_else(|| "?".into()),
                            studio_core::AutoTarget::BusGain(id) =>
                                p.busses.get(&id).map(|x| format!("bus {}", x.name))
                                    .unwrap_or_else(|| "?".into()),
                            // "bus verb · reverb · mix" — the effect by
                            // kind, since a chain index means nothing
                            // on screen; "(gone)" when the chain no
                            // longer reaches that far.
                            studio_core::AutoTarget::BusParam { bus, effect, key } => {
                                let b = p.busses.get(&bus);
                                let kind = b.and_then(|b| b.chain.get(effect as usize))
                                    .map(|e| e.kind.clone()).unwrap_or_else(|| "(gone)".into());
                                format!("bus {} · {kind} · {key}", b.map(|b| b.name.as_str()).unwrap_or("?"))
                            }
                            studio_core::AutoTarget::ClipParam { clip, effect, key } => {
                                let kind = p.clips.get(&clip).and_then(|c| c.effects.get(effect as usize))
                                    .map(|e| e.kind.clone()).unwrap_or_else(|| "(gone)".into());
                                format!("clip {} · {kind} · {key}", clip.0)
                            }
                        };
                        let gate_name = |g: studio_core::Gate| match g {
                            studio_core::Gate::Always => "always".to_string(),
                            studio_core::Gate::Clip(c) => if p.clips.contains_key(&c) {
                                format!("clip {}", c.0) } else { format!("clip {} (gone)", c.0) },
                            studio_core::Gate::Window { start, end } =>
                                format!("{:.2}–{:.2}s", start.to_seconds(), end.to_seconds()),
                            studio_core::Gate::Group(g) => match p.groups.get(&g) {
                                Some(grp) => format!("group {}", grp.name),
                                None => format!("group {} (gone)", g.0),
                            },
                        };
                        let mut pick = self.studio.auto_binding;
                        for (id, b) in &p.bindings {
                            let curve = p.curves.get(&b.curve).map(|c| c.name.as_str()).unwrap_or("?");
                            let shared = p.curve_share_count(b.curve);
                            // "shared ×N" is said BEFORE a point is moved, because
                            // the whole value of a shared curve is also its whole
                            // danger: eleven other clips hear the edit.
                            let label = if shared > 1 {
                                format!("{curve} (shared ×{shared}) → {} @ {}", target_name(b.target), gate_name(b.gate))
                            } else {
                                format!("{curve} → {} @ {}", target_name(b.target), gate_name(b.gate))
                            };
                            if ui.add(egui::Button::selectable(pick == Some(*id), label))
                                .on_hover_text("show this binding's curve in the strip")
                                .clicked() { pick = Some(*id); }
                            if shared > 1 && ui.small_button("unique")
                                .on_hover_text("give this binding its own copy of the curve; the others keep sharing")
                                .clicked() { uniq = Some(*id); }
                            if ui.small_button("×").on_hover_text("unbind (the curve stays)").clicked() {
                                unbind = Some(*id);
                            }
                        }
                        if pick != self.studio.auto_binding {
                            self.studio.auto_binding = pick;
                            self.studio.auto_target = None;
                        }
                    });
                    if new_curve {
                        let t = self.studio.auto_target;
                        self.studio.new_curve_on_selection(t);
                    }
                    if let Some(id) = unbind { self.studio.unbind(id); }
                    if let Some(id) = uniq { self.studio.make_unique(id); }
                    if let Some(target) = self.studio.auto_target {
                        let points = self.studio.session.doc.project.automation
                            .get(&target).cloned().unwrap_or_default();
                        let mut drag = self.studio.auto_drag;
                        let edits = studio_egui::auto::strip(
                            ui, &points, self.studio.current_view(), &mut drag, Some(ph), &colors, None);
                        self.studio.auto_drag = drag;
                        for e in edits {
                            match e {
                                studio_egui::auto::AutoEdit::Set { at, value } => self.studio.edit(
                                    studio_core::Op::SetAutoPoint { target, at, value },
                                    "automation point"),
                                studio_egui::auto::AutoEdit::Remove { at } => self.studio.edit(
                                    studio_core::Op::RemoveAutoPoint { target, at },
                                    "automation point removed"),
                            }
                        }
                    } else if let Some(bid) = self.studio.auto_binding {
                        // The binding's curve, shifted to the gate's origin
                        // so it draws where it plays; edits shift back into
                        // curve time. `shift_points` is pure and pinned.
                        let p = &self.studio.session.doc.project;
                        let b = p.bindings[&bid];
                        let opening = b.gate.opening(p);
                        match (p.curves.get(&b.curve), opening) {
                            (Some(curve), Some(opening)) => {
                                let curve_id = b.curve;
                                let points = shift_points(&curve.points, opening.origin);
                                let mut drag = self.studio.auto_drag;
                                let edits = studio_egui::auto::strip(
                                    ui, &points, self.studio.current_view(), &mut drag, Some(ph), &colors,
                                    opening.span);
                                self.studio.auto_drag = drag;
                                for e in edits {
                                    match e {
                                        studio_egui::auto::AutoEdit::Set { at, value } => self.studio.edit(
                                            studio_core::Op::SetCurvePoint { curve: curve_id,
                                                at: curve_time(at, opening.origin), value },
                                            "curve point"),
                                        studio_egui::auto::AutoEdit::Remove { at } => self.studio.edit(
                                            studio_core::Op::RemoveCurvePoint { curve: curve_id,
                                                at: curve_time(at, opening.origin) },
                                            "curve point removed"),
                                    }
                                }
                            }
                            _ => { ui.label("this binding's gate clip is gone — it plays nothing until the clip is back"); }
                        }
                    }
                }
            }
        
            // ── Pages-mode over-scroll navigation ────────────────────────
            // Only when the top bar actually chose Pages (real width, this
            // frame), and only from the central panel's own report. The
            // classify() rule that MOVED always wins is what makes normal
            // scrolling structurally unable to page; the timeline is
            // excluded (it owns both axes) by simply never reporting.
            if pages_mode {
                match edge {
                    Some(e) if e.moved => self.panel_nav.still_scrolling(),
                    Some(e) if e.overscroll != 0.0 => {
                        let now_ms = ctx.input(|i| i.time) * 1000.0;
                        if let Some(p) = self.panel_nav.on_overscroll(
                            self.studio.panel, e.overscroll, now_ms) {
                            self.studio.panel = p;
                            self.studio.status = format!("{}", p.label());
                        }
                    }
                    _ => {}
                }
            }
});
    }
}


/// Where the last archive was written, remembered across runs in the
/// freedesktop config directory — app-level state, so it belongs beside
/// the app's config and not in any one project's sidecar.
#[cfg(feature = "app")]
fn last_archive_note() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h|
            std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("studio/last-archive")
}

#[cfg(feature = "app")]
fn last_archive_root() -> Option<std::path::PathBuf> {
    let p = std::fs::read_to_string(last_archive_note()).ok()?;
    let p = std::path::PathBuf::from(p.trim());
    if p.is_dir() { Some(p) } else { None }
}

#[cfg(feature = "app")]
fn remember_archive_root(dir: &std::path::Path) {
    if let Some(parent) = last_archive_note().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(last_archive_note(), dir.display().to_string());
}

/// One proposed move, in words a mixing engineer reads at a glance.
#[cfg(feature = "app")]
fn describe_op(op: &studio_core::Op) -> String {
    match op {
        studio_core::Op::SetBusChain { chain, .. } => chain.iter().map(|e| {
            match e.kind.as_str() {
                "eq" => format!("EQ {:.0} Hz {:+.1} dB (Q {:.1})",
                    e.params.get("freq0").copied().unwrap_or(0.0),
                    e.params.get("gain0").copied().unwrap_or(0.0),
                    e.params.get("q0").copied().unwrap_or(1.0)),
                "compressor" => format!("compress {:.1} dB at {:.1}:1",
                    e.params.get("threshold").copied().unwrap_or(0.0),
                    e.params.get("ratio").copied().unwrap_or(1.0)),
                other => format!("add {other}"),
            }
        }).collect::<Vec<_>>().join(" · "),
        studio_core::Op::SetBusGain { gain, .. } =>
            format!("bus gain → {:.2} ({:+.1} dB)", gain, 20.0 * gain.max(1e-6).log10()),
        other => format!("{other:?}"),
    }
}

/// The find panel's state between frames.
#[cfg(feature = "app")]
struct FindUi {
    gate: find::Gate,
    by_sound: bool,
    query: String,
    hits: Vec<find::Hit>,
    ran: bool,
}

/// The mastering pass, mid-consideration.
#[cfg(feature = "app")]
struct MixPassUi {
    bus: studio_core::BusId,
    bus_name: String,
    spec: studio_fx::analysis::Spectrum,
    measured: mix_assistant::Measured,
    strength: f32,
    proposal: studio_copilot::Proposal,
}

/// What the workbench's right-hand slot shows.
#[cfg(feature = "app")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideSlot { Off, Viewport, Rack, Program, Mixer }

/// The archive popup's fields — the half-typed artist name survives a
/// frame the same way every other draft state here does.
#[cfg(feature = "app")]
#[derive(Default)]
struct ArchiveUi {
    level_ix: usize,
    artists: Vec<String>,
    artist_input: String,
    paths: Vec<String>,
    path_input: String,
    note: String,
}

#[cfg(feature = "app")]
impl App {
    /// The breakout loop's return: bounce the pad's sound to the library
    /// at the device's fidelity rung, decode THAT FILE back (the source of
    /// truth is the bounce, not the in-memory blend — what you hear later
    /// is what returned), then either swap the selected channel's sample
    /// or add a layer channel inheriting its steps.
    fn morph_return(&mut self, layer: bool) {
        let path = match morph_pad::bounce(&self.library_root, &self.morph,
                                           self.bounce_format) {
            Ok(p) => p,
            Err(e) => { self.studio.status = e; return; }
        };
        self.morph.last_bounce = Some(path.clone());
        self.library_tree.refresh();
        let decoded = std::fs::read(&path).ok().and_then(|raw| {
            let hint = path.extension().and_then(|e| e.to_str()).map(str::to_owned);
            studio_media::decode_bytes(raw.clone(), hint.as_deref()).ok()
                .map(|a| (raw, a))
        });
        let Some((raw, audio)) = decoded else {
            self.studio.status = format!("bounced {} but could not decode it back",
                                         path.display());
            return;
        };
        let name = path.file_stem().and_then(|x| x.to_str())
            .unwrap_or("morph").to_owned();
        let sel = self.studio.selected_channel;
        if layer {
            let id = self.studio.import_audio(&name, &path.display().to_string(), &audio);
            // import_audio appended a channel with the new sound; move it
            // into a LAYER of the breakout channel: same steps, then drop
            // the plain appended one.
            let appended = self.studio.rack.channels.len() - 1;
            let ch = self.studio.rack.channels[appended].clone();
            self.studio.rack.channels.truncate(appended);
            for p in &mut self.studio.rack.patterns {
                p.rows.truncate(self.studio.rack.channels.len());
            }
            let ix = self.studio.layer_channel(sel, ch);
            self.peaks.insert(id, self.build_peaks(&audio));
            self.audio.insert(id, audio);
            self.studio.register_asset(id, &raw);
            self.studio.status = format!(
                "'{name}' layered as channel {} — same steps, yours to diverge", ix + 1);
        } else {
            let id = self.studio.import_audio(&name, &path.display().to_string(), &audio);
            let appended = self.studio.rack.channels.len().saturating_sub(1);
            if sel < appended {
                self.studio.rack.channels[sel].kind =
                    studio_live::SynthKind::Sampler { source: id };
                self.studio.rack.channels[sel].name = name.clone();
                self.studio.rack.channels.truncate(appended);
                for p in &mut self.studio.rack.patterns {
                    p.rows.truncate(self.studio.rack.channels.len());
                }
                self.studio.selected_channel = sel;
                self.studio.loop_dirty = true;
                self.studio.status = format!("channel {} is now '{name}'", sel + 1);
            }
            self.peaks.insert(id, self.build_peaks(&audio));
            self.audio.insert(id, audio);
            self.studio.register_asset(id, &raw);
        }
    }

    /// The width a phone-safe dialog should ask for, given the screen it has
    /// to live on. Same formula as `logscope-egui::mobilelayout::dialog_width`
    /// (that crate hit this exact bug first — `Window::default_width` /
    /// `fixed_size` are desktop numbers, and on a phone-width screen they
    /// put the window's own edges off-screen or eat almost the whole
    /// display): never wider than the screen minus a margin on each side,
    /// never wider than desired, never below a usable floor.
    fn dialog_width(screen_pt: f32, desired_pt: f32) -> f32 {
        const MARGIN_PT: f32 = 12.0;
        const FLOOR_PT: f32 = 240.0;
        let usable = screen_pt - 2.0 * MARGIN_PT;
        desired_pt.min(usable).max(FLOOR_PT.min(desired_pt))
    }

    /// The command palette: a query box, ranked choices, and the wizard's
    /// breadcrumb. Enter takes the top choice, arrows move, Escape backs
    /// out one step (and out of the palette at the root) — the keyboard
    /// never has to reach for the mouse, which is the entire point.
    fn palette_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.palette.take() else { return };
        let project = self.studio.session.doc.project.clone();
        let choices = p.choices(&project);
        let mut chosen: Option<palette::Choice> = None;
        let mut close = false;
        let mut open = true;

        // Keys first: the list is drawn from the query, so navigation must
        // resolve before the widgets read it.
        let mut moved = false;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                if p.pending.is_some() {
                    p.pending = None;
                    p.path.clear();
                    p.query.clear();
                    p.selected = 0;
                } else { close = true; }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                p.selected += 1;
                moved = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                p.selected = p.selected.saturating_sub(1);
                moved = true;
            }
        });
        let ranked = palette::filter(&choices, &p.query);
        if p.selected >= ranked.len() { p.selected = ranked.len().saturating_sub(1); }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            chosen = ranked.get(p.selected).map(|c| (*c).clone());
        }

        let width = Self::dialog_width(ctx.content_rect().width(), 420.0);
        // max_width, not only default_width: a transcript line is as long as
        // someone talked, and without a cap the window grew to fit the longest
        // one — anchored at top-centre, it spread off BOTH edges and pushed the
        // very lines being searched out of view (seen on the Fragile Thing card).
        egui::Window::new("Command palette").open(&mut open).resizable(false)
            .collapsible(false).default_width(width).max_width(width)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .show(ctx, |ui| {
                if !p.path.is_empty() {
                    ui.colored_label(egui::Color32::GRAY, p.path.join(" › "));
                }
                let edit = ui.add(egui::TextEdit::singleline(&mut p.query).id_salt("lib-p_query")
                    .hint_text(match p.pending {
                        None => "type a command…",
                        Some(_) => "pick a target…",
                    }).desired_width(f32::INFINITY));
                edit.request_focus();
                ui.separator();
                if ranked.is_empty() {
                    ui.colored_label(egui::Color32::GRAY, "nothing matches");
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (i, c) in ranked.iter().enumerate() {
                        let row = ui.horizontal(|ui| {
                            // Shortened for DRAWING only: the full label is still what
                            // the filter matched against, so nothing becomes unfindable.
                            const MAX_CHARS: usize = 46; // leaves room for the hint column
                            let shown: std::borrow::Cow<str> = if c.label.chars().count() > MAX_CHARS {
                                format!("{}…", c.label.chars().take(MAX_CHARS).collect::<String>()).into()
                            } else { c.label.as_str().into() };
                            let hit = ui.selectable_label(i == p.selected, shown.as_ref())
                                .on_hover_text(&c.label);
                            if let Some(h) = &c.hint {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| { ui.weak(h); });
                            }
                            hit
                        }).inner;
                        // Arrows move `p.selected` above, but the scroll area
                        // doesn't know that happened — without this, the
                        // highlight moves past the visible rows and the user
                        // has to reach for the mouse wheel to find it again,
                        // which defeats the whole point of arrow navigation.
                        if moved && i == p.selected {
                            row.scroll_to_me(Some(egui::Align::Center));
                        }
                        if row.clicked() { chosen = Some((*c).clone()); }
                    }
                });
                ui.separator();
                ui.weak("enter: choose · esc: back · C-p: close");
            });

        if let Some(c) = chosen {
            match p.choose(&c) {
                Some(act) => { self.run_palette_act(act); close = true; }
                None => {} // descended into the next question
            }
        }
        if !close && open { self.palette = Some(p); }
    }

    /// Feed one recognized utterance to the voice dialog and run whatever
    /// it decides. The seam a speech backend calls — deliberately the
    /// only thing such a backend needs to know about this app.
    pub fn voice_utterance(&mut self, text: &str) {
        // The dialog is taken out for the turn: it borrows nothing from
        // the shell while `run_palette_act` mutates it, and goes back
        // afterwards carrying its state.
        let mut d = self.voice.take().unwrap_or_else(|| voice::Dialog {
            lexicon: self.lexicon.clone(), ..Default::default() });
        let project = self.studio.session.doc.project.clone();
        let act = d.hear(text, &project);
        if let voice::State::Failed { why, .. } = &d.state {
            self.studio.status = format!("voice: {why}");
        }
        self.voice = Some(d);
        if let Some(act) = act { self.run_palette_act(act); }
    }

    /// Execute a completed palette path. Ops go through `edit_group` — a
    /// palette command is ONE undo even when it is two ops (create a bus
    /// and route into it), so a mistake never leaves half of itself.
    fn run_palette_act(&mut self, act: palette::Act) {
        match act {
            palette::Act::Ops(ops) => {
                let what = if ops.len() > 1 { "palette command" } else { "palette edit" };
                self.studio.edit_group(ops, what);
            }
            // Delegating to the keymap rather than duplicating the verb is
            // why the palette cannot drift from the keyboard.
            palette::Act::Key(ch) => { self.studio.key(studio_egui::keys::Key::c(ch)); }
            palette::Act::Panel(panel) => {
                self.studio.panel = panel;
                self.studio.status = format!("{}", panel.label());
            }
            palette::Act::Shell(v) => match v {
                // OPEN and the collab verbs. All report through
                // `status` rather than acting silently: every one of
                // them either changes who can see the project or
                // changes where the view is pointing, and a person who
                // cannot tell whether an invite went out will send it
                // twice.
                palette::ShellVerb::AddTrack { video } => {
                    let kind = if video { studio_core::TrackKind::Video } else { studio_core::TrackKind::Audio };
                    let op = tracks::add_below(&self.studio.session.doc.project, self.studio.selection.first().copied(), kind);
                    let name = if let studio_core::Op::AddTrack { track, .. } = &op { track.name.clone() } else { String::new() };
                    self.studio.edit(op, "add track");
                    self.studio.status = format!("added '{name}'{}", if self.studio.selection.is_empty() { " at the bottom" } else { " below the selected clip's track" });
                }
                palette::ShellVerb::OpenProject => self.open_palette_at(palette::Ask::OpenProjectPick, "open project"),
                palette::ShellVerb::OpenProjectBrowse => match open_project::pick() {
                    Some(p) => self.request_open(p),
                    None => {
                        // cancelled, or no portal on this desktop: the two look the
                        // same from here, so offer the list either way
                        self.open_palette_at(palette::Ask::OpenProjectPick, "open project");
                        self.studio.status = "no project chosen (or no file picker on this desktop) — here are the ones found nearby".into();
                    }
                },
                palette::ShellVerb::OpenProjectFound(i) => match open_project::nearby().get(i as usize).cloned() {
                    Some(p) => self.request_open(p),
                    None => self.studio.status = "that project is no longer where it was — open the list again".into(),
                },
                palette::ShellVerb::OpenSaveThen => if let Some(p) = self.open_pending.take() {
                    match self.save_project() {
                        Ok(()) => self.switch_to(p),
                        Err(e) => self.studio.status = format!("could not save, so staying on this project: {e}"),
                    }
                },
                palette::ShellVerb::OpenDiscard => if let Some(p) = self.open_pending.take() { self.switch_to(p) },
                palette::ShellVerb::OpenCancel => {
                    self.open_pending = None;
                    self.studio.status = "kept working on this project".into();
                }
                palette::ShellVerb::CollabInvite => {
                    // The roster is what an invite would grow, and it is
                    // reported so the person can see the current state
                    // before and after. Sending the invite needs a
                    // contact to send it TO, which is the same picker
                    // gap as OpenProject.
                    let n = self.studio.session.roster().len();
                    self.studio.status =
                        format!("invite: {n} in the session — needs a contact picker (C90c)");
                }
                palette::ShellVerb::CopySessionId => {
                    let id = studio_collab::lan::SessionId(self.studio.carrier_key()).render();
                    self.clipboard_pending = Some(id.clone());
                    self.studio.status = format!("session id {id} — copied");
                }
                palette::ShellVerb::JoinSession => {
                    self.join = Some(JoinDialog::default());
                }
                palette::ShellVerb::SelectGroup(id) => {
                    self.studio.select_group(id);
                }
                palette::ShellVerb::CollabRemove => {
                    let n = self.studio.session.roster().len();
                    self.studio.status =
                        format!("remove: {n} in the session — needs a member picker (C90c)");
                }
                palette::ShellVerb::CollabFollow => {
                    // Follow is the one that CAN be honest today: the
                    // roster is known, and following is local view
                    // state rather than a network operation.
                    match self.studio.session.roster().first().map(|m| m.name.clone()) {
                        Some(name) => {
                            self.follow = Some(name.clone());
                            self.studio.status = format!("following {name}");
                        }
                        // Named rather than silent: "nothing happened"
                        // and "nobody is here" look identical otherwise.
                        None => self.studio.status = "nobody else is in this session".into(),
                    }
                }
                palette::ShellVerb::CollabUnfollow => {
                    self.studio.status = match self.follow.take() {
                        Some(who) => format!("stopped following {who}"),
                        None => "not following anyone".into(),
                    };
                }
                palette::ShellVerb::CollabResync => {
                    // Convergent by construction, so asking again costs
                    // bandwidth and changes nothing — which is why this
                    // is safe to offer as a plain, non-destructive
                    // command rather than behind a confirmation.
                    let n = self.studio.session.doc.history().len();
                    self.studio.status = format!("resync requested — {n} ops locally");
                }
                palette::ShellVerb::RecordTake => { self.toggle_take(); }
                palette::ShellVerb::CallToggle => { self.toggle_call(); }
                palette::ShellVerb::LoopSelection => { self.studio.loop_selection(); }
                palette::ShellVerb::Harmonize(d) => { self.studio.harmonize_roll(d as i32); }
                palette::ShellVerb::Arpeggiate(order) => { self.studio.arpeggiate_roll(order, 1); }
                palette::ShellVerb::LoopOff => { self.studio.loop_off(); }
                palette::ShellVerb::CompTakes => { self.studio.comp_selected_takes(); }
                palette::ShellVerb::MuteTakeLanes(m) => { self.studio.set_take_lanes_muted(m); }
                palette::ShellVerb::CallMute => {
                    let m = !self.call.muted();
                    self.call.set_muted(m);
                    self.studio.status = if m { "call: microphone muted" }
                                         else { "call: microphone open" }.into();
                }
                palette::ShellVerb::CallPushToTalk => {
                    let p = !self.call.push_to_talk();
                    self.call.set_push_to_talk(p);
                    self.studio.status = if p { "call: push-to-talk — hold the talk button" }
                                         else { "call: microphone open (push-to-talk off)" }.into();
                }
                palette::ShellVerb::ToggleKeyGuide => {
                    self.studio.key_guide = !self.studio.key_guide;
                    self.studio.status = format!("out-of-key dimming {}",
                        if self.studio.key_guide { "on" } else { "off" });
                }
                palette::ShellVerb::Find(mode) => {
                    self.find_ui = Some(FindUi {
                        gate: match mode { 2 => find::Gate::Silent,
                                           1 => find::Gate::WithAudio,
                                           _ => find::Gate::Everything },
                        by_sound: mode == 1,
                        query: String::new(),
                        hits: Vec::new(),
                        ran: mode == 2, // the silence gate needs no keyword
                    });
                    if mode == 2 { self.run_find(); }
                }
                palette::ShellVerb::VoiceVocab(i) => {
                    if let Some(d) = voice::Domain::ALL.get(i as usize).copied() {
                        self.lexicon.toggle(d);
                        if let Some(v) = &mut self.voice { v.lexicon = self.lexicon.clone(); }
                        self.studio.status = format!("voice vocabulary: {}",
                            if self.lexicon.to_tokens().is_empty() { "common only".into() }
                            else { self.lexicon.to_tokens() });
                    }
                }
                palette::ShellVerb::Multiview(group) => {
                    // Toggling: choosing the group that is already showing
                    // puts the monitor back on the cut, which is the way out
                    // without a second command to find.
                    self.multiview = if self.multiview == group { None } else { group };
                    self.frame_cache = viewport::FrameCache::default();
                    self.studio.status = match self.multiview {
                        Some(g) => format!("multiview: {}", self.studio.session.doc.project.groups
                            .get(&g).map(|x| x.name.clone()).unwrap_or_default()),
                        None => "program monitor back on the cut".into(),
                    };
                }
                palette::ShellVerb::ClipTail { ring_ms, fade_ms } => {
                    let tail = studio_audio::ClipTail {
                        ring: ring_ms as f64 / 1000.0, fade: fade_ms as f64 / 1000.0 };
                    let project = &self.studio.session.doc.project;
                    let ops: Vec<studio_core::Op> = self.studio.selection.iter()
                        .filter_map(|id| project.clips.get(id))
                        .map(|c| {
                            // Replace any previous tail setting; leave every
                            // real effect exactly where it was in the chain.
                            let mut fx: Vec<studio_core::Effect> = c.effects.iter()
                                .filter(|e| e.kind != studio_audio::TAIL_KIND).cloned().collect();
                            if tail.ring > 0.0 { fx.push(tail.effect()); }
                            studio_core::Op::SetEffects { id: c.id, effects: fx }
                        }).collect();
                    if ops.is_empty() {
                        self.studio.status = "select the clips whose tail to set".into();
                    } else {
                        let n = ops.len();
                        self.studio.edit_group(ops, "clip tail");
                        self.studio.status = if tail.ring == 0.0 {
                            format!("{n} clip(s): effects cut at the clip end")
                        } else {
                            format!("{n} clip(s): effects ring {:.1} s past the end{}", tail.ring,
                                    if tail.fade > 0.0 { format!(", fading the last {:.1} s", tail.fade) }
                                    else { ", then cut".into() })
                        };
                    }
                }
                palette::ShellVerb::WordFilter(kind) => {
                    self.word_filter = match kind {
                        palette::WordFilterKind::All => studio_voice::WordFilter::All,
                        palette::WordFilterKind::Filler => studio_voice::WordFilter::Filler,
                        palette::WordFilterKind::Unique => studio_voice::WordFilter::Unique,
                        palette::WordFilterKind::Dictionary => match export::pick_file() {
                            Some(p) => match std::fs::read_to_string(&p) {
                                Ok(t) => studio_voice::WordFilter::Dictionary(studio_voice::parse_dictionary(&t)),
                                Err(e) => { self.studio.status = format!("could not read {}: {e}", p.display()); return; }
                            },
                            None => { self.studio.status = "no word list chosen".into(); return; }
                        },
                    };
                    self.studio.status = if self.words_raw.is_empty() {
                        "no transcripts loaded yet — the filter applies once they are".into()
                    } else { self.refilter_words() };
                }
                palette::ShellVerb::CheckStory => {
                    let project = &self.studio.session.doc.project;
                    let (cs, unread) = story_check::gather(project, &self.words_raw, &self.project_path);
                    if cs.is_empty() {
                        self.studio.status = "constraints: nothing to check against -- outline.txt, constraints.txt, a .lrc, or loaded transcripts".into();
                    } else {
                        let (report, ops) = story_check::check(project, &cs, &self.words_raw);
                        if !ops.is_empty() { self.studio.edit_group(ops, "constraints checked"); }
                        self.studio.status = format!("constraints ({}): {}{}", cs.len(), report.summary(),
                            if unread.is_empty() { String::new() } else { format!(" -- could not read: {}", unread.join("; ")) });
                    }
                }
                palette::ShellVerb::DraftStory => {
                    self.studio.status = match assembly::draft_story(&self.studio.session.doc.project, &self.words_raw, &self.project_path) {
                        Ok((file, story)) => format!("drafted {}: {} scene{}, {} lines, {} -- edit it, then \"assemble\"{}",
                            file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            story.scenes.len(), if story.scenes.len() == 1 { "" } else { "s" }, story.line_count(), story.order,
                            if story.reviews.is_empty() { String::new() } else { format!(" ({} to check, listed at its top)", story.reviews.len()) }),
                        Err(e) => e,
                    };
                }
                palette::ShellVerb::Assemble => {
                    let outline = assembly::outline_beside(&self.project_path);
                    let (from, lines) = match outline {
                        Some((name, lines)) => (format!("{name}, {} lines", lines.len()), lines),
                        None => ("no outline: shot order".into(), Vec::new()),
                    };
                    let at = self.studio.playhead();
                    match assembly::plan(&self.studio.session.doc.project, &self.words_raw, &lines, at) {
                        Ok((asm, ops)) => {
                            let reviews = asm.reviews.len();
                            self.studio.edit_group(ops, "assembled");
                            self.studio.status = format!("assembled ({from}): {}{}", asm.summary(),
                                if reviews > 0 { " -- palette: jump to marker, type \"review\"" } else { "" });
                        }
                        Err(e) => self.studio.status = e,
                    }
                }
                palette::ShellVerb::StyleCopy => {
                    let project = &self.studio.session.doc.project;
                    let ph = self.studio.playhead();
                    let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    // the picture actually on screen: the topmost video layer at the playhead
                    match viewport::video_at(project, ph) {
                        None => self.studio.status = "no picture under the playhead to copy a look from".into(),
                        Some((path, media)) => match studio_video::frame_rgb(&base.join(&path), media, 320, 180) {
                            Ok(px) => {
                                let st = studio_video::style::stats(&px);
                                self.studio.status = format!("look copied: median lightness {:.0}, highlights tint a {:+.0} b {:+.0}, shadows a {:+.0} b {:+.0}",
                                    st.q[4], st.band[2][0], st.band[2][1], st.band[0][0], st.band[0][1]);
                                self.style_ref = Some(st);
                            }
                            Err(e) => self.studio.status = format!("could not read the frame: {e:?}"),
                        },
                    }
                }
                palette::ShellVerb::StylePaste(pct) => {
                    let Some(reference) = self.style_ref else {
                        self.studio.status = "copy a look first (style: copy the look under the playhead)".into();
                        return;
                    };
                    let project = &self.studio.session.doc.project;
                    let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    let mut ops = Vec::new();
                    let mut unread = 0;
                    for c in self.studio.selection.iter().filter_map(|id| project.clips.get(id)) {
                        let Some(src) = project.sources.get(&c.source).filter(|s| s.has_video) else { continue };
                        let mut fx: Vec<studio_core::Effect> = c.effects.iter()
                            .filter(|e| e.kind != studio_video::style::KIND).cloned().collect();
                        if pct > 0 {
                            // each clip is matched from its OWN look, measured mid-clip, so a
                            // dark take and a bright one both land on the reference
                            let mid = c.src_in.saturating_add(Ticks(c.len.0 / 2));
                            match studio_video::frame_rgb(&base.join(&src.path), mid, 320, 180) {
                                Ok(px) => fx.push(studio_video::style::Transfer::fit(
                                    &studio_video::style::stats(&px), &reference, pct as f32 / 100.0).effect()),
                                Err(_) => { unread += 1; continue; }
                            }
                        }
                        ops.push(studio_core::Op::SetEffects { id: c.id, effects: fx });
                    }
                    if ops.is_empty() {
                        self.studio.status = "select the picture clips to give the look to".into();
                    } else {
                        let n = ops.len();
                        self.studio.edit_group(ops, "style");
                        self.studio.status = format!("{n} clip(s): {} {}", if pct > 0 { format!("look at {pct}%") } else { "look removed".into() },
                            if unread > 0 { format!("({unread} could not be read)") } else { String::new() });
                    }
                }
                palette::ShellVerb::StageBind { bus, preset } => {
                    match stage::bind_ops(&self.studio.session.doc.project, &self.studio.selection.clone(), bus, preset as usize) {
                        Ok((ops, line)) => { self.studio.edit_group(ops, "stage"); self.studio.status = line; }
                        Err(why) => self.studio.status = why,
                    }
                }
                palette::ShellVerb::Kaleidoscope(segments) => {
                    let project = &self.studio.session.doc.project;
                    let ops: Vec<studio_core::Op> = self.studio.selection.iter()
                        .filter_map(|id| project.clips.get(id))
                        .filter(|c| project.sources.get(&c.source).map(|s| s.has_video).unwrap_or(false))
                        .map(|c| {
                            // replace any previous fold, keep every other effect where it was
                            let mut fx: Vec<studio_core::Effect> = c.effects.iter()
                                .filter(|e| e.kind != studio_video::kaleido::KIND).cloned().collect();
                            if segments >= 2 {
                                fx.push(studio_video::kaleido::Kaleido { segments, ..Default::default() }.effect());
                            }
                            studio_core::Op::SetEffects { id: c.id, effects: fx }
                        }).collect();
                    if ops.is_empty() {
                        self.studio.status = "select the picture clips to fold".into();
                    } else {
                        let n = ops.len();
                        self.studio.edit_group(ops, "kaleidoscope");
                        self.studio.status = if segments >= 2 {
                            format!("{n} clip(s): kaleidoscope, {segments} wedges — preview and export")
                        } else { format!("{n} clip(s): kaleidoscope off") };
                    }
                }
                palette::ShellVerb::InsertRange { source, from_ms, to_ms } => {
                    let at = self.studio.playhead();
                    let (a, b) = (insert_by_content::from_ms(from_ms), insert_by_content::from_ms(to_ms));
                    let ops = insert_by_content::place(&self.studio.session.doc.project, source, a, b, at);
                    if ops.is_empty() {
                        self.studio.status = "nothing to insert — the range is empty or the source is gone".into();
                    } else {
                        let name = insert_by_content::source_name(&self.studio.session.doc.project, source);
                        self.studio.edit_group(ops, "insert by content");
                        self.studio.status = format!("inserted {name} {:.2}–{:.2} s at {:.2} s",
                            a.to_seconds(), b.to_seconds(), at.to_seconds());
                    }
                }
                palette::ShellVerb::LoadTranscripts => match export::pick_folder() {
                    None => self.studio.status = "no folder chosen".into(),
                    Some(dir) => {
                        let l = transcripts::load(&self.studio.session.doc.project, &dir);
                        let summary = l.summary();
                        self.words_raw.extend(l.words);
                        self.refilter_words();
                        self.studio.status = summary;
                    }
                },
                palette::ShellVerb::PeakBand(band) => {
                    self.peak_band = band;
                    // Rebuilt from the audio already in memory — no decoding,
                    // no files, so switching bands is as cheap as a zoom.
                    for (id, audio) in &self.audio {
                        self.peaks.insert(*id, studio_audio::Peaks::build_band(audio, 256, 256, band));
                    }
                    self.studio.status = match band {
                        studio_audio::Band::Full => "waveforms: everything".into(),
                        b => format!("waveforms: {} band only — a way of looking, not an edit", b.label()),
                    };
                }
                palette::ShellVerb::RelinkToFolder => {
                    match export::pick_folder() {
                        None => self.studio.status = "no folder chosen".into(),
                        Some(dir) => {
                            let plan = relink::plan(&self.studio.session.doc.project, &dir);
                            let line = plan.summary();
                            if plan.ops.is_empty() {
                                // Nothing to do is not a failure, but it must
                                // not look like success either.
                                self.studio.status = format!("nothing relinked — {line}");
                            } else {
                                self.studio.edit(studio_core::Op::Group { ops: plan.ops }, &line);
                                self.studio.status = line;
                            }
                            for (path, found) in &plan.ambiguous {
                                eprintln!("relink: {path} matches {} files, left alone", found.len());
                            }
                        }
                    }
                }
                palette::ShellVerb::TouchUpStabilize(kind) => self.start_stabilize(kind),
                palette::ShellVerb::TouchUpParticles { preset, drive, ride } => self.start_particles(preset, drive, ride),
                palette::ShellVerb::TouchUpParticlesPlace { preset, drive } => self.open_track_points(preset, drive),
                palette::ShellVerb::TouchUpFollow { zoom_pct } => self.start_touchup(
                    studio_video::touchup::Fix::FollowMotion { zoom: zoom_pct as f32 / 100.0, smooth: 0.5 },
                    format!("following the motion at {:.1}x", zoom_pct as f32 / 100.0)),
                palette::ShellVerb::TouchUpCleanPlate { radius, moving } =>
                    self.start_clean_plate(radius, moving),
                palette::ShellVerb::MulticamLevel { group, bus } => {
                    // Defaults chosen to be watchable rather than clever: no
                    // shot under a second, 3 dB of hysteresis so a level
                    // sitting on a threshold does not chatter, and a third of
                    // a second of settle so a rise through a middle band does
                    // not cut to the middle angle on its way up.
                    let rules = studio_video::multicam::Rules {
                        min_shot: 1.0, hysteresis_db: 3.0, settle: 0.3 };
                    match multicam_cut::ops(&self.studio.session.doc.project, &self.audio, group, bus, rules) {
                        Ok((ops, note)) => self.studio.edit(
                            studio_core::Op::Group { ops }, &format!("multicam: {note}")),
                        Err(why) => self.studio.status = why,
                    }
                }
                palette::ShellVerb::MarkerHere => {
                    let at = self.studio.playhead();
                    let n = self.studio.session.doc.project.markers.len() + 1;
                    self.studio.edit(studio_core::Op::SetMarker {
                        at, name: format!("mark {n}") }, "marker");
                }
                palette::ShellVerb::GotoMarker(at) => {
                    self.studio.seek(at);
                    self.studio.status = format!("at {:.1}s", at.to_seconds());
                }
                palette::ShellVerb::GotoStart => {
                    self.studio.seek(studio_core::Ticks::ZERO);
                    self.studio.status = "playhead at start".into();
                }
                palette::ShellVerb::GotoEnd => {
                    let d = self.studio.session.doc.project.duration();
                    self.studio.seek(d);
                    self.studio.status = "playhead at end".into();
                }
                palette::ShellVerb::MixMaster => {
                    let target = self.studio.session.doc.project.busses.iter()
                        .find(|(_, b)| b.parent.is_none())
                        .map(|(id, b)| (*id, b.name.clone()));
                    match target {
                        Some(t) => self.studio.mix_pass_request = Some(t),
                        None => self.studio.status =
                            "no bus feeding master to analyse".into(),
                    }
                }
                palette::ShellVerb::NewCurveOnGroup => {
                    let t = self.studio.auto_target;
                    self.studio.new_curve_on_group(t);
                }
                palette::ShellVerb::NewCurve => {
                    let t = self.studio.auto_target;
                    self.studio.new_curve_on_selection(t);
                }
                palette::ShellVerb::SplitOnGrid(steps) => self.studio.split_selection_on_grid(steps as i64),
                palette::ShellVerb::SyncDelay(n) => { self.studio.sync_delays_to_tempo(n); }
                palette::ShellVerb::Overlap(policy) => {
                    let track = self.studio.selection.first()
                        .and_then(|c| self.studio.session.doc.project.clips.get(c)).map(|c| c.track);
                    self.studio.set_overlap(track, policy);
                }
                palette::ShellVerb::MakeCurveUnique => match self.studio.auto_binding {
                    Some(b) => self.studio.make_unique(b),
                    None => self.studio.status = "make unique: pick a binding in the curves row first".into(),
                },
                palette::ShellVerb::ArchiveDialog => {
                    self.archive_ui = Some(ArchiveUi::default());
                }
                palette::ShellVerb::ExportDialog => {
                    self.export_dialog = Some(Default::default());
                }
                palette::ShellVerb::ExportVideo => {
                    if self.export_run.is_some() {
                        self.studio.status = "an export is already running".into();
                    } else {
                        let out = self.project_path.with_extension("mp4");
                        self.export_done.clear();
                        self.export_run = Some(export::start_video(
                            self.studio.session.doc.project.clone(), self.audio.clone(), out,
                            self.sink.rate().max(8_000), 1280, 720, 30, self.waker()));
                    }
                }
                palette::ShellVerb::Settings => { self.settings_open = true; }
                palette::ShellVerb::ShowGroupGraph => { self.group_graph_open = true; }
                palette::ShellVerb::Workspace(ix) => self.apply_layout(ix as usize),
                palette::ShellVerb::SharePattern => {
                    match self.studio.export_pattern_pack(self.studio.current_pattern)
                        .and_then(|pack| pattern_pack::publish(
                            &pack, &pattern_pack::cache_dir())
                            .map_err(|e| e.to_string())) {
                        Ok(path) => self.studio.status =
                            format!("shared — other projects see it now ({})",
                                    path.display()),
                        Err(e) => self.studio.status = format!("share refused: {e}"),
                    }
                }
                palette::ShellVerb::ImportPack(i) => {
                    let shelf = pattern_pack::list(&pattern_pack::cache_dir());
                    match shelf.get(i as usize) {
                        None => self.studio.status =
                            "that pack is gone from the shelf".into(),
                        Some((_, path)) => {
                            match std::fs::read(path).ok()
                                .and_then(|b| pattern_pack::PatternPack::decode(&b))
                                .ok_or_else(|| "unreadable pack".to_string())
                                .and_then(|p| self.studio.import_pattern_pack(p)) {
                                Ok(landed) => for (id, audio, raw) in landed {
                                    self.peaks.insert(id,
                                        self.build_peaks(&audio));
                                    self.audio.insert(id, audio);
                                    self.studio.register_asset(id, &raw);
                                },
                                Err(e) => self.studio.status =
                                    format!("import refused: {e}"),
                            }
                        }
                    }
                }
                palette::ShellVerb::ToggleWorkbench => {
                    self.studio.combined = !self.studio.combined;
                }
                palette::ShellVerb::ToggleLibrary => {
                    self.library_open = !self.library_open;
                }
                palette::ShellVerb::SwitchSong(id) => {
                    self.studio.switch_song(id);
                    self.studio.status = match id {
                        Some(id) => match self.studio.session.doc.project.groups.get(&id) {
                            Some(g) => format!("song: {}", g.name),
                            None => "song".into(),
                        },
                        None => "whole project".into(),
                    };
                }
            },
        }
    }

    /// Start or finish a vocal take. A finished take is trimmed, imported
    /// as an ordinary source, and placed as a clip AT THE PLAYHEAD WHERE
    /// IT STARTED — recording then behaves like every other edit: one op,
    /// undoable, shareable, archivable. Its asset is stored FLAC-encoded,
    /// so a phone session's takes do not cost three times what they need.
    /// Put the shell in one of the named rooms — a preset writes only the
    /// layout fields it owns (see `workspace::LAYOUTS`' docs for why
    /// presets beat a dock here), through the same vocabulary the sidecar
    /// stores, so what C-1 does and what a saved sidecar restores can
    /// never drift apart.
    fn apply_layout(&mut self, ix: usize) {
        let Some(l) = workspace::LAYOUTS.get(ix) else { return };
        if let Some(p) = studio_egui::touch::Panel::ALL.iter()
            .find(|p| p.label() == l.panel) { self.studio.panel = *p; }
        self.studio.combined = l.combined;
        self.side_slot = match l.side_slot {
            "viewport" => SideSlot::Viewport, "rack" => SideSlot::Rack,
            "program" => SideSlot::Program, "mixer" => SideSlot::Mixer,
            _ => SideSlot::Off,
        };
        self.library_open = l.library_open;
        self.studio.status = format!("workspace: {}", l.name);
    }

    /// Join or leave the call. Refuses without a carrier: the call rides
    /// the same wire the ops do, and with no wire there is nobody on it.
    fn toggle_call(&mut self) {
        if self.carrier.is_none() && !self.call.on() {
            self.studio.status =
                "no session wire — start with STUDIO_LISTEN or STUDIO_CONNECT to call".into();
            return;
        }
        self.call.toggle(&mut *self.mic);
        self.studio.status = if self.call.on() {
            match self.call.listen_only() {
                Some(why) => format!("joined the call, listen-only: {why}"),
                None => format!("joined the call from {} at {} Hz — headphones avoid echo",
                                self.mic.device_name(), self.mic.rate()),
            }
        } else { "left the call".into() };
    }

    /// Should the sink's buffer be re-rendered and re-seeked this tick?
    /// Exactly three reasons, and this is the whole list: the CONTENT
    /// changed (`loaded != want` -- switched into/out of a loop, or the
    /// loop range itself changed), a PROJECT EDIT happened
    /// (`arrangement_dirty`), or the PLAYHEAD moved by user action
    /// (`seek_pending`). Pulled out as a pure function -- no `self`, no
    /// sink, no device -- specifically so the exact condition that was
    /// missing `seek_pending` (the backward-seek-then-play bug) can be
    /// tested directly, the same way `Transport`/`video_at` are tested
    /// directly rather than through a full running app. See
    /// `sink_resync_tests` below.
    /// What drives the playhead this tick when the sink reports no position.
    ///
    /// Pulled out as a pure function -- no `self`, no sink, no device -- for
    /// the same reason `should_resync` was: the bug it closes is invisible
    /// without an audio device and a loaded project, and this way it is a
    /// `cargo test` that runs in microseconds. See `silent_clock_tests`.
    ///
    /// `loaded` is what the sink currently holds. `Nothing` genuinely has
    /// nothing to play and must still stop; anything else means a buffer was
    /// loaded and simply has no audible content, which is a missing clock
    /// rather than the end of the timeline.
    fn silent_fallback(loaded: &SinkContent, sink_has_position: bool) -> SilentFallback {
        if sink_has_position {
            SilentFallback::SinkDrives
        } else if *loaded == SinkContent::Nothing {
            SilentFallback::Stop
        } else {
            SilentFallback::WallClock
        }
    }

    /// Peaks for a freshly decoded source, in whatever band the waveforms
    /// are currently showing — so an import lands looking like everything
    /// else on screen rather than reverting that view one clip at a time.
    /// Recompute the gutter from the raw transcripts through the current
    /// filter, and say how much it kept — "fillers: 143 across 55 clips" is
    /// the number a junk-cutting pass wants before it starts.
    fn refilter_words(&mut self) -> String {
        let corpus = studio_voice::corpus_counts(self.words_raw.values());
        self.words = self.words_raw.iter()
            .map(|(id, segs)| (*id, studio_voice::filter_words(segs, &self.word_filter, &corpus)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let hits: usize = self.words.values().map(|v| v.len()).sum();
        match &self.word_filter {
            studio_voice::WordFilter::All => format!("transcript gutter: all words ({} clips)", self.words.len()),
            f => format!("transcript gutter: {} — {hits} across {} clip(s) (word times are estimated within each line)",
                         f.label(), self.words.len()),
        }
    }

    fn build_peaks(&self, audio: &studio_audio::SourceAudio) -> studio_audio::Peaks {
        studio_audio::Peaks::build_band(audio, 256, 256, self.peak_band)
    }

    /// Which clock is driving the playhead right now, in one phrase for the
    /// status bar.
    ///
    /// Written because "play does nothing" is indistinguishable, on screen,
    /// from "play is running off the wrong clock": both are a playhead that
    /// does not move the way you expect, and the difference lives in state
    /// nobody can see. Now the bar says it.
    fn clock_note(playing: bool, rack: bool, sink_pos: Option<usize>, loaded_anything: bool) -> &'static str {
        match (playing || rack, sink_pos, loaded_anything) {
            (false, _, _) => "",
            (true, Some(_), _) => "audio clock",
            (true, None, true) => "wall clock — nothing rendered to play",
            (true, None, false) => "no clock — nothing loaded",
        }
    }

    /// Frames to advance for an elapsed wall-clock interval.
    ///
    /// Clamped so a stalled frame -- a modal dialog, a long render, the
    /// process being suspended -- cannot teleport the playhead across the
    /// timeline. Separate from `silent_fallback` so the clamp is testable on
    /// its own.
    fn silent_frames(dt_secs: f64, rate: u32) -> i64 {
        const MAX_STEP: f64 = 0.25;
        if !dt_secs.is_finite() || dt_secs <= 0.0 {
            return 0;
        }
        (dt_secs.min(MAX_STEP) * rate as f64) as i64
    }

    fn should_resync(loaded: &SinkContent, want: &SinkContent, arrangement_dirty: bool, seek_pending: bool) -> bool {
        loaded != want || arrangement_dirty || seek_pending
    }

    /// Unwraps a counter that cycles every `period` frames (a looped
    /// buffer's own read position) into how many NEW frames elapsed since
    /// the last call. Pure arithmetic, no state of its own -- the caller
    /// (`sync_sink`) owns `prev` across calls. Assumes at most one wrap
    /// between calls, which holds as long as this is called every UI tick
    /// (far more often than once per loop period); if that assumption is
    /// somehow violated, this under-counts by whole periods rather than
    /// going negative -- the safe direction to be wrong in for a
    /// display-only clock (video looks briefly paused, never jumps
    /// backward).
    fn unwrap_cycled_frames(prev: Option<usize>, current: usize, period: usize) -> i64 {
        match prev {
            None => 0,
            Some(p) if current >= p => (current - p) as i64,
            Some(p) => (period.saturating_sub(p) + current) as i64,
        }
    }

    /// Keep the sink truthful for whoever is currently playing --
    /// arrangement (timeline), rack loop, or neither. Extracted from the
    /// egui `ui()` method (it touches only `self.sink`/`self.studio`/
    /// `self.mic`/`self.passes`, never `ui`/`frame`) specifically so it can
    /// be called from a test with a fake `PlaybackSink` -- see
    /// `sink_sync_tests` below for the backward-seek regression this
    /// closes.
    fn sync_sink(&mut self) {
        // ── one sink, one owner ───────────────────────────────────────────
        // Priority is whoever the user pressed play on last (the key handler
        // enforces exclusivity); this block just keeps the sink's buffer
        // truthful for the current owner and idles it for nobody.
        if self.studio.transport.playing {
            let window = looping::Window::new(self.studio.transport.loop_range, self.sink.rate());
            let want = match window { Some(w) => SinkContent::ArrangementLoop(w), None => SinkContent::Arrangement };
            if Self::should_resync(&self.loaded, &want, self.studio.arrangement_dirty, self.studio.seek_pending) {
                let (buf, looped, seek) = match window {
                    // The range alone, looped by the callback — sample
                    // exact, which loop-record depends on.
                    Some(w) => (self.studio.render_arrangement_range(self.sink.rate(), &self.audio, w.start, w.end),
                                true, w.frame(self.studio.transport.position())),
                    None => (self.studio.render_arrangement(self.sink.rate(), &self.audio), false,
                             self.studio.transport.position().to_samples(self.sink.rate()).max(0) as usize),
                };
                self.sink.set_buffer(std::sync::Arc::new(buf), 2, looped);
                self.sink.seek(seek);
                self.studio.arrangement_dirty = false;
                self.studio.seek_pending = false;
                self.loaded = want;
                // Fresh buffer load: the video-facing clock restarts from
                // wherever the timeline actually is right now (NOT from
                // the loop's own start -- a seek into the middle of a loop
                // range must resume video from there, same as the audio
                // seek two lines up does).
                self.studio.loop_video_base = self.studio.transport.position();
                self.studio.loop_video_elapsed = 0;
                self.studio.loop_last_raw_frame = None;
            }
            self.sink.set_playing(true);
            if let Some(f) = self.sink.position() {
                self.silent_clock = None;
                // Video follows audio, never the reverse (transport's rule №1)
                // — the sink's consumed frames ARE the playhead. EXCEPT
                // while looping: the sink's own position cycles forever
                // inside the loop window, and "the timeline should
                // function like a timeline" (never freezing, never
                // trapped) means video must keep advancing regardless.
                // `at` (below) is genuinely wrapped -- still needed for
                // loop-record pass detection -- while the position that
                // actually drives video/the UI ruler is the separately
                // unwrapped unbounded clock.
                let at = match window {
                    Some(w) => w.timeline(f),
                    None => studio_core::Ticks::from_samples(f as i64, self.sink.rate()),
                };
                match window {
                    Some(w) => {
                        let delta = Self::unwrap_cycled_frames(self.studio.loop_last_raw_frame, f, w.frames());
                        self.studio.loop_video_elapsed += delta;
                        self.studio.loop_last_raw_frame = Some(f);
                        let video_at = self.studio.loop_video_base.saturating_add(studio_core::Ticks::from_samples(self.studio.loop_video_elapsed, self.sink.rate()));
                        self.studio.transport.set_position(video_at);
                    }
                    None => self.studio.transport.set_position(at),
                }
                // Loop-record: the playhead came round while the take
                // ran — what the microphone caught is one pass, placed
                // at the loop's start on the next free lane. Uses the
                // WRAPPED `at`, not the unwrapped video clock -- pass
                // detection is defined in terms of the loop's own cycle.
                if let Some(w) = window {
                    if self.mic.recording() && self.passes.observe(at) {
                        self.commit_pass(w);
                    }
                }
            } else if !self.sink.playing() {
                // THE PLAYHEAD-NEVER-ADVANCES BUG.
                //
                // "Video follows audio, never the reverse" (transport's rule
                // №1) is right for a project with audio, and wrong for one
                // without. `NullSink::position()` returns None when
                // `frames == 0`, and its `playing()` is
                // `playing && (looped || raw_pos() < frames)` — also false at
                // zero frames. So an arrangement that renders NO AUDIO landed
                // here and was stopped with "reached the end" before a single
                // frame elapsed. A video-only timeline — the kdenlive case —
                // could never play at all, and a project whose only clip was
                // a silent/unrendered pattern behaved the same way.
                //
                // An empty buffer is not the end of the timeline; it is the
                // absence of a clock. So supply one. `Transport::advance`
                // already exists for exactly this, takes a frame count rather
                // than wall time, and was until now called only from its own
                // tests — there was no clock in the app at all.
                //
                // The audio path is untouched: whenever the sink can report a
                // position it still wins, and `silent_clock` is cleared above
                // so a project that gains audio hands the clock straight back.
                match Self::silent_fallback(&self.loaded, false) {
                    SilentFallback::WallClock => {
                        let now = std::time::Instant::now();
                        let last = self.silent_clock.replace(now);
                        if let Some(prev) = last {
                            let dt = now.duration_since(prev).as_secs_f64();
                            let frames = Self::silent_frames(dt, self.sink.rate());
                            self.studio.transport.advance(frames);
                        }
                    }
                    SilentFallback::Stop | SilentFallback::SinkDrives => {
                        self.silent_clock = None;
                        self.studio.transport.stop();
                        self.studio.status = "reached the end".into();
                    }
                }
            }
        } else if self.studio.rack_playing {
            if self.loaded != SinkContent::RackLoop || self.studio.loop_dirty {
                let samples = self.studio.render_loop(self.sink.rate(), &self.audio);
                self.sink.set_buffer(std::sync::Arc::new(samples), 1, true);
                self.studio.loop_dirty = false;
                self.loaded = SinkContent::RackLoop;
            }
            self.sink.set_playing(true);
        } else {
            self.sink.set_playing(false);
        }
    }

    /// One loop-record pass: cut the take so far and place it at the
    /// loop's start, trimmed to the loop's length (the frame's worth of
    /// overshoot past the wrap belongs to the next pass and is dropped —
    /// see `looping`'s module docs).
    fn commit_pass(&mut self, w: looping::Window) {
        let Some(take) = self.mic.split_take() else { return };
        let mut audio = take.audio;
        let max = (w.frames() as f64 * audio.sample_rate as f64 / w.rate as f64) as usize;
        for ch in &mut audio.channels { ch.truncate(max); }
        if audio.frames() == 0 { return; }
        let name = format!("take {} pass {}", self.studio.session.doc.project.sources.len() + 1, self.passes.done);
        let path = format!("takes/{}.flac", export::sanitize(&name));
        let id = self.studio.import_audio(&name, &path, &audio);
        let appended = self.studio.rack.channels.len().saturating_sub(1);
        self.studio.rack.channels.truncate(appended);
        for p in &mut self.studio.rack.patterns { p.rows.truncate(appended); }
        let len = studio_core::Ticks::from_samples(audio.frames() as i64, audio.sample_rate);
        if let Some(track) = self.studio.take_target() {
            self.studio.place_take(id, len, w.start, track);
        }
        if let Ok(bytes) = studio_media::flac::encode_flac(&audio, id.0) {
            self.studio.register_asset(id, &bytes);
        }
        self.peaks.insert(id, self.build_peaks(&audio));
        self.audio.insert(id, audio);
        self.studio.status = format!("pass {}: {}", self.passes.done, self.studio.status);
        if take.dropped_blocks > 0 {
            self.studio.status = format!("{} — WARNING: {} block(s) dropped", self.studio.status, take.dropped_blocks);
        }
    }

    fn toggle_take(&mut self) {
        if self.mic.recording() {
            let Some(take) = self.mic.stop() else {
                self.studio.status = "take stopped — nothing was captured".into();
                return;
            };
            let dropped = take.dropped_blocks;
            let audio = capture::trim_silence(&take.audio, 0.004, 80.0);
            let name = format!("take {}", self.studio.session.doc.project.sources.len() + 1);
            let path = format!("takes/{}.flac", export::sanitize(&name));
            let id = self.studio.import_audio(&name, &path, &audio);
            // The rack channel import_audio appends is not what a vocal
            // take wants; drop it and place a CLIP instead.
            let appended = self.studio.rack.channels.len().saturating_sub(1);
            self.studio.rack.channels.truncate(appended);
            for p in &mut self.studio.rack.patterns { p.rows.truncate(appended); }
            let len = studio_core::Ticks::from_samples(
                audio.frames() as i64, audio.sample_rate);
            // The second take of a phrase goes to a lane, not onto the
            // first — see studio_core's takes module.
            if let Some(track) = self.studio.take_target() {
                self.studio.place_take(id, len, self.take_from, track);
            }
            if let Ok(bytes) = studio_media::flac::encode_flac(&audio, id.0) {
                self.studio.register_asset(id, &bytes);
            }
            self.peaks.insert(id, self.build_peaks(&audio));
            self.audio.insert(id, audio);
            if dropped > 0 {
                // Said, not swallowed: a take with holes must not be
                // built on unknowingly.
                self.studio.status = format!("{} — WARNING: {dropped} block(s) dropped, check the take",
                                             self.studio.status);
            }
        } else {
            self.take_from = self.studio.playhead();
            self.passes = looping::Passes::start();
            match self.mic.start() {
                Ok(()) => self.studio.status =
                    format!("recording from {} at {} Hz…{}",
                            self.mic.device_name(), self.mic.rate(),
                            if self.studio.transport.loop_range.is_some() { " (a take per pass of the loop)" } else { "" }),
                Err(e) => self.studio.status = e,
            }
        }
    }

    /// Run the search with what the panel currently holds.
    fn run_find(&mut self) {
        let Some(ui) = &self.find_ui else { return };
        // No TTS in this build: the acoustic pass needs a rendering of the
        // keyword and says so rather than pretending a name search was a
        // sound search. voiceterm's synthesis (or a recorded example) is
        // what fills this in.
        let spoken: Option<studio_audio::SourceAudio> = None;
        let hits = find::find(&self.studio.session.doc.project, &self.audio,
                              &ui.query, spoken.as_ref(), ui.gate,
                              &find::NoVerifier, 24);
        let sound_wanted = ui.by_sound;
        if let Some(ui) = &mut self.find_ui {
            ui.hits = hits;
            ui.ran = true;
        }
        if sound_wanted && spoken.is_none() {
            self.studio.status =
                "searched by name — the sound pass needs a spoken example \
                 (no synthesis in this build)".into();
        }
    }

    /// The find panel: a query, a gate, and results that jump the playhead.
    fn find_window(&mut self, ctx: &egui::Context) {
        let Some(mut ui_state) = self.find_ui.take() else { return };
        let mut open = true;
        let mut run = false;
        let mut goto: Option<studio_core::Ticks> = None;
        egui::Window::new("Find in the timeline").open(&mut open).resizable(false)
            .default_width(460.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut ui_state.query).id_salt("lib-ui_state_query"));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        run = true;
                    }
                    if ui.button("find").clicked() { run = true; }
                });
                ui.horizontal(|ui| {
                    for (g, label) in [(find::Gate::Everything, "everything"),
                                       (find::Gate::WithAudio, "only tracks with audio"),
                                       (find::Gate::Silent, "silent only")] {
                        if ui.add(egui::Button::selectable(ui_state.gate == g, label))
                            .clicked() { ui_state.gate = g; run = true; }
                    }
                });
                ui.separator();
                if ui_state.ran && ui_state.hits.is_empty() {
                    ui.colored_label(egui::Color32::GRAY, "nothing found");
                }
                for h in &ui_state.hits {
                    ui.horizontal(|ui| {
                        if ui.small_button("→").on_hover_text("go there").clicked() {
                            goto = Some(h.at);
                        }
                        // The provenance of every hit is on the row: a
                        // name match, a cheap acoustic guess, or something
                        // a recognizer actually read.
                        let tag = match (h.why, &h.verified) {
                            (find::Why::Name, _) => "name".to_string(),
                            (find::Why::Silence, _) => "silent".to_string(),
                            (find::Why::Sound, Some(t)) => format!("heard: {t}"),
                            (find::Why::Sound, None) =>
                                format!("sound ~{:.2}", h.distance.unwrap_or(0.0)),
                        };
                        ui.weak(tag);
                        ui.label(&h.label);
                        ui.weak(format!("{:.1}s", h.at.to_seconds()));
                    });
                }
            });
        if run { self.find_ui = Some(ui_state); self.run_find(); }
        else if open { self.find_ui = Some(ui_state); }
        if let Some(at) = goto { self.studio.seek(at); }
    }

    /// The mastering pass window: measurements, findings, the strength
    /// dial, and one apply. Diagnosis stays readable and the dial stays
    /// movable BEFORE anything touches the project — the assistant
    /// module's own thesis, which a UI that applied on click would betray.
    fn mix_pass_window(&mut self, ctx: &egui::Context) {
        let Some(mut m) = self.mix_pass.take() else { return };
        let mut open = true;
        let mut apply = false;
        let mut close = false;
        egui::Window::new("Mix & master pass").open(&mut open).resizable(false)
            .collapsible(false).show(ctx, |ui| {
                ui.label(format!("measured on '{}':", m.bus_name));
                ui.monospace(format!(
                    "peak {:+.1} dB · crest {:.1} dB · correlation {:+.2} · low width {:.2}",
                    m.measured.peak_db, m.measured.crest_db,
                    m.measured.correlation, m.measured.low_width));
                ui.separator();
                ui.label(&m.proposal.summary);
                if m.proposal.ops.is_empty() {
                    ui.colored_label(egui::Color32::GRAY,
                        "nothing proposed — which is a real answer, not a failure");
                } else {
                    ui.label(format!("{} move(s), confidence {:.0}%",
                                     m.proposal.ops.len(), m.proposal.confidence * 100.0));
                    for op in &m.proposal.ops {
                        ui.monospace(describe_op(op));
                    }
                }
                ui.separator();
                let before = m.strength;
                ui.add(egui::Slider::new(&mut m.strength, 0.0..=1.0).text("strength"));
                ui.colored_label(egui::Color32::GRAY,
                    "0 proposes nothing; 0.3 is the default, the middle of the range \
                     people actually keep");
                if (m.strength - before).abs() > 1e-4 {
                    m.proposal = studio_copilot::mix_pass(
                        &m.spec, m.measured.crest_db, m.measured.correlation,
                        m.measured.low_width, m.bus,
                        self.studio.session.doc.project.busses.get(&m.bus)
                            .map(|b| b.gain).unwrap_or(1.0),
                        m.strength);
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!m.proposal.ops.is_empty(),
                                      egui::Button::new("apply as one edit")).clicked() {
                        apply = true;
                    }
                    if ui.button("close").clicked() { close = true; }
                });
            });
        if apply {
            // Composed onto the bus's CURRENT chain (mix_pass emits
            // standalone one-effect chains it cannot compose itself), then
            // applied as one undo group — a mastering pass that takes six
            // presses of `u` to reverse is a pass nobody dares try.
            let existing = self.studio.session.doc.project.busses.get(&m.bus)
                .map(|b| b.chain.clone()).unwrap_or_default();
            let ops = mix_assistant::compose(&existing, &m.proposal.ops, m.bus);
            let n = ops.len();
            self.studio.edit_group(ops, &format!("mix pass on '{}' ({n} move(s))", m.bus_name));
            return; // the window closes on apply: the proposal is now history
        }
        if open && !close { self.mix_pass = Some(m); }
    }

    /// The save-archive popup: the fidelity slider, the credits/authorized
    /// collaborators, the per-device search paths — then the picker, then
    /// one file on disk that opens whole or refuses whole.
    fn archive_window(&mut self, ctx: &egui::Context) {
        let Some(mut a) = self.archive_ui.take() else { return };
        let mut open = true;
        let mut close = false;
        egui::Window::new("Save archive").open(&mut open).resizable(false)
            .collapsible(false).show(ctx, |ui| {
                ui.label("project fidelity — what the embedded audio costs and sounds like:");
                let levels = archive::Level::ALL;
                a.level_ix = a.level_ix.min(levels.len() - 1);
                ui.add(egui::Slider::new(&mut a.level_ix, 0..=levels.len() - 1)
                    .show_value(false));
                ui.label(levels[a.level_ix].label());
                ui.separator();
                ui.label("artists / authorized collaborators:");
                let mut remove = None;
                for (i, name) in a.artists.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if ui.small_button("✕").clicked() { remove = Some(i); }
                    });
                }
                if let Some(i) = remove { a.artists.remove(i); }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.artist_input).id_salt("lib-a_artist_input"));
                    if ui.button("add").clicked() && !a.artist_input.trim().is_empty() {
                        a.artists.push(a.artist_input.trim().to_string());
                        a.artist_input.clear();
                    }
                });
                ui.separator();
                ui.label("search paths for full-quality per-device artifacts:");
                let mut remove = None;
                for (i, p) in a.paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(p);
                        if ui.small_button("✕").clicked() { remove = Some(i); }
                    });
                }
                if let Some(i) = remove { a.paths.remove(i); }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.path_input).id_salt("lib-a_path_input"));
                    if ui.button("add").clicked() && !a.path_input.trim().is_empty() {
                        a.paths.push(a.path_input.trim().to_string());
                        a.path_input.clear();
                    }
                });
                if !a.note.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &a.note);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("choose destination…").clicked() {
                        let meta = archive::ArchiveMeta {
                            artists: a.artists.clone(),
                            search_paths: a.paths.clone(),
                            level: levels[a.level_ix],
                        };
                        let name = format!("{}.studioarch",
                            export::sanitize(&self.studio.session.doc.project.name));
                        let dest = match export::pick_save(&name) {
                            Some(p) => Some(p),
                            None if export::PICKER_AVAILABLE => None, // cancelled
                            None => {
                                self.studio.status =
                                    "no native file dialog — archiving to ./exports".into();
                                Some(std::path::Path::new("exports").join(&name))
                            }
                        };
                        if let Some(dest) = dest {
                            match self.studio.pack_archive(&self.audio, &meta) {
                                Ok((bytes, report)) => {
                                    if let Some(d) = dest.parent() {
                                        let _ = std::fs::create_dir_all(d);
                                    }
                                    match std::fs::write(&dest, &bytes) {
                                        Ok(()) => {
                                            let mut extra = String::new();
                                            if report.deduped_bytes > 0 {
                                                extra.push_str(&format!(
                                                    ", dedup saved {:.1} MB",
                                                    report.deduped_bytes as f64 / 1e6));
                                            }
                                            if !report.similar.is_empty() {
                                                // A report, never an action:
                                                // see archive's module docs
                                                // on distortion.
                                                extra.push_str(&format!(
                                                    ", {} near-duplicate pair{} (consolidate by hand if they are one take)",
                                                    report.similar.len(),
                                                    if report.similar.len() == 1 { "" } else { "s" }));
                                            }
                                            if !report.missing.is_empty() {
                                                extra.push_str(&format!(
                                                    ", {} sources have NO audio",
                                                    report.missing.len()));
                                            }
                                            // Remembered app-wide: the
                                            // next unnamed project is born
                                            // beside the last archive, so
                                            // "wherever I archived" is
                                            // where work accumulates.
                                            if let Some(d) = dest.parent() {
                                                remember_archive_root(d);
                                            }
                                            self.studio.status = format!(
                                                "archived {} ({:.1} MB{extra})",
                                                dest.display(),
                                                bytes.len() as f64 / 1e6);
                                            close = true;
                                        }
                                        Err(e) => a.note = format!("write failed: {e}"),
                                    }
                                }
                                Err(e) => a.note = format!("refused: {e}"),
                            }
                        }
                    }
                    if ui.button("cancel").clicked() { close = true; }
                });
            });
        if !close && open { self.archive_ui = Some(a); }
    }

    /// The export dialog: the plan's questions, answered as values, then
    /// the native picker, then the run. A refusal (empty selection, all
    /// channels muted) is shown INLINE — a bad plan is a message to fix,
    /// not a window that closed on you.
    fn export_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut plan) = self.export_dialog else { return };
        let mut open = true;
        let mut close = false;
        egui::Window::new("Export").open(&mut open).resizable(false)
            .collapsible(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("format:");
                    if ExportFormat::Opus.available() {
                        ui.selectable_value(&mut plan.format, ExportFormat::Opus, "opus");
                    }
                    ui.selectable_value(&mut plan.format, ExportFormat::Wav, "wav");
                    ui.selectable_value(&mut plan.format, ExportFormat::Mp3, "mp3")
                        .on_hover_text("for the group chat — through ffmpeg");
                    ui.selectable_value(&mut plan.format, ExportFormat::M4a, "m4a")
                        .on_hover_text("for iMessage and the like — through ffmpeg");
                });
                ui.horizontal(|ui| {
                    ui.label("scope:");
                    ui.selectable_value(&mut plan.scope, export::Scope::Full, "full arrangement");
                    let has_sel = !self.studio.selection.is_empty();
                    ui.add_enabled_ui(has_sel, |ui| {
                        ui.selectable_value(&mut plan.scope, export::Scope::Selection, "selection")
                            .on_disabled_hover_text("select clips on the timeline first");
                    });
                    ui.selectable_value(&mut plan.scope, export::Scope::RackLoop, "rack loop");
                    // The album: one file per song, timeline order. Greyed
                    // rather than hidden with no songs — a control that
                    // vanishes teaches nothing about why.
                    let has_songs = !self.studio.session.doc.project.groups.is_empty();
                    ui.add_enabled_ui(has_songs, |ui| {
                        ui.selectable_value(&mut plan.scope, export::Scope::EachSong, "each song")
                            .on_disabled_hover_text("group clips into songs first");
                    });
                });
                ui.checkbox(&mut plan.stems,
                    if plan.scope == export::Scope::RackLoop { "stems — one file per channel" }
                    else { "stems — one file per track" });
                match plan.format {
                    ExportFormat::Wav => {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("exp-rate")
                                .selected_text(format!("{} Hz", plan.rate))
                                .show_ui(ui, |ui| {
                                    for r in [44_100u32, 48_000, 96_000, 192_000] {
                                        ui.selectable_value(&mut plan.rate, r, format!("{r} Hz"));
                                    }
                                });
                            egui::ComboBox::from_id_salt("exp-layout")
                                .selected_text(plan.layout.label())
                                .show_ui(ui, |ui| {
                                    for f in studio_audio::format::ChannelFormat::ALL {
                                        ui.selectable_value(&mut plan.layout, f, f.label());
                                    }
                                });
                        });
                    }
                    ExportFormat::Opus => {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("exp-bitrate")
                                .selected_text(format!("{} kbps", plan.opus_bitrate / 1000))
                                .show_ui(ui, |ui| {
                                    for b in [96_000u32, 128_000, 160_000, 192_000, 256_000] {
                                        ui.selectable_value(&mut plan.opus_bitrate, b,
                                                            format!("{} kbps", b / 1000));
                                    }
                                });
                            ui.colored_label(egui::Color32::GRAY,
                                "48 kHz stereo — the codec's contract");
                        });
                    }
                    ExportFormat::Mp3 | ExportFormat::M4a => {
                        ui.colored_label(egui::Color32::GRAY,
                            "stereo, 192 kbps, encoded by ffmpeg — the format a phone shares");
                    }
                }
                if !self.export_note.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.export_note);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("choose destination…").clicked() {
                        close = self.begin_export(&plan);
                    }
                    if ui.button("cancel").clicked() { close = true; }
                });
            });
        self.export_dialog = if close || !open { None } else { Some(plan) };
    }

    /// Plan → picker → run. Returns true when the dialog should close
    /// (a run started, or the fallback path was taken); false keeps it
    /// open with `export_note` explaining why.
    fn begin_export(&mut self, plan: &export::Plan) -> bool {
        let project = &self.studio.session.doc.project;
        let suggested = format!("{}.{}", export::sanitize(&project.name), plan.format.ext());
        // Plan first: a refusal must show BEFORE a native dialog opens over it.
        enum Planned {
            Arr(Vec<export::Job>),
            Rack(Vec<(String, studio_live::Rack)>),
        }
        let planned = if plan.scope == export::Scope::RackLoop {
            export::plan_rack_jobs(plan.stems, &self.studio.rack, self.studio.current_pattern)
                .map(Planned::Rack)
        } else {
            export::plan_jobs(plan, project, &self.studio.selection).map(Planned::Arr)
        };
        let planned = match planned {
            Ok(p) => p,
            Err(e) => { self.export_note = format!("refused: {e}"); return false; }
        };
        let dest = if plan.stems {
            export::pick_folder().map(export::Dest::Folder)
        } else {
            export::pick_save(&suggested).map(export::Dest::File)
        };
        let dest = match dest {
            Some(d) => d,
            // With a real picker, None means the person cancelled it —
            // honour that. Without one, None means there is nothing to
            // open: fall back to ./exports and SAY so.
            None if export::PICKER_AVAILABLE => return false,
            None => {
                self.studio.status =
                    "no native file dialog on this system — exporting to ./exports".into();
                if plan.stems {
                    export::Dest::Folder("exports".into())
                } else {
                    export::Dest::File(std::path::Path::new("exports").join(&suggested))
                }
            }
        };
        self.export_done.clear();
        self.export_progress = (0, 0, 0.0);
        self.export_run = Some(match planned {
            Planned::Arr(jobs) => export::start_arrangement(
                *plan, jobs, dest, project.clone(), self.audio.clone(), self.waker()),
            Planned::Rack(jobs) => export::start_rack(
                *plan, jobs, dest, self.studio.current_pattern,
                self.audio.clone(), project.name.clone(), self.waker()),
        });
        true
    }

    /// The render tracker: a bar that moves, the files as they land, and a
    /// cancel that means soon. Repaint is requested while a run is live —
    /// Raise a stabilize ticket for the selected clip and render it on a
    /// worker. Handles are half a second each side, so a later nudge of the
    /// trim still lands inside the fixed picture.
    fn start_stabilize(&mut self, kind: palette::StabilizeKind) {
        use studio_video::stabilize::{Budget, Flavor};
        if self.touchup_run.is_some() {
            self.studio.status = "a touch-up is already rendering".into();
            return;
        }
        let Some(&id) = self.studio.selection.first() else {
            self.studio.status = "select the clip to stabilize first".into();
            return;
        };
        let project = &self.studio.session.doc.project;
        let Some(clip) = project.clips.get(&id).cloned() else { return };
        let Some(src) = project.sources.get(&clip.source).cloned() else { return };
        if !src.has_video {
            self.studio.status = "that clip has no picture to stabilize".into();
            return;
        }
        let path = std::path::PathBuf::from(&src.path);
        let Ok(probe) = studio_video::probe(&path) else {
            self.studio.status = format!("could not read {}", src.path);
            return;
        };
        let (flavor, budget) = match kind {
            // Each flavour gets the crop it actually needs: locking a shot
            // costs the most, de-jerking the least. A single budget for all
            // three would either starve Still or overspend Dejerk.
            palette::StabilizeKind::Still => (Flavor::Still, Budget { max_zoom: 1.15 }),
            palette::StabilizeKind::Smooth => (Flavor::Smooth { sigma: 12.0 }, Budget { max_zoom: 1.10 }),
            palette::StabilizeKind::Dejerk => (Flavor::Dejerk { threshold: 3.0 }, Budget { max_zoom: 1.04 }),
        };
        let _ = (path, probe);
        self.start_touchup(studio_video::touchup::Fix::Stabilize { flavor, budget },
                           format!("stabilizing ({kind:?})"));
    }

    /// Raise a ticket for the selected clip and render it on a worker. The
    /// selection, source and probe checks are the same for every fix, so
    /// they live here rather than once per flavour of fix.
    fn start_touchup(&mut self, fix: studio_video::touchup::Fix, what: String) {
        if self.touchup_run.is_some() {
            self.studio.status = "a touch-up is already rendering".into();
            return;
        }
        let Some(&id) = self.studio.selection.first() else {
            self.studio.status = "select the clip to fix first".into();
            return;
        };
        let project = &self.studio.session.doc.project;
        let Some(clip) = project.clips.get(&id).cloned() else { return };
        let Some(src) = project.sources.get(&clip.source).cloned() else { return };
        if !src.has_video {
            self.studio.status = "that clip has no picture to fix".into();
            return;
        }
        let path = std::path::PathBuf::from(&src.path);
        let Ok(probe) = studio_video::probe(&path) else {
            self.studio.status = format!("could not read {}", src.path);
            return;
        };
        let ticket = studio_video::touchup::Ticket {
            clip: id, source: clip.source, path: path.clone(),
            src_in: clip.src_in, len: clip.len, handle: 0.5,
            fixes: vec![fix], note: what.clone(),
        };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "shot".into());
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir);
        let mut out = dir.join(format!("{stem}-touchup.mp4"));
        let mut n = 2;
        while out.exists() { out = dir.join(format!("{stem}-touchup-{n}.mp4")); n += 1; }
        let size = studio_video::framepipe::Size { w: probe.width, h: probe.height };
        self.touchup_run = Some(touchup_job::start(ticket, out, size, probe.fps.max(1.0), self.waker()));
        self.studio.status = format!("{what} — this runs in the background");
    }

    /// Clean-plate the selected clip. Same ticket machinery as the
    /// stabilizer, a different fix.
    fn start_clean_plate(&mut self, radius: usize, moving: bool) {
        let fix = studio_video::touchup::Fix::CleanPlate {
            settings: studio_video::cleanplate::Settings::default(),
            radius, align: moving,
        };
        self.start_touchup(fix, format!("clean plate ±{radius} frames"));
    }

    /// Open the gallery on the first selected picture clip's frame at the
    /// playhead (or its middle, if the playhead is not over it).
    fn open_kaleido_gallery(&mut self) {
        let project = &self.studio.session.doc.project;
        let Some(clip) = self.studio.selection.iter().filter_map(|id| project.clips.get(id))
            .find(|c| project.sources.get(&c.source).map(|s| s.has_video).unwrap_or(false)).cloned() else {
            self.studio.status = "select a picture clip for the gallery".into();
            return;
        };
        let src = &project.sources[&clip.source];
        let ph = self.studio.playhead();
        let at = if ph >= clip.pos && ph < clip.end() { clip.src_in.saturating_add(ph.saturating_sub(clip.pos)) }
                 else { clip.src_in.saturating_add(Ticks(clip.len.0 / 2)) };
        let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        match studio_video::frame_rgb(&base.join(&src.path), at, 320, 180) {
            Ok(px) => self.kaleido_gallery = Some(kaleido_gallery::Gallery::new(px, 320, 180)),
            Err(e) => self.studio.status = format!("could not read a frame for the gallery: {e:?}"),
        }
    }

    /// Build the lanes for a camera group's span and open the window.
    fn open_signals(&mut self, group: studio_core::GroupId) {
        use studio_video::signals;
        const HOP: f64 = signals_panel::HOP;
        let project = &self.studio.session.doc.project;
        let Ok((_, start, end)) = studio_video::multicam::from_group(project, group) else {
            self.studio.status = "that group is not a camera group".into(); return;
        };
        let span = (end.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
        let mut lanes = Vec::new();
        // the kick: the low band of the mix over the span
        let mix = self.studio.render_arrangement_range(16_000, &self.audio, start, end);
        if !mix.is_empty() {
            let mono: Vec<f32> = mix.chunks(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(0.0)) * 0.5).collect();
            lanes.push(signals::onset_lane("kick (mix)", &mono, 16_000, HOP, (20.0, 150.0)));
        }
        // every control channel's level
        for b in project.busses.values().filter(|b| b.silent) {
            if let Some(st) = studio_audio::bus_signal(project, &self.audio, &Default::default(), start, end, 16_000, b.id) {
                let mono: Vec<f32> = st.chunks(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(0.0)) * 0.5).collect();
                lanes.push(signals::Signal::new(&format!("level: {}", b.name), HOP,
                    studio_video::particles::envelope(&mono, 16_000, 1.0 / HOP)));
            }
        }
        // where lines start, from the loaded transcripts
        let starts = signals_panel::line_starts(project, group, start, &self.words_raw);
        if !starts.is_empty() { lanes.push(signals::pulses("line start", &starts, span, HOP, 0.12)); }
        // performer motion per angle: each angle's stretch decoded small, the
        // camera's own motion cancelled -- the one signal that held up in the
        // "All My Life" analysis. Also combined into a lane for WHEN.
        let (angles, _, _) = studio_video::multicam::from_group(project, group).unwrap_or((Vec::new(), start, end));
        let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut angle_motion = Vec::new();
        const MFPS: f64 = 25.0;
        for (k, a) in angles.iter().enumerate() {
            let Some(src) = project.sources.get(&a.source) else { continue };
            let from = start.to_seconds() + a.offset;
            let pipe = studio_video::framepipe::Pipe { input: base.join(&src.path), range: Some((from.max(0.0), span)),
                size: studio_video::framepipe::Size { w: 160, h: 90 }, fps: MFPS };
            let Ok(grey) = pipe.gray() else { continue };
            let m = studio_video::action::body_motion(&grey, 10);
            let mut sorted = m.clone(); sorted.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            let top = sorted.get((sorted.len() as f32 * 0.95) as usize).copied().unwrap_or(1.0).max(1e-3);
            angle_motion.push(signals::Signal::new(&format!("motion: angle {}", k + 1), 1.0 / MFPS,
                m.iter().map(|v| (v / top).min(1.0)).collect()));
        }
        let mut angle_inputs = Vec::new();
        if angle_motion.len() == angles.len() && !angle_motion.is_empty() {
            let n = angle_motion.iter().map(|s| s.values.len()).min().unwrap_or(0);
            let any: Vec<f32> = (0..n).map(|i| angle_motion.iter().map(|s| s.values[i]).fold(0.0, f32::max)).collect();
            let mut lane = signals::Signal::new("motion (any angle)", 1.0 / MFPS, any);
            lane.weight = 0.5;   // on, but a soft tendency -- as measured
            lanes.push(lane);
            angle_inputs.push(signals::AngleInput { name: "motion".into(), per_angle: angle_motion, weight: 1.0 });
        }
        // who is heard: each angle's OWN mic in the speech band, relative to the
        // loudest -- whoever speaks is loudest on the camera nearest them
        let mut envs = Vec::new();
        for a in &angles {
            match self.speech_db(a.source, start.to_seconds() + a.offset, span, MFPS) { Some(e) => envs.push(e), None => break }
        }
        if envs.len() == angles.len() && !envs.is_empty() {
            angle_inputs.push(signals::AngleInput { name: "voice".into(),
                per_angle: signals::relative_voice(&envs, 1.0 / MFPS, 12.0), weight: 0.0 });
        }
        // checked AFTER the motion lanes: a first version gave up here before
        // building them, so pictures with no audio never got their motion lane
        if lanes.is_empty() {
            self.studio.status = "no signals over this group yet: no audio, motion, control channels or transcripts".into();
            return;
        }
        let marks = signals_panel::marks(project, start, end);
        let prefer = if angle_inputs.is_empty() { signals::Prefer::RoundRobin } else { signals::Prefer::MostMotion };
        self.signals_panel = Some(signals_panel::Panel { group, lanes, marks, span, threshold: 0.35, min_gap: 1.5,
            planned: Vec::new(), angle_inputs, prefer, no_repeat: true });
    }

    /// A source's own speech-band loudness in dB, one value per 1/`fps` s,
    /// over `span` seconds from `from` in the file: one-pole 300 Hz high-pass
    /// and 3 kHz low-pass, then RMS. `None` if its audio is not decoded yet.
    fn speech_db(&self, source: studio_core::SourceId, from: f64, span: f64, fps: f64) -> Option<Vec<f32>> {
        let au = self.audio.get(&source)?;
        let sr = au.sample_rate as f64;
        let a0 = (from.max(0.0) * sr) as usize;
        let a1 = (a0 as f64 + span * sr) as usize;
        let chans = au.channels.len().max(1);
        let len = au.channels.first().map(|c| c.len()).unwrap_or(0);
        let mono: Vec<f32> = (a0..a1.min(len)).map(|i| au.channels.iter().map(|c| c[i]).sum::<f32>() / chans as f32).collect();
        let (ah, al) = ((-std::f64::consts::TAU * 300.0 / sr).exp() as f32, (-std::f64::consts::TAU * 3000.0 / sr).exp() as f32);
        let (mut hy, mut hx, mut ly) = (0.0f32, 0.0f32, 0.0f32);
        let band: Vec<f32> = mono.iter().map(|&x| { hy = ah * (hy + x - hx); hx = x; ly = (1.0 - al) * hy + al * ly; ly }).collect();
        let hop = ((sr / fps) as usize).max(1);
        Some(band.chunks(hop).map(|c| 20.0 * ((c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).max(1e-6).log10()).collect())
    }

    /// The comic auto mode on a camera group: plan from its transcripts, sides
    /// from its mics, render on a worker, land when done.
    /// Open the comic setup over a camera group: one frame per angle from the
    /// middle of the group, for choosing roles and regions by looking.
    fn open_comic_setup(&mut self, group: studio_core::GroupId) {
        let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let project = &self.studio.session.doc.project;
        let Ok((angles, start, end)) = studio_video::multicam::from_group(project, group) else {
            self.studio.status = "that group is not a camera group".into(); return;
        };
        let mid = (start.to_seconds() + end.to_seconds()) / 2.0;
        let mut frames = Vec::new();
        for a in &angles {
            let Some(src) = project.sources.get(&a.source) else { continue };
            match studio_video::frame_rgb(&base.join(&src.path), Ticks::from_seconds((mid + a.offset).max(0.0)), 320, 180) {
                Ok(px) => frames.push(px),
                Err(_) => frames.push(vec![20u8; 320 * 180 * 3]),   // unreadable: a dark placeholder, still choosable
            }
        }
        self.comic_setup = Some(comic_job::SetupWindow::new(group, frames));
    }

    /// Put rack pattern `ix` on the timeline as a LINKED clip at `at`: on
    /// `on` (a track it was dropped on) or the selected clip's audio track if
    /// it fits, else a new audio track below. One undo.
    fn pattern_to_timeline(&mut self, ix: usize, at: Ticks, on: Option<studio_core::TrackId>) {
        match pattern_clips::place_ops(&self.studio.session.doc.project, &self.studio.rack, ix, at,
                                       self.studio.selection.first().copied(), on) {
            Ok(ops) => {
                let name = self.studio.rack.patterns.get(ix).map(|p| p.name.clone()).unwrap_or_default();
                self.studio.edit_group(ops, "pattern to timeline");
                self.studio.status = format!("'{name}' on the timeline, linked to the rack — edit the pattern and the clip follows");
            }
            Err(e) => self.studio.status = e,
        }
    }

    /// Open the palette straight at one of its questions.
    fn open_palette_at(&mut self, ask: palette::Ask, crumb: &str) {
        self.palette = Some(palette::Palette { pending: Some(ask), path: vec![crumb.to_string()], ..Default::default() });
    }

    /// Open `path`, asking first if the live project has unsaved changes.
    fn request_open(&mut self, path: std::path::PathBuf) {
        if !open_project::is_project(&path) || !path.is_file() {
            self.studio.status = format!("not a Studio project: {}", path.display());
            return;
        }
        // Load it once BEFORE leaving this project. At start-up a file that
        // will not load falls back to a demo (there is nothing else to show);
        // from the palette that would throw away the session you are in and
        // leave you on a demo pointed at the broken file, where Ctrl+S
        // overwrites it. Here, a file that will not open changes nothing.
        let team = Team::from_contacts("solo", &["sc-local".into()]);
        let probe = if path.extension().and_then(|e| e.to_str()) == Some("studioarch") {
            Studio::open_archive(&path, SiteId(1), team).map(|_| ())
        } else {
            Studio::load_from(&path, SiteId(1), team).map(|_| ())
        };
        if let Err(e) = probe {
            self.studio.status = format!("could not open {}: {e} — still on this project", path.display());
            return;
        }
        if open_project::needs_save(&self.studio.session.doc.project, self.saved_project.as_ref()) {
            self.studio.status = format!("this project has unsaved changes — before opening {}:",
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            self.open_pending = Some(path);
            self.open_palette_at(palette::Ask::UnsavedBeforeOpen, "open project");
        } else {
            self.switch_to(path);
        }
    }

    /// Replace the whole shell with one opened on `path` -- exactly what a
    /// launch with that project builds, so nothing of this one lingers.
    fn switch_to(&mut self, path: std::path::PathBuf) {
        let touch = self.touch_seen;
        *self = App::with_project(path.clone());
        self.touch_seen = touch;
        self.studio.status = format!("opened {} — {}", path.display(), self.studio.status);
    }

    /// Save the project where it lives: the workspace sidecar, the file,
    /// and a history commit. The toolbar's save and the open flow's "save,
    /// then open" both come here -- one save, not a second simplified one.
    fn save_project(&mut self) -> Result<(), String> {
        let path = self.project_path.clone();
        workspace::save(&path, &workspace::WorkspaceState {
            library_audio: self.library_roots.audio.display().to_string(),
            library_video: self.library_roots.video.as_ref()
                .map(|p| p.display().to_string()).unwrap_or_default(),
            combined: self.studio.combined,
            panel: self.studio.panel.label().to_string(),
            side_slot: match self.side_slot {
                SideSlot::Off => "off", SideSlot::Viewport => "viewport",
                SideSlot::Mixer => "mixer", SideSlot::Rack => "rack",
                SideSlot::Program => "program",
            }.to_string(),
            px_per_sec: self.studio.current_view().px_per_sec,
            scroll_ticks: self.studio.current_view().scroll.0,
            closed_groups: self.studio.closed_groups.iter()
                .map(|g| g.0).collect(),
            bounce_format: self.bounce_format.token().to_string(),
            voice_domains: self.lexicon.to_tokens(),
            pinned: self.library_prefs.pinned.iter()
                .map(|p| p.display().to_string()).collect(),
            demoted: self.library_prefs.demoted.iter()
                .map(|p| p.display().to_string()).collect(),
            library_open: self.library_open,
            display_name: self.studio.display_name.clone(),
            rebinds: self.rebinds.encode(),
            piano_zones: self.zones.encode(),
            overlap: self.studio.overlap.name().to_string(),
            overlap_tracks: self.studio.overlap_tracks.iter()
                .map(|(t, p)| format!("{}:{}", t.0, p.name())).collect::<Vec<_>>().join(","),
        });
        // The directory must exist (and be ours) BEFORE the
        // file is written: a save into a fresh project root
        // otherwise fails on the open, and a save into
        // someone's source tree must fail here rather than
        // after leaving a file in it.
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if let Err(e) = project_dir::ensure_project_dir(dir) {
                self.studio.status = e.clone();
                return Err(e);
            }
        }
        let saved = self.studio.save_to(&path);
        if saved.is_ok() {
            self.saved_project = Some(self.studio.session.doc.project.clone());
            // The save happened; history is the extra. A missing
            // git binary demotes to a note, never a lost save.
            let dir = path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let p = &self.studio.session.doc.project;
            let meta = project_dir::Meta {
                name: p.name.clone(),
                bpm: self.studio.rack.tempo_bpm,
                tracks: p.tracks.len(),
                clips: p.clips.len(),
                busses: p.busses.len(),
                patterns: self.studio.rack.patterns.len(),
                channels: self.studio.rack.channels.len(),
            };
            match project_dir::save_versioned(
                &dir, &meta, project_dir::MediaMode::Committed, "save") {
                Ok(Some(id)) => self.studio.status =
                    format!("{} — commit {id}", self.studio.status),
                Ok(None) => {}
                Err(e) => self.studio.status =
                    format!("{} (history skipped: {})",
                            self.studio.status, e.lines().next().unwrap_or("git error")),
            }
        }
        saved
    }

    /// A wake for background workers: after each message they send, the
    /// window redraws to show it.
    fn waker(&self) -> wake::Wake {
        let w = self.ui_waker.clone();
        std::sync::Arc::new(move || { if let Some(c) = w.lock().ok().and_then(|g| g.clone()) { c.request_repaint(); } })
    }

    /// How tool windows should present on this screen, and a window built to
    /// match: a full-screen sheet on a narrow screen (the editor's
    /// one-thing-at-a-time rule), a floating window elsewhere.
    fn tool_layout(&self, ctx: &egui::Context) -> studio_egui::touch::ToolLayout {
        studio_egui::touch::ToolLayout::for_screen(ctx.content_rect().width(), self.touch_seen)
    }
    fn tool_window<'a>(ctx: &egui::Context, title: &'a str, layout: studio_egui::touch::ToolLayout, open: &'a mut bool) -> egui::Window<'a> {
        let w = egui::Window::new(title).open(open).collapsible(false);
        if layout.sheet {
            w.fixed_rect(ctx.content_rect().shrink(4.0)).resizable(false).vscroll(true)
        } else {
            w.resizable(false)
        }
    }

    fn comic_setup_window(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.comic_setup else { return };
        let mut open = true;
        let mut go = None;
        let layout = studio_egui::touch::ToolLayout::for_screen(ctx.content_rect().width(), self.touch_seen);
        Self::tool_window(ctx, "Comic — who goes where", layout, &mut open).show(ctx, |ui| { go = w.show(ui, layout); });
        if let Some(setup) = go {
            let group = w.group;
            self.comic_setup = None;
            self.start_comic(group, setup);
            return;
        }
        if !open { self.comic_setup = None; }
    }

    fn start_comic(&mut self, group: studio_core::GroupId, setup: comic_job::Setup) {
        if self.comic_run.is_some() { self.studio.status = "a comic is already rendering".into(); return; }
        let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let project = &self.studio.session.doc.project;
        let (roles, start, end) = match comic_job::roles_with(project, group, &base, setup) {
            Ok(r) => r, Err(e) => { self.studio.status = e; return; }
        };
        let span = (end.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
        // sides: which side's own mic hears a line louder, when the panels are
        // different angles (with one angle for both there is no evidence)
        const FPS: f64 = 25.0;
        let (angles, _, _) = studio_video::multicam::from_group(project, group).unwrap_or((Vec::new(), start, end));
        let (ldb, rdb) = match (angles.get(setup.left), angles.get(setup.right)) {
            (Some(l), Some(r)) if setup.left != setup.right =>
                (self.speech_db(l.source, start.to_seconds() + l.offset, span, FPS),
                 self.speech_db(r.source, start.to_seconds() + r.offset, span, FPS)),
            _ => (None, None),   // one angle for both panels: its mic cannot tell sides apart
        };
        let side_at = |a: f64, b: f64| -> Option<studio_video::comic::Side> {
            let (l, r) = (ldb.as_ref()?, rdb.as_ref()?);
            let (i0, i1) = ((a * FPS) as usize, ((b * FPS) as usize).max((a * FPS) as usize + 1));
            let mean = |v: &Vec<f32>| { let s = &v[i0.min(v.len())..i1.min(v.len())]; if s.is_empty() { None } else { Some(s.iter().sum::<f32>() / s.len() as f32) } };
            let (ml, mr) = (mean(l)?, mean(r)?);
            if (ml - mr).abs() < 3.0 { None } else if ml > mr { Some(studio_video::comic::Side::Left) } else { Some(studio_video::comic::Side::Right) }
        };
        let lines = comic_job::lines(project, group, start, span, &self.words_raw, &side_at);
        let mut out = base.join(format!("comic-group{}.mp4", group.0));
        let mut n = 2;
        while out.exists() { out = base.join(format!("comic-group{}-{n}.mp4", group.0)); n += 1; }
        let w = self.ui_waker.clone();
        let nlines = lines.len();
        self.comic_run = Some(comic_job::start(roles, lines, span, out, group, start, move || {
            if let Some(c) = w.lock().ok().and_then(|g| g.clone()) { c.request_repaint(); }
        }));
        self.studio.status = format!("comic: planning {nlines} line(s) over {span:.1} s — rendering in the background");
    }

    fn comic_tracker(&mut self) {
        let Some(job) = &self.comic_run else { return };
        let Ok(ev) = job.rx.try_recv() else { return };
        let (start, span) = (job.start, job.span);
        self.comic_run = None;
        match ev {
            comic_job::Event::Done(path, frames) => {
                let ops = comic_job::land_ops(&self.studio.session.doc.project, &path, start, span);
                self.studio.edit_group(ops, "comic");
                self.studio.status = format!("comic landed: {frames} frames on a new 'comic' track");
            }
            comic_job::Event::Failed(e) => self.studio.status = format!("comic failed: {e}"),
        }
    }

    fn signals_window(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.signals_panel else { return };
        let mut open = true;
        let mut go = None;
        let layout = studio_egui::touch::ToolLayout::for_screen(ctx.content_rect().width(), self.touch_seen);
        Self::tool_window(ctx, "Signals — when to cut", layout, &mut open).show(ctx, |ui| { layout.apply(ui); go = p.show(ui); });
        if let Some(cands) = go {
            match signals_panel::plan_ops(&self.studio.session.doc.project, p.group, &cands, &p.angle_inputs, p.prefer, p.no_repeat) {
                Ok((ops, shots, lines)) => {
                    self.studio.edit_group(ops, "multicam by signals");
                    self.studio.status = format!("{shots} shot(s) on a new track, a 'why' marker at each cut");
                    p.planned = lines;
                    let _ = cands;
                }
                Err(e) => self.studio.status = e,
            }
        }
        if !open { self.signals_panel = None; }
    }

    fn kaleido_gallery_window(&mut self, ctx: &egui::Context) {
        let Some(g) = &mut self.kaleido_gallery else { return };
        let mut open = true;
        let mut chosen = None;
        let layout = studio_egui::touch::ToolLayout::for_screen(ctx.content_rect().width(), self.touch_seen);
        Self::tool_window(ctx, "Kaleidoscope", layout, &mut open).show(ctx, |ui| { layout.apply(ui); chosen = g.show(ui); });
        if let Some(k) = chosen {
            let project = &self.studio.session.doc.project;
            let ops: Vec<studio_core::Op> = self.studio.selection.iter().filter_map(|id| project.clips.get(id))
                .filter(|c| project.sources.get(&c.source).map(|s| s.has_video).unwrap_or(false))
                .map(|c| {
                    let mut fx: Vec<studio_core::Effect> = c.effects.iter()
                        .filter(|e| e.kind != studio_video::kaleido::KIND).cloned().collect();
                    fx.push(k.effect());
                    studio_core::Op::SetEffects { id: c.id, effects: fx }
                }).collect();
            if !ops.is_empty() {
                self.studio.edit_group(ops, "kaleidoscope");
                self.studio.status = format!("kaleidoscope: {} wedges, turned {:.0}°, centre at {:.0}% — preview and export",
                                             k.segments, k.angle, k.cx * 100.0);
            }
            open = false;
        }
        if !open { self.kaleido_gallery = None; }
    }

    /// Particles into the selected clip. The drive, if a control channel is
    /// chosen, is that bus rendered over EXACTLY the stretch the touch-up
    /// covers -- the clip plus its handles -- so frame i of the render reads
    /// the sound at frame i. A drive computed for the clip alone would run
    /// half a second early against the handled render.
    fn start_particles(&mut self, preset: u8, drive_bus: Option<studio_core::BusId>, ride: bool) {
        self.start_particles_with(preset, drive_bus, ride, Vec::new());
    }

    /// Open the track-points window on the selected clip's FIRST RENDERED
    /// frame -- the render starts half a second (the handle) before the clip,
    /// and that is the frame the points are tracked from.
    fn open_track_points(&mut self, preset: u8, drive: Option<studio_core::BusId>) {
        let project = &self.studio.session.doc.project;
        let Some(clip) = self.studio.selection.first().and_then(|id| project.clips.get(id)).cloned() else {
            self.studio.status = "select the clip first".into(); return;
        };
        let Some(src) = project.sources.get(&clip.source).filter(|s| s.has_video) else {
            self.studio.status = "the selected clip has no picture".into(); return;
        };
        let base = self.project_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let first = Ticks::from_seconds((clip.src_in.to_seconds() - 0.5).max(0.0));   // 0.5 = start_touchup's handle
        match studio_video::frame_rgb(&base.join(&src.path), first, 320, 180) {
            Ok(px) => self.track_points = Some(track_points::Window::new(px, preset, drive)),
            Err(e) => self.studio.status = format!("could not read the first frame: {e:?}"),
        }
    }

    fn track_points_window(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.track_points else { return };
        let mut open = true;
        let mut go = None;
        let layout = studio_egui::touch::ToolLayout::for_screen(ctx.content_rect().width(), self.touch_seen);
        Self::tool_window(ctx, "Track points", layout, &mut open).show(ctx, |ui| { go = w.show(ui, layout); });
        if let Some(points) = go {
            let (preset, drive) = (w.preset, w.drive);
            self.track_points = None;
            self.start_particles_with(preset, drive, true, points);
            return;
        }
        if !open { self.track_points = None; }
    }

    fn start_particles_with(&mut self, preset: u8, drive_bus: Option<studio_core::BusId>, ride: bool, points: Vec<(f32, f32)>) {
        use studio_video::particles::Params;
        let params = match preset { 0 => Params::floating_dirt(), 1 => Params::space_dust(), _ => Params::creepy_smoke() };
        let emitter = match preset { 0 => (0.5, 0.95), 1 => (0.5, 0.5), _ => (0.5, 0.75) };
        let project = &self.studio.session.doc.project;
        let Some(&id) = self.studio.selection.first() else { self.studio.status = "select the clip first".into(); return };
        let Some(clip) = project.clips.get(&id).cloned() else { return };
        let fps = project.sources.get(&clip.source)
            .and_then(|s| studio_video::probe(&std::path::PathBuf::from(&s.path)).ok()).map(|p| p.fps).unwrap_or(24.0);
        let drive = match drive_bus {
            None => Vec::new(),
            Some(bus) => {
                let (from, to, pad) = touchup_job::drive_span(&clip, 0.5, fps);   // 0.5 = start_touchup's handle
                match studio_audio::bus_signal(project, &self.audio, &Default::default(),
                        Ticks::from_seconds(from), Ticks::from_seconds(to), 48_000, bus) {
                    Some(st) => {
                        let mono: Vec<f32> = st.chunks(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(0.0)) * 0.5).collect();
                        let mut d = vec![0.0; pad];
                        d.extend(studio_video::particles::envelope(&mono, 48_000, fps));
                        if d.iter().all(|v| *v < 0.01) {
                            self.studio.status = "that control channel is silent over this clip — send something into it".into();
                            return;
                        }
                        d
                    }
                    None => { self.studio.status = "that control channel is muted or gone".into(); return; }
                }
            }
        };
        let what = match preset { 0 => "floating dirt", 1 => "spaceship dust", _ => "creepy smoke" };
        let placed = points.len();
        self.start_touchup(studio_video::touchup::Fix::Particles { params, seed: id.0, emitter, drive, ride, points },
                           format!("{what}{}{}", if drive_bus.is_some() { ", driven by sound" } else { "" },
                                   if placed > 0 { format!(", riding {placed} placed point(s)") } else { String::new() }));
    }

    /// Poll the touch-up worker. Same shape as the export tracker: egui only
    /// repaints on input, and a render is not input.
    fn touchup_tracker(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.touchup_run else { return };
        let mut finished = false;
        while let Ok(e) = job.rx.try_recv() {
            match e {
                touchup_job::Event::Note(m) => self.studio.status = m,
                touchup_job::Event::Done(v) => { self.studio.land_touchup(&v); finished = true; }
                touchup_job::Event::Failed(e) => {
                    self.studio.status = format!("touch-up failed: {e}");
                    finished = true;
                }
            }
        }
        if finished { self.touchup_run = None; return; }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// egui only redraws on input, and a background render is not input.
    fn export_tracker_window(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.export_run else { return };
        let mut finished = false;
        while let Ok(e) = run.rx.try_recv() {
            match e {
                export::Event::Progress { done, jobs, frac } => {
                    self.export_progress = (done, jobs, frac);
                }
                export::Event::File(p) => self.export_done.push(p),
                export::Event::Note(m) => self.studio.status = m,
                export::Event::Done => {
                    if let Some(clip) = self.upscale_clip.take() {
                        if let Some(p) = self.export_done.last().cloned() {
                            self.studio.land_upscaled(clip, &p);
                        }
                    } else {
                        self.studio.status = format!(
                            "exported {} file{}", self.export_done.len(),
                            if self.export_done.len() == 1 { "" } else { "s" });
                    }
                    finished = true;
                }
                export::Event::Cancelled => {
                    // A cancelled bounce is still a playable file: land it,
                    // with its real (shorter) length, rather than lose it.
                    if let Some(clip) = self.upscale_clip.take() {
                        if let Some(p) = self.export_done.last().cloned() {
                            self.studio.land_upscaled(clip, &p);
                        }
                    } else {
                        self.studio.status = format!(
                            "export cancelled — {} file{} kept", self.export_done.len(),
                            if self.export_done.len() == 1 { "" } else { "s" });
                    }
                    finished = true;
                }
                export::Event::Failed(e) => {
                    self.upscale_clip = None;
                    self.studio.status = format!("export failed: {e}");
                    finished = true;
                }
            }
        }
        if finished { self.export_run = None; return; }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let (done, jobs, frac) = self.export_progress;
        let overall = if jobs == 0 { 0.0 } else { (done as f32 + frac) / jobs as f32 };
        let mut cancel = false;
        egui::Window::new("Exporting").resizable(false).collapsible(false)
            .show(ctx, |ui| {
                ui.add(egui::ProgressBar::new(overall)
                    .text(format!("{}/{} files", self.export_done.len().max(done), jobs.max(1))));
                for f in &self.export_done {
                    ui.label(f.file_name().map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| f.display().to_string()));
                }
                if ui.button("cancel").clicked() { cancel = true; }
            });
        if cancel {
            if let Some(run) = &self.export_run {
                run.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Open a window and run.
#[cfg(feature = "app")]
pub fn run(title: &str) -> eframe::Result<()> {
    // The command line first, before any window: --help, and anything that is
    // not a project, are answered on the terminal and stop here -- starting a
    // demo instead of the file someone named would look like the file was lost.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match open_project::from_args(&args) {
        open_project::Launch::Help => { println!("{}", open_project::USAGE); return Ok(()); }
        open_project::Launch::Refused(why) => {
            eprintln!("studio: {why}\n\n{}", open_project::USAGE);
            std::process::exit(2);
        }
        open_project::Launch::Project(_) | open_project::Launch::Default => {}
    }
    // Why the window will not open, said before the panic in a dependency does. The main
    // here delegates into this function, so the check belongs at the run entry rather than
    // at `fn main` — see `uikit_egui::backend_check`. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(title, opts, Box::new(|_cc| Ok(Box::<App>::default())))
}

/// The real analog backend behind `studio_egui::playkeys::AnalogSource`.
///
/// Two halves of one feature met here: the view crate defined the seam (a
/// trait plus `NullAnalog`, so the layout and mode logic could be written
/// and tested with no hardware anywhere), and `shared/analog-kit` is the
/// Wooting SDK loader that fills it — factored out of
/// `games/skydribble/src/analog.rs`, which proved the dlopen-and-fall-back
/// approach first. The trait speaks egui `Key`s and the SDK speaks HID
/// usage codes, so the translation lives here, in the one place that
/// touches both.
struct WootingAnalog(analog_kit::Analog);

impl Default for WootingAnalog {
    fn default() -> Self { WootingAnalog(analog_kit::Analog::new()) }
}

impl WootingAnalog {
    /// `keys::Key` carries the PHYSICAL character playkeys resolved (its
    /// docs explain why physical), so the HID mapping is a-z/0-9 plus the
    /// punctuation the layout can reach — USB HID usage page 7.
    fn hid_of(key: &studio_egui::keys::Key) -> Option<u16> {
        let c = key.ch.to_ascii_lowercase();
        Some(match c {
            'a'..='z' => 0x04 + (c as u16 - 'a' as u16),
            '1'..='9' => 0x1E + (c as u16 - '1' as u16),
            '0' => 0x27,
            ';' => 0x33, ',' => 0x36, '.' => 0x37, '/' => 0x38,
            '[' => 0x2F, ']' => 0x30, '-' => 0x2D, '=' => 0x2E,
            _ => return None,
        })
    }
}

impl studio_egui::playkeys::AnalogSource for WootingAnalog {
    fn travel(&self, key: &studio_egui::keys::Key, held: bool) -> f32 {
        // The musical curve, not skydribble's steering curve: see
        // analog-kit's `velocity` docs on why those differ.
        analog_kit::velocity(self.0.depth(Self::hid_of(key), held), 1.6)
    }
    fn active(&self) -> bool { self.0.active() }
}

#[cfg(all(test, feature = "app"))]   // these drive App, which is the window
mod sink_resync_tests {
    use super::*;

    /// THE VIDEO-ONLY-TIMELINE BUG. An arrangement that renders no audio gave
    /// `sink.position() == None` AND `sink.playing() == false` (NullSink's
    /// playing() is `playing && (looped || raw_pos() < frames)`, false at zero
    /// frames), so sync_sink stopped it with "reached the end" before a frame
    /// elapsed. A video-only timeline -- the kdenlive case -- could never play.
    #[test]
    fn a_loaded_but_silent_arrangement_runs_a_wall_clock_instead_of_stopping() {
        // SinkContent deliberately has no Debug -- ArrangementLoop carries a
        // looping::Window -- so the label is spelled out rather than derived.
        for (name, loaded) in [("Arrangement", SinkContent::Arrangement),
                               ("RackLoop", SinkContent::RackLoop)] {
            assert_eq!(
                App::silent_fallback(&loaded, false),
                SilentFallback::WallClock,
                "{name} is loaded and merely silent -- a missing clock, not the end"
            );
        }
    }

    /// Nothing loaded is genuinely the end; the old behaviour must survive.
    #[test]
    fn nothing_loaded_still_stops() {
        assert_eq!(App::silent_fallback(&SinkContent::Nothing, false), SilentFallback::Stop);
    }

    /// Audio keeps priority whenever it can report a position -- transport
    /// rule No.1 is unchanged for every project that has sound.
    #[test]
    fn the_sink_still_wins_whenever_it_has_a_position() {
        for loaded in [SinkContent::Nothing, SinkContent::Arrangement, SinkContent::RackLoop] {
            assert_eq!(App::silent_fallback(&loaded, true), SilentFallback::SinkDrives);
        }
    }

    /// A stalled frame must not teleport the playhead across the timeline.
    #[test]
    fn a_long_stall_is_clamped_to_one_step() {
        let rate = 48_000;
        let normal = App::silent_frames(1.0 / 60.0, rate);
        assert!(normal > 0 && normal < 900, "a 60fps tick is ~800 frames, got {normal}");
        // Ten seconds of stall yields the same as the 250ms cap, not 10s.
        assert_eq!(App::silent_frames(10.0, rate), App::silent_frames(0.25, rate));
        assert_eq!(App::silent_frames(10.0, rate), 12_000);
    }

    /// Non-advancing or nonsensical intervals must move nothing rather than
    /// running the playhead backwards.
    #[test]
    fn a_zero_or_bad_interval_advances_nothing() {
        for dt in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let f = App::silent_frames(dt, 48_000);
            assert!(f >= 0, "dt {dt} produced {f}");
            if dt <= 0.0 || dt.is_nan() {
                assert_eq!(f, 0, "dt {dt} must not move the playhead");
            }
        }
        // Infinity is finite-checked into the clamp, never a huge jump.
        assert_eq!(App::silent_frames(f64::INFINITY, 48_000), 0);
    }

    /// The clock must scale with the device rate, not assume 48k.
    #[test]
    fn frames_scale_with_the_sample_rate() {
        assert_eq!(App::silent_frames(0.25, 44_100), 11_025);
        assert_eq!(App::silent_frames(0.25, 96_000), 24_000);
    }

    /// The exact bug: pause, seek backward, play. Nothing about WHAT is
    /// loaded changes (still a plain, non-looped arrangement) and no
    /// project edit happened, so `loaded == want` and `arrangement_dirty
    /// == false` -- the two conditions `should_resync` checked BEFORE this
    /// fix. Without `seek_pending`, this returns `false`, meaning
    /// `sync_sink` skips `self.sink.seek(...)` entirely and the sink
    /// resumes from its own stale internal cursor -- exactly the reported
    /// symptom. With `seek_pending`, it returns `true`.
    #[test]
    fn a_bare_seek_with_no_content_change_still_forces_a_resync() {
        let loaded = SinkContent::Arrangement;
        let want = SinkContent::Arrangement;

        // Before the fix, this line would have been `loaded != want ||
        // arrangement_dirty` -- both false here -- so the call below
        // would have returned false, silently skipping the reseek.
        assert!(
            App::should_resync(&loaded, &want, false, true),
            "a user-initiated seek with unchanged content and no project edit MUST still force a resync"
        );
    }

    /// The inverse, equally important: nothing pending, nothing dirty, no
    /// content change -- must NOT resync every tick. Without this,
    /// `sync_sink` would re-render and re-seek the sink on every single
    /// frame it's called, which is the "resync forced constantly" failure
    /// mode a careless fix (e.g. defaulting `seek_pending` to `true`)
    /// could introduce instead.
    #[test]
    fn steady_playback_with_nothing_pending_does_not_resync() {
        let loaded = SinkContent::Arrangement;
        let want = SinkContent::Arrangement;
        assert!(!App::should_resync(&loaded, &want, false, false));
    }

    /// The two pre-existing reasons still work, unchanged by this fix.
    #[test]
    fn content_change_and_project_edit_still_force_a_resync_on_their_own() {
        let arrangement = SinkContent::Arrangement;
        let looped = SinkContent::ArrangementLoop(looping::Window::new(Some((Ticks::ZERO, Ticks::from_seconds(2.0))), 48_000).unwrap());

        assert!(App::should_resync(&arrangement, &looped, false, false), "switching into a loop must resync");
        assert!(App::should_resync(&arrangement, &arrangement, true, false), "a project edit must resync");
    }

    /// The video-decoupling fix: a cycling sink position (0..period,
    /// wrapping forever while a beat pattern loops) must unwrap into a
    /// monotonically increasing total -- this is what lets video keep
    /// advancing "like a normal timeline" instead of freezing or being
    /// trapped in the loop range. Computed independently in Python first
    /// (not hand-derived) to cross-check the arithmetic before trusting
    /// this assertion.
    #[test]
    fn unwrap_cycled_frames_accumulates_monotonically_across_repeated_wraps() {
        let period = 100usize;
        let mut prev = None;
        let mut total = 0i64;
        for f in [0usize, 40, 80, 10, 55, 95, 5, 50] {
            total += App::unwrap_cycled_frames(prev, f, period);
            prev = Some(f);
        }
        assert_eq!(total, 250, "cross-checked independently: 40+40+30+45+40+10+45=250");
    }

    #[test]
    fn unwrap_cycled_frames_first_call_has_no_delta() {
        // No previous sample means no basis for a delta -- must be 0, not
        // panic and not treat `current` itself as elapsed time.
        assert_eq!(App::unwrap_cycled_frames(None, 73, 100), 0);
    }

    #[test]
    fn unwrap_cycled_frames_handles_a_single_wrap_correctly() {
        // period=100, was at 90, now at 5: travelled 10 (90->100) + 5 = 15.
        assert_eq!(App::unwrap_cycled_frames(Some(90), 5, 100), 15);
    }
}

#[cfg(test)]
mod rack_shell_tests {
    use super::*;
    use studio_egui::keys::Key;

    #[test]
    fn space_on_the_rack_drives_the_loop_and_leaves_the_tape_alone() {
        // One key, the thing you are looking at. Space starting audio you
        // cannot see is the disorienting version.
        let mut s = Studio::demo();
        s.panel = Panel::Rack;
        s.key(Key::c(' '));
        assert!(s.rack_playing, "the loop pedal went down");
        assert!(!s.transport.playing, "and the tape did not roll");
        s.key(Key::c(' '));
        assert!(!s.rack_playing);
        s.panel = Panel::Timeline;
        s.key(Key::c(' '));
        assert!(s.transport.playing, "on the timeline, space is the transport");
        assert!(!s.rack_playing);
    }

    #[test]
    fn the_demo_rack_is_audible_with_zero_assets() {
        let s = Studio::demo();
        let out = s.render_loop(48_000, &Default::default());
        assert!(!out.is_empty());
        assert!(out.iter().map(|v| v.abs()).sum::<f32>() > 50.0,
                "the trap kit makes an actual bar of noise");
    }

    #[test]
    fn a_bounce_lands_an_undoable_clip_the_length_of_the_pattern() {
        let mut s = Studio::demo();
        let clips_before = s.session.doc.project.clips.len();
        let (id, audio) = s.bounce_current_pattern(48_000, &Default::default()).expect("demo has an audio track");
        assert_eq!(s.session.doc.project.clips.len(), clips_before + 1);
        let clip = s.session.doc.project.clips.values()
            .find(|c| c.source == id).expect("the clip points at the bounce");
        // The clip is exactly as long as the audio it holds. (Not the pure
        // tick-length of the pattern: render_pattern floors each step to
        // whole samples, so at 140bpm a 16-step bar is ~14 samples shorter
        // than the tick math — the clip must match the SAMPLES, or the
        // timeline claims audio that does not exist.)
        assert_eq!(clip.len, Ticks::from_samples(audio.frames() as i64, 48_000),
                   "clip length is the audio's length");
        assert_eq!(audio.channels.len(), 1, "a bounce is mono");
        assert!(audio.frames() > 0);
        // Two ops went through history (AddSource + AddClip): two undos
        // return the project exactly to where it was — a bounce that cannot
        // be taken back is a recording, not an edit.
        s.key(Key::c('u'));
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips.len(), clips_before);
        assert!(!s.session.doc.project.sources.contains_key(&id));
    }

    #[test]
    fn a_bounce_with_no_audio_track_refuses_out_loud() {
        // The worst failure an editor has is a silently dropped edit.
        let p = Project { name: "empty".into(), ..Default::default() };
        let mut s = Studio::new(p, SiteId(1), Team::from_contacts("solo", &["sc-local".into()]));
        assert!(s.bounce_current_pattern(48_000, &Default::default()).is_none());
        assert!(s.status.contains("refused"), "status was {:?}", s.status);
    }

    #[test]
    fn editing_the_rack_marks_the_loop_dirty_the_way_the_panel_will() {
        // The panel detects edits by value comparison; pin that a step
        // toggle actually changes the value it compares.
        let mut s = Studio::demo();
        let before = s.rack.clone();
        s.rack.patterns[0].toggle(1, 3);
        assert_ne!(s.rack, before);
    }
}

#[cfg(test)]
mod import_shell_tests {
    use super::*;

    #[test]
    fn an_imported_sample_becomes_an_undoable_source_and_a_playable_channel() {
        let mut s = Studio::demo();
        let channels_before = s.rack.channels.len();
        // A 100ms click at 48k — synthetic here; studio-media's own tests
        // prove real .opus bytes land in this exact SourceAudio shape.
        let mut click = vec![0.0f32; 4800];
        click[0] = 1.0; click[1] = 0.5;
        let audio = studio_audio::SourceAudio { sample_rate: 48_000, channels: vec![click] };
        let id = s.import_audio("snap", "samples/snap.opus", &audio);

        let src = s.session.doc.project.sources.get(&id).expect("source landed as an op");
        assert_eq!(src.path, "samples/snap.opus",
                   "the project references the compressed file, not an inflated copy");
        assert_eq!(s.rack.channels.len(), channels_before + 1);
        assert!(s.loop_dirty, "the audition loop knows to re-render");

        // Sequence it and hear it: the new channel's steps are not no-ops.
        let ch = s.rack.channels.len() - 1;
        s.rack.patterns[0].set(ch, 0, true);
        let mut sources = std::collections::BTreeMap::new();
        let mut click = vec![0.0f32; 4800];
        click[0] = 1.0; click[1] = 0.5;
        sources.insert(id, studio_audio::SourceAudio { sample_rate: 48_000, channels: vec![click] });
        // Mute the kit so the only audio in step 0's first ms is the click.
        for c in &mut s.rack.channels[..channels_before] { c.muted = true; }
        let out = s.render_loop(48_000, &sources);
        assert!(out[0].abs() > 0.5, "the imported sample fires on its step");

        // And undo takes the source back out — an import is an edit.
        s.key(studio_egui::keys::Key::c('u'));
        assert!(!s.session.doc.project.sources.contains_key(&id));
    }

    #[test]
    fn a_low_rate_import_is_playable_but_says_so() {
        let mut s = Studio::demo();
        let audio = studio_audio::SourceAudio {
            sample_rate: 44_100, channels: vec![vec![0.1; 4410]],
        };
        s.import_audio("legacy", "samples/legacy.wav", &audio);
        assert!(s.status.contains("44100"), "status was {:?}", s.status);
    }
}

#[cfg(test)]
mod placed_pattern_tests {
    use super::*;

    fn first_audio(s: &Studio) -> studio_core::TrackId {
        s.session.doc.project.tracks.iter().find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().id
    }

    /// A drop is a bounce at the drop's time and track — an ordinary AddSource + AddClip.
    #[test]
    fn a_placed_pattern_lands_where_it_was_dropped_and_is_linked() {
        let mut s = Studio::demo();
        let track = first_audio(&s);
        let end = s.session.doc.project.clips.values().map(|c| c.end()).max().unwrap_or(studio_core::Ticks::ZERO);
        let at = studio_core::Ticks(end.0 + studio_core::Ticks::from_seconds(3.25).0);
        let (id, audio) = s.place_pattern(0, at, track, 48_000, &Default::default()).expect("placed");
        let clip = s.session.doc.project.clips.values().find(|c| c.source == id).expect("clip");
        assert_eq!(clip.pos, at);
        assert_eq!(clip.track, track);
        assert_eq!(clip.len, s.session.doc.project.sources[&id].len);
        assert!(!audio.channels[0].is_empty());
        assert_eq!(s.pattern_links.get(&id).map(String::as_str), Some(s.rack.patterns[0].name.as_str()));
        assert!(s.status.contains("live"), "{}", s.status);
    }

    /// A drop on top of an existing clip is refused before any op lands — no orphan source.
    #[test]
    fn a_drop_that_would_overlap_is_refused_whole() {
        let mut s = Studio::demo();
        let (track, at) = {
            let p = &s.session.doc.project;
            let c = p.clips.values()
                .find(|c| p.tracks.iter().any(|t| t.id == c.track && t.kind == studio_core::TrackKind::Audio))
                .expect("demo has an audio clip");
            (c.track, c.pos)
        };
        let n_sources = s.session.doc.project.sources.len();
        assert!(s.place_pattern(0, at, track, 48_000, &Default::default()).is_none());
        assert_eq!(s.session.doc.project.sources.len(), n_sources, "no source without its clip");
        assert!(s.pattern_links.is_empty());
        assert!(s.status.contains("overlap"), "{}", s.status);
    }

    #[test]
    fn a_pattern_cannot_land_on_a_video_track() {
        let mut s = Studio::demo();
        let Some(video) = s.session.doc.project.tracks.iter().find(|t| t.kind == studio_core::TrackKind::Video).map(|t| t.id)
            else { return };
        assert!(s.place_pattern(0, studio_core::Ticks::ZERO, video, 48_000, &Default::default()).is_none());
        assert!(s.status.contains("audio track"), "{}", s.status);
    }

    /// The live half: edit the rack, and the placed source re-renders to the edited groove;
    /// delete the pattern, and the source keeps its take and is unlinked.
    #[test]
    fn rack_edits_re_render_the_placed_source_until_the_pattern_is_gone() {
        let mut s = Studio::demo();
        let track = first_audio(&s);
        let (id, before) = s.place_pattern(0, studio_core::Ticks::ZERO, track, 48_000, &Default::default()).unwrap();
        // Silence every row of the pattern: the re-render must differ from the placed take.
        for row in s.rack.patterns[0].rows.iter_mut() { *row = 0; }
        let fresh = s.rerender_pattern_links(48_000, &Default::default());
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].0, id);
        assert_ne!(fresh[0].1.channels[0], before.channels[0], "the re-render is the edited pattern");
        assert!(fresh[0].1.channels[0].iter().all(|x| *x == 0.0), "no hits, no sound");
        s.rack.patterns.remove(0);
        let fresh = s.rerender_pattern_links(48_000, &Default::default());
        assert!(fresh.is_empty());
        assert!(!s.pattern_links.contains_key(&id), "a deleted pattern unlinks its source");
        assert!(s.status.contains("keeps the last render"), "{}", s.status);
    }
}

#[cfg(test)]
mod arrangement_tests {
    use super::*;
    use studio_egui::keys::Key;

    #[test]
    fn a_bounce_is_audible_in_the_arrangement_render() {
        // The full circle: pattern → bounce → clip → mixdown. If this holds,
        // the timeline's play button has real audio behind it.
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let out = s.render_arrangement(48_000, &sources);
        assert!(!out.is_empty());
        // The bounce sits at tick 0; its first beat must be in the mix.
        let first_beat: f32 = out[..9600].iter().map(|v| v.abs()).sum();
        assert!(first_beat > 10.0, "the bounced kick is in the mixdown ({first_beat})");
        // Interleaved stereo: even length, both channels carry it.
        assert_eq!(out.len() % 2, 0);
        let (l, r): (f32, f32) = out[..9600].chunks(2)
            .fold((0.0, 0.0), |(l, r), fr| (l + fr[0].abs(), r + fr[1].abs()));
        assert!(l > 1.0 && r > 1.0, "both channels are live: L={l} R={r}");
    }

    #[test]
    fn edits_mark_the_arrangement_dirty_and_undo_does_too() {
        let mut s = Studio::demo();
        s.arrangement_dirty = false;
        let clip = *s.session.doc.project.clips.keys().next().unwrap();
        s.edit(studio_core::Op::RemoveClip { id: clip }, "deleted");
        assert!(s.arrangement_dirty, "an edit stales the render");
        s.arrangement_dirty = false;
        s.key(Key::c('u'));
        assert!(s.arrangement_dirty, "so does putting it back");
    }

    #[test]
    fn the_two_play_buttons_take_the_sink_from_each_other() {
        let mut s = Studio::demo();
        s.panel = Panel::Rack;
        s.key(Key::c(' '));
        assert!(s.rack_playing && !s.transport.playing);
        s.panel = Panel::Timeline;
        s.key(Key::c(' '));
        assert!(s.transport.playing && !s.rack_playing,
                "the tape took the sink from the loop pedal");
        s.panel = Panel::Rack;
        s.key(Key::c(' '));
        assert!(s.rack_playing && !s.transport.playing,
                "and the loop pedal takes it back");
    }

    #[test]
    fn a_melody_loop_import_lands_a_clip_at_the_playhead() {
        let mut s = Studio::demo();
        s.transport.set_position(Ticks::from_seconds(2.0));
        let clips_before = s.session.doc.project.clips.len();
        let audio = studio_audio::SourceAudio {
            sample_rate: 48_000, channels: vec![vec![0.2; 96_000], vec![0.2; 96_000]],
        };
        let id = s.import_clip("melody", "loops/melody.opus", &audio).expect("a track has room");
        assert_eq!(s.session.doc.project.clips.len(), clips_before + 1);
        let clip = s.session.doc.project.clips.values().find(|c| c.source == id).unwrap();
        assert_eq!(clip.pos, Ticks::from_seconds(2.0), "at the playhead, not at zero");
        assert_eq!(clip.len, Ticks::from_samples(96_000, 48_000), "the whole 2s loop, uncut");
        // A1 is busy at 2.0–4.0s (the demo's clip at 3.5s) — the import must
        // have skipped to a track with room rather than bouncing off
        // check_room, the way a hand placement would.
        let a1 = s.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().id;
        assert_ne!(clip.track, a1, "landed on the next free audio track");
        // Two ops, two undos, gone — an import is an edit.
        s.key(Key::c('u'));
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips.len(), clips_before);
    }
}

/// What `Studio::export_arrangement` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat { Wav, Opus, Mp3, M4a }

/// Opus is compiled in (the `audio` feature: libopus via studio-media).
/// Without it studio builds codec-free for devices with no audio output (a
/// Paper Pro without Bluetooth audio): exports default to WAV, the export
/// dialog does not offer Opus, and Opus bounces are written as FLAC.
pub const OPUS_BUILT: bool = cfg!(feature = "audio");

impl ExportFormat {
    /// Can this build encode it? (Mp3/M4a go through an external ffmpeg,
    /// so they are offered regardless and fail with ffmpeg's own message.)
    pub fn available(self) -> bool {
        self != ExportFormat::Opus || OPUS_BUILT
    }
    pub fn ext(self) -> &'static str {
        match self { ExportFormat::Wav => "wav", ExportFormat::Opus => "opus",
                     ExportFormat::Mp3 => "mp3", ExportFormat::M4a => "m4a" }
    }
    /// The lossy ones are stereo at the render rate through ffmpeg; the
    /// ffmpeg encoder name, for [`ExportFormat::Mp3`] and [`ExportFormat::M4a`].
    pub fn ffmpeg_codec(self) -> Option<&'static str> {
        match self { ExportFormat::Mp3 => Some("libmp3lame"), ExportFormat::M4a => Some("aac"), _ => None }
    }
    /// Stereo only, like Opus: the formats a phone shares.
    pub fn stereo_only(self) -> bool { !matches!(self, ExportFormat::Wav) }
}

#[cfg(test)]
mod export_tests {
    use super::*;

    fn bounced_studio() -> (Studio, std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>) {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        (s, sources)
    }

    #[test]
    fn both_export_formats_roundtrip_through_the_apps_own_decoder() {
        // The exported file is read back by the same studio-media that reads
        // the user's libraries — if the app can't eat its own exports,
        // nothing else should be trusted to.
        let (mut s, sources) = bounced_studio();
        let dir = std::env::temp_dir().join("studio-export-test");
        let expect_frames = s.render_arrangement(48_000, &sources).len() / 2;

        for fmt in [ExportFormat::Wav, ExportFormat::Opus].into_iter().filter(|f| f.available()) {
            let path = s.export_arrangement(&sources, &dir, fmt, 48_000, studio_audio::format::ChannelFormat::Stereo).expect("exports");
            assert!(s.status.contains("exported"), "status names it: {:?}", s.status);
            let back = studio_media::decode_file(&path).expect("our decoder reads it");
            assert_eq!(back.channels.len(), 2, "{fmt:?} kept stereo");
            assert_eq!(back.sample_rate, 48_000);
            assert_eq!(back.frames(), expect_frames,
                       "{fmt:?} came back the exact arrangement length");
            // And it isn't silence: the bounced kick is in there.
            let head: f32 = back.channels[0][..4800].iter().map(|v| v.abs()).sum();
            assert!(head > 5.0, "{fmt:?} export is audible ({head})");
        }
    }

    #[test]
    fn the_phones_formats_come_out_of_ffmpeg_the_right_length_or_say_what_is_missing() {
        // mp3 and m4a are not decoded exactly by anything (encoder delay
        // and padding are the formats' own), so the check is ffprobe's:
        // audio, stereo-only layout enforced, the arrangement's length
        // within a codec frame's worth. Without ffmpeg's encoder the
        // export fails with a sentence that names it, not a panic.
        let (mut s, sources) = bounced_studio();
        let dir = std::env::temp_dir().join("studio-export-phone");
        let expect = s.render_arrangement(48_000, &sources).len() as f64 / 2.0 / 48_000.0;
        for fmt in [ExportFormat::Mp3, ExportFormat::M4a] {
            match s.export_arrangement(&sources, &dir, fmt, 44_100, studio_audio::format::ChannelFormat::Surround51) {
                Ok(path) => {
                    assert_eq!(path.extension().and_then(|e| e.to_str()), Some(fmt.ext()));
                    assert!(s.status.contains("(2.0)"), "5.1 asked, stereo delivered, said: {}", s.status);
                    assert!(!path.with_extension("tmp.wav").exists(), "the WAV beside it is gone");
                    // Read back with the app's own decoder — a phone's
                    // file the app cannot open is not one it should make.
                    let back = studio_media::decode_file(&path).expect("our decoder reads it");
                    assert_eq!(back.channels.len(), 2, "{fmt:?}: stereo");
                    let secs = back.frames() as f64 / back.sample_rate as f64;
                    assert!((secs - expect).abs() < 0.15, "{fmt:?}: {secs} s vs {expect}");
                    let head: f32 = back.channels[0][..4800].iter().map(|v| v.abs()).sum();
                    assert!(head > 1.0, "{fmt:?}: audible ({head})");
                }
                Err(e) => {
                    assert!(e.contains("ffmpeg"), "{fmt:?}: a missing encoder is named: {e}");
                }
            }
        }
    }

    #[cfg(feature = "audio")] // measures real Opus encoding
    #[test]
    fn the_opus_export_is_the_small_one() {
        // The stated reason this format exists. Not a benchmark — an order
        // check: opus at 128k must land well under half the 16-bit WAV.
        let (mut s, sources) = bounced_studio();
        let dir = std::env::temp_dir().join("studio-export-size");
        let wav = s.export_arrangement(&sources, &dir, ExportFormat::Wav, 48_000, studio_audio::format::ChannelFormat::Stereo).unwrap();
        let opus = s.export_arrangement(&sources, &dir, ExportFormat::Opus, 48_000, studio_audio::format::ChannelFormat::Stereo).unwrap();
        let (w, o) = (std::fs::metadata(&wav).unwrap().len(),
                      std::fs::metadata(&opus).unwrap().len());
        assert!(o * 2 < w, "opus {o} bytes vs wav {w} bytes");
    }

    #[test]
    fn an_empty_arrangement_refuses_to_export_out_loud() {
        let p = Project { name: "empty".into(), ..Default::default() };
        let mut s = Studio::new(p, SiteId(1), Team::from_contacts("solo", &["sc-local".into()]));
        let err = s.export_arrangement(&Default::default(),
                                       &std::env::temp_dir(), ExportFormat::Wav, 48_000,
                                       studio_audio::format::ChannelFormat::Stereo);
        assert!(err.is_err());
        assert!(s.status.contains("refused"), "status was {:?}", s.status);
    }
}

#[cfg(test)]
mod mixer_tests {
    use super::*;
    use studio_egui::keys::Key;

    #[test]
    fn solo_shapes_the_arrangement_render_without_touching_the_project() {
        // Two tracks with distinct audio; solo one, the other vanishes from
        // the render — while the project itself is byte-identical, because
        // solo is monitoring, not an edit.
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("kick on A1");
        sources.insert(id, audio);
        let melody = studio_audio::SourceAudio {
            sample_rate: 48_000, channels: vec![vec![0.3; 48_000], vec![0.3; 48_000]],
        };
        s.transport.set_position(Ticks::ZERO);
        let mid = s.import_clip("pad", "loops/pad.opus", &melody).expect("lands on A2");
        sources.insert(mid, melody);
        let a2 = s.session.doc.project.clips.values()
            .find(|c| c.source == mid).unwrap().track;

        let before = s.session.doc.project.clone();
        let both = s.render_arrangement(48_000, &sources);
        s.solo = vec![a2];
        let solo = s.render_arrangement(48_000, &sources);
        assert_eq!(s.session.doc.project, before, "solo left the project untouched");

        // The pad is a constant 0.3; the kick's transients push peaks well
        // above it. Solo'd, the early window's peak collapses to pad level.
        let peak = |v: &[f32]| v[..9600].iter().fold(0f32, |a, s| a.max(s.abs()));
        assert!(peak(&both) > 0.5, "kick present unsolo'd: {}", peak(&both));
        assert!(peak(&solo) < 0.35, "solo'd to the pad, kick gone: {}", peak(&solo));
    }

    #[test]
    fn a_delay_syncs_to_the_tempo_as_an_op_and_undoes() {
        let mut s = Studio::demo();
        s.rack.tempo_bpm = 140.0;
        let bus = studio_core::BusId(77);
        s.edit(studio_core::Op::AddBus { bus: studio_core::Bus {
            id: bus, name: "verb".into(), parent: None, gain: 1.0,
            chain: vec![mixer_panel::effect("delay", &[("time", 100.0), ("feedback", 0.4)]),
                        mixer_panel::effect("gain", &[("db", 0.0)])],
            sends: vec![], muted: false, silent: false } }, "bus");
        let before = s.session.doc.history().len();
        assert_eq!(s.sync_delays_to_tempo(3), 1, "one chain had a delay");
        let chain = &s.session.doc.project.busses[&bus].chain;
        assert!((chain[0].params["time"] - 321.428).abs() < 0.01, "dotted eighth at 140: {}", chain[0].params["time"]);
        assert_eq!(chain[0].params["feedback"], 0.4, "the other params are kept");
        assert_eq!(chain[1].kind, "gain", "and the rest of the chain is untouched");
        assert_eq!(s.session.doc.history().len(), before + 1, "one op, so one undo");
        assert!(s.status.contains("321.4 ms"), "{}", s.status);
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.session.doc.project.busses[&bus].chain[0].params["time"], 100.0, "undo restores");
        assert_eq!(s.sync_delays_to_tempo(1), 1);
        assert!((s.session.doc.project.busses[&bus].chain[0].params["time"] - 107.14).abs() < 0.01, "a sixteenth");
        s.edit(studio_core::Op::SetBusChain { id: bus, chain: vec![] }, "clear");
        assert_eq!(s.sync_delays_to_tempo(2), 0, "nothing to sync says so");
        assert!(s.status.contains("no delay"), "{}", s.status);
    }

    #[test]
    fn a_fader_op_lands_in_history_like_any_edit() {
        let mut s = Studio::demo();
        let t = s.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().id;
        s.arrangement_dirty = false;
        s.edit(studio_core::Op::SetTrackGain { id: t, gain: 0.4 }, "gain");
        let g = |s: &Studio| s.session.doc.project.tracks.iter()
            .find(|x| x.id == t).unwrap().gain;
        assert_eq!(g(&s), 0.4);
        assert!(s.arrangement_dirty, "the render knows the mix moved");
        s.key(Key::c('u'));
        assert_eq!(g(&s), 1.0, "one gesture, one undo step");
    }
}

#[cfg(test)]
mod collab_tests {
    use super::*;
    use studio_egui::keys::Key;

    /// Two peers on the same base project, different sites — and a wire
    /// between them made of nothing but `drain_outbox` → `integrate_packet`,
    /// which is exactly the seam any real carrier will occupy.
    fn pair() -> (Studio, Studio) {
        let a = Studio::demo();
        let base = a.session.doc.project.clone();
        let b = Studio::new(base, SiteId(2), Team::from_contacts("duo", &["sc-b".into()]));
        (a, b)
    }

    #[test]
    fn a_voice_frame_goes_to_the_inbox_and_nowhere_near_the_history() {
        let (_, mut b) = pair();
        let before = b.session.doc.history().len();
        let v = studio_collab::wire::Packet::Voice { site: SiteId(1), seq: 7, frame: vec![1, 2, 3] };
        let replies = b.integrate_packet(v.clone(), 0);
        assert!(replies.is_empty(), "a voice frame earns no reply");
        assert_eq!(b.voice_inbox, vec![v], "and waits in the inbox for the call");
        assert_eq!(b.session.doc.history().len(), before, "never an op");
        assert!(b.drain_outbox().is_empty(), "and nothing to broadcast");
    }

    fn pipe(from: &mut Studio, to: &mut Studio) {
        for p in from.drain_outbox() {
            to.integrate_packet(p, 0);
        }
    }

    #[test]
    fn two_studios_converge_on_a_beat() {
        let (mut a, mut b) = pair();
        // A programs; the rack register carries it over.
        a.rack.patterns[0].toggle(1, 3);
        a.rack.swing = 0.22;
        a.sync_rack_if_changed();
        pipe(&mut a, &mut b);
        assert_eq!(b.rack, a.rack, "B hears A's groove");
        assert!(b.loop_dirty, "and knows to re-render it");
        // B answers with a sound-design change; A adopts it.
        if let studio_live::SynthKind::Sub { decay_ms, .. } = &mut b.rack.channels[0].kind {
            *decay_ms = 500.0;
        }
        b.sync_rack_if_changed();
        pipe(&mut b, &mut a);
        assert_eq!(a.rack, b.rack, "A hears B's 808");
        assert!(a.status.contains("peer"), "and was told: {:?}", a.status);
    }

    #[test]
    fn concurrent_project_edits_converge_without_a_referee() {
        let (mut a, mut b) = pair();
        let clip = *a.session.doc.project.clips.keys().next().unwrap();
        let track = a.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().id;
        // Both edit BEFORE either hears the other — the order-independence
        // the Doc promises, exercised through the shell's own door.
        a.edit(studio_core::Op::RemoveClip { id: clip }, "deleted");
        b.edit(studio_core::Op::SetTrackGain { id: track, gain: 0.6 }, "gain");
        pipe(&mut a, &mut b);
        pipe(&mut b, &mut a);
        assert_eq!(a.session.doc.project, b.session.doc.project,
                   "both peers hold the same project");
        assert!(!a.session.doc.project.clips.contains_key(&clip));
        assert_eq!(a.session.doc.project.tracks.iter()
                   .find(|t| t.id == track).unwrap().gain, 0.6);
    }

    #[test]
    fn an_undo_travels_as_the_edit_it_is() {
        let (mut a, mut b) = pair();
        let clips_before = a.session.doc.project.clips.len();
        a.bounce_current_pattern(48_000, &Default::default()).expect("bounced");
        pipe(&mut a, &mut b);
        assert_eq!(b.session.doc.project.clips.len(), clips_before + 1,
                   "B sees the bounce land");
        a.key(Key::c('u'));
        a.key(Key::c('u'));
        pipe(&mut a, &mut b);
        assert_eq!(b.session.doc.project.clips.len(), clips_before,
                   "and sees it taken back — an undo is an op, not local time travel");
        assert_eq!(a.session.doc.project, b.session.doc.project);
    }

    #[test]
    fn a_hello_catches_up_a_peer_who_missed_the_wire() {
        // The Hello/Catchup flow is for peers who SHARE the base and missed
        // ops — a flaky link, not a new face.
        let (mut a, mut b) = pair();
        a.rack.swing = 0.3;
        a.sync_rack_if_changed();
        let clip = *a.session.doc.project.clips.keys().next().unwrap();
        a.edit(studio_core::Op::RemoveClip { id: clip }, "deleted");
        a.drain_outbox(); // lost in transit — B heard none of it

        let replies = a.integrate_packet(
            studio_collab::wire::Packet::Hello { site: SiteId(2), since: 0 }, 0);
        assert!(!replies.is_empty(), "a Hello is answered");
        for p in replies {
            b.integrate_packet(p, 0);
        }
        assert_eq!(b.session.doc.project, a.session.doc.project, "B caught up on the project");
        assert_eq!(b.rack, a.rack, "and on the rack");
    }

    #[test]
    fn a_true_newcomer_joins_from_the_snapshot_not_the_log() {
        // Session::broadcast()'s own docs record the double-apply bug that
        // snapshot+log replay caused; the shell must follow the documented
        // join flow — broadcast() for the project, rack_broadcast() for the
        // rack — and this pins that the two suffice.
        let (mut a, _) = pair();
        a.rack.patterns[0].toggle(4, 9);
        a.sync_rack_if_changed();
        a.drain_outbox();

        let mut c = Studio::new(
            Project { name: a.session.doc.project.name.clone(), ..Default::default() },
            SiteId(3), Team::from_contacts("trio", &["sc-c".into()]));
        c.integrate_packet(a.session.broadcast(), 0);
        if let Some(rack) = a.session.rack_broadcast() {
            c.integrate_packet(rack, 0);
        }
        assert_eq!(c.session.doc.project, a.session.doc.project, "C holds the project");
        assert_eq!(c.rack, a.rack, "and the rack");
    }

    #[test]
    fn simultaneous_rack_edits_end_in_one_shared_rack() {
        // Whole-value LWW: someone's version wins by (seq, site) — the point
        // is not WHO wins but that both peers agree on it without a server.
        let (mut a, mut b) = pair();
        a.rack.swing = 0.10;
        b.rack.swing = 0.40;
        a.sync_rack_if_changed();
        b.sync_rack_if_changed();
        let from_a = a.drain_outbox();
        let from_b = b.drain_outbox();
        for p in from_a { b.integrate_packet(p, 0); }
        for p in from_b { a.integrate_packet(p, 0); }
        assert_eq!(a.rack, b.rack, "one rack, agreed without a referee");
        assert!(a.rack.swing == 0.10 || a.rack.swing == 0.40);
    }
}

#[cfg(test)]
mod asset_transfer_tests {
    use super::*;

    /// Deliver packets both ways until nothing is in flight — outboxes AND
    /// the reply chains integrate_packet produces. This loop is a stand-in
    /// for any real carrier, which is exactly the claim under test.
    fn pump(a: &mut Studio, b: &mut Studio) {
        for _ in 0..10 {
            let mut quiet = true;
            let mut to_b: Vec<_> = a.drain_outbox();
            let mut to_a: Vec<_> = b.drain_outbox();
            while let Some(p) = to_b.pop() {
                quiet = false;
                to_a.extend(b.integrate_packet(p, 0));
            }
            while let Some(p) = to_a.pop() {
                quiet = false;
                to_b.extend(a.integrate_packet(p, 0));
            }
            while let Some(p) = to_b.pop() {
                to_a.extend(b.integrate_packet(p, 0));
            }
            for p in to_a {
                a.integrate_packet(p, 0);
            }
            if quiet && a.outbox.is_empty() && b.outbox.is_empty() {
                break;
            }
        }
    }

    fn pair() -> (Studio, Studio) {
        let a = Studio::demo();
        let base = a.session.doc.project.clone();
        let b = Studio::new(base, SiteId(2), Team::from_contacts("duo", &["sc-b".into()]));
        (a, b)
    }

    #[test]
    fn a_bounce_arrives_at_the_peer_as_sound_not_a_silent_clip() {
        // The whole point of the asset layer, end to end: A makes a beat,
        // B can RENDER it — op, manifest, chunks, reassembly, decode, mix.
        let (mut a, mut b) = pair();
        let (id, audio) = a.bounce_current_pattern(48_000, &Default::default()).expect("bounced");
        let wav = studio_audio::export::export_wav_dithered_bytes(
            &audio.channels[0], 1, 48_000, id.0).unwrap();
        a.register_asset(id, &wav);

        pump(&mut a, &mut b);

        let arrived: std::collections::BTreeMap<_, _> =
            std::mem::take(&mut b.incoming_audio).into_iter().collect();
        let got = arrived.get(&id).expect("B holds the bounce's audio");
        assert_eq!(got.frames(), audio.frames(), "sample-for-sample the same length");
        let out = b.render_arrangement(48_000, &arrived);
        let head: f32 = out[..9600].iter().map(|v| v.abs()).sum();
        assert!(head > 5.0, "B's own mixdown plays A's kick ({head})");
    }

    #[test]
    fn chunks_arriving_shuffled_still_assemble_verified() {
        let (mut a, mut b) = pair();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) % 256) as u8).collect();
        // A source op so B knows the id (path names a decodable type: none —
        // deliberately raw; decode fails, transfer must still complete and
        // say so, because bytes-integrity and media-meaning are different
        // failures).
        let src = studio_core::SourceId(99);
        a.edit(studio_core::Op::AddSource { source: studio_core::Source {
            id: src, path: "blob.bin".into(), len: Ticks::from_seconds(1.0),
            sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }, "seeded");
        a.register_asset(src, &data);

        // Hand-deliver: op+manifest to B, then B's chunk request to A, then
        // A's chunk replies to B IN REVERSE ORDER.
        let mut to_b = a.drain_outbox();
        let mut req = Vec::new();
        for p in to_b.drain(..) { req.extend(b.integrate_packet(p, 0)); }
        let mut chunks = Vec::new();
        for p in req { chunks.extend(a.integrate_packet(p, 0)); }
        assert!(chunks.len() >= 2, "big enough to have several chunks: {}", chunks.len());
        chunks.reverse();
        for p in chunks { b.integrate_packet(p, 0); }
        assert!(b.pending_assets.is_empty(), "assembled despite the order");
        assert!(b.manifests.contains_key(&src), "and B can now serve it onward");
        assert!(b.status.contains("undecodable"), "the media failure was said: {:?}", b.status);
    }

    #[test]
    fn a_corrupt_chunk_is_dropped_not_assembled() {
        let (mut _a, mut b) = pair();
        let real = vec![7u8; 20_000];
        let id = studio_collab::assets::hash(&real);
        b.integrate_packet(studio_collab::wire::Packet::Chunk {
            id, bytes: vec![8u8; 20_000], // wrong content for that id
        }, 0);
        assert!(!b.chunk_store.contains_key(&id), "a lying chunk never enters the store");
        b.integrate_packet(studio_collab::wire::Packet::Chunk { id, bytes: real }, 0);
        assert!(b.chunk_store.contains_key(&id), "the honest one does");
    }

    #[test]
    fn shared_content_transfers_nothing() {
        // Content-addressing's promise: if B already holds every chunk (it
        // imported the same sample), a manifest completes with zero chunk
        // requests.
        let (mut a, mut b) = pair();
        let sample = vec![42u8; 100_000];
        let src_a = studio_core::SourceId(50);
        let src_b = studio_core::SourceId(60);
        b.register_asset(src_b, &sample); // B has the bytes under its own id
        b.drain_outbox();
        a.edit(studio_core::Op::AddSource { source: studio_core::Source {
            id: src_a, path: "kick.bin".into(), len: Ticks::from_seconds(1.0),
            sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }, "seeded");
        a.register_asset(src_a, &sample);
        let mut replies = Vec::new();
        for p in a.drain_outbox() { replies.extend(b.integrate_packet(p, 0)); }
        assert!(replies.iter().all(|p| !matches!(p,
            studio_collab::wire::Packet::ChunkRequest { .. })),
            "no chunks were requested for bytes already held");
        assert!(b.manifests.contains_key(&src_a), "yet the asset completed");
    }
}

#[cfg(test)]
mod tcp_carrier_tests {
    use super::*;
    use studio_collab::stream::StreamPeer;

    /// The shell's per-frame pump, extracted for the test: events → hello,
    /// incoming → integrate (+replies), outbox → wire. Exactly what App::ui
    /// runs when a carrier exists.
    fn pump(s: &mut Studio, c: &StreamPeer) {
        for ev in c.events.try_iter() {
            if let studio_collab::stream::PeerEvent::Connected(_) = ev {
                c.send(studio_collab::wire::Packet::Hello {
                    site: s.session.doc.site, since: 0 });
                if let Some(rack) = s.session.rack_broadcast() {
                    c.send(rack);
                }
            }
        }
        for p in c.poll() {
            for r in s.integrate_packet(p, 0) {
                c.send(r);
            }
        }
        for p in s.drain_outbox() {
            c.send(p);
        }
    }

    #[test]
    fn the_flagship_over_an_actual_socket() {
        // Everything previous turns proved through Vec pipes, now through
        // 127.0.0.1: A bounces and edits the rack; B — connected only by
        // TCP — ends up able to RENDER A's kick, holding A's groove.
        let mut a = Studio::demo();
        let mut b = Studio::new(a.session.doc.project.clone(), SiteId(2),
                                Team::from_contacts("duo", &["sc-b".into()]));
        let ca = StreamPeer::listen("127.0.0.1:0").expect("bind");
        let cb = StreamPeer::connect(ca.local.unwrap()).expect("dial");

        let (id, audio) = a.bounce_current_pattern(48_000, &Default::default()).expect("bounced");
        let wav = studio_audio::export::export_wav_dithered_bytes(
            &audio.channels[0], 1, 48_000, id.0).unwrap();
        a.register_asset(id, &wav);
        a.rack.swing = 0.19;
        a.sync_rack_if_changed();

        let mut arrived = None;
        for _ in 0..400 {
            pump(&mut a, &ca);
            pump(&mut b, &cb);
            if let Some(pos) = b.incoming_audio.iter().position(|(s, _)| *s == id) {
                arrived = Some(b.incoming_audio.remove(pos).1);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let got = arrived.expect("A's bounce crossed the wire");
        assert_eq!(got.frames(), audio.frames());
        assert_eq!(b.rack.swing, 0.19, "the groove crossed too");
        assert_eq!(b.session.doc.project, a.session.doc.project, "and the project converged");
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(id, got);
        let out = b.render_arrangement(48_000, &sources);
        let head: f32 = out[..9600].iter().map(|v| v.abs()).sum();
        assert!(head > 5.0, "B's mixdown plays A's kick, delivered by TCP ({head})");
    }

    #[test]
    fn a_late_joiner_over_tcp_catches_up_from_hello() {
        // B exists (same base), then A edits while NOBODY is connected —
        // the outbox drains to the void. When the socket finally opens, the
        // since:0 Hello is the only thing that recovers those edits; see
        // the comment at the send site for why Doc::hello() would not.
        let mut a = Studio::demo();
        let mut b = Studio::new(a.session.doc.project.clone(), SiteId(2),
                                Team::from_contacts("duo", &["sc-b".into()]));
        let clip = *a.session.doc.project.clips.keys().next().unwrap();
        a.edit(studio_core::Op::RemoveClip { id: clip }, "deleted");
        a.drain_outbox(); // nobody was listening yet

        let ca = StreamPeer::listen("127.0.0.1:0").expect("bind");
        let cb = StreamPeer::connect(ca.local.unwrap()).expect("dial");
        for _ in 0..200 {
            pump(&mut a, &ca);
            pump(&mut b, &cb);
            if a.session.doc.project == b.session.doc.project {
                return; // converged purely from the connection handshake
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("projects never converged over the socket");
    }

    #[test]
    fn a_word_crosses_the_socket_and_comes_out_of_the_far_sink() {
        // The call end to end with the ONLY thing faked being the two
        // devices: A's Call encodes (Opus, if this build has it) into
        // packets, A's carrier ships them, B's Studio integrates them into
        // its inbox, B's Call decodes them into B's sink. The document is
        // untouched by all of it.
        use crate::call::fakes::{FakeMic, FakeSink, sine};
        use studio_collab::voice::FRAME;
        let mut a = Studio::demo();
        let mut b = Studio::new(a.session.doc.project.clone(), SiteId(2),
                                Team::from_contacts("duo", &["sc-b".into()]));
        let ca = StreamPeer::listen("127.0.0.1:0").expect("bind");
        let cb = StreamPeer::connect(ca.local.unwrap()).expect("dial");
        let (mut call_a, mut call_b) = (call::Call::new(), call::Call::new());
        let (mut mic_a, mut sink_a) = (FakeMic::new(48_000), FakeSink::new(48_000));
        let (mut mic_b, mut sink_b) = (FakeMic::new(48_000), FakeSink::new(48_000));
        call_a.start(&mut mic_a);
        call_b.start(&mut mic_b);
        let history = b.session.doc.history().len();
        mic_a.say(&sine(FRAME * 20, 440.0, 48_000, 0.5));
        let mut inbox_b = 0;
        for _ in 0..400 {
            // A's microphone keeps running after the word — a real tap
            // yields room tone every frame, and a peer that goes silent
            // on the wire is what concealment is for, not this.
            mic_a.say(&[0.0; FRAME]);
            // Exactly the shell's order: wire → integrate → inbox → call → wire.
            pump(&mut a, &ca);
            for p in a.voice_inbox.drain(..) { call_a.receive(&p); }
            for p in call_a.pump(&mut mic_a, &mut sink_a, a.session.doc.site) { ca.send(p); }
            pump(&mut b, &cb);
            for p in b.voice_inbox.drain(..) { inbox_b += 1; call_b.receive(&p); }
            // B's device plays one frame per turn; the pump refills one slot.
            sink_b.consume(FRAME);
            for p in call_b.pump(&mut mic_b, &mut sink_b, b.session.doc.site) { cb.send(p); }
            if inbox_b >= 40 && sink_b.played.len() >= FRAME * 30 { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(inbox_b >= 20, "B's Studio handed {inbox_b} voice frames to the call");
        let peak = sink_b.played.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.3, "A's tone came out of B's sink over TCP: peak {peak}");
        let m = call_b.meter();
        assert_eq!((m.peers, m.playing, m.unknown), (1, 1, 0), "{m:?}");
        assert!(m.lost <= 1, "a steady sender over loopback needs no concealment: {m:?}");
        assert_eq!(b.session.doc.history().len(), history, "the call never touched the document");
        assert_eq!(a.session.doc.project, b.session.doc.project);
        assert!(call_b.describe(&mic_b).starts_with("call ("), "{}", call_b.describe(&mic_b));
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn a_beat_survives_a_restart() {
        // The whole feature in one arc: program, bounce, mix, save; then a
        // FRESH Studio opens the file and holds the same project, the same
        // groove, and — because the bounce's WAV sits next to the project —
        // the same audible kick.
        let dir = std::env::temp_dir().join(format!("studio-persist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Studio::demo();
        a.rack.patterns[0].toggle(2, 9);
        a.rack.swing = 0.17;
        let (id, audio) = a.bounce_current_pattern(48_000, &Default::default()).expect("bounced");
        let track = a.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().id;
        a.edit(studio_core::Op::SetTrackGain { id: track, gain: 0.8 }, "gain");
        // Write the bounce's WAV where its op's path says, relative to the
        // project — the same layout the shell produces.
        let src_path = a.session.doc.project.sources.get(&id).unwrap().path.clone();
        let wav = studio_audio::export::export_wav_dithered_bytes(
            &audio.channels[0], 1, 48_000, id.0).unwrap();
        let disk = dir.join(&src_path);
        std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
        std::fs::write(&disk, &wav).unwrap();

        let file = dir.join("beat.studio");
        a.save_to(&file).expect("saves");
        assert!(a.status.contains("saved"));

        let mut b = Studio::load_from(&file, SiteId(1),
            Team::from_contacts("solo", &["sc-local".into()])).expect("opens");
        assert_eq!(b.session.doc.project, a.session.doc.project, "the arrangement survived");
        assert_eq!(b.rack, a.rack, "the groove survived");
        let arrived: std::collections::BTreeMap<_, _> =
            std::mem::take(&mut b.incoming_audio).into_iter().collect();
        assert!(arrived.contains_key(&id), "the bounce's media reloaded from disk");
        let out = b.render_arrangement(48_000, &arrived);
        let head: f32 = out[..9600].iter().map(|v| v.abs()).sum();
        assert!(head > 4.0, "and it is audible after the restart ({head})");
        // The demo project's placeholder "demo.wav" never existed on disk;
        // exactly ONE missing source is therefore the correct report — the
        // bounce (asserted arrived above) is not it.
        assert!(b.status.contains("1 source file(s) missing"),
                "status was {:?}", b.status);
    }

    #[test]
    fn a_project_with_a_lost_sample_opens_and_says_so() {
        let dir = std::env::temp_dir().join(format!("studio-lost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Studio::demo();
        a.bounce_current_pattern(48_000, &Default::default()).expect("bounced");
        // Deliberately do NOT write the bounce's WAV.
        let file = dir.join("beat.studio");
        a.save_to(&file).expect("saves");
        let b = Studio::load_from(&file, SiteId(1),
            Team::from_contacts("solo", &["sc-local".into()])).expect("still opens");
        assert!(b.status.contains("missing"), "the loss is named: {:?}", b.status);
    }

    #[test]
    fn garbage_refuses_to_open_rather_than_becoming_an_empty_project() {
        let dir = std::env::temp_dir().join(format!("studio-garbage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-project.studio");
        std::fs::write(&file, b"three chords and the truth").unwrap();
        let err = Studio::load_from(&file, SiteId(1),
            Team::from_contacts("solo", &["sc-local".into()]));
        assert!(err.is_err(), "quietly loading empty over someone's work loses it");
    }
}

#[cfg(test)]
mod trim_shell_tests {
    use super::*;
    use studio_egui::keys::Key;

    #[test]
    fn a_left_trim_lands_as_two_facts_and_two_undos_restore_it() {
        // The view reports one gesture; the model records the two things it
        // truly was — material window and position — the same shape a
        // bounce's two ops set as precedent.
        let mut s = Studio::demo();
        let (id, clip) = s.session.doc.project.clips.iter()
            .map(|(i, c)| (*i, c.clone())).next().unwrap();
        let new_pos = Ticks(clip.pos.0 + Ticks::from_seconds(0.5).0);
        s.edit(studio_core::Op::TrimClip {
            id, src_in: Ticks(clip.src_in.0 + Ticks::from_seconds(0.5).0),
            len: Ticks(clip.len.0 - Ticks::from_seconds(0.5).0) }, "trimmed");
        s.edit(studio_core::Op::MoveClip { id, pos: new_pos, track: clip.track }, "trimmed");
        let now = &s.session.doc.project.clips[&id];
        assert_eq!(now.pos, new_pos);
        assert_eq!(Ticks(now.pos.0 + now.len.0), Ticks(clip.pos.0 + clip.len.0),
                   "the right edge did not move — that is what makes it a LEFT trim");
        s.key(Key::c('u'));
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips[&id], clip, "two undos, whole gesture back");
    }
}

#[cfg(test)]
mod bus_routing_tests {
    use super::*;

    #[test]
    fn the_drum_bus_shape_track_into_saturated_bus_into_master() {
        // The user-facing claim behind the Mixer's bus section, at the op
        // level the UI drives: route a track into a bus carrying tape
        // saturation, and the render CHANGES; mute the bus, and everything
        // through it is gone.
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let track = s.session.doc.project.clips.values()
            .find(|c| c.source == id).unwrap().track;

        let dry = s.render_arrangement(48_000, &sources);

        let bus = studio_core::BusId(1);
        s.edit(studio_core::Op::AddBus { bus: studio_core::Bus {
            id: bus, name: "Drums".into(), parent: None, gain: 1.0,
            chain: Vec::new(), sends: Vec::new(), muted: false, silent: false } }, "bus");
        s.edit(studio_core::Op::SetRoute { track, bus: Some(bus) }, "routed");
        let mut params = std::collections::BTreeMap::new();
        params.insert("profile".to_string(), 1.0); // tape
        params.insert("drive".to_string(), 0.8);
        s.edit(studio_core::Op::SetBusChain { id: bus, chain: vec![
            studio_core::Effect { kind: "saturation".into(), params, enabled: true }
        ] }, "chain");

        let wet = s.render_arrangement(48_000, &sources);
        assert_eq!(dry.len(), wet.len());
        let diff: f32 = dry.iter().zip(&wet).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "the bus chain shaped the sound (diff {diff})");
        let wet_e: f32 = wet[..9600].iter().map(|v| v.abs()).sum();
        assert!(wet_e > 5.0, "and the drums still come through it ({wet_e})");

        s.edit(studio_core::Op::SetBusFlags { id: bus, muted: true }, "bus muted");
        let muted = s.render_arrangement(48_000, &sources);
        let muted_e: f32 = muted[..9600].iter().map(|v| v.abs()).sum();
        assert!(muted_e < wet_e * 0.02,
                "a muted bus takes its whole tree out of the mix ({muted_e})");
        // Four ops, four undos, and the dry render comes back exactly.
        for _ in 0..4 { s.key(studio_egui::keys::Key::c('u')); }
        assert_eq!(s.render_arrangement(48_000, &sources), dry,
                   "the whole routing story is undoable");
    }
}

#[cfg(test)]
mod export_rate_tests {
    use super::*;

    #[test]
    fn a_96k_wav_export_is_the_same_music_at_twice_the_frames() {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let dir = std::env::temp_dir().join("studio-export-rate");
        let p48 = s.export_arrangement(&sources, &dir, ExportFormat::Wav, 48_000, studio_audio::format::ChannelFormat::Stereo).unwrap();
        let p96 = s.export_arrangement(&sources, &dir, ExportFormat::Wav, 96_000, studio_audio::format::ChannelFormat::Stereo).unwrap();
        let a = studio_media::decode_file(&p48).unwrap();
        let b = studio_media::decode_file(&p96).unwrap();
        assert_eq!(a.sample_rate, 48_000);
        assert_eq!(b.sample_rate, 96_000);
        let ratio = b.frames() as f64 / a.frames() as f64;
        assert!((ratio - 2.0).abs() < 0.01, "same duration, double the frames ({ratio})");
        // Same music: the bounced 48k source is resampled INTO the 96k
        // render (the mixdown source-rate fix), not pitch-shifted.
        let e = |x: &studio_audio::SourceAudio, upto: f64|
            x.channels[0][..(x.sample_rate as f64 * upto) as usize]
                .iter().map(|v| v.abs()).sum::<f32>() / x.sample_rate as f32;
        let (ea, eb) = (e(&a, 0.2), e(&b, 0.2));
        assert!((ea - eb).abs() < ea * 0.1,
                "the first 200ms carry the same energy per second: {ea} vs {eb}");
    }
}

#[cfg(test)]
mod playing_mode_tests {
    use super::*;
    use studio_egui::keys::Key;
    use studio_egui::playkeys::{play_note, InputMode, NullAnalog, PlayNote};

    #[test]
    fn a_played_note_lands_in_the_pattern_as_a_tuned_step() {
        let mut s = Studio::demo();
        s.selected_channel = 0; // the 808
        let note = play_note('d', 0, &Key::c('d'), &NullAnalog,
            &studio_egui::playkeys::Zones::default()).expect("E natural");
        s.play_note_at(note, 4);
        let p = &s.rack.patterns[s.current_pattern];
        assert!(p.is_set(0, 4), "the step is on");
        assert_eq!(p.mod_at(0, 4).tune, 4, "and carries the note's pitch");
        assert!(s.loop_dirty, "the audition loop knows to re-render");
    }

    #[test]
    fn analog_travel_becomes_step_velocity() {
        let mut s = Studio::demo();
        s.play_note_at(PlayNote { semitone: 0, velocity: 0.35 }, 2);
        assert!((s.rack.patterns[0].mod_at(0, 2).vel - 0.35).abs() < 1e-6,
                "a soft press stays soft in the pattern");
    }

    #[test]
    fn a_note_past_the_pattern_or_the_channels_is_dropped_not_panicked() {
        // Octave and column both come from live input; neither is trusted.
        let mut s = Studio::demo();
        let steps = s.rack.patterns[0].steps;
        s.play_note_at(PlayNote { semitone: 0, velocity: 1.0 }, steps + 5);
        s.selected_channel = 999;
        s.play_note_at(PlayNote { semitone: 0, velocity: 1.0 }, 0);
        assert_eq!(s.rack.patterns[0].hits(), studio_live::Rack::trap_kit().patterns[0].hits(),
                   "nothing was written, nothing crashed");
    }

    #[test]
    fn playing_mode_makes_the_editor_keys_inert_and_escape_gets_out() {
        // The trap this mode must not be: `d` in playing mode is a note, so
        // it must NOT delete — and Escape must always work.
        let mut s = Studio::demo();
        let clips = s.session.doc.project.clips.len();
        s.selection = vec![*s.session.doc.project.clips.keys().next().unwrap()];
        s.input_mode = InputMode::Playing;
        s.key(Key::c('d'));
        assert_eq!(s.session.doc.project.clips.len(), clips, "no edit happened");
        s.key(Key::c(' '));
        assert!(!s.transport.playing && !s.rack_playing, "not even transport");
        s.key(Key::c('\u{1b}'));
        assert_eq!(s.input_mode, InputMode::Motions, "escape is always the way out");
        s.key(Key::c(' '));
        assert!(s.rack_playing || s.transport.playing, "and the keymap is back");
    }
}

#[cfg(test)]
mod analog_adapter_tests {
    use super::*;
    use studio_egui::playkeys::AnalogSource;

    #[test]
    fn without_the_sdk_every_played_key_is_full_velocity() {
        // The property that lets playkeys have exactly one code path: no
        // runtime, no branch — held is 1.0, up is 0.0.
        let a = WootingAnalog::default();
        if !a.active() {
            assert_eq!(a.travel(&studio_egui::keys::Key::c('a'), true), 1.0);
            assert_eq!(a.travel(&studio_egui::keys::Key::c('a'), false), 0.0);
            // A key with no HID mapping still answers digitally rather than
            // going silent — a note that vanishes on one keyboard and not
            // another is the worst kind of input bug.
            assert_eq!(a.travel(&studio_egui::keys::Key::c('§'), true), 1.0);
        }
    }

    #[test]
    fn every_letter_and_digit_maps_to_its_hid_usage() {
        // Spot-check the arithmetic against the USB HID table: a is 0x04,
        // z is 0x1D, 1 is 0x1E, 0 is 0x27 (not 0x28 — the zero key sits
        // after nine, which is exactly the off-by-one worth pinning).
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('a')), Some(0x04));
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('z')), Some(0x1D));
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('1')), Some(0x1E));
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('9')), Some(0x26));
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('0')), Some(0x27));
        assert_eq!(WootingAnalog::hid_of(&studio_egui::keys::Key::c('Q')), Some(0x14),
                   "shifted or not, it is the same physical key");
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;
    use crate::audition::PlaybackSink;

    #[test]
    fn voices_are_additive_and_do_not_disturb_the_loop() {
        // A played note must not stop, restart, or reposition the pattern
        // the person is playing along to.
        let mut sink = audition::NullSink::new(48_000);
        sink.set_buffer(std::sync::Arc::new(vec![0.0; 48_000]), 1, true);
        sink.set_playing(true);
        std::thread::sleep(std::time::Duration::from_millis(15));
        let before = sink.position().expect("looping");
        sink.trigger(std::sync::Arc::new(vec![0.5; 4800]));
        sink.trigger(std::sync::Arc::new(vec![0.5; 4800]));
        assert_eq!(sink.voices(), 2, "both notes were taken");
        assert!(sink.playing(), "the loop kept playing");
        assert!(sink.position().unwrap() >= before, "and did not rewind");
    }
}

#[cfg(test)]
mod bus_sends_tests {
    use super::*;

    /// A send is a PARALLEL tap: the dry path survives it. That distinction
    /// is the whole reason the Mixer needed a sends row beside routing —
    /// routing a drum bus into a reverb bus replaces the drums with reverb,
    /// which is not what "add reverb" means.
    #[test]
    fn a_send_adds_a_parallel_copy_without_removing_the_dry_signal() {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let track = s.session.doc.project.clips.values()
            .find(|c| c.source == id).unwrap().track;

        // Drums -> their own bus; a second bus stands in for the effect
        // return (its chain is irrelevant to the routing question).
        let drums = studio_core::BusId(1);
        let fx = studio_core::BusId(2);
        for (id, name) in [(drums, "Drums"), (fx, "FX")] {
            s.edit(studio_core::Op::AddBus { bus: studio_core::Bus {
                id, name: name.into(), parent: None, gain: 1.0,
                chain: Vec::new(), sends: Vec::new(), muted: false, silent: false } }, "bus");
        }
        s.edit(studio_core::Op::SetRoute { track, bus: Some(drums) }, "routed");
        let dry = s.render_arrangement(48_000, &sources);

        s.edit(studio_core::Op::SetBusSends { id: drums, sends: vec![
            studio_core::Send { to: fx, level: 0.5, pre_fader: false } ] }, "send");
        let sent = s.render_arrangement(48_000, &sources);

        let energy = |v: &[f32]| v.iter().map(|x| x.abs()).sum::<f32>();
        assert!(energy(&sent) > energy(&dry) * 1.2,
                "the copy arrives on top of the dry path ({} vs {})",
                energy(&sent), energy(&dry));

        // And muting the RETURN leaves the dry path standing — the proof it
        // was parallel rather than in series.
        s.edit(studio_core::Op::SetBusFlags { id: fx, muted: true }, "fx muted");
        let muted_return = s.render_arrangement(48_000, &sources);
        let diff: f32 = dry.iter().zip(&muted_return).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff < energy(&dry) * 0.01,
                "with the return muted the mix is the dry mix again (diff {diff})");
    }

    #[test]
    fn send_level_scales_the_copy() {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let track = s.session.doc.project.clips.values()
            .find(|c| c.source == id).unwrap().track;
        let a = studio_core::BusId(1);
        let b = studio_core::BusId(2);
        for (id, name) in [(a, "A"), (b, "B")] {
            s.edit(studio_core::Op::AddBus { bus: studio_core::Bus {
                id, name: name.into(), parent: None, gain: 1.0,
                chain: Vec::new(), sends: Vec::new(), muted: false, silent: false } }, "bus");
        }
        s.edit(studio_core::Op::SetRoute { track, bus: Some(a) }, "routed");
        let energy = |v: &[f32]| v.iter().map(|x| x.abs()).sum::<f32>();
        let dry = energy(&s.render_arrangement(48_000, &sources));

        let at = |s: &mut Studio, level: f32| {
            s.edit(studio_core::Op::SetBusSends { id: a, sends: vec![
                studio_core::Send { to: b, level, pre_fader: false } ] }, "send");
            energy(&s.render_arrangement(48_000, &sources))
        };
        let quarter = at(&mut s, 0.25);
        let full = at(&mut s, 1.0);
        assert!(full - dry > (quarter - dry) * 3.0,
                "a louder send is a louder copy: +{} vs +{}", full - dry, quarter - dry);
    }
}

#[cfg(test)]
mod midi_shell_tests {
    use super::*;

    #[test]
    fn a_controller_note_sounds_through_the_selected_channel() {
        // One note path: a MIDI note and a keyboard note are the same
        // PlayNote and hit the same voice, so they cannot drift.
        let mut s = Studio::demo();
        s.selected_channel = 1; // the kit's Kick
        let n = midi::play_note(60, 100);
        let out = s.render_play_note(&n, 48_000);
        assert!(!out.is_empty());
        let e: f32 = out.iter().map(|v| v.abs()).sum();
        assert!(e > 1.0, "the note is audible ({e})");
        // Velocity scales it, and a harder note is louder.
        let soft: f32 = s.render_play_note(&midi::play_note(60, 20), 48_000)
            .iter().map(|v| v.abs()).sum();
        assert!(soft < e, "velocity 20 is quieter than 100: {soft} vs {e}");
    }

    #[test]
    fn a_recorded_note_lands_quantised_on_a_real_step() {
        let mut s = Studio::demo();
        s.selected_channel = 2;
        // Mid-pattern, deliberately off-grid: a step sequencer has no
        // between-steps place to put it.
        let step_len = s.rack.tempo().step_len();
        s.transport.set_position(Ticks(step_len.0 * 5 + step_len.0 / 3));
        let step = s.recording_step();
        assert_eq!(step, 5, "a third of a step past 5 quantises back to 5");
        s.play_note_at(midi::play_note(67, 100), step);
        let p = &s.rack.patterns[s.current_pattern];
        assert!(p.is_set(2, 5));
        assert_eq!(p.mod_at(2, 5).tune, 7, "a G above middle C is +7");
    }

    #[test]
    fn recording_past_the_pattern_wraps_instead_of_vanishing() {
        // A 16-step pattern under a transport that has run for minutes must
        // still record — the note belongs to the bar it is heard in.
        let mut s = Studio::demo();
        let steps = s.rack.patterns[s.current_pattern].steps as i64;
        let step_len = s.rack.tempo().step_len();
        s.transport.set_position(Ticks(step_len.0 * (steps * 7 + 3)));
        assert_eq!(s.recording_step(), 3);
        // And a count-in (negative position) lands on a real step, not a panic.
        s.transport.set_position(Ticks(-step_len.0 * 2));
        assert!(s.recording_step() < steps as u32);
    }
}

#[cfg(test)]
mod autotune_take_tests {
    use super::*;
    use studio_fx::retune::{Mode, Scale};

    /// A detuned sine, long enough for several analysis hops.
    fn detuned_take(hz: f32, secs: f32) -> studio_audio::SourceAudio {
        let n = (48_000.0 * secs) as usize;
        let ch: Vec<f32> = (0..n)
            .map(|i| (i as f32 / 48_000.0 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        studio_audio::SourceAudio { sample_rate: 48_000, channels: vec![ch] }
    }

    fn studio_with_take() -> (Studio, std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>) {
        let mut s = Studio::demo();
        // 452 Hz: a badly sharp A4 (440). In A minor it must come home to A.
        let take = detuned_take(452.0, 1.0);
        s.transport.set_position(Ticks::ZERO);
        let id = s.import_clip("vox", "takes/vox.wav", &take).expect("lands");
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(id, take);
        let clip = *s.session.doc.project.clips.iter()
            .find(|(_, c)| c.source == id).map(|(k, _)| k).unwrap();
        s.selection = vec![clip];
        (s, sources)
    }

    #[test]
    fn tuning_a_take_pulls_it_toward_the_key_and_keeps_the_dry_one() {
        let (mut s, sources) = studio_with_take();
        let clips_before = s.session.doc.project.clips.len();
        let (id, tuned) = s.autotune_selected(&sources, Scale { root: 9, mode: Mode::Minor })
            .expect("tuned");

        assert_eq!(s.session.doc.project.clips.len(), clips_before + 1,
                   "the tuned take is a NEW clip; the dry one survives for A/B");
        assert_eq!(tuned.frames(), sources.values().next().unwrap().frames(),
                   "same length — a tuned take still lines up with the beat");

        // Measure the result's pitch the same way the tuner does.
        let mid = &tuned.channels[0][12_000..24_000];
        let got = studio_fx::retune::yin_pitch(mid, 48_000.0, 80.0, 1000.0, 0.15)
            .expect("the tuned take still has a detectable pitch");
        assert!((got.hz - 440.0).abs() < (452.0 - 440.0) * 0.5,
                "pulled toward A440, landed at {:.1} Hz", got.hz);
        // And the new source is real in the project.
        assert!(s.session.doc.project.sources.contains_key(&id));
    }

    #[test]
    fn tuning_is_undoable_as_the_two_ops_it_is() {
        let (mut s, sources) = studio_with_take();
        let before = s.session.doc.project.clone();
        s.autotune_selected(&sources, Scale { root: 9, mode: Mode::Minor }).expect("tuned");
        s.key(studio_egui::keys::Key::c('u'));
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.session.doc.project, before, "AddSource + AddClip, both taken back");
    }

    #[test]
    fn with_nothing_selected_it_says_so_instead_of_guessing() {
        let (mut s, sources) = studio_with_take();
        s.selection.clear();
        assert!(s.autotune_selected(&sources, Scale { root: 0, mode: Mode::Minor }).is_none());
        assert!(s.status.contains("select a clip"), "status was {:?}", s.status);
    }

    #[test]
    fn a_clip_whose_audio_is_not_loaded_refuses_rather_than_tuning_silence() {
        // A peer's clip before its media arrives: tuning an absent source
        // would land a silent "take" the user would have to discover later.
        let (mut s, _) = studio_with_take();
        assert!(s.autotune_selected(&Default::default(),
                                    Scale { root: 9, mode: Mode::Minor }).is_none());
        assert!(s.status.contains("not loaded"), "status was {:?}", s.status);
    }
}

#[cfg(test)]
mod surround_export_tests {
    use super::*;
    use studio_audio::format::ChannelFormat;

    #[test]
    fn a_five_one_export_is_six_real_channels_the_app_can_read_back() {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let dir = std::env::temp_dir().join("studio-surround");
        let path = s.export_arrangement(&sources, &dir, ExportFormat::Wav,
                                        48_000, ChannelFormat::Surround51).expect("exports");
        assert!(path.to_string_lossy().contains("5.1"),
                "the layout is in the filename: {}", path.display());
        let back = studio_media::decode_file(&path).expect("our decoder reads it");
        assert_eq!(back.channels.len(), 6);
        // The mains carry the music, the LFE carries bass, and the file is
        // not just stereo padded with silence.
        let e = |c: usize| back.channels[c].iter().map(|v| v.abs()).sum::<f32>();
        assert!(e(0) > 1.0 && e(1) > 1.0, "mains are live");
        assert!(e(2) > 0.0, "centre carries the correlated content");
        assert!(e(3) > 0.0, "LFE carries the low end");
    }

    #[cfg(feature = "audio")] // measures real Opus encoding
    #[test]
    fn opus_refuses_to_pretend_it_can_carry_surround() {
        // Mapping family 0 is mono/stereo; an encoder error at the end of a
        // long render is a worse way to learn that than a stereo file with
        // a status line that says stereo.
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let dir = std::env::temp_dir().join("studio-surround-opus");
        let path = s.export_arrangement(&sources, &dir, ExportFormat::Opus,
                                        48_000, ChannelFormat::Surround51).expect("exports");
        let back = studio_media::decode_file(&path).expect("decodes");
        assert_eq!(back.channels.len(), 2);
        assert!(s.status.contains("2.0"), "and it says so: {:?}", s.status);
    }

    #[test]
    fn every_layout_writes_a_file_with_its_own_channel_count() {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let dir = std::env::temp_dir().join("studio-layouts");
        for layout in ChannelFormat::ALL {
            if layout == ChannelFormat::Mono { continue } // expand has no mono target
            let path = s.export_arrangement(&sources, &dir, ExportFormat::Wav,
                                            48_000, layout).expect("exports");
            let back = studio_media::decode_file(&path).expect("decodes");
            assert_eq!(back.channels.len(), layout.channels(),
                       "{layout} wrote {} channels", back.channels.len());
        }
    }
}

#[cfg(test)]
mod clip_fx_tests {
    use super::*;

    fn tone(secs: f32) -> studio_audio::SourceAudio {
        let n = (48_000.0 * secs) as usize;
        let ch: Vec<f32> = (0..n)
            .map(|i| (i as f32 / 48_000.0 * 220.0 * std::f32::consts::TAU).sin() * 0.4)
            .collect();
        studio_audio::SourceAudio { sample_rate: 48_000, channels: vec![ch] }
    }

    /// Two clips on the timeline, each on its own track, so a per-clip edit
    /// can be shown to touch ONE of them — the distinction that justifies
    /// per-clip effects existing beside bus chains.
    fn two_clips() -> (Studio, std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
                       Vec<studio_core::ClipId>) {
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let mut ids = Vec::new();
        for i in 0..2 {
            let a = tone(0.5);
            s.transport.set_position(Ticks::ZERO);
            let id = s.import_clip(&format!("t{i}"), &format!("takes/t{i}.wav"), &a)
                .expect("lands on a free track");
            sources.insert(id, a);
            ids.push(*s.session.doc.project.clips.iter()
                .find(|(_, c)| c.source == id).map(|(k, _)| k).unwrap());
        }
        (s, sources, ids)
    }

    #[test]
    fn a_clip_effect_changes_that_clip_and_leaves_its_neighbour_alone() {
        let (mut s, sources, ids) = two_clips();
        let before = s.render_arrangement(48_000, &sources);
        let mut params = std::collections::BTreeMap::new();
        params.insert("profile".to_string(), 1.0);
        params.insert("drive".to_string(), 0.9);
        s.selection = vec![ids[0]];
        s.edit(studio_core::Op::SetEffects { id: ids[0], effects: vec![
            studio_core::Effect { kind: "saturation".into(), params, enabled: true } ] },
            "clip fx");
        let after = s.render_arrangement(48_000, &sources);
        let diff: f32 = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "the clip's own chain reached the render ({diff})");

        // Its neighbour is untouched: remove the first clip from the project
        // in both renders and the remainder must be identical.
        s.edit(studio_core::Op::RemoveClip { id: ids[0] }, "removed");
        let solo_neighbour = s.render_arrangement(48_000, &sources);
        s.key(studio_egui::keys::Key::c('u'));
        s.edit(studio_core::Op::SetEffects { id: ids[0], effects: vec![] }, "cleared");
        s.edit(studio_core::Op::RemoveClip { id: ids[0] }, "removed");
        let solo_neighbour_dry = s.render_arrangement(48_000, &sources);
        assert_eq!(solo_neighbour, solo_neighbour_dry,
                   "the other clip never heard the first clip's effect");
    }

    #[test]
    fn disabling_an_effect_keeps_it_in_the_chain_but_out_of_the_sound() {
        // The A/B a person wants is "with and without this one", in place —
        // not "delete it and add it back at the end".
        let (mut s, sources, ids) = two_clips();
        let mut params = std::collections::BTreeMap::new();
        params.insert("profile".to_string(), 1.0);
        params.insert("drive".to_string(), 0.9);
        let fx = studio_core::Effect { kind: "saturation".into(), params, enabled: true };
        s.edit(studio_core::Op::SetEffects { id: ids[0], effects: vec![fx.clone()] }, "fx");
        let wet = s.render_arrangement(48_000, &sources);

        let off = studio_core::Effect { enabled: false, ..fx };
        s.edit(studio_core::Op::SetEffects { id: ids[0], effects: vec![off] }, "bypassed");
        let dry = s.render_arrangement(48_000, &sources);
        assert_ne!(wet, dry, "bypass is audible");
        assert_eq!(s.session.doc.project.clips[&ids[0]].effects.len(), 1,
                   "and the effect kept its place in the chain");
    }

    #[test]
    fn clip_gain_is_a_single_undoable_op() {
        let (mut s, sources, ids) = two_clips();
        let loud = s.render_arrangement(48_000, &sources);
        s.edit(studio_core::Op::SetGain { id: ids[0], gain: 0.25 }, "clip gain");
        let quiet = s.render_arrangement(48_000, &sources);
        let e = |v: &[f32]| v.iter().map(|x| x.abs()).sum::<f32>();
        assert!(e(&quiet) < e(&loud), "the fader moved the clip");
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.render_arrangement(48_000, &sources), loud, "one gesture, one undo");
    }
}

/// A curve's points shifted from curve time to timeline time for the
/// strip: `+origin`, the gate's own definition of where t = 0 sits.
pub fn shift_points(points: &[(Ticks, f32)], origin: Ticks) -> Vec<(Ticks, f32)> {
    points.iter().map(|(t, v)| (t.saturating_add(origin), *v)).collect()
}

/// The inverse for an edit the strip reports in timeline time: back into
/// curve time, clamped at zero — a click left of a clip's start is a point
/// at the clip's start, never a negative curve time nothing could play.
pub fn curve_time(at: Ticks, origin: Ticks) -> Ticks {
    Ticks(at.saturating_sub(origin).0.max(0))
}

#[cfg(test)]
mod kit_tests {
    use super::*;

    #[test]
    fn a_folder_of_hits_becomes_channels_in_name_order_and_the_rest_is_counted() {
        let dir = std::env::temp_dir().join(format!("studio-kit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |n: usize| studio_audio::export::export_wav_dithered_bytes(
            &vec![0.2; 480 * n], 1, 48_000, 1).unwrap();
        std::fs::write(dir.join("kick.wav"), wav(10)).unwrap();
        std::fs::write(dir.join("clap.WAV"), wav(5)).unwrap();
        std::fs::write(dir.join("hat.wav"), wav(2)).unwrap();
        std::fs::write(dir.join("readme.txt"), b"not audio").unwrap();
        std::fs::write(dir.join("broken.flac"), b"nope").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut s = Studio::demo();
        let before = s.rack.channels.len();
        let hits = s.import_kit(&dir).unwrap();
        assert_eq!(hits.len(), 3);
        let names: Vec<&str> = s.rack.channels[before..].iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["clap", "hat", "kick"], "name order, extension case ignored");
        assert!(s.status.starts_with("kit '"), "{}", s.status);
        assert!(s.status.contains("3 channels added, 1 file(s) could not be decoded"), "{}", s.status);
        assert!(s.rack.channels[before..].iter().all(|c| matches!(c.kind, studio_live::SynthKind::Sampler { .. })));
        assert_eq!(hits[0].1.frames(), 480 * 5, "clap's audio came back with its id");
        assert_eq!(s.session.doc.project.sources.len(), 3 + Studio::demo().session.doc.project.sources.len());
        // An empty folder is refused, saying so.
        let empty = dir.join("sub");
        assert!(s.import_kit(&empty).unwrap_err().starts_with("no audio files"));
        assert!(s.import_kit(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod chord_tool_tests {
    use super::*;

    #[test]
    fn the_roll_tools_edit_the_selected_channel_and_say_what_they_did() {
        let mut s = Studio::demo();
        let ch = s.selected_channel;
        let pat = s.current_pattern;
        let note = |key: i8, start: u32| studio_live::Note { key, start, len: 4, vel: 0.8, slide: false };
        s.rack.patterns[pat].add_note(ch, note(3, 0));
        s.rack.patterns[pat].add_note(ch, note(3, 8));
        s.loop_dirty = false;
        assert_eq!(s.harmonize_roll(4), 2);
        assert!(s.status.starts_with("harmonize: 2 fifths added"), "{}", s.status);
        assert!(s.loop_dirty, "the loop must re-render");
        assert_eq!(s.rack.patterns[pat].roll(ch).len(), 4);
        assert_eq!(s.arpeggiate_roll(studio_live::Arp::Up, 1), 2, "two chords now");
        assert!(s.status.starts_with("arpeggiate: 2 chords"), "{}", s.status);
        assert_eq!(s.rack.patterns[pat].roll(ch).len(), 8, "four one-step notes per four-step chord");
        assert_eq!(s.arpeggiate_roll(studio_live::Arp::Up, 1), 0);
        assert!(s.status.contains("no chords"), "{}", s.status);
        // With a key set, the harmony is diatonic: C major, a third above C is E.
        s.edit(studio_core::Op::SetKey { key: Some(studio_core::Key { root: 0, scale: studio_core::Scale::Major }) }, "key");
        s.rack.patterns[pat].rolls.clear();
        s.rack.patterns[pat].add_note(ch, note(3, 0));
        s.harmonize_roll(2);
        assert!(s.status.ends_with("in key"), "{}", s.status);
        assert_eq!(s.rack.patterns[pat].roll(ch).iter().map(|n| n.key).collect::<Vec<_>>(), vec![3, 7]);
    }
}

#[cfg(test)]
mod take_shell_tests {
    //! The shell half of takes: a re-take lands on a lane, the comp verb
    //! copies the chosen lane clips onto the root, the lanes mute as a
    //! group, and the loop range is the selection's span.
    use super::*;
    use studio_core::{ClipId, Op, Ticks, TrackId};
    use studio_egui::keys::Key;

    /// A one-second mono source registered on the demo project.
    fn source(s: &mut Studio, n: u64) -> studio_core::SourceId {
        let audio = studio_audio::SourceAudio { sample_rate: 48_000, channels: vec![vec![0.1 * n as f32; 48_000]] };
        let id = s.import_audio(&format!("t{n}"), &format!("takes/t{n}.flac"), &audio);
        // import_audio appends a rack channel a vocal take does not want;
        // the shell drops it, the way toggle_take does.
        let appended = s.rack.channels.len().saturating_sub(1);
        s.rack.channels.truncate(appended);
        for p in &mut s.rack.patterns { p.rows.truncate(appended); }
        id
    }

    #[test]
    fn three_takes_at_the_playhead_are_three_lanes_and_one_undo_each() {
        let mut s = Studio::demo();
        s.selection.clear();
        let root = s.take_target().expect("an audio track");
        let far = Ticks::from_seconds(100.0); // where the demo has nothing
        let len = Ticks::from_seconds(1.0);
        let a = source(&mut s, 1);
        let b = source(&mut s, 2);
        let c = source(&mut s, 3);
        let (l1, c1) = s.place_take(a, len, far, root).unwrap();
        assert_eq!(l1, root, "the first take: on the track");
        assert!(s.status.starts_with("take 1.0s on"), "{}", s.status);
        let (l2, c2) = s.place_take(b, len, far, root).unwrap();
        assert_ne!(l2, root);
        assert!(s.status.contains("lane 2 of the chain"), "{}", s.status);
        let (l3, _c3) = s.place_take(c, len, far, root).unwrap();
        assert_eq!(s.session.doc.project.take_lanes(root), vec![l1, l2, l3]);
        // The lane and its clip were one undo step: `u` removes both.
        let tracks = s.session.doc.project.tracks.len();
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.tracks.len(), tracks - 1, "the third lane is gone with its take");
        assert_eq!(s.session.doc.project.take_lanes(root), vec![l1, l2]);
        // With a clip on a lane selected, the take target is the ROOT.
        s.selection = vec![c2];
        assert_eq!(s.take_target(), Some(root), "a re-take of a lane's phrase goes to the chain, not under the lane");
        // A take somewhere else lands on the root again.
        let (l4, _) = s.place_take(c, len, Ticks::from_seconds(200.0), root).unwrap();
        assert_eq!(l4, root);
        let _ = c1;
    }

    #[test]
    fn comp_copies_the_selected_lane_clips_onto_the_root_and_mutes_the_lanes() {
        let mut s = Studio::demo();
        s.selection.clear();
        let root = s.take_target().unwrap();
        let far = Ticks::from_seconds(100.0);
        let len = Ticks::from_seconds(1.0);
        let a = source(&mut s, 1);
        let b = source(&mut s, 2);
        let (_, _c1) = s.place_take(a, len, far, root).unwrap();
        let (l2, c2) = s.place_take(b, len, far, root).unwrap();
        // Split the second take in half on its lane, choose its second half.
        s.selection = vec![c2];
        s.transport.set_position(far.saturating_add(Ticks::from_seconds(0.5)));
        s.key(Key::c('s'));
        let halves: Vec<ClipId> = {
            let p = &s.session.doc.project;
            let mut v: Vec<&studio_core::Clip> = p.clips.values().filter(|c| c.track == l2).collect();
            v.sort_by_key(|c| c.pos);
            v.iter().map(|c| c.id).collect()
        };
        assert_eq!(halves.len(), 2, "split on the lane: two pieces");
        s.selection = vec![halves[1]];
        assert_eq!(s.comp_selected_takes(), 1);
        assert!(s.status.starts_with("comp: 1 take on the track"), "{}", s.status);
        let p = &s.session.doc.project;
        let mut on_root: Vec<&studio_core::Clip> = p.clips.values().filter(|c| c.track == root && c.pos >= far).collect();
        on_root.sort_by_key(|c| c.pos);
        assert_eq!(on_root.len(), 2, "take 1's head, take 2's tail");
        assert_eq!((on_root[0].source, on_root[0].len), (a, Ticks::from_seconds(0.5)));
        assert_eq!((on_root[1].source, on_root[1].src_in), (b, Ticks::from_seconds(0.5)), "the offset into take 2 is carried");
        assert_eq!(p.clips.values().filter(|c| c.track == l2).count(), 2, "the lane keeps both halves");
        // Mute the lanes: the comp is what plays. Then undo it all.
        s.selection = vec![on_root[0].id];
        assert_eq!(s.set_take_lanes_muted(true), 1);
        assert!(s.session.doc.project.tracks.iter().find(|t| t.id == l2).unwrap().muted);
        assert_eq!(s.set_take_lanes_muted(true), 0, "nothing left to mute");
        assert_eq!(s.set_take_lanes_muted(false), 1);
        s.key(Key::c('u')); // unmute
        s.key(Key::c('u')); // mute
        s.key(Key::c('u')); // the comp, as one step
        let p = &s.session.doc.project;
        let on_root: Vec<&studio_core::Clip> = p.clips.values().filter(|c| c.track == root && c.pos >= far).collect();
        assert_eq!(on_root.len(), 1, "one undo removed the comp piece and restored take 1 whole");
        assert_eq!(on_root[0].len, len);
        // A root clip selected: nothing to comp, said.
        s.selection = vec![on_root[0].id];
        assert_eq!(s.comp_selected_takes(), 0);
        assert!(s.status.starts_with("comp: select a take on a lane"), "{}", s.status);
    }

    #[test]
    fn the_loop_range_is_the_selections_span_and_off_clears_it() {
        let mut s = Studio::demo();
        let p = &s.session.doc.project;
        let c1 = ClipId(1);
        let (pos, end) = (p.clips[&c1].pos, p.clips[&c1].end());
        s.selection = vec![c1];
        s.loop_selection();
        assert_eq!(s.transport.loop_range, Some((pos, end)));
        assert!(s.arrangement_dirty, "the sink must reload with the range alone");
        assert!(s.status.starts_with("looping"), "{}", s.status);
        s.selection.clear();
        s.loop_selection();
        assert_eq!(s.transport.loop_range, Some((pos, end)), "no selection: the range is kept, and said");
        assert!(s.status.starts_with("loop: select"), "{}", s.status);
        s.loop_off();
        assert_eq!(s.transport.loop_range, None);
        // The range renders on its own: exactly the span's frames.
        s.selection = vec![c1];
        s.loop_selection();
        let sources: std::collections::BTreeMap<_, _> = Default::default();
        let out = s.render_arrangement_range(48_000, &sources, pos, end);
        assert_eq!(out.len() as i64, (end.to_samples(48_000) - pos.to_samples(48_000)) * 2);
        let _ = Op::SetTrackGain { id: TrackId(1), gain: 1.0 };
    }
}

#[cfg(test)]
mod curve_shell_tests {
    //! The shell half of the curves module: `y`/`p` carry a clip's
    //! bindings, a curve is minted from the selection, and the strip's
    //! time shift is an exact inverse.
    use super::*;
    use studio_core::{AutoTarget, Gate, Op, Ticks};
    use studio_egui::keys::Key;

    /// Demo project, clip 1 selected, a curve gated to it with a dip
    /// halfway through the clip.
    fn with_bound_clip() -> (Studio, studio_core::ClipId, studio_core::CurveId) {
        let mut s = Studio::demo();
        let clip = studio_core::ClipId(1);
        s.selection = vec![clip];
        s.new_curve_on_selection(None);
        let curve = s.session.doc.project.bindings.values().next().expect("bound").curve;
        s.edit(Op::SetCurvePoint { curve, at: Ticks::ZERO, value: 1.0 }, "pt");
        s.edit(Op::SetCurvePoint { curve, at: Ticks::from_seconds(1.5), value: 0.0 }, "pt");
        s.edit(Op::SetCurvePoint { curve, at: Ticks::from_seconds(3.0), value: 1.0 }, "pt");
        (s, clip, curve)
    }

    #[test]
    fn a_new_curve_on_the_selection_is_gated_to_the_clip_and_rides_its_track() {
        let (s, clip, curve) = with_bound_clip();
        let p = &s.session.doc.project;
        assert_eq!(p.curves.len(), 1);
        assert_eq!(p.bindings.len(), 1);
        let b = p.bindings.values().next().unwrap();
        assert_eq!(b.gate, Gate::Clip(clip));
        assert_eq!(b.target, AutoTarget::TrackGain(p.clips[&clip].track), "the clip's own track gain");
        assert_eq!(b.curve, curve);
        assert_eq!(s.auto_binding, p.bindings.keys().next().copied(), "the strip switched to it");
        assert!(s.auto_target.is_none());
        // The dip is real, halfway into clip 1 (pos 0 s, len 3 s).
        assert!((p.auto_value(b.target, Ticks::from_seconds(1.5))).abs() < 1e-6);
    }

    #[test]
    fn a_curve_minted_on_an_effect_parameter_is_the_structural_sidechain_on_a_knob() {
        // A reverb return bus; select the vocal clip; mint a curve on the
        // reverb's MIX gated to that clip. The clip's presence now drives
        // the knob — what the fader's menu did for gain, on a parameter.
        use studio_core::{BusId, ParamKey};
        let mut s = Studio::demo();
        let bus = BusId(40);
        s.edit(Op::AddBus { bus: studio_core::Bus { id: bus, name: "verb".into(), parent: None, gain: 1.0,
            chain: vec![mixer_panel::effect("reverb", &[("mix", 1.0)])], sends: vec![], muted: false, silent: false } }, "bus");
        let clip = studio_core::ClipId(1);
        s.selection = vec![clip];
        let target = AutoTarget::BusParam { bus, effect: 0, key: ParamKey::new("mix").unwrap() };
        s.new_curve_on_selection(Some(target));
        let p = &s.session.doc.project;
        let (bid, b) = p.bindings.iter().next().expect("bound");
        assert_eq!(b.target, target);
        assert_eq!(b.gate, Gate::Clip(clip));
        assert_eq!(s.auto_binding, Some(*bid), "the strip shows the new binding");
        assert!(s.status.contains("bound to 1 target"), "{}", s.status);
        // The render's question answers with it, and the target is live.
        assert_eq!(p.bus_param_targets(bus).len(), 1);
        assert!(studio_fx::Instance::is_live_param("reverb", "mix"));
        // Duplicating the clip re-aims a binding at the copy (the
        // curves module's promise, unchanged by the target's kind).
        s.key(Key::c('y'));
        s.transport.set_position(Ticks::from_seconds(100.0));
        s.key(Key::c('p'));
        let p = &s.session.doc.project;
        assert_eq!(p.bindings.len(), 2);
        assert!(p.bindings.values().all(|b| b.target == target), "both bindings aim at the same knob");
        assert_eq!(p.bus_param_targets(bus).len(), 1, "one target, two gates — deduplicated");
        // Undo the paste, then the mint: the bus is back to a plain reverb.
        s.key(Key::c('u'));
        s.key(Key::c('u'));
        assert!(s.session.doc.project.bindings.is_empty());
        assert!(s.session.doc.project.bus_param_targets(bus).is_empty());
    }

    #[test]
    fn a_curve_over_a_group_spans_the_hull_and_is_one_undo() {
        let mut s = Studio::demo();
        let p = &s.session.doc.project;
        // Two clips on one track, apart: group them.
        let track = p.clips[&studio_core::ClipId(1)].track;
        let mut on_track: Vec<&studio_core::Clip> = p.clips.values().filter(|c| c.track == track).collect();
        on_track.sort_by_key(|c| c.pos);
        assert!(on_track.len() >= 2, "the demo has two clips on this track");
        let (a, b) = (on_track[0].id, on_track[1].id);
        let hull = (on_track[0].pos, on_track[1].end());
        let gid = studio_core::GroupId(70);
        s.edit(Op::SetGroup { id: gid, name: "drop".into(), clips: vec![a, b] }, "group");
        s.selection = vec![b];
        s.new_curve_on_group(None);
        let p = &s.session.doc.project;
        assert_eq!(p.bindings.len(), 1);
        let bnd = p.bindings.values().next().unwrap();
        assert_eq!(bnd.gate, Gate::Group(gid));
        assert_eq!(bnd.target, AutoTarget::TrackGain(track));
        assert_eq!(p.group_hull(gid), Some(hull));
        assert!(s.status.contains("over drop"), "{}", s.status);
        assert!(s.auto_binding.is_some());
        s.key(Key::c('u'));
        assert!(s.session.doc.project.bindings.is_empty() && s.session.doc.project.curves.is_empty(),
                "curve and binding undo together");
        // A clip in no group is refused, and said.
        s.selection = vec![studio_core::ClipId(3)];
        s.edit(Op::RemoveGroup { id: gid }, "ungroup");
        s.new_curve_on_group(None);
        assert!(s.status.contains("in no group"), "{}", s.status);
        assert!(s.session.doc.project.bindings.is_empty());
    }

    #[test]
    fn yank_then_paste_copies_the_clip_with_its_binding_as_one_undo() {
        let (mut s, clip, curve) = with_bound_clip();
        let track = s.session.doc.project.clips[&clip].track;
        s.key(Key::c('y'));
        assert!(s.status.starts_with("yanked 1 clip"), "status was: {}", s.status);
        // Far right, where every track is empty in the demo.
        let at = Ticks::from_seconds(100.0);
        s.transport.set_position(at);
        s.key(Key::c('p'));
        assert!(s.status.contains("pasted 1 clip with 1 automation binding"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.clips.len(), 7);
        let copy = p.clips.values().find(|c| c.pos == at).expect("the copy landed at the playhead");
        assert_eq!(copy.track, track, "same track");
        assert_eq!(p.curves.len(), 1, "the curve is shared, never copied");
        assert_eq!(p.bindings.len(), 2);
        assert!(p.bindings.values().any(|b| b.gate == Gate::Clip(copy.id) && b.curve == curve),
                "a binding re-aimed at the copy");
        assert!((p.auto_value(AutoTarget::TrackGain(track), Ticks::from_seconds(101.5))).abs() < 1e-6,
                "the copy rides the same dip 1.5 s in");
        assert_eq!(s.selection, vec![copy.id], "the paste is what is selected now");
        // ONE undo takes the clip and its binding away together.
        s.key(Key::c('u'));
        let p = &s.session.doc.project;
        assert_eq!(p.clips.len(), 6);
        assert_eq!(p.bindings.len(), 1);
    }

    #[test]
    fn a_paste_with_no_room_makes_room_by_default_and_refuses_whole_when_told_to() {
        let (mut s, _clip, _) = with_bound_clip();
        s.yank();
        // Clip 4 sits on track 1 at 10.5 s; pasting a 3 s clip at 9 s
        // overlaps its head. Newest wins: 4 is left-trimmed and slid to
        // 12 s, the paste lands, one undo restores everything.
        let before = s.session.doc.project.clone();
        s.paste_at(Ticks::from_seconds(9.0));
        assert!(s.status.starts_with("pasted 1 clip"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(12.0));
        assert_eq!(p.clips[&studio_core::ClipId(4)].len, Ticks::from_seconds(1.5));
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project, before, "one undo, and the take is whole again");
        // Refuse: nothing may land — not the clip, not the binding.
        s.overlap = studio_core::Overlap::Refuse;
        s.paste_at(Ticks::from_seconds(9.0));
        assert!(s.status.contains("refused"), "status was: {}", s.status);
        assert_eq!(s.session.doc.project, before, "a refused paste leaves nothing behind");
    }

    #[test]
    fn a_yanked_clip_deleted_before_the_paste_is_counted_not_pasted() {
        let (mut s, clip, _) = with_bound_clip();
        s.selection = vec![clip, studio_core::ClipId(2)];
        s.yank();
        s.edit(Op::RemoveClip { id: studio_core::ClipId(2) }, "deleted");
        s.paste_at(Ticks::from_seconds(100.0));
        assert!(s.status.contains("pasted 1 clip") && s.status.contains("1 yanked clip no longer exists"),
                "status was: {}", s.status);
        s.clipboard.clear();
        s.paste_at(Ticks::from_seconds(200.0));
        assert_eq!(s.status, "nothing yanked");
    }

    #[test]
    fn a_curve_with_nothing_to_aim_at_says_so_and_a_lane_target_binds_always() {
        let mut s = Studio::demo();
        s.selection.clear();
        s.new_curve_on_selection(None);
        assert!(s.session.doc.project.curves.is_empty());
        assert!(s.status.contains("select clips"), "status was: {}", s.status);
        let t = AutoTarget::TrackGain(s.session.doc.project.tracks[0].id);
        s.new_curve_on_selection(Some(t));
        let p = &s.session.doc.project;
        assert_eq!(p.bindings.len(), 1);
        let b = p.bindings.values().next().unwrap();
        assert_eq!(b.gate, Gate::Always);
        assert_eq!(b.target, t);
    }

    #[test]
    fn unbind_drops_the_binding_and_keeps_the_curve() {
        let (mut s, _, curve) = with_bound_clip();
        let id = *s.session.doc.project.bindings.keys().next().unwrap();
        s.unbind(id);
        let p = &s.session.doc.project;
        assert!(p.bindings.is_empty());
        assert!(p.curves.contains_key(&curve), "an unbound curve is inert, not garbage");
        assert!(s.auto_binding.is_none(), "the strip stopped showing what no longer exists");
    }

    /// Demo project with clips 1 and 4 (both on track 1, at 0 s and
    /// 10.5 s) grouped as "hook".
    fn with_group() -> Studio {
        let mut s = Studio::demo();
        s.edit(Op::SetGroup { id: studio_core::GroupId(1), name: "hook".into(),
            clips: vec![studio_core::ClipId(1), studio_core::ClipId(4)] }, "grouped");
        s
    }

    #[test]
    fn dropping_one_member_moves_the_whole_composition_as_one_undo() {
        let mut s = with_group();
        let t = s.session.doc.project.clips[&studio_core::ClipId(1)].track;
        s.selection.clear();
        s.drop_clip(studio_core::ClipId(1), Ticks::from_seconds(50.0), t, false);
        assert!(s.status.contains("moved 2 together"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.clips[&studio_core::ClipId(1)].pos, Ticks::from_seconds(50.0));
        assert_eq!(p.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(60.5), "the other member kept its 10.5 s spacing");
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(10.5), "one undo, both back");
        // Alt-drag moves just the one.
        s.drop_clip(studio_core::ClipId(1), Ticks::from_seconds(50.0), t, true);
        assert_eq!(s.status, "moved");
        assert_eq!(s.session.doc.project.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(10.5));
    }

    #[test]
    fn a_nudge_of_adjacent_clips_lands_where_one_at_a_time_used_to_refuse() {
        let mut s = Studio::demo();
        // Pack three clips shoulder to shoulder on track 1, then nudge right.
        let t = s.session.doc.project.clips[&studio_core::ClipId(1)].track;
        s.edit(Op::MoveClip { id: studio_core::ClipId(4), pos: Ticks::from_seconds(3.0), track: t }, "packed");
        assert_eq!(s.status, "packed", "clip 4 sits flush against clip 1 now");
        s.selection = vec![studio_core::ClipId(1), studio_core::ClipId(4)];
        s.nudge_selected(Ticks::from_seconds(1.0));
        assert!(s.status.starts_with("nudged 2"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.clips[&studio_core::ClipId(1)].pos, Ticks::from_seconds(1.0));
        assert_eq!(p.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(4.0));
    }

    #[test]
    fn pasting_a_composition_recreates_it_over_the_copies() {
        let mut s = with_group();
        s.selection = vec![studio_core::ClipId(1), studio_core::ClipId(4)];
        s.yank();
        s.paste_at(Ticks::from_seconds(100.0));
        assert!(s.status.contains("in 1 composition"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.groups.len(), 2);
        let copy = p.groups.values().find(|g| g.name == "hook copy").expect("the copy is named after its original");
        assert_eq!(copy.clips.len(), 2);
        assert!(copy.clips.iter().all(|c| p.clips[c].pos >= Ticks::from_seconds(100.0)), "made of the pasted clips");
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.groups.len(), 1, "one undo takes the copies and their composition");
    }

    #[test]
    fn cutting_on_beats_keeps_the_pieces_in_the_composition_and_says_how_many() {
        let mut s = with_group();
        // The demo's rack is 140 BPM-ish; whatever it is, a 3 s clip has
        // more than one beat inside it.
        s.selection = vec![studio_core::ClipId(1)];
        let beat = s.rack.tempo().step_len().0 * 4;
        let expect_cuts = (Ticks::from_seconds(3.0).0 - 1) / beat;
        s.split_selection_on_grid(4);
        assert!(s.status.starts_with(&format!("cut {expect_cuts} time")), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.groups[&studio_core::GroupId(1)].clips.len(), 2 + expect_cuts as usize, "pieces joined the hook");
        assert_eq!(s.selection.len(), 1 + expect_cuts as usize, "the pieces are selected with their original");
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.clips.len(), 6, "one undo");
        s.selection.clear();
        s.split_selection_on_grid(4);
        assert_eq!(s.status, "split on grid: nothing selected");
    }

    #[test]
    fn make_unique_needs_a_shared_curve_and_then_shows_the_split_count() {
        let (mut s, _, curve) = with_bound_clip();
        let id = *s.session.doc.project.bindings.keys().next().unwrap();
        s.make_unique(id);
        assert!(s.status.starts_with("already unique"), "status was: {}", s.status);
        // Share it with clip 2, then make clip 2's binding unique.
        s.edit(Op::SetBinding { id: studio_core::BindingId(2), binding: studio_core::Binding {
            curve, target: AutoTarget::TrackGain(s.session.doc.project.clips[&studio_core::ClipId(2)].track),
            gate: Gate::Clip(studio_core::ClipId(2)), response: None, fit_span: false } }, "bound");
        assert_eq!(s.session.doc.project.curve_share_count(curve), 2);
        s.make_unique(studio_core::BindingId(2));
        assert_eq!(s.status, "curve made unique — one undo reverses all 5", "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.curve_share_count(curve), 1);
        assert_eq!(p.curves.len(), 2);
        assert_ne!(p.bindings[&studio_core::BindingId(2)].curve, curve);
    }

    #[test]
    fn a_drop_onto_a_long_take_trims_it_by_default_and_refuses_only_when_asked() {
        // Demo: clips 1 (0–3 s) and 4 (10.5–13.5 s) share track 1. Drop 4
        // onto 1's tail: newest wins, 1 is trimmed, one undo restores both.
        let mut s = Studio::demo();
        let t = s.session.doc.project.clips[&studio_core::ClipId(1)].track;
        assert_eq!(s.policy_for(t), studio_core::Overlap::Trim, "the default");
        s.selection.clear();
        s.drop_clip(studio_core::ClipId(4), Ticks::from_seconds(2.0), t, true);
        assert!(s.status.contains("made room"), "status was: {}", s.status);
        let p = &s.session.doc.project;
        assert_eq!(p.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(2.0));
        assert_eq!(p.clips[&studio_core::ClipId(1)].len, Ticks::from_seconds(2.0), "the take gave way to the newcomer");
        s.key(Key::c('u'));
        let p = &s.session.doc.project;
        assert_eq!(p.clips[&studio_core::ClipId(1)].len, Ticks::from_seconds(3.0));
        assert_eq!(p.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(10.5), "one undo, both facts back");
        s.set_overlap(Some(t), studio_core::Overlap::Refuse);
        s.drop_clip(studio_core::ClipId(4), Ticks::from_seconds(2.0), t, true);
        assert!(s.status.contains("refused"), "status was: {}", s.status);
        assert_eq!(s.session.doc.project.clips[&studio_core::ClipId(4)].pos, Ticks::from_seconds(10.5));
    }

    #[test]
    fn the_lane_policy_lands_the_overlap_on_a_shadow_track_through_the_same_bus() {
        let mut s = Studio::demo();
        let t = s.session.doc.project.clips[&studio_core::ClipId(1)].track;
        s.set_overlap(Some(t), studio_core::Overlap::Lane);
        assert!(s.status.ends_with(": lane"), "status was: {}", s.status);
        let tracks_before = s.session.doc.project.tracks.len();
        s.selection.clear();
        s.drop_clip(studio_core::ClipId(4), Ticks::from_seconds(1.0), t, true);
        let p = &s.session.doc.project;
        assert_eq!(p.tracks.len(), tracks_before + 1, "a shadow lane was made");
        let lane = p.shadow_lane(t).expect("found by name beside its parent");
        let moved = &p.clips[&studio_core::ClipId(4)];
        assert_eq!(moved.track, lane);
        assert_eq!(moved.pos, Ticks::from_seconds(1.0));
        assert_eq!(p.clips[&studio_core::ClipId(1)].len, Ticks::from_seconds(3.0), "the take on the parent is untouched");
        assert_eq!(p.tracks.iter().find(|x| x.id == lane).unwrap().route,
                   p.tracks.iter().find(|x| x.id == t).unwrap().route, "same bus");
        // A second overlap reuses the lane rather than making another.
        s.drop_clip(studio_core::ClipId(5), Ticks::from_seconds(0.5), t, true);
        assert_eq!(s.session.doc.project.tracks.len(), tracks_before + 1);
        s.key(Key::c('u'));
        s.key(Key::c('u'));
        assert_eq!(s.session.doc.project.tracks.len(), tracks_before, "undo removes the lane it made");
    }

    #[test]
    fn overlap_policy_survives_the_workspace_sidecar() {
        let mut ws = workspace::WorkspaceState::default();
        ws.overlap = "lane".into();
        ws.overlap_tracks = "3:refuse,7:trim, junk ,9:nope".into();
        let back = workspace::WorkspaceState::decode(&ws.encode());
        assert_eq!(back.overlap, "lane");
        assert_eq!(back.overlap_tracks, ws.overlap_tracks);
        assert_eq!(studio_core::Overlap::parse(&back.overlap), Some(studio_core::Overlap::Lane));
        let parsed: Vec<_> = back.overlap_tracks.split(',')
            .filter_map(|kv| { let (t, p) = kv.split_once(':')?;
                Some((t.trim().parse::<u64>().ok()?, studio_core::Overlap::parse(p)?)) }).collect();
        assert_eq!(parsed, vec![(3, studio_core::Overlap::Refuse), (7, studio_core::Overlap::Trim)],
                   "junk and unknown policies are dropped, never load-bearing");
    }

    #[test]
    fn presence_says_when_a_take_is_rolling() {
        let mut s = Studio::demo();
        s.recording = true;
        let pkt = s.presence_beat(10_000, None, studio_collab::ViewMode::Plain).expect("first presence is always due");
        match pkt { studio_collab::wire::Packet::Presence(p) => assert!(p.recording), other => panic!("{other:?}") }
    }

    #[test]
    fn the_strips_time_shift_is_exact_and_clamps_at_the_gate() {
        let origin = Ticks(1_000);
        let pts = vec![(Ticks(0), 1.0), (Ticks(500), 0.0)];
        let shown = shift_points(&pts, origin);
        assert_eq!(shown, vec![(Ticks(1_000), 1.0), (Ticks(1_500), 0.0)]);
        for (t, _) in &shown { assert_eq!(curve_time(*t, origin), Ticks(t.0 - origin.0)); }
        assert_eq!(curve_time(Ticks(200), origin), Ticks::ZERO, "left of the gate is the gate's start");
    }
}

#[cfg(test)]
mod automation_shell_tests {
    use super::*;
    use studio_core::{AutoTarget, Op, Ticks};
    use studio_egui::keys::Key;

    #[test]
    fn an_automation_edit_is_undoable_and_stales_the_render() {
        let mut s = Studio::demo();
        let track = s.session.doc.project.tracks[0].id;
        let target = AutoTarget::TrackGain(track);
        s.arrangement_dirty = false;
        s.edit(Op::SetAutoPoint { target, at: Ticks(100), value: 0.5 }, "automation point");
        assert!(s.session.doc.project.automation.get(&target).is_some(), "the point landed");
        assert!(s.arrangement_dirty, "a ride changes what the transport must play");
        s.key(Key::c('u'));
        assert!(s.session.doc.project.automation.get(&target).is_none(),
                "undo removed the point through the op-log door like any edit");
    }

    #[test]
    fn a_refused_automation_edit_says_so_instead_of_vanishing() {
        // The strip can be aimed at a lane whose removal already happened on
        // a peer; the refusal must reach the status line, not the void.
        let mut s = Studio::demo();
        let track = s.session.doc.project.tracks[0].id;
        s.edit(Op::RemoveAutoPoint { target: AutoTarget::TrackGain(track), at: Ticks(9) },
               "automation point removed");
        assert!(s.status.contains("refused"), "status was: {}", s.status);
    }

    #[test]
    fn the_lane_reaches_the_arrangement_the_shell_actually_renders() {
        // End to end through the SHELL's own render path: a bounce is
        // audible, then a flat zero lane on its track silences it — the
        // strip edits the same mix the play button plays.
        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        sources.insert(id, audio);
        let loud: f32 = s.render_arrangement(48_000, &sources)[..9600]
            .iter().map(|v| v.abs()).sum();
        assert!(loud > 10.0, "the bounce is audible before the ride ({loud})");

        // The demo seeds six clips of its own; the ride must land on the
        // track the BOUNCE landed on, found by its source — grabbing "the
        // first clip" automated a silent demo track and proved nothing
        // (which is exactly how this test's first draft failed).
        let track = s.session.doc.project.clips.values()
            .find(|c| c.source == id).expect("the bounce placed a clip").track;
        s.edit(Op::SetAutoPoint { target: AutoTarget::TrackGain(track),
                                  at: Ticks::ZERO, value: 0.0 }, "automation point");
        let ridden: f32 = s.render_arrangement(48_000, &sources)[..9600]
            .iter().map(|v| v.abs()).sum();
        assert!(ridden < loud * 0.001,
                "a flat zero lane silences the track: {ridden} vs {loud}");
    }
}

#[cfg(test)]
mod archive_shell_tests {
    use super::*;

    #[test]
    fn a_session_survives_the_one_file_round_trip_audio_and_all() {
        // The professional-viability path end to end: bounce a beat, pack
        // the session at proxy fidelity, reopen the ARCHIVE ALONE from a
        // bare directory — no takes/, no exports/, nothing beside it — and
        // the beat is still audible through the reopened session's own
        // render. This is exactly the situation that loses files in a
        // folder-of-paths save, pinned as the situation this format ends.
        let dir = std::env::temp_dir()
            .join(format!("studio-arch-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut s = Studio::demo();
        let mut sources = std::collections::BTreeMap::new();
        let (id, audio) = s.bounce_current_pattern(48_000, &sources).expect("bounced");
        s.register_asset(id, &studio_audio::export::export_wav_dithered_bytes(
            &audio.channels[0], 1, 48_000, id.0).unwrap());
        sources.insert(id, audio);

        let meta = archive::ArchiveMeta {
            artists: vec!["ada".into()],
            search_paths: vec!["hq".into()],
            level: archive::Level::Demo,
        };
        let (bytes, _report) = s.pack_archive(&sources, &meta).expect("packs");
        let path = dir.join("cut.studioarch");
        std::fs::write(&path, &bytes).unwrap();

        let team = Team::from_contacts("solo", &["sc-local".into()]);
        let mut back = Studio::open_archive(&path, SiteId(2), team).expect("opens whole");
        assert_eq!(back.session.doc.project, s.session.doc.project,
                   "state converged: tracks, clips, busses, automation");
        assert_eq!(back.rack, s.rack, "patterns and rolls came along");
        // Embedded media arrives through the incoming door; render proves
        // it is REAL audio, not entries.
        let mut opened_sources = std::collections::BTreeMap::new();
        for (sid, a) in back.incoming_audio.drain(..) {
            opened_sources.insert(sid, a);
        }
        let out = back.render_arrangement(48_000, &opened_sources);
        let energy: f32 = out[..9600.min(out.len())].iter().map(|v| v.abs()).sum();
        assert!(energy > 5.0, "the bounced beat plays from the archive alone ({energy})");
    }

    #[test]
    fn the_archives_roster_is_the_team_and_outsiders_are_refused_at_the_door() {
        // The metadata is enforced now: the artists the popup names ARE
        // the session's authorized collaborators, by the same derived-id
        // math a hand-built team uses.
        let dir = std::env::temp_dir()
            .join(format!("studio-arch-team-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = Studio::demo();
        let meta = archive::ArchiveMeta {
            artists: vec!["sc-ada".into(), "sc-lin".into()],
            search_paths: vec![],
            level: archive::Level::Sketch,
        };
        let (bytes, _) = s.pack_archive(&Default::default(), &meta).unwrap();
        let path = dir.join("cut.studioarch");
        std::fs::write(&path, &bytes).unwrap();
        let caller_team = Team::from_contacts("solo", &["sc-local".into()]);
        let mut back = Studio::open_archive(&path, SiteId(2), caller_team).unwrap();
        assert!(back.session.join(SiteId(9), "sc-stranger", "??", 0).is_none(),
                "a contact outside the archive's roster is refused");
        assert!(back.session.join(SiteId(3), "sc-ada", "ada", 0).is_some(),
                "a named collaborator joins");
        // And two people opening the same archive independently land on
        // one session, because the team id derives from the SORTED roster.
        let t1 = Team::from_contacts("cut", &["sc-ada".into(), "sc-lin".into()]);
        let t2 = Team::from_contacts("whatever", &["sc-lin".into(), "sc-ada".into()]);
        assert_eq!(t1.id, t2.id);
    }
}

#[cfg(test)]
mod alignment_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Ticks, TrackId};

    fn burst(rate: u32, delay_ms: usize) -> studio_audio::SourceAudio {
        let mut v = vec![0.0f32; rate as usize / 1000 * delay_ms];
        for i in 0..(rate as usize / 4) {
            let t = i as f32 / rate as f32;
            v.push((t * 300.0 * std::f32::consts::TAU).sin() * (-t * 10.0).exp());
        }
        v.extend(vec![0.0f32; rate as usize / 10]);
        studio_audio::SourceAudio { sample_rate: rate, channels: vec![v] }
    }

    fn studio_with_pair(delay_ms: usize)
        -> (Studio, std::collections::BTreeMap<SourceId, studio_audio::SourceAudio>)
    {
        let mut s = Studio::demo();
        let mut audio = std::collections::BTreeMap::new();
        for (sid, d) in [(100u64, 0usize), (101, delay_ms)] {
            let a = burst(48_000, d);
            let len = Ticks::from_samples(a.frames() as i64, 48_000);
            s.edit(Op::AddSource { source: Source { id: SourceId(sid),
                path: format!("takes/{sid}.wav"), len, sample_rate: 48_000,
                channels: 1, has_video: false, kind: studio_core::SourceKind::File } }, "src");
            audio.insert(SourceId(sid), a);
        }
        // Far past the demo clips, on the two audio tracks, same position:
        // the classic double — recorded against the lead, landed sloppy.
        let len = Ticks::from_seconds(0.4);
        for (cid, sid, track) in [(100u64, 100u64, 2u64), (101, 101, 3)] {
            s.edit(Op::AddClip { clip: Clip { id: ClipId(cid), source: SourceId(sid),
                track: TrackId(track), pos: Ticks::from_seconds(60.0),
                src_in: Ticks::ZERO, len, gain: 1.0, effects: vec![] } }, "clip");
        }
        s.selection = vec![ClipId(100), ClipId(101)];
        (s, audio)
    }

    #[test]
    fn press_a_and_the_double_lands_on_the_lead() {
        // The take's burst is 37 ms late inside its window; aligned, its
        // clip must sit 37 ms EARLIER than the reference — the sign this
        // function's first draft got backwards, which would have doubled
        // the error instead of fixing it.
        let (mut s, audio) = studio_with_pair(37);
        s.align_selected(&audio);
        let take = s.session.doc.project.clips[&ClipId(101)].pos;
        let want = Ticks::from_seconds(60.0 - 0.037);
        let err_ms = (take.0 - want.0).abs() as f64
            / studio_core::TICKS_PER_SECOND as f64 * 1000.0;
        assert!(err_ms <= 1.5, "landed {err_ms:.2} ms from the lead");
        // And it is an ordinary op: one undo puts the sloppy take back.
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.session.doc.project.clips[&ClipId(101)].pos,
                   Ticks::from_seconds(60.0));
    }

    #[test]
    fn align_refuses_out_loud_without_exactly_two_clips_or_their_media() {
        let (mut s, audio) = studio_with_pair(10);
        s.selection = vec![ClipId(100)];
        s.align_selected(&audio);
        assert!(s.status.contains("select the reference first"), "{}", s.status);
        s.selection = vec![ClipId(100), ClipId(101)];
        s.align_selected(&Default::default()); // media not arrived
        assert!(s.status.contains("has not arrived"), "{}", s.status);
    }

    #[test]
    fn quantize_snaps_to_the_nearest_step_and_nudge_walks_by_one() {
        let (mut s, _) = studio_with_pair(0);
        let step = Ticks(s.rack.tempo().step_len().0);
        // Park the take just past a step boundary, then quantize.
        let off = Ticks(step.0 * 500 + step.0 / 3);
        s.edit(Op::MoveClip { id: ClipId(101), pos: off, track: TrackId(3) }, "park");
        s.selection = vec![ClipId(101)];
        s.quantize_selected();
        assert_eq!(s.session.doc.project.clips[&ClipId(101)].pos, Ticks(step.0 * 500),
                   "a third past the line snaps back, not forward");
        s.nudge_selected(step);
        assert_eq!(s.session.doc.project.clips[&ClipId(101)].pos, Ticks(step.0 * 501));
        s.nudge_selected(Ticks(-step.0 * 600));
        assert_eq!(s.session.doc.project.clips[&ClipId(101)].pos, Ticks(0),
                   "a nudge past zero clamps rather than going negative");
    }

    #[test]
    fn the_alignment_keys_are_unbound_in_the_keymap_and_taught_by_the_tooltip() {
        // Two drift-proofing pins in one place. (1) The chars the shell
        // intercepts must stay unbound in the Helix keymap — a future
        // binding would be silently shadowed. (2) Every intercepted key
        // must appear in the timeline's CLIP_HINTS: a shortcut the clip
        // does not show is one nobody learns, in every editor ever
        // shipped — the tooltip and the behavior are one source of truth.
        use studio_egui::keys::{Key, Keymap, Resolution};
        let hints = studio_egui::CLIP_HINTS.join(" ");
        for ch in ['A', 'Q', ',', '.', '<', '>', 'n', 'D', '[', ']'] {
            let mut k = Keymap::new();
            assert!(!matches!(k.feed(Key::c(ch)), Resolution::Act(_)),
                    "'{ch}' must stay free in the keymap");
            assert!(hints.contains(ch),
                    "'{ch}' is intercepted but the clip tooltip never teaches it");
        }
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    #[test]
    fn a_layer_inherits_steps_and_mods_and_then_diverges_freely() {
        let mut s = Studio::demo();
        let from = 0usize;
        let steps_before = s.rack.patterns[0].rows[from];
        assert_ne!(steps_before, 0, "the demo kick has steps to inherit");
        s.rack.patterns[0].mods.insert((from, 2),
            studio_live::StepMod { vel: 0.5, ..Default::default() });
        let ix = s.layer_channel(from, studio_live::Channel {
            name: "kick-tone".into(),
            kind: s.rack.channels[from].kind.clone(),
            gain: 0.9, pan: 0.0, muted: false, choke_group: None,
        });
        assert_eq!(s.rack.patterns[0].rows[ix], steps_before,
                   "the layer fires in lockstep with its source");
        assert_eq!(s.rack.patterns[0].mods.get(&(ix, 2)).map(|m| m.vel),
                   Some(0.5), "step mods came along");
        // Divergence is a copy's whole point.
        s.rack.patterns[0].rows[ix] = 0b1;
        assert_eq!(s.rack.patterns[0].rows[from], steps_before,
                   "editing the layer never touches the source");
    }
}

#[cfg(test)]
mod palette_shell_tests {
    use super::*;

    #[test]
    fn a_palette_path_applies_as_one_edit_through_the_same_ops_the_keyboard_uses() {
        // "route the bass to a new bus" is two ops — create and route —
        // and must be ONE undo, or the palette leaves orphan busses
        // behind every time someone changes their mind.
        let mut s = Studio::demo();
        let track = s.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio).unwrap().clone();
        let busses_before = s.session.doc.project.busses.len();
        let act = palette::Palette::resolve(&s.session.doc.project,
                                            &["route", &track.name, "new bus"])
            .expect("the wizard resolves");
        let palette::Act::Ops(ops) = act else { panic!("expected ops") };
        s.edit_group(ops, "palette command");
        assert_eq!(s.session.doc.project.busses.len(), busses_before + 1);
        let routed = s.session.doc.project.tracks.iter()
            .find(|t| t.id == track.id).unwrap().route;
        assert!(routed.is_some(), "the track went into the new bus");
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.session.doc.project.busses.len(), busses_before,
                   "one undo, no orphan bus");
        assert_eq!(s.session.doc.project.tracks.iter()
                       .find(|t| t.id == track.id).unwrap().route, track.route);
    }
}

#[cfg(test)]
mod mix_pass_tests {
    use super::*;
    use studio_core::{Bus, BusId, Effect, Op};

    #[test]
    fn a_mastering_pass_is_one_undo_and_keeps_the_users_own_effects() {
        // The whole reason edit_group and compose exist: several proposed
        // moves must land as ONE step (a pass that takes six presses of u
        // to reverse is a pass nobody dares try) and must not delete the
        // reverb the user put on the bus.
        let mut s = Studio::demo();
        let bus = BusId(90);
        let mine = Effect { kind: "reverb".into(), params: Default::default(),
                            enabled: true };
        s.edit(Op::AddBus { bus: Bus { id: bus, name: "master".into(), parent: None,
            gain: 1.0, chain: vec![mine.clone()], sends: vec![], muted: false, silent: false } }, "bus");
        let before_gain = s.session.doc.project.busses[&bus].gain;

        let proposed = vec![
            Op::SetBusChain { id: bus, chain: vec![Effect { kind: "eq".into(),
                params: [("freq0".to_string(), 250.0)].into_iter().collect(),
                enabled: true }] },
            Op::SetBusChain { id: bus, chain: vec![Effect { kind: "compressor".into(),
                params: [("ratio".to_string(), 2.0)].into_iter().collect(),
                enabled: true }] },
            Op::SetBusGain { id: bus, gain: 0.8 },
        ];
        let existing = s.session.doc.project.busses[&bus].chain.clone();
        let ops = mix_assistant::compose(&existing, &proposed, bus);
        s.edit_group(ops, "mix pass");

        let after = &s.session.doc.project.busses[&bus];
        assert_eq!(after.chain.len(), 3, "reverb + eq + compressor");
        assert_eq!(after.chain[0], mine, "the user's effect kept its place");
        assert!((after.gain - 0.8).abs() < 1e-6);

        // ONE undo reverses the whole pass.
        s.key(studio_egui::keys::Key::c('u'));
        let back = &s.session.doc.project.busses[&bus];
        assert_eq!(back.chain, vec![mine], "one press, the chain is yours again");
        assert!((back.gain - before_gain).abs() < 1e-6, "and the gain came back too");
    }

    #[test]
    fn a_group_that_cannot_apply_leaves_nothing_behind() {
        // Half a mastering pass is worse than none: a mid-group failure
        // rolls back what already landed and says so.
        let mut s = Studio::demo();
        let bus = BusId(91);
        s.edit(Op::AddBus { bus: Bus { id: bus, name: "m".into(), parent: None,
            gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false } }, "bus");
        s.edit_group(vec![
            Op::SetBusGain { id: bus, gain: 0.5 },
            Op::SetBusGain { id: BusId(999), gain: 0.5 }, // no such bus
        ], "mix pass");
        assert!((s.session.doc.project.busses[&bus].gain - 1.0).abs() < 1e-6,
                "the first move was rolled back");
        assert!(s.status.contains("nothing applied"), "{}", s.status);
    }

#[cfg(test)]
mod comp_split_tests {
    use super::*;
    use studio_core::{GroupId, Op};

    /// Build a comp over the demo project's first two clips.
    fn with_comp(s: &mut Studio) -> (GroupId, Ticks, Ticks) {
        let ids: Vec<_> = s.session.doc.project.clips.keys().copied().take(2).collect();
        assert_eq!(ids.len(), 2, "the demo project should have clips to group");
        let gid = GroupId(1);
        // SetGroup is { id, name, clips } only -- key/tempo/active/window are
        // carried forward by the op's own handler rather than restated here,
        // "so a rename must not silently erase a trim".
        s.session.doc.project
            .apply(&Op::SetGroup { id: gid, name: "comp".into(), clips: ids })
            .expect("group");
        let (a, b) = s.session.doc.project.group_span(gid).expect("a span");
        (gid, a, b)
    }

    /// THE BUG: splitting a FOLDED comp went through `split_ops` and cut a
    /// member clip -- material spliced out of a block the user cannot see
    /// into. `split_group_ops` existed for this and nothing called it.
    #[test]
    fn a_folded_comp_is_found_under_the_playhead() {
        let mut s = Studio::demo();
        let (gid, a, b) = with_comp(&mut s);
        let mid = Ticks((a.0 + b.0) / 2);

        // OPEN: the clips are showing, so cutting them is what the user sees.
        assert_eq!(s.folded_group_at(mid), None, "an open comp splits its clips");

        s.closed_groups.insert(gid);
        assert_eq!(s.folded_group_at(mid), Some(gid), "a folded comp is cut as a comp");
    }

    /// The edges are a non-event, matching `split_group_ops`' own refusal --
    /// splitting a comp at its boundary is like splitting a clip at its start.
    #[test]
    fn the_comp_edges_are_not_a_split_point() {
        let mut s = Studio::demo();
        let (gid, a, b) = with_comp(&mut s);
        s.closed_groups.insert(gid);
        assert_eq!(s.folded_group_at(a), None, "the start edge");
        assert_eq!(s.folded_group_at(b), None, "the end edge");
        assert_eq!(s.folded_group_at(Ticks(b.0 + 1000)), None, "past the end");
    }

    /// A folded comp's split touches NO clips: two windows over the same
    /// member list. That is the whole point -- nothing inside is spliced.
    #[test]
    fn splitting_a_folded_comp_emits_no_clip_ops() {
        let mut s = Studio::demo();
        let (gid, a, b) = with_comp(&mut s);
        let mid = Ticks((a.0 + b.0) / 2);
        let ops = s.session.doc.project
            .split_group_ops(gid, mid, GroupId(2))
            .expect("a split at the midpoint");
        assert!(!ops.is_empty());
        for op in &ops {
            assert!(
                matches!(op, Op::SetGroupWindow { .. } | Op::SetGroup { .. }),
                "a comp split must not touch clips, got {op:?}"
            );
        }
    }
}
}


/// The timeline's keys, fed through `Studio::key` -- the SHELL, not the
/// keymap. The keymap's own tests proved every hinted key parsed; eleven of
/// them then fell into a `_ => {}` here and did nothing (c20).
#[cfg(test)]
mod arrange_key_tests {
    use super::*;
    use studio_egui::keys::Key;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};

    fn secs(x: f64) -> Ticks { Ticks::from_seconds(x) }

    /// Two tracks far to the right of the demo's content:
    /// A: 200-202 (#901), 202-204 (#902), 205-206 (#903); B: 201-203 (#904).
    fn arranged() -> Studio {
        let mut s = Studio::demo();
        for (i, (id, name)) in [(900u64, "arr A"), (901, "arr B")].into_iter().enumerate() {
            let at = s.session.doc.project.tracks.len();
            s.edit(Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Audio, name: name.into(),
                muted: false, locked: false, gain: 1.0, route: None }, at }, "t");
            let _ = i;
        }
        s.edit(Op::AddSource { source: Source { id: SourceId(900), path: "arr.wav".into(), len: secs(60.0),
            sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }, "src");
        for (id, track, pos, len) in [(901u64, 900u64, 200.0, 2.0), (902, 900, 202.0, 2.0), (903, 900, 205.0, 1.0), (904, 901, 201.0, 2.0)] {
            s.edit(Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(900), track: TrackId(track),
                pos: secs(pos), src_in: secs(pos - 200.0), len: secs(len), gain: 1.0, effects: vec![] } }, "c");
        }
        s.selection.clear();
        s
    }
    fn at(s: &mut Studio, x: f64) { s.transport.set_position(secs(x)); }
    fn clip(s: &Studio, id: u64) -> &Clip { &s.session.doc.project.clips[&ClipId(id)] }

    #[test]
    fn x_selects_the_clip_under_the_playhead_and_again_extends_along_the_track() {
        let mut s = arranged();
        at(&mut s, 200.5);
        s.key(Key::c('x'));
        assert_eq!(s.selection, vec![ClipId(901)], "{}", s.status);
        s.key(Key::c('x'));
        assert_eq!(s.selection, vec![ClipId(901), ClipId(902)], "{}", s.status);
        s.key(Key::c('x'));
        assert_eq!(s.selection, vec![ClipId(901), ClipId(902), ClipId(903)]);
        s.key(Key::c(';'));
        assert!(s.selection.is_empty());
    }

    #[test]
    fn w_b_and_e_walk_the_cuts_and_select_mode_gathers_what_they_pass() {
        let mut s = arranged();
        at(&mut s, 200.5);
        s.key(Key::c('x'));                       // focus: track A
        s.key(Key::c('w'));
        assert_eq!(s.transport.position(), secs(202.0), "{}", s.status);
        s.key(Key::c('2')); s.key(Key::c('w'));   // a count: two cuts on
        assert_eq!(s.transport.position(), secs(205.0));
        s.key(Key::c('b'));
        assert_eq!(s.transport.position(), secs(204.0));
        s.key(Key::c('e'));
        assert_eq!(s.transport.position(), secs(204.0), "no clip under 204: stays, says so ({})", s.status);
        at(&mut s, 202.5);
        s.key(Key::c('e'));
        assert_eq!(s.transport.position(), secs(204.0));
        // Select mode: w extends over what it passes
        s.selection.clear();
        at(&mut s, 200.0);
        s.selection.push(ClipId(901));
        s.key(Key::c('v'));
        s.key(Key::c('w')); s.key(Key::c('w'));
        assert!(s.selection.contains(&ClipId(902)), "{:?}", s.selection);
    }

    #[test]
    fn c_adds_the_overlapping_clip_on_the_track_below() {
        let mut s = arranged();
        at(&mut s, 202.5);
        s.key(Key::c('x'));
        assert_eq!(s.selection, vec![ClipId(902)]);
        s.key(Key::shift('c'));
        assert_eq!(s.selection, vec![ClipId(902), ClipId(904)], "{}", s.status);
    }

    #[test]
    fn gain_mute_and_lock_act_on_the_selection_and_undo_in_one() {
        let mut s = arranged();
        s.selection = vec![ClipId(901), ClipId(904)];
        s.key(Key::c('-'));
        assert!((clip(&s, 901).gain - 0.95).abs() < 1e-6 && (clip(&s, 904).gain - 0.95).abs() < 1e-6);
        s.key(Key::c('u'));
        assert_eq!(clip(&s, 901).gain, 1.0, "one undo for the whole selection");
        s.key(Key::c('m'));
        let tr = |s: &Studio, id: u64| s.session.doc.project.tracks.iter().find(|t| t.id == TrackId(id)).unwrap().clone();
        assert!(tr(&s, 900).muted && tr(&s, 901).muted, "{}", s.status);
        s.key(Key::c('m'));
        assert!(!tr(&s, 900).muted);
        s.key(Key::c('l'));
        assert!(tr(&s, 900).locked && !tr(&s, 900).muted);
    }

    #[test]
    fn loop_record_and_goto_playhead_do_what_they_say() {
        let mut s = arranged();
        s.selection = vec![ClipId(901), ClipId(902)];
        s.key(Key::shift('l'));
        assert_eq!(s.transport.loop_range, Some((secs(200.0), secs(204.0))), "{}", s.status);
        s.key(Key::shift('l'));
        assert_eq!(s.transport.loop_range, None);
        s.key(Key::c('r'));
        assert!(s.take_requested, "r asks the shell for a take");
        at(&mut s, 150.0);
        s.view.scroll = Ticks::ZERO;
        s.key(Key::c('g')); s.key(Key::c('c'));
        assert!(s.view.scroll > secs(140.0) && s.view.scroll < secs(150.0), "{:?}", s.view.scroll);
    }

    #[test]
    fn duplicate_lands_right_after_the_selection_and_leaves_the_clipboard() {
        let mut s = arranged();
        s.clipboard = vec![ClipId(903)];
        s.selection = vec![ClipId(901), ClipId(902)];
        s.duplicate_selected();
        let p = &s.session.doc.project;
        let copies: Vec<_> = s.selection.iter().map(|id| (p.clips[id].pos, p.clips[id].track)).collect();
        // the copy of 901 lands at 204 -- where 903 is not (205), so no room needed
        assert!(copies.contains(&(secs(204.0), TrackId(900))), "{copies:?} ({})", s.status);
        assert_eq!(s.clipboard, vec![ClipId(903)], "D is not a yank");
    }

    #[test]
    fn brackets_fade_in_to_and_out_from_the_playhead() {
        let mut s = arranged();
        s.selection = vec![ClipId(902)];          // 202-204
        at(&mut s, 202.5);
        s.fade_to_playhead(true);
        assert_eq!(s.session.doc.project.fades.get(&ClipId(902)), Some(&(secs(0.5), Ticks::ZERO)), "{}", s.status);
        at(&mut s, 203.25);
        s.fade_to_playhead(false);
        assert_eq!(s.session.doc.project.fades.get(&ClipId(902)), Some(&(secs(0.5), secs(0.75))), "the fade-in is kept");
        at(&mut s, 210.0);
        s.fade_to_playhead(true);
        assert!(s.status.contains("inside"), "{}", s.status);
    }

    #[test]
    fn normalize_brings_the_played_part_of_the_source_to_minus_one_dbfs() {
        let mut s = arranged();
        // the source: 0.25 peak for the first 2 s, 0.8 after -- clip 901
        // plays 0-2 s, clip 902 plays 2-4 s
        let rate = 1000;
        let ch: Vec<f32> = (0..6000).map(|i| if i < 2000 { 0.25 * ((i % 7) as f32 / 7.0) } else { -0.8 }).collect();
        let peak_a = ch[..2000].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let audio = [(SourceId(900), studio_audio::SourceAudio { sample_rate: rate, channels: vec![ch] })].into();
        s.selection = vec![ClipId(901), ClipId(902)];
        s.normalize_selected(&audio);
        assert!((clip(&s, 901).gain - (0.891_250_9 / peak_a).min(1.5)).abs() < 1e-4, "{} {}", clip(&s, 901).gain, s.status);
        assert!((clip(&s, 902).gain - 0.891_250_9 / 0.8).abs() < 1e-4, "{}", clip(&s, 902).gain);
        assert!(s.status.contains("capped"), "0.89/0.21 > 1.5 is capped and said: {}", s.status);
    }
}
