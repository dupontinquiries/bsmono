//! Section watching: chapters, search, and a watch plan, as a panel.
//!
//! Same contract as `gestures.rs` and `bar.rs`: the widget holds only its own
//! UI state (the query box, which chapters are ticked) and returns
//! [`SectionAction`]s. It never seeks, never plans, never transcribes — the
//! app owns playback and the transcription loop, and decides what an action
//! means. That keeps the rules testable without a frame and the widget usable
//! from a shell whose transcript comes from anywhere.
//!
//! The strip at the top is the transcription loop made visible: what has been
//! transcribed, where the chapters fall, what the plan keeps, where the
//! playhead is. A viewer can see the work happening and steer it with the
//! scope buttons, rather than wait on a spinner.

use std::collections::BTreeSet;
use std::time::Duration;

use egui::{Color32, Rect, Sense, Stroke, Ui, Vec2};
use vidkit_relevance::sections::{Chapter, Hit};
use vidkit_relevance::Section;

use crate::view::format_time;

/// How much of the video the transcriber may work on. Mirrors
/// `vidkit_transcribe::coverage::Mode` without depending on it: the kit takes
/// no transcription dependency (whisper.cpp) for a button row.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Scope {
    Paused,
    /// A horizon ahead of the playhead. The battery choice.
    Ahead,
    /// The whole video, playhead first. Needed for a complete chapter list.
    All,
}

#[derive(Clone, PartialEq, Debug)]
pub enum SectionAction {
    Seek(Duration),
    /// The query changed; the app searches and passes hits back next frame.
    Search(String),
    /// Play only these spans.
    WatchSpans(Vec<(Duration, Duration)>),
    /// Play only what matches this prompt (the app scores and plans).
    WatchMatching(String),
    ClearPlan,
    /// Write these spans to a file.
    Export(Vec<(Duration, Duration)>),
    SetScope(Scope),
}

/// Everything the panel shows. Borrowed, rebuilt by the app each frame.
pub struct SectionsModel<'a> {
    pub chapters: &'a [Chapter],
    pub hits: &'a [Hit],
    pub total: Option<Duration>,
    pub position: Duration,
    /// Transcribed time ranges.
    pub transcribed: &'a [(Duration, Duration)],
    /// 0..1, or `None` while the length is unknown.
    pub progress: Option<f32>,
    /// One line from the transcription loop: what it is doing now.
    pub status: &'a str,
    /// The active watch plan, empty when watching everything.
    pub plan: &'a [Section],
    pub scope: Scope,
    /// Whether an export can run (local files only).
    pub can_export: bool,
}

#[derive(Default)]
pub struct SectionsPanel {
    pub query: String,
    pub selected: BTreeSet<usize>,
    sent_query: String,
}

/// The chapter containing `pos`.
pub fn current_chapter(chapters: &[Chapter], pos: Duration) -> Option<usize> {
    chapters.iter().position(|c| pos >= c.start && pos < c.end)
}

/// The ticked chapters as spans, adjacent ones merged, in time order.
/// Indices past the list (a list that shrank as transcription refined it)
/// are ignored rather than panicking.
pub fn selected_spans(chapters: &[Chapter], selected: &BTreeSet<usize>) -> Vec<(Duration, Duration)> {
    let mut out: Vec<(Duration, Duration)> = Vec::new();
    for &i in selected {
        let Some(c) = chapters.get(i) else { continue };
        match out.last_mut() {
            Some(last) if last.1 >= c.start => last.1 = last.1.max(c.end),
            _ => out.push((c.start, c.end)),
        }
    }
    out
}

/// Horizontal position of `t` in a strip of `width` for a video of `total`.
pub fn strip_x(t: Duration, total: Duration, width: f32) -> f32 {
    if total.is_zero() {
        return 0.0;
    }
    (t.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0) as f32 * width
}

