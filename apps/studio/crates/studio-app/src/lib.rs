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
                name: (*nm).into(), muted: false, locked: false }, at: i }).ok();
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
            Ok(()) => self.status = what.into(),
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
                        if self.transport.playing { self.transport.stop(); self.status = "stopped".into(); }
                        else { self.transport.play(); self.status = "playing".into(); }
                    }
                    Action::Undo => {
                        self.status = if self.history.undo(&mut self.session.doc.project).is_some()
                            { "undo".into() } else { "nothing to undo".into() };
                    }
                    Action::Redo => {
                        self.status = if self.history.redo(&mut self.session.doc.project).is_some()
                            { "redo".into() } else { "nothing to redo".into() };
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
}

#[cfg(feature = "app")]
impl Default for App {
    fn default() -> Self { App { studio: Studio::demo(), peaks: Default::default() } }
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
                    if ui.selectable_label(self.studio.panel == p, p.label()).clicked() {
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
        egui::CentralPanel::default().show(ctx, |ui| {
            let ph = self.studio.playhead();
            let sel = self.studio.selection.clone();
            studio_egui::timeline(ui, &self.studio.session.doc, &mut self.studio.view,
                                  &colors, ph, &sel, &self.peaks);
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
