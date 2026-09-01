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

pub mod archive;
pub mod audition;
pub mod capture;
/// Playing a synth, and getting a pattern back.
pub mod perform;
pub mod export;
pub mod clip_inspector;
pub mod find;
pub mod library;
pub mod midi;
pub mod mix_assistant;
pub mod mixer_panel;
pub mod morph_pad;
pub mod program_panel;
pub mod palette;
pub mod project_dir;
pub mod viewport;
pub mod voice;
pub mod workspace;
pub mod rebinds;
pub mod scroll_edge;
pub mod pattern_pack;
pub mod rack_panel;

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
    /// Manifests for sources whose bytes this peer can serve.
    manifests: std::collections::BTreeMap<studio_core::SourceId, studio_collab::assets::Manifest>,
    /// The content-addressed chunk store — shared across sources on purpose:
    /// a re-bounce that changed one bar re-ships one bar.
    chunk_store: std::collections::BTreeMap<studio_collab::assets::ChunkId, Vec<u8>>,
    /// Manifests still collecting chunks.
    pending_assets: std::collections::BTreeMap<studio_core::SourceId, studio_collab::assets::Manifest>,
    /// Media that finished arriving and decoded — the shell drains this into
    /// its audio/peaks maps. A queue rather than a callback so the headless
    /// tests can watch the same door the GUI uses.
    pub incoming_audio: Vec<(studio_core::SourceId, studio_audio::SourceAudio)>,
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
    /// Set by any rack edit; the shell re-renders the loop and clears it.
    pub loop_dirty: bool,
    /// Set by any PROJECT edit (including undo/redo and a bounce); the shell
    /// re-renders the arrangement buffer before the next timeline playback.
    pub arrangement_dirty: bool,
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
    pub roll_drag: Option<studio_egui::roll::NoteDrag>,
    /// Which lane the automation strip under the timeline shows, or `None`
    /// for no strip. Local monitoring-adjacent state like `solo`: what you
    /// are LOOKING at is not a project fact.
    pub auto_target: Option<studio_core::AutoTarget>,
    /// The strip's point mid-drag — ghost until release, ops on release,
    /// the fader rule.
    pub auto_drag: Option<studio_egui::auto::PointDrag>,
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
}

