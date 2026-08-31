//! frame_probe — run a renderer through a real egui frame, in a test.
//!
//! Written after a frame test found that five lists in this app drew rows
//! nobody could click: each built a row with `ui.horizontal(…).response`
//! and re-interacted it for a click, which registers no clickable region.
//! Every model test passed the whole time. The lesson is not "test the
//! model harder" — it is that a renderer nobody runs is a renderer nobody
//! has checked.
//!
//! So this exists to make running one cheap, because the alternative is
//! four copies of the same twenty lines and the copies stop being made.
//!
//! ## The two mistakes this encodes
//!
//! Both were mine, in the first frame test I wrote, and both produce a
//! test that looks like it clicks and does not:
//!
//! * **Click where the thing actually is.** [`Probe::pos_of`] asks the
//!   frame where a piece of text landed. Clicking a guessed coordinate is
//!   how a UI test passes on a layout it never checked, or fails for a
//!   reason unrelated to the behaviour under test.
//! * **Press and release in separate frames.** egui decides a click from
//!   state that spans frames. [`Probe::click`] does the whole sequence —
//!   move, press, release — one frame at a time.

use egui::{Context, Event, PointerButton, Pos2, RawInput};

/// What one frame drew.
#[derive(Debug, Clone, Default)]
pub struct Drawn {
    /// Every text shape, in paint order.
    pub texts: Vec<String>,
    /// The same texts with where they were painted.
    pub placed: Vec<(String, Pos2)>,
}

impl Drawn {
    pub fn has(&self, needle: &str) -> bool {
        self.texts.iter().any(|t| t.contains(needle))
    }

    /// How many text shapes contain this. Useful for the failure a model
    /// test cannot see: the same row drawn twice.
    pub fn count(&self, needle: &str) -> usize {
        self.texts.iter().filter(|t| t.contains(needle)).count()
    }
}

/// A context plus a surface size, driving one renderer repeatedly.
pub struct Probe {
    ctx: Context,
    size: egui::Vec2,
}

impl Default for Probe {
    fn default() -> Self {
        Probe::new(400.0, 800.0)
    }
}

impl Probe {
    /// A phone-shaped surface by default — the form factor most of these
    /// views exist for, and the one where a layout runs out of room.
    pub fn new(w: f32, h: f32) -> Probe {
        Probe { ctx: Context::default(), size: egui::vec2(w, h) }
    }

    pub fn colors(&self) -> uikit_egui::PaletteColors {
        uikit_egui::palette_colors("voice")
    }

    /// Run one frame with the given input events.
    pub fn frame_with<R>(&self, events: Vec<Event>,
                         mut body: impl FnMut(&mut egui::Ui) -> R) -> (Drawn, R) {
        let mut out = None;
        let done = self.ctx.run(RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, self.size)),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = Some(body(ui));
            });
        });
        let mut d = Drawn::default();
        for s in done.shapes.iter() {
            if let egui::epaint::Shape::Text(t) = &s.shape {
                d.texts.push(t.galley.text().to_string());
                d.placed.push((t.galley.text().to_string(), t.pos));
            }
        }
        (d, out.expect("the panel body did not run"))
    }

    /// Run one frame with no input.
    pub fn frame<R>(&self, body: impl FnMut(&mut egui::Ui) -> R) -> (Drawn, R) {
        self.frame_with(Vec::new(), body)
    }

    /// Where a piece of text was painted, if it was.
    pub fn pos_of(&self, drawn: &Drawn, needle: &str) -> Option<Pos2> {
        drawn.placed.iter().find(|(t, _)| t.contains(needle)).map(|(_, p)| *p)
    }

    /// Move, press, release — one frame each — and return whatever the
    /// renderer produced on the frame that saw the release.
    ///
    /// Several release frames are attempted because a widget laid out
    /// inside a `ScrollArea` may need one more pass before its rect is
    /// where the click is; stopping at the first `Some` keeps a test that
    /// works from depending on how many that took.
    pub fn click<R>(&self, at: Pos2, mut body: impl FnMut(&mut egui::Ui) -> Option<R>)
        -> Option<R> {
        let btn = |pressed: bool| Event::PointerButton {
            pos: at, button: PointerButton::Primary, pressed,
            modifiers: Default::default(),
        };
        self.frame_with(vec![Event::PointerMoved(at)], |ui| { body(ui); });
        self.frame_with(vec![btn(true)], |ui| { body(ui); });
        for _ in 0..3 {
            let (_, r) = self.frame_with(vec![btn(false)], &mut body);
            if r.is_some() {
                return r;
            }
        }
        None
    }

    /// Draw once to find `needle`, then click it. The whole pattern, since
    /// it is the one every list test wants.
    pub fn click_text<R>(&self, needle: &str, mut body: impl FnMut(&mut egui::Ui) -> Option<R>)
        -> Option<R> {
        let (drawn, _) = self.frame(|ui| { body(ui); });
        let p = self.pos_of(&drawn, needle)
            .unwrap_or_else(|| panic!("{needle:?} was never drawn: {:?}", drawn.texts));
        // The y is trustworthy, the x is not — see the module note. Sweep
        // the row, left to right, and stop at the first hit.
        let y = p.y + 4.0;
        let mut x = 4.0;
        while x < self.size.x {
            if let Some(r) = self.click(Pos2::new(x, y), &mut body) {
                return Some(r);
            }
            x += 6.0;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_sees_what_was_drawn_and_where() {
        let p = Probe::default();
        let (d, ()) = p.frame(|ui| { ui.label("hello"); });
        assert!(d.has("hello"));
        assert!(p.pos_of(&d, "hello").is_some());
        assert!(!d.has("goodbye"));
    }

    #[test]
    fn a_click_on_a_button_reaches_it() {
        // The harness proving itself: if this fails, every test built on it
        // is measuring the harness rather than the renderer.
        let p = Probe::default();
        let hit = p.click_text("press me", |ui| {
            if ui.button("press me").clicked() { Some(()) } else { None }
        });
        assert!(hit.is_some(), "the harness cannot click a plain button");
    }

    #[test]
    fn a_link_in_a_rendered_document_can_be_followed() {
        // Isolating the renderer from any view that embeds it: if this
        // fails, a failing manpage test is about `uikit_web::render`, not
        // about the manual.
        let p = Probe::default();
        let c = p.colors();
        let doc = uikit_web::parse_html("<p>see <a href=\"#sed\">sed</a></p>");
        let href = p.click_text("sed", |ui| uikit_web::render(ui, &doc, &c));
        assert_eq!(href.as_deref(), Some("#sed"));
    }

    #[test]
    fn a_click_somewhere_else_does_not_reach_it() {
        // The other half: a harness that reports a click everywhere is
        // worse than one that reports none, because it passes.
        let p = Probe::default();
        let hit = p.click(Pos2::new(390.0, 780.0), |ui| {
            if ui.button("press me").clicked() { Some(()) } else { None }
        });
        assert!(hit.is_none(), "a click far from the button still hit it");
    }
}
