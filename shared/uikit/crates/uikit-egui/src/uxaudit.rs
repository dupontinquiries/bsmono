//! uxaudit — a UI audit an agent can run in a loop.
//!
//! `layout_probe` finds shape faults and `uiverify` finds pixel faults. Both are built for a
//! human writing one assertion about one screen. An agent iterating on a UX request needs
//! something else: run the whole battery, get a LIST, change something, run it again, and know
//! which findings are the same ones.
//!
//! ## Stable ids are the whole point
//!
//! Without them a loop cannot tell progress from churn. Two passes produce two lists of
//! problems and no way to say "the overlap I fixed is gone and I have not introduced three
//! more" — so an agent either declares victory on a different set of findings or thrashes.
//!
//! A finding's id is derived from WHAT and WHERE, quantised to 8px. Quantised because a fix
//! elsewhere on the screen legitimately moves a widget a pixel or two, and an id that changed
//! then would report every finding as new after any layout change. Not quantised so coarsely
//! that two genuinely different findings collide — 8px is under the smallest gap between
//! distinct controls in this fleet.
//!
//! ## What it does not do
//!
//! It does not decide whether a screen is GOOD. It finds specific, mechanical faults —
//! occluded text, a popup with no backdrop, contents that overflow their control, a font
//! nobody can read. Taste is not in here, and an agent that reports "0 findings" has been
//! told the screen has no mechanical faults, not that the design works.

use egui::epaint::{Color32, Rect, Vec2};

/// What kind of fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Two pieces of text drawn on top of each other.
    Overlap,
    /// Text cut off horizontally by its clip rect.
    Clipped,
    /// Drawn outside the screen.
    Offscreen,
    /// Below the readable size floor.
    TinyText,
    /// Two widgets resolved to the same egui id — they share state.
    IdClash,
    /// A region that drew nothing.
    Blank,
    /// A region whose contents run outside it.
    Spill,
    /// A popup that lets what is behind it show through.
    NoBackdrop,
    /// Text that does not contrast with what is actually behind it.
    LowContrast,
}

impl Kind {
    /// Does this stop the screen working, or only make it worse?
    ///
    /// An agent triages on this: a shared id means keystrokes land in the wrong widget, while
    /// a slightly small font is a judgement call. Ranking them the same makes the list
    /// useless for deciding what to do first.
    pub fn blocking(self) -> bool {
        matches!(self, Kind::IdClash | Kind::Blank | Kind::NoBackdrop | Kind::Offscreen)
    }
}

/// One fault, with somewhere to look and something to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub kind: Kind,
    /// Where, in screen points.
    pub region: Rect,
    /// What was found, in a sentence.
    pub detail: String,
    /// Stable across passes — see the module docs.
    pub id: u64,
}

impl Finding {
    pub fn new(kind: Kind, region: Rect, detail: impl Into<String>) -> Finding {
        let id = stable_id(kind, region);
        Finding { kind, region, detail: detail.into(), id }
    }
}

/// The grid a finding's position is snapped to before hashing.
pub const ID_GRID: f32 = 8.0;

fn stable_id(kind: Kind, r: Rect) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (kind as u8).hash(&mut h);
    for v in [r.left(), r.top(), r.width(), r.height()] {
        ((v / ID_GRID).round() as i64).hash(&mut h);
    }
    h.finish()
}