impl Studio {
    pub fn new(project: Project, site: SiteId, team: Team) -> Studio {
        let name = project.name.clone();
        Studio {
            session: Session::open(team, &name, Doc::new(site, project)),
            transport: Transport::new(48_000),
            view: studio_egui::View::default(),
            song_views: std::collections::BTreeMap::new(),
            active_song: None,
            panel: Panel::Timeline,
            mode: ViewMode::Split,
            selection: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            outbox: Vec::new(),
            manifests: Default::default(),
            chunk_store: Default::default(),
            pending_assets: Default::default(),
            incoming_audio: Vec::new(),
            keys: studio_egui::keys::Keymap::new(),
            status: "ready".into(),
            rack: studio_live::Rack::trap_kit(),
            current_pattern: 0,
            selected_channel: 0,
            lane: studio_egui::lanes::Lane::default(),
            paint: studio_egui::grid::Paint::default(),
            rack_playing: false,
            loop_dirty: true,
            arrangement_dirty: true,
            solo: Vec::new(),
            mixer_drag: None,
            clip_gain_drag: None,
            roll_drag: None,
            auto_target: None,
            auto_drag: None,
            combined: false,
            closed_groups: Default::default(),
            key_guide: true,
            mix_pass_request: None,
            bus_drag: None,
            input_mode: Default::default(),
            octave: 0,
            trim_side: studio_egui::TrimSide::default(),
            pending_bounce: false,
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
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true } }).ok();
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
        let path = format!("tuned/{}-{}.wav",
            std::path::Path::new(&old.path).file_stem()
                .and_then(|s| s.to_str()).unwrap_or("take"), id.0);
        self.edit(studio_core::Op::AddSource { source: studio_core::Source {
            id, path, len: studio_core::Ticks::from_samples(tuned.frames() as i64, rate),
            sample_rate: rate, channels: tuned.channels.len() as u16, has_video: false,
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
            let p = base.join(&src.path);
            let Ok(raw) = std::fs::read(&p) else { missing += 1; continue };
            let hint = p.extension().and_then(|e| e.to_str());
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
    pub fn presence_beat(&mut self, now_ms: i64) -> Option<studio_collab::wire::Packet> {
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
            seq: self.presence_seq + 1,
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
    pub fn register_asset(&mut self, source: studio_core::SourceId, bytes: &[u8]) {
        let (manifest, chunks) = studio_collab::assets::Manifest::build(bytes);
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
        for id in self.selection.clone() {
            let Some(c) = self.session.doc.project.clips.get(&id).cloned() else { continue };
            let pos = studio_core::Ticks((c.pos.0 + delta.0).max(0));
            if pos != c.pos {
                self.edit(studio_core::Op::MoveClip { id, pos, track: c.track }, "nudged");
            }
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
                    Action::GotoStart => self.transport.set_position(Ticks::ZERO),
                    Action::GotoEnd => {
                        let d = self.session.doc.project.duration();
                        self.transport.set_position(d);
                    }
                    _ => {}
                }
                true
            }
            Resolution::Pending(p) => { self.status = format!("…{p}"); true }
            Resolution::None => false,
        }
    }

    fn split_at_playhead(&mut self) {
        let at = self.transport.position();
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
        let rate = match format { ExportFormat::Opus => 48_000, ExportFormat::Wav => rate };
        // Opus's mapping family 0 is mono/stereo only (studio-media refuses
        // wider, loudly), so a surround deliverable is a WAV — said here
        // rather than discovered as a confusing encoder error.
        let layout = match format {
            ExportFormat::Opus => studio_audio::format::ChannelFormat::Stereo,
            ExportFormat::Wav => layout,
        };
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
        let end = self.session.doc.project.duration();
        if end <= Ticks::ZERO { return Vec::new(); }
        studio_audio::mixdown(
            &self.session.doc.project, sources, &std::collections::BTreeMap::new(),
            Ticks::ZERO, end, rate, &self.solo, &[],
        )
    }

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
                has_video: false,
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
    /// video FILE — `has_video: true`, length from the probe (a video may
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
                has_video: true,
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
                let bytes = std::fs::read(&src.path)
                    .map_err(|e| format!("'{}': {e} — the pack would ship without \
                                          its sample", src.path))?;
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
                    has_video: false,
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
                has_video: false,
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
        let pattern = self.rack.patterns.get(self.current_pattern)?.clone();
        let track = self.session.doc.project.tracks.iter()
            .find(|t| t.kind == studio_core::TrackKind::Audio)
            .map(|t| t.id);
        let Some(track) = track else {
            self.status = "bounce refused: no audio track".into();
            return None;
        };
        let samples = self.render_loop(rate, sources);
        let source = studio_core::SourceId(
            self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let clip = studio_core::ClipId(
            self.session.doc.project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        let at = self.playhead();
        // The source id in the name keeps every bounce's file distinct: two
        // bounces of 'Trap A' are two sources, and the first must not be
        // overwritten on disk while the project still points at it.
        let path = format!("bounces/{}-{}.wav", pattern.name, source.0);
        let ops = studio_live::bounce_ops(&pattern, &self.rack.tempo(), rate,
                                          source, clip, track, at, &path);
        for op in ops {
            self.edit(op, "bounced");
        }
        self.status = format!("bounced '{}' to the timeline", pattern.name);
        Some((source, studio_audio::SourceAudio { sample_rate: rate, channels: vec![samples] }))
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
        let first = s.presence_beat(10_000).expect("first beat sends");
        let pr = match first {
            studio_collab::wire::Packet::Presence(pr) => pr,
            other => panic!("expected Presence, got {other:?}"),
        };
        assert_eq!(pr.site, s.session.doc.site);
        assert_eq!(pr.name, format!("site {}", s.session.doc.site.0),
                   "empty display_name falls back, never sends an empty string");
        assert_eq!(pr.seq, 1);

        // Nothing changed, heartbeat not due: an idle shell is near-silent.
        assert!(s.presence_beat(10_016).is_none(), "16ms later, idle: no packet");

        // The playhead moved right after a send: news, but rate-limited —
        // the gap counts from the LAST SEND, so a scrub is ~10 pkt/s, not 60.
        s.transport.set_position(studio_core::Ticks::from_seconds(3.0));
        assert!(s.presence_beat(10_050).is_none(),
                "changed but only 50ms since the last send: held");
        let moved = s.presence_beat(10_120).expect("past the gap: the move ships");
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
        assert!(s.presence_beat(10_120 + hb - 1).is_none());
        assert!(s.presence_beat(10_120 + hb).is_some(), "heartbeat due: sends unchanged state");
        assert!(hb * 2 < studio_collab::session::LIVENESS_MS,
                "two consecutive lost heartbeats still leave a beat inside the window");
    }

    /// Selection changes are presence too — the warning edge peers draw
    /// around a clip someone else is inside is only as fresh as this.
    #[test]
    fn presence_beat_announces_selection_and_derived_track() {
        let mut s = Studio::demo();
        s.presence_beat(0);
        let (clip, track) = {
            let (id, c) = s.session.doc.project.clips.iter().next()
                .expect("demo project has clips");
            (*id, c.track)
        };
        s.selection = vec![clip];
        match s.presence_beat(1_000) {
            Some(studio_collab::wire::Packet::Presence(pr)) => {
                assert_eq!(pr.selection, vec![clip]);
                assert_eq!(pr.active_track, Some(track),
                           "active track derives from the first selected clip");
            }
            other => panic!("selection change must ship: {other:?}"),
        }
    }


    /// C12-6a's join, headless: a video import carries has_video: true and
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
            sample_rate: rate, channels: 1, has_video: false } }, "s");
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
pub struct App {
    pub studio: Studio,
    peaks: std::collections::BTreeMap<studio_core::SourceId, studio_audio::Peaks>,
    /// Decoded / synthesised audio per source — bounces and imports land
    /// here; the mixdown and the sampler both read it.
    audio: std::collections::BTreeMap<studio_core::SourceId, studio_audio::SourceAudio>,
    sink: Box<dyn audition::PlaybackSink>,
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
    /// The voice dialog. Present when voice is engaged; the popup renders
    /// from its state, and utterances run through the SAME palette acts
    /// the keyboard uses — no second command system.
    voice: Option<voice::Dialog>,
    /// The vocabularies the fastpath listens with — kept on the App (not
    /// in the Dialog) so a dialog can be opened and closed without
    /// forgetting which genres this device works in.
    lexicon: voice::Lexicon,
    /// The find panel: how to search, what was typed, and what came back.
    find_ui: Option<FindUi>,
    /// Whose viewport this device is following, by collaborator name.
    ///
    /// LOCAL, and not in the project or the op log: what I am looking at
    /// is not part of the document, and syncing it would let one person
    /// drag everybody else's screen around.
    follow: Option<String>,
    mic: Box<dyn capture::Capture>,
    /// Where the take started, so it lands where it was sung.
    take_from: studio_core::Ticks,
    settings_open: bool,
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
    export_done: Vec<std::path::PathBuf>,
    export_progress: (usize, usize, f32),
    /// The archive popup's working state while open.
    archive_ui: Option<ArchiveUi>,
    /// The workbench's right slot: the viewport (default) or another tool,
    /// or nothing. The music-video loop is the point: the frame under the
    /// playhead beside the rack, instead of export → import → watch.
    side_slot: SideSlot,
    frame_cache: viewport::FrameCache,
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

#[derive(PartialEq, Clone, Copy)]
enum SinkContent { Nothing, RackLoop, Arrangement }

#[cfg(feature = "app")]
impl Default for App {
    fn default() -> Self {
        let sink = audition::open_sink();
        // Never `./project.studio` any more: an unnamed project gets its
        // OWN directory under the projects root (see project_dir's
        // containment rule and the accident that motivated it). The root
        // is $STUDIO_PROJECTS, else the last place an archive was written
        // — a person who archives to an SD card wants the working copy
        // beside it — else the freedesktop data directory.
        let project_path = match std::env::var("STUDIO_PROJECT") {
            Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => {
                let root = last_archive_root()
                    .unwrap_or_else(project_dir::projects_root);
                project_dir::project_dir(&root, "untitled").join("project.studio")
            }
        };
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
        let library_root = std::env::var("STUDIO_LIBRARY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| "library".into());
        let library_tree = library::TreeState::default();
        let midi = midi::MidiIn::open(std::env::var("STUDIO_MIDI").ok().as_deref()).ok();
        let analog = WootingAnalog::default();
        studio.status = format!("{} — {}", studio.status, analog.0.describe());
        if let Some(m) = &midi {
            studio.status = format!("{} — midi: {}", studio.status, m.port_name());
        }
        App { studio, midi, midi_held: Default::default(), peaks: Default::default(), audio: Default::default(), sink,
            follow: None,
              carrier, project_path, library_roots: library::Roots::from_env(library_root.clone()),
              library_root, library_tree, library_prefs,
              morph: morph_pad::PadState::default(),
              bounce_format,
              mix_pass: None,
              palette: None,
              voice: None,
              lexicon,
              find_ui: None,
              mic: capture::open_capture(),
              take_from: studio_core::Ticks::ZERO,
              settings_open: false, library_open: ws.library_open,
              rebinds: rebinds::Rebinds::decode(&ws.rebinds), rebind_capture: None,
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
              export_dialog: None,
              export_note: String::new(),
              export_run: None,
              export_done: Vec::new(),
              export_progress: (0, 0, 0.0),
              tune_scale: None,
              analog: Box::new(analog),
              loaded: SinkContent::Nothing }
    }
}

#[cfg(feature = "app")]
impl eframe::App for App {
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
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            self.palette = match self.palette {
                Some(_) => None, // C-p again closes: a toggle, not a stack
                None => Some(palette::Palette::open()),
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

        let colors = uikit_egui::palette_colors("midnight");
        // Whether the top bar chose Pages this frame — read by the central
        // panel below to gate over-scroll navigation. A local, not a field:
        // it is one frame's fact, recomputed from real width every time.
        let mut pages_mode = false;
        egui::Panel::top("bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
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
                                self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
                if ui.button("save").clicked()
                    || ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
                {
                    let path = self.project_path.clone();
                    workspace::save(&path, &workspace::WorkspaceState {
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
                    });
                    // The directory must exist (and be ours) BEFORE the
                    // file is written: a save into a fresh project root
                    // otherwise fails on the open, and a save into
                    // someone's source tree must fail here rather than
                    // after leaving a file in it.
                    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                        if let Err(e) = project_dir::ensure_project_dir(dir) {
                            self.studio.status = e;
                            return;
                        }
                    }
                    if self.studio.save_to(&path).is_ok() {
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
                }
                let mut combined = self.studio.combined;
                if ui.add(egui::Button::selectable(combined, "workbench"))
                    .on_hover_text("timeline above, rack/program/mixer below — instead of tabs")
                    .clicked() { combined = !combined; }
                self.studio.combined = combined;
                // The playhead's visible controls. `g g` and `g e` have
                // always done this; nothing on screen said so, which
                // meant a new user had no way back to the start at all.
                // The take button. Red while live, with the elapsed time
                // on it — a recording UI that does not show you it is
                // recording is how takes get lost.
                let live = self.mic.recording();
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
                if ui.button("⏮").on_hover_text("playhead to start (g g)").clicked() {
                    self.studio.transport.set_position(studio_core::Ticks::ZERO);
                    self.studio.status = "playhead at start".into();
                }
                if ui.button("⏭").on_hover_text("playhead to end (g e)").clicked() {
                    let d = self.studio.session.doc.project.duration();
                    self.studio.transport.set_position(d);
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
                Some(_) => None,
                None => Some(voice::Dialog { lexicon: self.lexicon.clone(),
                                             ..Default::default() }),
            };
        }
        if let Some(d) = &self.voice {
            voice::feedback_popup(ctx, d, &colors);
        }
        self.palette_window(ctx);
        self.find_window(ctx);
        self.mix_pass_window(ctx);
        self.archive_window(ctx);
        self.export_dialog_window(ctx);
        self.export_tracker_window(ctx);

        // ── files dropped on the window become rack channels ─────────────
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
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
                        self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
            // Our own presence: the sending half of what integrate_packet
            // merges. Rate-limited and heartbeat'd inside; called every
            // frame, sends when it should.
            if let Some(p) = self.studio.presence_beat(now_ms) {
                carrier.send(p);
            }
            // And the silence sweep — members time out with an announcement,
            // presence-only ghosts are pruned quietly.
            for ev in self.studio.session.sweep(now_ms) {
                if let studio_collab::session::Event::TimedOut(site) = ev {
                    self.studio.status = format!("site {} timed out", site.0);
                }
            }
            // Packets arrive between frames; a connected shell keeps frames
            // coming so a peer's edit shows without waiting for a local one.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
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

        // ── media that finished arriving from peers ──────────────────────
        for (id, audio) in std::mem::take(&mut self.studio.incoming_audio) {
            self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
        // Priority is whoever the user pressed play on last (the key handler
        // enforces exclusivity); this block just keeps the sink's buffer
        // truthful for the current owner and idles it for nobody.
        if self.studio.transport.playing {
            if self.loaded != SinkContent::Arrangement || self.studio.arrangement_dirty {
                let buf = self.studio.render_arrangement(self.sink.rate(), &self.audio);
                self.sink.set_buffer(std::sync::Arc::new(buf), 2, false);
                self.sink.seek(
                    self.studio.transport.position().to_samples(self.sink.rate()).max(0) as usize);
                self.studio.arrangement_dirty = false;
                self.loaded = SinkContent::Arrangement;
            }
            self.sink.set_playing(true);
            if let Some(f) = self.sink.position() {
                // Video follows audio, never the reverse (transport's rule №1)
                // — the sink's consumed frames ARE the playhead.
                self.studio.transport.set_position(
                    studio_core::Ticks::from_samples(f as i64, self.sink.rate()));
            } else if !self.sink.playing() {
                self.studio.transport.stop();
                self.studio.status = "reached the end".into();
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
        if self.studio.pending_bounce {
            self.studio.pending_bounce = false;
            // The timeline's rate, not the sink's: a bounce is project
            // material and must not change length because of the sound card.
            if let Some((id, audio)) = self.studio.bounce_current_pattern(48_000, &self.audio) {
                self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
            egui::Panel::left("library")
                .resizable(true)
                .default_size(210.0)
                .size_range(160.0..=420.0)
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
                            self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
                                    sends: vec![], muted: false } }, "drum bus");
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
                                    has_video: false } }, "sample");
                            let cid = studio_core::ClipId(
                                self.studio.session.doc.project.clips.keys()
                                    .map(|x| x.0).max().unwrap_or(0) + 1);
                            self.studio.edit(studio_core::Op::AddClip {
                                clip: studio_core::Clip { id: cid, source: sid,
                                    track, pos: self.studio.playhead(),
                                    src_in: studio_core::Ticks::ZERO, len,
                                    gain: 1.0, effects: vec![] } }, "placed");
                            self.peaks.insert(sid, studio_audio::Peaks::build(&audio, 256, 256));
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
                    // Bounds-checked rather than indexed: the action was
                    // produced from last frame's rack, and the rack can have
                    // shrunk since (a pattern deleted in the same frame the
                    // send button was pressed). An out-of-date index is a
                    // status line, not a panic.
                    match self.studio.rack.patterns.get(i) {
                        Some(pat) => {
                            let name = if pat.name.trim().is_empty() {
                                format!("pattern {}", i + 1)
                            } else {
                                pat.name.clone()
                            };
                            // NOT WIRED to a real timeline insert yet, and
                            // deliberately not faked: placing a pattern needs
                            // a decision about what a pattern IS on the
                            // timeline (a clip referencing the rack? a bounced
                            // audio region? a MIDI-ish region the renderer
                            // expands?), and each answer implies a different
                            // studio_core::Op. Guessing one here would put a
                            // wrong shape into the undo log, which is the one
                            // structure in this app that must not accumulate
                            // junk. The sidebar half of the report — patterns
                            // being invisible — is fixed; this half is
                            // reported honestly rather than half-built.
                            self.studio.status = format!(
                                "\"{name}\" is in the sidebar now, but placing a pattern on the \
                                 timeline is not implemented yet — see C13_TODO");
                        }
                        None => {
                            self.studio.status =
                                "that pattern no longer exists".into();
                        }
                    }
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
            // cannot open an mp4, and the old flow's silent has_video:false
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
                                self.peaks.insert(id, studio_audio::Peaks::build(&a, 256, 256));
                                self.audio.insert(id, a);
                            }
                            if let Ok(raw) = std::fs::read(&path) {
                                self.studio.register_asset(id, &raw);
                            }
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
                                self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
                Panel::Timeline => {
                    // The inspector sits above the arrangement rather than
                    // in a side panel: it is about the thing you just
                    // clicked, so it belongs where your eyes already are,
                    // and it collapses to nothing when nothing is selected.
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
                                                   &colors, ph, &sel, &self.peaks,
                                                   *trim_side,
                                                   closed_groups,
                                                   mode, input_mode, &pending,
                                                   song_tempo);
                    if let Some(t) = tr.sought {
                        // Scrub. The transport owns position, so this is a
                        // seek and not a model edit — moving the playhead
                        // is not something a collaborator should undo.
                        self.studio.transport.set_position(t);
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
                        // back wordlessly.
                        self.studio.edit(
                            studio_core::Op::MoveClip { id, pos, track }, "moved");
                    }