impl SectionsPanel {
    pub fn ui(&mut self, ui: &mut Ui, m: &SectionsModel) -> Vec<SectionAction> {
        let mut out = Vec::new();
        self.strip(ui, m, &mut out);

        ui.horizontal_wrapped(|ui| {
            ui.small(m.status);
            if let Some(p) = m.progress {
                ui.small(format!("· {:.0}% transcribed", p * 100.0));
            }
        });
        ui.horizontal(|ui| {
            for (scope, label, tip) in [
                (Scope::Paused, "Pause", "stop transcribing after the current chunk"),
                (Scope::Ahead, "Ahead", "only the next few minutes: saves battery"),
                (Scope::All, "All", "the whole video, from the playhead outward"),
            ] {
                if ui.selectable_label(m.scope == scope, label).on_hover_text(tip).clicked() && m.scope != scope {
                    out.push(SectionAction::SetScope(scope));
                }
            }
        });

        ui.separator();
        let resp = ui.add(egui::TextEdit::singleline(&mut self.query).id_salt("vidkit-sections-query").hint_text("find where they discuss…"));
        if resp.changed() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            let q = self.query.trim().to_string();
            if q != self.sent_query {
                self.sent_query = q.clone();
                out.push(SectionAction::Search(q));
            }
        }
        if !self.query.trim().is_empty() {
            if ui.button("Watch only matching").on_hover_text("skip the sections that do not mention this").clicked() {
                out.push(SectionAction::WatchMatching(self.query.trim().to_string()));
            }
            for h in m.hits.iter().take(6) {
                let snippet: String = h.text.chars().take(70).collect();
                if ui.link(format!("{}  {snippet}…", format_time(h.at))).clicked() {
                    out.push(SectionAction::Seek(h.at));
                }
            }
        }

        ui.separator();
        let spans = selected_spans(m.chapters, &self.selected);
        ui.horizontal(|ui| {
            if ui.add_enabled(!spans.is_empty(), egui::Button::new("Watch selected")).clicked() {
                out.push(SectionAction::WatchSpans(spans.clone()));
            }
            if ui
                .add_enabled(!spans.is_empty() && m.can_export, egui::Button::new("Export selected"))
                .on_disabled_hover_text("tick chapters of a local file")
                .clicked()
            {
                out.push(SectionAction::Export(spans.clone()));
            }
            if !m.plan.is_empty() && ui.button("Watch everything").clicked() {
                out.push(SectionAction::ClearPlan);
            }
        });