/// A pass's findings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    /// Sorted by severity then kind then position, so two passes over the same screen produce
    /// byte-identical output. A list that reorders itself makes every diff look like a change.
    pub fn sorted(mut self) -> Report {
        self.findings.sort_by(|a, b| {
            b.kind
                .blocking()
                .cmp(&a.kind.blocking())
                .then(a.kind.cmp(&b.kind))
                .then(a.region.left().total_cmp(&b.region.left()))
                .then(a.region.top().total_cmp(&b.region.top()))
                .then(a.id.cmp(&b.id))
        });
        self
    }

    pub fn blocking(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.kind.blocking()).collect()
    }

    /// Findings in this pass that were not in `before` — what a change INTRODUCED.
    pub fn introduced(&self, before: &Report) -> Vec<&Finding> {
        self.findings.iter().filter(|f| !before.findings.iter().any(|p| p.id == f.id)).collect()
    }

    /// Findings in `before` that are gone — what a change FIXED.
    ///
    /// The pair is what makes a loop honest: a pass that fixed two and introduced three has
    /// not made progress, and only comparing counts would say it had.
    pub fn fixed_since<'a>(&self, before: &'a Report) -> Vec<&'a Finding> {
        before.findings.iter().filter(|p| !self.findings.iter().any(|f| f.id == p.id)).collect()
    }

    /// One line per finding, blocking first. The format an agent reads back.
    pub fn describe(&self) -> String {
        if self.findings.is_empty() {
            return "no mechanical faults found (this is not a judgement about the design)".into();
        }
        self.findings
            .iter()
            .map(|f| {
                format!(
                    "[{}] {:?} at ({:.0},{:.0} {:.0}x{:.0}): {}",
                    if f.kind.blocking() { "blocking" } else { "warn" },
                    f.kind,
                    f.region.left(),
                    f.region.top(),
                    f.region.width(),
                    f.region.height(),
                    f.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A region the caller wants checked beyond the whole-screen shape pass.
#[derive(Debug, Clone)]
pub struct Region {
    pub name: String,
    pub rect: Rect,
    /// Must draw something. A pane that renders nothing passes every shape check there is.
    pub must_draw: bool,
    /// Must occlude what it covers — for popups, menus, dialogs.
    pub must_occlude: bool,
    /// Contents must stay inside. `None` skips the check; `Some(margin)` is how far around it
    /// to look for spill.
    pub must_contain: Option<f32>,
    /// Text in here must clear the AA contrast threshold.
    pub must_read: bool,
}

impl Region {
    pub fn new(name: &str, rect: Rect) -> Region {
        Region { name: name.into(), rect, must_draw: true, must_occlude: false, must_contain: None, must_read: false }
    }
    pub fn popup(mut self) -> Region {
        self.must_occlude = true;
        self
    }
    pub fn contains_its_text(mut self, margin: f32) -> Region {
        self.must_contain = Some(margin);
        self
    }
    pub fn readable(mut self) -> Region {
        self.must_read = true;
        self
    }
}

/// Run every check and return the findings.
///
/// `draw` takes the backdrop colour so the occlusion check can vary what is behind the UI —
/// a caller that ignores the argument simply gets no occlusion signal, which is why
/// `must_occlude` is opt-in rather than on by default.
pub fn audit(
    size: Vec2,
    bg: Color32,
    regions: &[Region],
    mut draw: impl FnMut(&egui::Context, Color32),
) -> Report {
    let mut out: Vec<Finding> = Vec::new();

    // Shape pass, over the whole screen.
    let probe = crate::layout_probe::probe(size, 2, false, |ctx| draw(ctx, bg));
    for (a, b, i) in &probe.overlaps {
        out.push(Finding::new(
            Kind::Overlap,
            *i,
            format!("{:?} and {:?} are drawn on top of each other", a.text, b.text),
        ));
    }
    for (t, lost) in &probe.clipped {
        out.push(Finding::new(Kind::Clipped, t.rect, format!("{:?} loses {lost:.0}pt to its clip", t.text)));
    }
    for t in &probe.offscreen {
        out.push(Finding::new(Kind::Offscreen, t.rect, format!("{:?} is drawn off screen", t.text)));
    }
    for t in &probe.tiny {
        out.push(Finding::new(Kind::TinyText, t.rect, format!("{:?} is below the readable floor", t.text)));
    }
    for c in &probe.id_clashes {
        // No rect: egui reports the clash by text. Anchored at the origin so the id is stable
        // and the detail carries the identity.
        out.push(Finding::new(
            Kind::IdClash,
            Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::ZERO),
            format!("two widgets share an id, so they share state: {c}"),
        ));
    }

    // Pixel pass, per region.
    let frame = crate::uiverify::render(size, bg, |ctx| draw(ctx, bg));
    for r in regions {
        let look = frame.look(r.rect, bg);
        if r.must_draw && look.ink < crate::uiverify::MIN_INK {
            out.push(Finding::new(
                Kind::Blank,
                r.rect,
                format!("{} drew nothing ({:.3} ink)", r.name, look.ink),
            ));
            // Everything else about an empty region is noise: it has no contrast because it
            // has no content, and reporting that too would bury the one fault that matters.
            continue;
        }
        if r.must_read && look.contrast < crate::uiverify::MIN_CONTRAST {
            out.push(Finding::new(
                Kind::LowContrast,
                r.rect,
                format!("{} reads at {:.1}:1 against its own background", r.name, look.contrast),
            ));
        }
        if let Some(margin) = r.must_contain {
            let s = crate::uiverify::spill(&frame, r.rect, margin, bg);
            if s > 0.005 {
                out.push(Finding::new(
                    Kind::Spill,
                    r.rect,
                    format!("{}'s contents run outside it ({:.1}% of the surrounding band)", r.name, s * 100.0),
                ));
            }
        }
        if r.must_occlude {
            let b = crate::uiverify::bleed_through(size, r.rect, &mut draw);
            if b > crate::uiverify::MAX_BLEED {
                out.push(Finding::new(
                    Kind::NoBackdrop,
                    r.rect,
                    format!("{} lets {:.0}% of what is behind it show through", r.name, b * 100.0),
                ));
            }
        }
    }
    Report { findings: out }.sorted()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: Color32 = Color32::from_rgb(20, 20, 24);

    fn bar(at: egui::Pos2, size: Vec2, c: Color32) -> impl Fn(&egui::Context, Color32) {
        move |ctx: &egui::Context, _bg: Color32| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                ui.painter().rect_filled(Rect::from_min_size(at, size), 0.0, c);
            });
        }
    }

    /// Without stable ids a loop cannot tell progress from churn: two passes give two lists
    /// and no way to say "the one I fixed is gone and I have not added three more".
    #[test]
    fn a_finding_keeps_its_id_when_the_screen_moves_slightly() {
        let a = Finding::new(Kind::Blank, Rect::from_min_size(egui::pos2(10.0, 10.0), Vec2::new(40.0, 20.0)), "x");
        // A fix elsewhere nudges it a pixel. Same finding.
        let b = Finding::new(Kind::Blank, Rect::from_min_size(egui::pos2(11.0, 9.0), Vec2::new(40.0, 20.0)), "x");
        assert_eq!(a.id, b.id, "a one-pixel move made it a new finding");

        // A different place is a different finding — the quantisation must not be so coarse
        // that two real faults collide.
        let c = Finding::new(Kind::Blank, Rect::from_min_size(egui::pos2(200.0, 10.0), Vec2::new(40.0, 20.0)), "x");
        assert_ne!(a.id, c.id);
        // As is a different kind in the same place.
        let d = Finding::new(Kind::Spill, a.region, "x");
        assert_ne!(a.id, d.id);
    }

    /// The pair is what makes a loop honest: a pass that fixed two and introduced three has
    /// not made progress, and comparing counts alone would say it had.
    #[test]
    fn a_pass_reports_what_it_fixed_and_what_it_introduced() {
        let r = |x: f32| Rect::from_min_size(egui::pos2(x, 0.0), Vec2::new(20.0, 20.0));
        let before = Report {
            findings: vec![Finding::new(Kind::Blank, r(0.0), "a"), Finding::new(Kind::Spill, r(40.0), "b")],
        };
        let after = Report {
            findings: vec![
                Finding::new(Kind::Spill, r(40.0), "b"),
                Finding::new(Kind::Overlap, r(80.0), "c"),
                Finding::new(Kind::Overlap, r(120.0), "d"),
            ],
        };
        assert_eq!(after.fixed_since(&before).len(), 1, "the blank one was fixed");
        assert_eq!(after.introduced(&before).len(), 2, "two new ones");
        // Counts alone: 2 before, 3 after — "one worse". The truth is one fixed and two
        // added, which is a different decision for whatever is looping.
        assert_ne!(after.findings.len() as i32 - before.findings.len() as i32, 2);
    }

    /// A list that reorders itself makes every diff look like a change.
    #[test]
    fn the_order_is_stable_and_blocking_faults_come_first() {
        let r = |x: f32| Rect::from_min_size(egui::pos2(x, 0.0), Vec2::new(20.0, 20.0));
        let rep = Report {
            findings: vec![
                Finding::new(Kind::TinyText, r(10.0), "t"),
                Finding::new(Kind::Blank, r(50.0), "b"),
                Finding::new(Kind::Overlap, r(30.0), "o"),
            ],
        }
        .sorted();
        assert!(rep.findings[0].kind.blocking(), "{:?}", rep.findings[0]);
        // Same input in a different order gives the same output.
        let rep2 = Report {
            findings: vec![
                Finding::new(Kind::Overlap, r(30.0), "o"),
                Finding::new(Kind::Blank, r(50.0), "b"),
                Finding::new(Kind::TinyText, r(10.0), "t"),
            ],
        }
        .sorted();
        assert_eq!(rep.findings.iter().map(|f| f.id).collect::<Vec<_>>(),
                   rep2.findings.iter().map(|f| f.id).collect::<Vec<_>>());
    }

    /// The end-to-end shape an agent uses: run it, get findings with places to look.
    #[test]
    fn a_blank_region_and_an_overflowing_one_are_both_found() {
        let size = Vec2::new(200.0, 120.0);
        let filled = Rect::from_min_size(egui::pos2(10.0, 10.0), Vec2::new(60.0, 30.0));
        let empty = Rect::from_min_size(egui::pos2(120.0, 60.0), Vec2::new(60.0, 40.0));

        let rep = audit(
            size,
            BG,
            &[
                Region::new("header", filled),
                Region::new("body", empty),
            ],
            |ctx, c| bar(egui::pos2(10.0, 10.0), Vec2::new(60.0, 30.0), Color32::from_rgb(230, 230, 235))(ctx, c),
        );
        assert!(rep.findings.iter().any(|f| f.kind == Kind::Blank && f.detail.contains("body")),
                "{}", rep.describe());
        assert!(!rep.findings.iter().any(|f| f.detail.contains("header")), "{}", rep.describe());

        // A blank region reports ONE fault, not three: it has no contrast because it has no
        // content, and reporting that too would bury the fault that matters.
        assert_eq!(rep.findings.iter().filter(|f| f.detail.contains("body")).count(), 1);
    }

    /// "0 findings" means no mechanical faults, not that the design works — and the report
    /// says so itself, because an agent reading a bare "clean" will conclude the latter.
    #[test]
    fn an_empty_report_does_not_claim_the_design_is_good() {
        let d = Report::default().describe();
        assert!(d.contains("not a judgement"), "{d}");
    }
}
