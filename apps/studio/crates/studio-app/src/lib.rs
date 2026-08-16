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
pub mod rack_panel;

/// Everything the running editor holds.
pub struct Studio {
    pub session: Session,
    pub transport: Transport,
    pub view: studio_egui::View,
    pub panel: Panel,
    pub mode: ViewMode,
    pub selection: Vec<studio_core::ClipId>,
    pub history: studio_core::History,
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
            history: studio_core::History::default(),
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
                name: (*nm).into(), muted: false, locked: false, route: None }, at: i }).ok();
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

    /// Apply an edit through history, so undo works and the status line says so.
    pub fn edit(&mut self, op: studio_core::Op, what: &str) {
        match self.history.apply(&mut self.session.doc.project, &op) {
            Ok(()) => { self.status = what.into(); self.arrangement_dirty = true; }
            // An edit that cannot apply must SAY so. A silently dropped edit is
            // the worst failure an editor has: the user believes it worked.
            Err(e) => self.status = format!("{what} refused: {e:?}"),
        }
    }

    /// Route a key through the modal map. Returns true if it was consumed.
    pub fn key(&mut self, k: studio_egui::keys::Key) -> bool {
        use studio_egui::keys::{Action, Resolution};
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
                        self.status = if self.history.undo(&mut self.session.doc.project).is_some()
                            { self.arrangement_dirty = true; "undo".into() }
                            else { "nothing to undo".into() };
                    }
                    Action::Redo => {
                        self.status = if self.history.redo(&mut self.session.doc.project).is_some()
                            { self.arrangement_dirty = true; "redo".into() }
                            else { "nothing to redo".into() };
                    }
                    Action::Delete => {
                        let sel = std::mem::take(&mut self.selection);
                        for id in sel { self.edit(studio_core::Op::RemoveClip { id }, "deleted"); }
                    }
                    Action::Split => self.split_at_playhead(),
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
            Ticks::ZERO, end, rate, &[], &[],
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
        let path = format!("bounces/{}.wav", pattern.name);
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
        let mut studio = Studio::demo();
        studio.status = format!("ready — audition: {}", sink.describe());
        App { studio, peaks: Default::default(), audio: Default::default(), sink,
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
        // ── files dropped on the window become rack channels ─────────────
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let name = f.path.as_ref()
                .and_then(|p| p.file_stem()).and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| f.name.clone());
            let decoded = match (&f.path, &f.bytes) {
                (Some(p), _) => studio_media::decode_file(p)
                    .map(|a| (a, p.display().to_string())),
                (None, Some(b)) => {
                    let hint = f.name.rsplit('.').next();
                    studio_media::decode_bytes(b.to_vec(), hint)
                        .map(|a| (a, f.name.clone()))
                }
                (None, None) => continue,
            };
            match decoded {
                Ok((audio, path)) => {
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
                    }
                }
                // A decode error is not a missing-plugin error (studio-media's
                // own rule): say exactly what failed, keep editing.
                Err(e) => self.studio.status = format!("import '{name}' failed: {e}"),
            }
        }

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

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.studio.panel {
                Panel::Rack => {
                    rack_panel::show(ui, &mut self.studio, &colors, playhead_col);
                }
                Panel::Mixer | Panel::Program => {
                    // Honest placeholder beats a silent no-op: a tab that
                    // switches to nothing reads as a broken tab.
                    ui.colored_label(colors.text_weak,
                        format!("{} — not built yet.", self.studio.panel.label()));
                }
                Panel::Timeline => {
                    let ph = self.studio.playhead();
                    let sel = self.studio.selection.clone();
                    studio_egui::timeline(ui, &self.studio.session.doc, &mut self.studio.view,
                                          &colors, ph, &sel, &self.peaks);
                }
            }
        });
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