                    // ── the automation strip ─────────────────────────────
                    // A target whose track or bus has been removed clears
                    // itself rather than offering a strip every edit on
                    // which would be refused.
                    if let Some(t) = self.studio.auto_target {
                        let p = &self.studio.session.doc.project;
                        let alive = match t {
                            studio_core::AutoTarget::TrackGain(id) =>
                                p.tracks.iter().any(|tr| tr.id == id),
                            studio_core::AutoTarget::BusGain(id) =>
                                p.busses.contains_key(&id),
                        };
                        if !alive { self.studio.auto_target = None; }
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("automate:");
                        let mut target = self.studio.auto_target;
                        if ui.add(egui::Button::selectable(target.is_none(), "off")).clicked() {
                            target = None;
                        }
                        for t in &self.studio.session.doc.project.tracks {
                            let this = Some(studio_core::AutoTarget::TrackGain(t.id));
                            if ui.add(egui::Button::selectable(target == this, &t.name)).clicked() {
                                target = this;
                            }
                        }
                        for b in self.studio.session.doc.project.busses.values() {
                            let this = Some(studio_core::AutoTarget::BusGain(b.id));
                            if ui.add(egui::Button::selectable(target == this,
                                                               format!("bus: {}", b.name))).clicked() {
                                target = this;
                            }
                        }
                        self.studio.auto_target = target;
                    });
                    if let Some(target) = self.studio.auto_target {
                        let points = self.studio.session.doc.project.automation
                            .get(&target).cloned().unwrap_or_default();
                        let mut drag = self.studio.auto_drag;
                        let edits = studio_egui::auto::strip(
                            ui, &points, self.studio.current_view(), &mut drag, Some(ph), &colors);
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
            self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
            self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
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
        egui::Window::new("Command palette").open(&mut open).resizable(false)
            .collapsible(false).default_width(width)
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
                            let hit = ui.selectable_label(i == p.selected, &c.label);
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
                palette::ShellVerb::OpenProject => {
                    // Deliberately NOT loading here. Choosing a project
                    // is a file-picker step and the palette's own model
                    // has a shape for that — `Then::Ask` — which this
                    // verb does not use yet. Saying so beats loading
                    // some guessed path over unsaved work.
                    self.studio.status =
                        "open project: needs a picker step (C90c)".into();
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
                palette::ShellVerb::MarkerHere => {
                    let at = self.studio.playhead();
                    let n = self.studio.session.doc.project.markers.len() + 1;
                    self.studio.edit(studio_core::Op::SetMarker {
                        at, name: format!("mark {n}") }, "marker");
                }
                palette::ShellVerb::GotoMarker(at) => {
                    self.studio.transport.set_position(at);
                    self.studio.status = format!("at {:.1}s", at.to_seconds());
                }
                palette::ShellVerb::GotoStart => {
                    self.studio.transport.set_position(studio_core::Ticks::ZERO);
                    self.studio.status = "playhead at start".into();
                }
                palette::ShellVerb::GotoEnd => {
                    let d = self.studio.session.doc.project.duration();
                    self.studio.transport.set_position(d);
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
                palette::ShellVerb::ArchiveDialog => {
                    self.archive_ui = Some(ArchiveUi::default());
                }
                palette::ShellVerb::ExportDialog => {
                    self.export_dialog = Some(Default::default());
                }
                palette::ShellVerb::Settings => { self.settings_open = true; }
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
                                        studio_audio::Peaks::build(&audio, 256, 256));
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
            let track = self.studio.session.doc.project.tracks.iter()
                .find(|t| t.kind == studio_core::TrackKind::Audio).map(|t| t.id);
            let len = studio_core::Ticks::from_samples(
                audio.frames() as i64, audio.sample_rate);
            if let Some(track) = track {
                let cid = studio_core::ClipId(
                    self.studio.session.doc.project.clips.keys()
                        .map(|c| c.0).max().unwrap_or(0) + 1);
                self.studio.edit(studio_core::Op::AddClip { clip: studio_core::Clip {
                    id: cid, source: id, track, pos: self.take_from,
                    src_in: studio_core::Ticks::ZERO, len, gain: 1.0,
                    effects: vec![] } }, "take");
            }
            if let Ok(bytes) = studio_media::flac::encode_flac(&audio, id.0) {
                self.studio.register_asset(id, &bytes);
            }
            self.peaks.insert(id, studio_audio::Peaks::build(&audio, 256, 256));
            self.audio.insert(id, audio);
            self.studio.status = format!("take: {:.1}s{}", len.to_seconds(),
                if dropped > 0 {
                    // Said, not swallowed: a take with holes must not be
                    // built on unknowingly.
                    format!(" — WARNING: {dropped} block(s) dropped, check the take")
                } else { String::new() });
        } else {
            self.take_from = self.studio.playhead();
            match self.mic.start() {
                Ok(()) => self.studio.status =
                    format!("recording from {} at {} Hz…",
                            self.mic.device_name(), self.mic.rate()),
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
        if let Some(at) = goto { self.studio.transport.set_position(at); }
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
                    ui.selectable_value(&mut plan.format, ExportFormat::Opus, "opus");
                    ui.selectable_value(&mut plan.format, ExportFormat::Wav, "wav");
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
                *plan, jobs, dest, project.clone(), self.audio.clone()),
            Planned::Rack(jobs) => export::start_rack(
                *plan, jobs, dest, self.studio.current_pattern,
                self.audio.clone(), project.name.clone()),
        });
        true
    }

    /// The render tracker: a bar that moves, the files as they land, and a
    /// cancel that means soon. Repaint is requested while a run is live —
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
                export::Event::Done => {
                    self.studio.status = format!(
                        "exported {} file{}", self.export_done.len(),
                        if self.export_done.len() == 1 { "" } else { "s" });
                    finished = true;
                }
                export::Event::Cancelled => {
                    self.studio.status = format!(
                        "export cancelled — {} file{} kept", self.export_done.len(),
                        if self.export_done.len() == 1 { "" } else { "s" });
                    finished = true;
                }
                export::Event::Failed(e) => {
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
pub enum ExportFormat { Wav, Opus }
impl ExportFormat {
    pub fn ext(self) -> &'static str {
        match self { ExportFormat::Wav => "wav", ExportFormat::Opus => "opus" }
    }
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

        for fmt in [ExportFormat::Wav, ExportFormat::Opus] {
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
            sample_rate: 48_000, channels: 1, has_video: false } }, "seeded");
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
            sample_rate: 48_000, channels: 1, has_video: false } }, "seeded");
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
            chain: Vec::new(), sends: Vec::new(), muted: false } }, "bus");
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
                chain: Vec::new(), sends: Vec::new(), muted: false } }, "bus");
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
                chain: Vec::new(), sends: Vec::new(), muted: false } }, "bus");
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
                channels: 1, has_video: false } }, "src");
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
        for ch in ['A', 'Q', ',', '.', '<', '>'] {
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
            gain: 1.0, chain: vec![mine.clone()], sends: vec![], muted: false } }, "bus");
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
            gain: 1.0, chain: vec![], sends: vec![], muted: false } }, "bus");
        s.edit_group(vec![
            Op::SetBusGain { id: bus, gain: 0.5 },
            Op::SetBusGain { id: BusId(999), gain: 0.5 }, // no such bus
        ], "mix pass");
        assert!((s.session.doc.project.busses[&bus].gain - 1.0).abs() < 1e-6,
                "the first move was rolled back");
        assert!(s.status.contains("nothing applied"), "{}", s.status);
    }
}
