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

    /// The page renderer, driven through frames.
    ///
    /// `doc.rs` carried the page model, a parser, `FormState` and `submit`
    /// with NO renderer for the life of the port — the model that exists to
    /// replace web2qml had never been on screen. These live here rather
    /// than in the kit because that is where the harness is, and a
    /// renderer's first tests should be the ones that run it.
    mod page {
        use super::*;
        use uikit_web::doc::{parse_page, FieldKind, FormState, Kind, PageAction};

        const LOGIN: &str = "<form action=\"/in\" method=\"post\">\
            <input type=\"hidden\" name=\"csrf\" value=\"tok123\">\
            <input type=\"text\" name=\"user\" placeholder=\"Username\">\
            <input type=\"password\" name=\"pw\" placeholder=\"Password\">\
            <input type=\"submit\" value=\"Sign in\">\
            </form>";

        #[test]
        fn a_form_submits_what_was_typed_including_the_hidden_token() {
            // The CSRF case this crate's own doc calls out: a hidden field
            // is never drawn and must still submit, because a login that
            // posts without one fails in a way that looks like a wrong
            // password.
            let p = Probe::default();
            let c = p.colors();
            let page = parse_page(LOGIN);
            let mut st = FormState::default();
            // Type into the username field by id — the harness clicks, it
            // does not type, and what is under test is the submission.
            let user = page.fields.iter().copied()
                .find(|id| matches!(page.root.find(*id).map(|n| &n.kind),
                    Some(Kind::Field { name, .. }) if name == "user"))
                .expect("no username field");
            st.set(user, "ada");

            let act = p.click_text("Sign in", |ui|
                uikit_web::doc::render_page(ui, &page, &mut st, &c));
            match act {
                Some(PageAction::Submit(s)) => {
                    assert_eq!(s.action, "/in");
                    assert!(s.query.contains("csrf=tok123"),
                            "the hidden token did not submit: {}", s.query);
                    assert!(s.query.contains("user=ada"), "{}", s.query);
                }
                other => panic!("submit produced {other:?}"),
            }
        }

        #[test]
        fn a_password_field_is_drawn_and_its_value_is_not() {
            // A password echoed into the frame is a password in a
            // screenshot. Asserted on what was PAINTED, which is the only
            // place this can be checked.
            let p = Probe::default();
            let c = p.colors();
            let page = parse_page(LOGIN);
            let mut st = FormState::default();
            let pw = page.fields.iter().copied()
                .find(|id| matches!(page.root.find(*id).map(|n| &n.kind),
                    Some(Kind::Field { kind: FieldKind::Password { .. }, .. })))
                .expect("no password field");
            st.set(pw, "hunter2");
            let (d, _) = p.frame(|ui| uikit_web::doc::render_page(ui, &page, &mut st, &c));
            assert!(!d.has("hunter2"), "the password was painted: {:?}", d.texts);
            // Masked, not absent — the field is drawn, its value is not.
            // (The placeholder is gone precisely BECAUSE there is a value,
            // which is what a hint text means; my first version asserted
            // the opposite and was wrong about egui, not about the code.)
            assert!(d.texts.iter().any(|t| t.chars().all(|ch| ch == '•') && !t.is_empty()),
                    "the password field drew nothing at all: {:?}", d.texts);
        }

        #[test]
        fn a_hidden_field_is_not_drawn_as_a_box() {
            // Drawing it would put a CSRF token on screen as an editable
            // field, which is both wrong and an invitation.
            let p = Probe::default();
            let c = p.colors();
            let page = parse_page(LOGIN);
            let mut st = FormState::default();
            let (d, _) = p.frame(|ui| uikit_web::doc::render_page(ui, &page, &mut st, &c));
            assert!(!d.has("tok123"), "the hidden value was drawn: {:?}", d.texts);
        }

        #[test]
        fn a_link_in_a_page_is_followed_and_an_image_shows_its_alt() {
            // No decoder and no network here, so an image is its alt text
            // — frequently the only description of a chart the page has.
            let p = Probe::default();
            let c = p.colors();
            let page = parse_page(
                "<p><a href=\"/next\">onward</a></p>\
                 <p><img src=\"/chart.png\" alt=\"revenue by quarter\"></p>");
            let mut st = FormState::default();
            let (d, _) = p.frame(|ui| uikit_web::doc::render_page(ui, &page, &mut st, &c));
            assert!(d.has("revenue by quarter"), "the alt text is missing: {:?}", d.texts);

            let act = p.click_text("onward", |ui|
                uikit_web::doc::render_page(ui, &page, &mut st, &c));
            assert_eq!(act, Some(PageAction::Follow("/next".into())));
        }

        /// COLD-GLOVE MODE OVER A WEB PAGE.
        ///
        /// The accessibility case this exists for: someone outdoors in
        /// gloves who cannot aim at a link. Every piece was already here —
        /// `glove::Ratchet` turns tilt into steps, `a11y::A11yScene`
        /// collects targets, `Cursor::step` moves between them — and a
        /// rendered page registered NOTHING, so there was nothing to steer
        /// to. These tests are the proof that the chain now closes.
        mod glove {
            use super::*;
            use uikit_egui::a11y::A11yScene;
            use uikit_web::doc::PageUi;

            const PAGE: &str = "<p><a href=\"/one\">first link</a></p>\
                <p><a href=\"/two\">second link</a></p>\
                <form action=\"/go\"><input type=\"checkbox\" name=\"agree\">\
                <input type=\"submit\" value=\"Continue\"></form>";

            /// Render once and hand back what glove mode can aim at.
            fn targets(p: &Probe, page: &uikit_web::doc::Page, st: &mut uikit_web::doc::FormState)
                -> Vec<uikit_egui::glove::Target> {
                let c = p.colors();
                let mut scene = A11yScene::new();
                scene.begin_frame();
                let mut io = PageUi { a11y: Some(&mut scene), activate: None };
                p.frame(|ui| uikit_web::doc::render_page_with(ui, page, st, &c, &mut io));
                scene.glove_targets()
            }

            #[test]
            fn a_rendered_page_offers_its_actionable_parts_as_glove_targets() {
                // Links, the checkbox and the submit — and NOT the prose,
                // because a cursor that snaps to paragraphs is a cursor
                // that never reaches the link between them.
                let p = Probe::default();
                let page = uikit_web::doc::parse_page(PAGE);
                let mut st = uikit_web::doc::FormState::default();
                let ts = targets(&p, &page, &mut st);
                let labels: Vec<&str> = ts.iter().map(|t| t.label.as_str()).collect();
                assert!(labels.contains(&"first link"), "{labels:?}");
                assert!(labels.contains(&"second link"), "{labels:?}");
                assert!(labels.contains(&"Continue"), "{labels:?}");
                assert_eq!(ts.len(), 4, "expected 2 links, a checkbox and a submit: {labels:?}");
                // Every target has somewhere to be, or the cursor cannot
                // travel to it.
                assert!(ts.iter().all(|t| t.at.1 > 0.0), "a target has no position: {ts:?}");
            }

            #[test]
            fn stepping_the_cursor_walks_the_page_and_activating_follows_the_link() {
                // The whole gesture, without a pointer: step to a target,
                // activate it by id, get the navigation.
                let p = Probe::default();
                let c = p.colors();
                let page = uikit_web::doc::parse_page(PAGE);
                let mut st = uikit_web::doc::FormState::default();
                let ts = targets(&p, &page, &mut st);

                let mut cur = uikit_egui::glove::GloveCursor::default();
                let first = cur.step(&ts, 1).expect("nothing to step to");
                let second = cur.step(&ts, 1).expect("the cursor did not move on");
                assert_ne!(first, second, "stepping stayed on the same target");

                // Activate whatever is snapped, by identity — a fake click
                // has to land somewhere, and "somewhere" is what a person
                // who cannot aim does not have.
                let id: u32 = second.parse().expect("target ids are node ids");
                let mut scene = A11yScene::new();
                scene.begin_frame();
                let mut io = PageUi {
                    a11y: Some(&mut scene),
                    activate: Some(uikit_web::doc::NodeId(id)),
                };
                let (_, act) = p.frame(|ui|
                    uikit_web::doc::render_page_with(ui, &page, &mut st, &c, &mut io));
                assert!(matches!(act, Some(uikit_web::doc::PageAction::Follow(_))),
                        "activating a snapped link produced {act:?}");
            }

            #[test]
            fn activating_a_checkbox_toggles_it_without_a_tap() {
                // The control a gloved hand is least able to hit and most
                // likely to need: consent boxes gate everything.
                let p = Probe::default();
                let c = p.colors();
                let page = uikit_web::doc::parse_page(PAGE);
                let mut st = uikit_web::doc::FormState::default();
                let cb = page.fields.iter().copied()
                    .find(|id| matches!(page.root.find(*id).map(|n| &n.kind),
                        Some(uikit_web::doc::Kind::Field {
                            kind: uikit_web::doc::FieldKind::Checkbox, .. })))
                    .expect("no checkbox");
                assert!(!st.checked(cb));
                let mut io = PageUi { a11y: None, activate: Some(cb) };
                p.frame(|ui| uikit_web::doc::render_page_with(ui, &page, &mut st, &c, &mut io));
                assert!(st.checked(cb), "activation did not toggle the box");
            }

            #[test]
            fn an_activation_fires_once_and_not_every_frame() {
                // Left set, a submit button posts the same form sixty times
                // a second. The request is consumed when it is used.
                let p = Probe::default();
                let c = p.colors();
                let page = uikit_web::doc::parse_page(PAGE);
                let mut st = uikit_web::doc::FormState::default();
                let cb = page.fields.iter().copied()
                    .find(|id| matches!(page.root.find(*id).map(|n| &n.kind),
                        Some(uikit_web::doc::Kind::Field {
                            kind: uikit_web::doc::FieldKind::Checkbox, .. })))
                    .unwrap();
                let mut io = PageUi { a11y: None, activate: Some(cb) };
                p.frame(|ui| uikit_web::doc::render_page_with(ui, &page, &mut st, &c, &mut io));
                assert!(st.checked(cb));
                assert_eq!(io.activate, None, "the request was not consumed");
                // A second pass with the same (now-cleared) io must not
                // toggle it back.
                p.frame(|ui| uikit_web::doc::render_page_with(ui, &page, &mut st, &c, &mut io));
                assert!(st.checked(cb), "the box toggled twice from one request");
            }
        }

        #[test]
        fn a_submit_button_outside_a_form_posts_nothing() {
            // What a browser does. Inventing a target would send the page's
            // fields somewhere nobody asked for.
            let p = Probe::default();
            let c = p.colors();
            let page = parse_page("<input type=\"submit\" value=\"Go\">");
            let mut st = FormState::default();
            let act = p.click_text("Go", |ui|
                uikit_web::doc::render_page(ui, &page, &mut st, &c));
            assert_eq!(act, None);
        }
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
