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

pub mod audition;
pub mod export;
pub mod clip_inspector;
pub mod library;
pub mod midi;
pub mod mixer_panel;
pub mod program_panel;
pub mod project_dir;
pub mod rack_panel;

/// Everything the running editor holds.
pub struct Studio {
    pub session: Session,
    pub transport: Transport,
    pub view: studio_egui::View,
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
}

impl Studio {
    pub fn new(project: Project, site: SiteId, team: Team) -> Studio {
        let name = project.name.clone();
        Studio {
            session: Session::open(team, &name, Doc::new(site, project)),
            transport: Transport::new(48_000),
            view: studio_egui::View::default(),
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
            bus_drag: None,
            input_mode: Default::default(),
            octave: 0,
            trim_side: studio_egui::TrimSide::default(),
            pending_bounce: false,
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
    pub fn integrate_packet(&mut self, p: studio_collab::wire::Packet)
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
                self.session.doc.presence(pr);
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
    /// Where Ctrl+S writes: STUDIO_PROJECT if set, else ./project.studio.
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
    library: Vec<library::LibraryEntry>,
    library_open: bool,
    /// The export dialog's plan while it is open, plus its last refusal —
    /// shown inline so a bad plan is a message, not a closed window.
    export_dialog: Option<export::Plan>,
    export_note: String,
    /// A render in flight, drained once per frame into the tracker.
    export_run: Option<export::Run>,
    export_done: Vec<std::path::PathBuf>,
    export_progress: (usize, usize, f32),
    tune_root: u8,
    tune_mode: studio_fx::retune::Mode,
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
        let project_path = std::env::var("STUDIO_PROJECT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| "project.studio".into());
        // Open the named project when it exists; a demo otherwise. A load
        // FAILURE is neither: overwriting a damaged-but-real file with a
        // demo on the next Ctrl+S would finish what the damage started, so
        // the shell starts demo'd but says exactly why.
        let mut load_note = None;
        let mut studio = if project_path.exists() {
            match Studio::load_from(&project_path, SiteId(1),
                                    Team::from_contacts("solo", &["sc-local".into()])) {
                Ok(s) => s,
                Err(e) => { load_note = Some(e); Studio::demo() }
            }
        } else {
            Studio::demo()
        };
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
        let library = library::scan(&library_root);
        let midi = midi::MidiIn::open(std::env::var("STUDIO_MIDI").ok().as_deref()).ok();
        let analog = WootingAnalog::default();
        studio.status = format!("{} — {}", studio.status, analog.0.describe());
        if let Some(m) = &midi {
            studio.status = format!("{} — midi: {}", studio.status, m.port_name());
        }
        App { studio, midi, midi_held: Default::default(), peaks: Default::default(), audio: Default::default(), sink,
              carrier, project_path, library_root, library, library_open: true,
              export_dialog: None,
              export_note: String::new(),
              export_run: None,
              export_done: Vec::new(),
              export_progress: (0, 0, 0.0),
              tune_root: 0, tune_mode: studio_fx::retune::Mode::Minor,
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
        // Keys first, so a shortcut is never eaten by a widget holding focus.
        let typed: Vec<char> = ctx.input(|i| i.events.iter().flat_map(|e| match e {
            egui::Event::Text(t) => t.chars().collect::<Vec<_>>(),
            _ => Vec::new(),
        }).collect());
        for ch in typed { self.studio.key(studio_egui::keys::Key::c(ch)); }

        let colors = uikit_egui::palette_colors("midnight");
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for p in Panel::ALL {
                    if ui.add(egui::Button::selectable(self.studio.panel == p, p.label())).clicked() {
                        self.studio.panel = p;
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
                        .selected_text(format!("{} {}",
                            ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"]
                                [self.tune_root as usize % 12],
                            match self.tune_mode {
                                studio_fx::retune::Mode::Minor => "min",
                                studio_fx::retune::Mode::HarmonicMinor => "harm",
                                studio_fx::retune::Mode::Chromatic => "chrom",
                            }))
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            for (i, n) in ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"]
                                .iter().enumerate() {
                                ui.selectable_value(&mut self.tune_root, i as u8, *n);
                            }
                            ui.separator();
                            for (m, n) in [(studio_fx::retune::Mode::Minor, "minor"),
                                           (studio_fx::retune::Mode::HarmonicMinor, "harmonic"),
                                           (studio_fx::retune::Mode::Chromatic, "chromatic")] {
                                ui.selectable_value(&mut self.tune_mode, m, n);
                            }
                        });
                    if ui.button("tune take")
                        .on_hover_text("print-side autotune → a new clip beside the dry one")
                        .clicked()
                    {
                        let scale = studio_fx::retune::Scale {
                            root: self.tune_root, mode: self.tune_mode };
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
                if ui.button("save").clicked()
                    || ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
                {
                    let path = self.project_path.clone();
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
                if ui.button("export…").clicked() {
                    self.export_dialog = Some(export::Plan::default());
                    self.export_note.clear();
                }
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.studio.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} clips", self.studio.session.doc.project.clips.len()));
                });
            });
        });
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
                    c, self.studio.octave, &studio_egui::keys::Key::c(c), &*self.analog)
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
            for p in carrier.poll() {
                for reply in self.studio.integrate_packet(p) {
                    carrier.send(reply);
                }
            }
            for p in self.studio.drain_outbox() {
                carrier.send(p);
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
            egui::SidePanel::left("library").default_width(210.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("⟳").on_hover_text("rescan").clicked() {
                        self.library = library::scan(&self.library_root);
                    }
                    if ui.small_button("hide").clicked() {
                        self.library_open = false;
                    }
                });
                lib_action = library::show(ui, &self.library_root, &self.library, &colors);
            });
        }
        if let Some(action) = lib_action {
            // The sidebar's two buttons are the two import doors drops
            // already use, minus the panel-guessing: the button says which.
            let (path, to_rack) = match action {
                library::LibraryAction::ToRack(p) => (p, true),
                library::LibraryAction::ToTimeline(p) => (p, false),
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

        if self.studio.combined {
            egui::TopBottomPanel::bottom("workbench").resizable(true)
                .default_height(300.0).show(ctx, |ui| {
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
                        Panel::Rack => rack_panel::show(ui, &mut self.studio, &colors, playhead_col),
                        Panel::Program => program_panel::show(ui, &mut self.studio, &colors, playhead_col),
                        Panel::Mixer => mixer_panel::show(ui, &mut self.studio, &colors),
                        Panel::Timeline => {}
                    }
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            match if self.studio.combined { Panel::Timeline } else { self.studio.panel } {
                Panel::Rack => {
                    rack_panel::show(ui, &mut self.studio, &colors, playhead_col);
                }
                Panel::Mixer => {
                    mixer_panel::show(ui, &mut self.studio, &colors);
                }
                Panel::Program => {
                    program_panel::show(ui, &mut self.studio, &colors, playhead_col);
                }
                Panel::Timeline => {
                    // The inspector sits above the arrangement rather than
                    // in a side panel: it is about the thing you just
                    // clicked, so it belongs where your eyes already are,
                    // and it collapses to nothing when nothing is selected.
                    clip_inspector::show(ui, &mut self.studio, &colors);
                    let ph = self.studio.playhead();
                    let sel = self.studio.selection.clone();
                    let tr = studio_egui::timeline(ui, &self.studio.session.doc,
                                                   &mut self.studio.view,
                                                   &colors, ph, &sel, &self.peaks,
                                                   self.studio.trim_side);
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
                            ui, &points, &self.studio.view, &mut drag, Some(ph), &colors);
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
        });
    }
}


#[cfg(feature = "app")]
impl App {
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
            to.integrate_packet(p);
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
            studio_collab::wire::Packet::Hello { site: SiteId(2), since: 0 });
        assert!(!replies.is_empty(), "a Hello is answered");
        for p in replies {
            b.integrate_packet(p);
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
        c.integrate_packet(a.session.broadcast());
        if let Some(rack) = a.session.rack_broadcast() {
            c.integrate_packet(rack);
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
        for p in from_a { b.integrate_packet(p); }
        for p in from_b { a.integrate_packet(p); }
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
                to_a.extend(b.integrate_packet(p));
            }
            while let Some(p) = to_a.pop() {
                quiet = false;
                to_b.extend(a.integrate_packet(p));
            }
            while let Some(p) = to_b.pop() {
                to_a.extend(b.integrate_packet(p));
            }
            for p in to_a {
                a.integrate_packet(p);
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
        for p in to_b.drain(..) { req.extend(b.integrate_packet(p)); }
        let mut chunks = Vec::new();
        for p in req { chunks.extend(a.integrate_packet(p)); }
        assert!(chunks.len() >= 2, "big enough to have several chunks: {}", chunks.len());
        chunks.reverse();
        for p in chunks { b.integrate_packet(p); }
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
        });
        assert!(!b.chunk_store.contains_key(&id), "a lying chunk never enters the store");
        b.integrate_packet(studio_collab::wire::Packet::Chunk { id, bytes: real });
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
        for p in a.drain_outbox() { replies.extend(b.integrate_packet(p)); }
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
            for r in s.integrate_packet(p) {
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
        let note = play_note('d', 0, &Key::c('d'), &NullAnalog).expect("E natural");
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