        let here = current_chapter(m.chapters, m.position);
        egui::ScrollArea::vertical().max_height(ui.available_height().max(120.0)).show(ui, |ui| {
            if m.chapters.is_empty() {
                ui.weak("No chapters yet — they appear as the transcript grows.");
            }
            for (i, c) in m.chapters.iter().enumerate() {
                ui.horizontal(|ui| {
                    // The theme's inactive checkbox is a fill barely off the
                    // panel colour: in a screenshot the boxes were invisible
                    // until ticked. An outline in the weak text colour, for
                    // this row only.
                    let edge = Stroke::new(1.0, ui.visuals().weak_text_color());
                    ui.visuals_mut().widgets.inactive.bg_stroke = edge;
                    ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::new(1.5, ui.visuals().text_color());
                    let mut ticked = self.selected.contains(&i);
                    if ui.checkbox(&mut ticked, "").changed() {
                        if ticked {
                            self.selected.insert(i);
                        } else {
                            self.selected.remove(&i);
                        }
                    }
                    let label = format!("{}  {}", format_time(c.start), c.title());
                    let text = if Some(i) == here { egui::RichText::new(label).strong() } else { egui::RichText::new(label) };
                    if ui.link(text).clicked() {
                        out.push(SectionAction::Seek(c.start));
                    }
                    ui.weak(format_time(c.duration()));
                });
            }
        });
        out
    }

    fn strip(&self, ui: &mut Ui, m: &SectionsModel, out: &mut Vec<SectionAction>) {
        let width = ui.available_width().max(60.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::click());
        let Some(total) = m.total.filter(|t| !t.is_zero()) else {
            ui.painter().rect_filled(rect, 3.0, ui.visuals().faint_bg_color);
            return;
        };
        let p = ui.painter_at(rect);
        let v = ui.visuals();
        let x = |t: Duration| rect.left() + strip_x(t, total, rect.width());
        let band = |a: Duration, b: Duration, top: f32, bottom: f32, c: Color32| {
            p.rect_filled(Rect::from_min_max(egui::pos2(x(a), top), egui::pos2(x(b).max(x(a) + 1.0), bottom)), 0.0, c);
        };
        p.rect_filled(rect, 3.0, v.faint_bg_color);
        // Transcribed: the lower half, so plan and coverage read separately.
        for &(a, b) in m.transcribed {
            band(a, b, rect.center().y, rect.bottom(), v.widgets.inactive.bg_fill);
        }
        // Kept by the plan: the upper half, in the selection colour.
        for s in m.plan.iter().filter(|s| s.keep) {
            band(s.start, s.end, rect.top(), rect.center().y, v.selection.bg_fill);
        }
        for c in m.chapters.iter().skip(1) {
            p.line_segment([egui::pos2(x(c.start), rect.top()), egui::pos2(x(c.start), rect.bottom())], Stroke::new(1.0, v.text_color()));
        }
        let px = x(m.position);
        p.line_segment([egui::pos2(px, rect.top() - 2.0), egui::pos2(px, rect.bottom() + 2.0)], Stroke::new(2.0, v.strong_text_color()));
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let f = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
                out.push(SectionAction::Seek(Duration::from_secs_f64(total.as_secs_f64() * f)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(a: u64, b: u64, kw: &[&str]) -> Chapter {
        Chapter {
            start: Duration::from_secs(a),
            end: Duration::from_secs(b),
            keywords: kw.iter().map(|s| s.to_string()).collect(),
            windows: 0..1,
        }
    }

    #[test]
    fn ticked_neighbours_merge_and_gaps_stay() {
        let cs = [ch(0, 120, &[]), ch(120, 300, &[]), ch(300, 420, &[]), ch(420, 600, &[])];
        let sel: BTreeSet<usize> = [0, 1, 3, 9].into_iter().collect();
        let s = |n: u64| Duration::from_secs(n);
        assert_eq!(selected_spans(&cs, &sel), vec![(s(0), s(300)), (s(420), s(600))]);
        assert!(selected_spans(&cs, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn the_current_chapter_is_found_and_end_is_exclusive() {
        let cs = [ch(0, 120, &[]), ch(120, 300, &[])];
        assert_eq!(current_chapter(&cs, Duration::from_secs(119)), Some(0));
        assert_eq!(current_chapter(&cs, Duration::from_secs(120)), Some(1));
        assert_eq!(current_chapter(&cs, Duration::from_secs(300)), None);
    }

    #[test]
    fn strip_positions_are_clamped_and_a_zero_total_is_safe() {
        let s = |n: u64| Duration::from_secs(n);
        assert_eq!(strip_x(s(50), s(100), 200.0), 100.0);
        assert_eq!(strip_x(s(500), s(100), 200.0), 200.0);
        assert_eq!(strip_x(s(5), Duration::ZERO, 200.0), 0.0);
    }

    /// Painting needs a live context, and nothing else reaches it headless:
    /// the rotation panic this crate once shipped was of exactly that kind.
    #[test]
    #[allow(deprecated)]
    fn it_paints_without_panicking_empty_or_full() {
        let ctx = egui::Context::default();
        let chapters = [ch(0, 120, &["fourier", "transform"]), ch(120, 400, &["laplace"])];
        let hits = [Hit { at: Duration::from_secs(130), window: 2, score: 1.0, text: "the laplace transform of a step".into() }];
        let plan = vidkit_relevance::plan_keeping(&[(Duration::ZERO, Duration::from_secs(120))], Duration::from_secs(400));
        let transcribed = [(Duration::ZERO, Duration::from_secs(90))];
        let mut panel = SectionsPanel { query: "laplace".into(), ..Default::default() };
        panel.selected.insert(1);
        for full in [false, true] {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let m = SectionsModel {
                        chapters: if full { &chapters } else { &[] },
                        hits: if full { &hits } else { &[] },
                        total: full.then(|| Duration::from_secs(400)),
                        position: Duration::from_secs(150),
                        transcribed: if full { &transcribed } else { &[] },
                        progress: full.then_some(0.25),
                        status: "whisper: chunk 4 of 14",
                        plan: if full { &plan } else { &[] },
                        scope: Scope::All,
                        can_export: full,
                    };
                    let _ = panel.ui(ui, &m);
                });
            });
        }
    }
}
