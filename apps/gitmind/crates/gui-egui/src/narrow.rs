//! narrow — what a 360-point-wide screen needs, in one place.
//!
//! A photo of gitmind on a Fairphone showed the Network page with the `user` field stretched
//! across the row and the `host` field pushed off the right edge: a label with no box after
//! it. `tests/phone_layout.rs` then found the same shape on four pages — three descriptions
//! wider than the screen (one by 298 points), a "look up" button placed at x = 360.9 on a
//! 360-wide screen, and text drawn on top of other text.
//!
//! One cause, three symptoms:
//!
//! **`ui.horizontal()` does not wrap.** Anything past the right edge is still drawn — off
//! the screen, or on top of whatever else landed there. It is not an error and nothing
//! fails; the control is simply unusable, and the overlapping glyphs are what makes a label
//! read as noise. (logscope's `mobilelayout.rs` recorded the same finding; this is that rule
//! applied here.)
//!
//! **A `Label` only wraps if it is told to.** A one-line description of a daemon is fine on
//! a laptop and runs off a phone.
//!
//! **A `TextEdit` with no width takes what it likes.** Two of them in one row means the
//! second is off-screen. And a field that lands right at a wrap threshold is worse than
//! ugly: typing a character measures wider, flips the wrap, moves the widget, changes its
//! auto-id, and egui drops focus — the field takes one character and ignores the rest. An
//! explicit `id_salt` holds focus through that; sizing the field so the row is not sitting
//! on the threshold stops it happening at all.

/// Below this width, a row is a phone row: it wraps, and descriptions go on their own line.
pub const NARROW_PT: f32 = 520.0;

/// Is this a narrow screen?
pub fn is_narrow(ui: &egui::Ui) -> bool {
    ui.available_width() < NARROW_PT
}

/// Secondary text that always wraps and never widens its parent.
///
/// `ui.label(RichText::new(long).small())` inside a horizontal row does neither, which is
/// how a 447-point description ended up on a 360-point screen.
pub fn desc(ui: &mut egui::Ui, text: &str) {
    // `Label::wrap()` and nothing else. An earlier version also poked at the returned rect,
    // which does nothing (the Response is a temporary) and hid what the wrap was doing.
    //
    // NOT `.small()` on a phone. Small resolves to 9pt here, under the readable floor the
    // adaptive layer sets for compact screens — and 9pt weak grey on a dark panel is what
    // the photo shows as an unreadable smear under the keyboard. Weak, at body size, on a
    // narrow screen; small stays on a laptop where it is 12pt and fine.
    let rich = egui::RichText::new(text).weak();
    let rich = if is_narrow(ui) { rich } else { rich.small() };
    // Wrapped to a width a hair under what is available. `Label::wrap()` wraps at exactly
    // `available_width`, and inside a scroll area that is a point or two wider than the
    // CLIP rect — so the last line of a wrapped paragraph was still being reported as cut
    // at the right edge. The margin is the scrollbar's, and it costs one character.
    // Wrapped to a width a hair under what is available. `Label::wrap()` wraps at exactly
    // `available_width`, and inside a scroll area that is a point or two wider than the
    // CLIP rect — so the last line of a wrapped paragraph was still reported as cut at the
    // right edge. The margin is the scrollbar's, and it costs one character.
    // To the CLIP's right edge, not `available_width`. On the Fleet page the panel's
    // max_rect runs a few points past the screen, so `available_width` was 365 on a
    // 360-point phone and a paragraph wrapped at 359 still ended up 7 points over the
    // edge. The clip rect is what is actually visible.
    let room = ui.clip_rect().right() - ui.cursor().left() - 6.0;
    let max = room.min(ui.available_width() - 6.0).max(48.0);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = max;
    let (font, color) = (
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().weak_text_color(),
    );
    let font = if is_narrow(ui) { font } else { egui::TextStyle::Small.resolve(ui.style()) };
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
    let _ = rich;
    // LAID OUT HERE, not by the Label. `Label::new(job)` re-wraps the job at the widget's
    // own available width and throws `job.wrap.max_width` away — which is why a paragraph
    // wrapped at 346 still came out 359 wide on the Fleet page, where available_width runs
    // past the screen. A pre-laid galley is used verbatim.
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    ui.add(egui::Label::new(galley));
}

/// A chip: a short label on a tinted background.
///
/// Painted as a background-coloured text run rather than a `Frame`. A `Frame` nested inside
/// `horizontal_wrapped` did not contribute its height to the row, so the next thing drawn
/// landed on top of the row — this module's own test caught the title and the chip both
/// overlapping the description by a full line.
pub fn chip(ui: &mut egui::Ui, text: &str, fill: egui::Color32, fg: egui::Color32) {
    ui.label(
        egui::RichText::new(format!(" {text} "))
            .small()
            .color(fg)
            .background_color(fill),
    );
}

/// A `name · chip · description` row that survives a narrow screen: on a phone the
/// description drops to its own wrapped line under the name, on a laptop it stays inline.
pub fn titled_row(
    ui: &mut egui::Ui,
    title: &str,
    chip: Option<(&str, egui::Color32, egui::Color32)>,
    description: &str,
    mut trailing: impl FnMut(&mut egui::Ui),
) {
    let narrow = is_narrow(ui);
    ui.vertical(|ui| {
        // TITLE AND CHIP IN ONE GALLEY. Two labels side by side in a `horizontal_wrapped`
        // came out on top of each other — a `Frame`-wrapped chip contributed no height to
        // the row, and replacing the frame with a background-coloured label still put the
        // chip at the title's position. One text run cannot overlap itself, and the chip
        // keeps its colours as a `TextFormat`. (Same trick as the step strip in snapclone,
        // for the same reason.)
        let body = egui::TextStyle::Body.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        job.append(
            title,
            0.0,
            egui::TextFormat { font_id: body.clone(), color: ui.visuals().text_color(), ..Default::default() },
        );
        if let Some((text, fill, fg)) = chip {
            job.append(
                &format!("  {text} "),
                0.0,
                egui::TextFormat {
                    font_id: small.clone(),
                    color: fg,
                    background: fill,
                    ..Default::default()
                },
            );
        }
        if !narrow && !description.is_empty() {
            job.append(
                &format!("   {description}"),
                0.0,
                egui::TextFormat {
                    font_id: small,
                    color: ui.visuals().weak_text_color(),
                    ..Default::default()
                },
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(job).wrap());
            trailing(ui);
        });
        if narrow && !description.is_empty() {
            desc(ui, description);
        }
    });
}

/// A labelled text field sized to fit beside its label, in a row that wraps.
///
/// `frac` is how much of the row the field may take (0..1). The explicit `id_salt` is the
/// part that matters most: it is what makes focus survive a reflow, and without it a field
/// at the wrap threshold takes one character and then ignores the keyboard.
pub fn field(ui: &mut egui::Ui, label: &str, id: &str, buf: &mut String, frac: f32) -> egui::Response {
    let avail = ui.available_width();
    let width = (avail * frac.clamp(0.1, 1.0)).clamp(80.0, 320.0);
    ui.label(label);
    ui.add(egui::TextEdit::singleline(buf).id_salt(id).desired_width(width))
}

/// Two labelled fields that share a row on a laptop and stack on a phone.
///
/// Stacked rather than wrapped: a `horizontal_wrapped` row whose second field wrapped put
/// the two labels 32 points on top of each other — the row's height was measured before the
/// wrap moved the field down. A phone has the vertical room; it does not have the width.
pub fn field_pair(
    ui: &mut egui::Ui,
    a: (&str, &str, &mut String),
    b: (&str, &str, &mut String),
) {
    if is_narrow(ui) {
        ui.horizontal(|ui| field(ui, a.0, a.1, a.2, 0.62));
        ui.horizontal(|ui| field(ui, b.0, b.1, b.2, 0.62));
    } else {
        ui.horizontal(|ui| {
            field(ui, a.0, a.1, a.2, 0.3);
            field(ui, b.0, b.1, b.2, 0.3);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::layout_probe;

    fn probe(size: egui::Vec2, mut draw: impl FnMut(&mut egui::Ui)) -> layout_probe::Report {
        layout_probe::probe(size, 2, true, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draw(ui));
        })
    }

    /// The shape from the photo: two labelled fields in one row on a 360-wide screen.
    #[test]
    fn two_fields_in_a_row_both_stay_on_a_phone_screen() {
        let (mut a, mut b) = ("user".to_string(), "host".to_string());
        let r = probe(egui::vec2(360.0, 400.0), |ui| {
            ui.horizontal_wrapped(|ui| {
                field(ui, "user", "t-user", &mut a, 0.35);
                field(ui, "host", "t-host", &mut b, 0.35);
            });
        });
        assert!(r.offscreen.is_empty(), "{}", r.describe());
        assert!(r.clipped.is_empty(), "{}", r.describe());
        assert!(r.id_clashes.is_empty(), "{}", r.describe());
        // The un-fixed row cannot be caught here: the probe reads TEXT shapes, and an empty
        // field paints none, so a field pushed off the edge is invisible to it. What it
        // catches is the LABEL that follows the over-wide field, which is why the
        // reproduction lives at page level in `tests/phone_layout.rs` — on the real Network
        // page the second label had a whole daemon description behind it.
        //
        // What this asserts instead is the property the helper promises: both fields fit,
        // both are on screen, and neither shares an id with the other.
        let w: Vec<f32> = vec![360.0, 420.0, 1000.0];
        for width in w {
            let (mut a, mut b) = ("user".to_string(), "host".to_string());
            let r = probe(egui::vec2(width, 400.0), |ui| {
                ui.horizontal_wrapped(|ui| {
                    field(ui, "user", "t-user", &mut a, 0.35);
                    field(ui, "host", "t-host", &mut b, 0.35);
                });
            });
            assert!(r.offscreen.is_empty() && r.clipped.is_empty() && r.id_clashes.is_empty(),
                    "at {width}pt: {}", r.describe());
        }
    }

    /// The photo's row. Stacked on a phone, side by side on a laptop, and never on top of
    /// each other — a wrapped row measured its height before the wrap and overlapped the
    /// two labels by a full field.
    #[test]
    fn a_field_pair_stacks_on_a_phone_and_shares_a_row_on_a_laptop() {
        for width in [360.0f32, 420.0, 1000.0] {
            let (mut u, mut h) = ("user".to_string(), "host".to_string());
            let r = probe(egui::vec2(width, 400.0), |ui| {
                field_pair(ui, ("user", "p-user", &mut u), ("host", "p-host", &mut h));
            });
            assert!(r.overlaps.is_empty(), "at {width}pt: {}", r.describe());
            assert!(r.offscreen.is_empty() && r.clipped.is_empty(), "at {width}pt: {}", r.describe());
            assert!(r.id_clashes.is_empty(), "at {width}pt: {}", r.describe());
        }
    }

    #[test]
    fn a_long_description_wraps_instead_of_running_off() {
        let long = "logscope distributed fanout — broadcaster discovery + remote worker dispatch \
                    (fuzz/summarize across devices)";
        let r = probe(egui::vec2(360.0, 400.0), |ui| desc(ui, long));
        assert!(r.clipped.is_empty() && r.offscreen.is_empty(), "{}", r.describe());
    }

    #[test]
    fn a_titled_row_drops_its_description_below_on_a_phone_and_keeps_it_inline_on_a_laptop() {
        let long = "voice intent routing (voiced-core: registry + matcher)";
        let chip = Some(("voiced-core", egui::Color32::from_rgb(0x2a, 0x3a, 0x5a), egui::Color32::WHITE));
        let phone = probe(egui::vec2(360.0, 400.0), |ui| {
            titled_row(ui, "● voiced", chip, long, |_| {});
        });
        assert!(phone.clipped.is_empty() && phone.offscreen.is_empty(), "{}", phone.describe());
        assert!(phone.overlaps.is_empty(), "{}", phone.describe());
        let laptop = probe(egui::vec2(1000.0, 400.0), |ui| {
            titled_row(ui, "● voiced", chip, long, |_| {});
        });
        assert!(laptop.clean(), "{}", laptop.describe());
        // inline on a laptop means fewer rows: the description shares the title's line
        assert!(laptop.texts <= phone.texts);
    }

#[cfg(test)]
mod focus_survives_reflow {
    use super::*;

    /// Render `field_pair`'s two layouts by hand around the REAL `field`, and
    /// return field b's response. `narrow` = each field in its own row (phone);
    /// otherwise both in one row (laptop). This is exactly the parent change a
    /// width flip causes.
    fn frame(ctx: &egui::Context, narrow: bool, a: &mut String, b: &mut String,
             focus_b: bool) -> egui::Response {
        let mut out = None;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = if narrow {
                    ui.horizontal(|ui| { field(ui, "a", "t-a", a, 0.6); });
                    ui.horizontal(|ui| field(ui, "b", "t-b", b, 0.6)).inner
                } else {
                    ui.horizontal(|ui| { field(ui, "a", "t-a", a, 0.3); field(ui, "b", "t-b", b, 0.3) }).inner
                };
                if focus_b { r.request_focus(); }
                out = Some(r);
            });
        });
        out.unwrap()
    }

    /// A field keeps its id and focus when its ROW LAYOUT changes.
    ///
    /// Written to reproduce the virtual-keyboard bug, on the theory that
    /// `id_salt` is relative to the parent `Ui` and `field_pair` moves field b
    /// between parents. IT PASSED ON THE UNFIXED CODE, disproving that: egui's
    /// anonymous child `Ui`s (`horizontal`) INHERIT their parent's stable id, so
    /// `id_salt` is relative only to the nearest `Ui` with an EXPLICIT id. Kept
    /// as a guard, since it asserts a property the phone layout depends on.
    #[test]
    fn a_focused_field_keeps_focus_when_its_row_layout_changes() {
        let ctx = egui::Context::default();
        let (mut a, mut b) = (String::new(), String::new());
        let r0 = frame(&ctx, true, &mut a, &mut b, true);   // focus b, phone layout
        let r1 = frame(&ctx, true, &mut a, &mut b, false);  // steady
        assert!(r1.has_focus(), "baseline: b holds focus in a steady layout");
        let r2 = frame(&ctx, false, &mut a, &mut b, false); // layout flips
        assert_eq!(r2.id, r0.id, "b's id changed when its row changed");
        assert!(r2.has_focus(), "b lost focus when its row changed");
    }
}

#[cfg(test)]
mod multi_char_input {
    use super::*;

    /// Feed `events` to the real `field`, focused, and return what it holds.
    fn type_into(events: Vec<egui::Event>) -> String {
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)));
        let mut run = |events: Vec<egui::Event>, focus: bool, buf: &mut String| {
            let input = egui::RawInput { screen_rect: screen, events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = field(ui, "x", "t-mc", buf, 0.5);
                    if focus { r.request_focus(); }
                });
            });
        };
        run(vec![], true, &mut buf);   // focus it
        run(vec![], false, &mut buf);  // settle
        run(events, false, &mut buf);  // type
        buf
    }

    /// Per-key text, as a keycode-based virtual keyboard sends it.
    #[test]
    fn one_event_per_character_is_accepted() {
        let evs = "hello".chars().map(|c| egui::Event::Text(c.to_string())).collect();
        assert_eq!(type_into(evs), "hello");
    }

    /// A WHOLE WORD in one event, as swipe typing or a prediction commit sends
    /// it. The literal reading of the report: "inputs longer than one character".
    #[test]
    fn a_multi_character_text_event_is_accepted_whole() {
        assert_eq!(type_into(vec![egui::Event::Text("hello".into())]), "hello");
    }

    /// An IME composition committing a word, as a Wayland text-input keyboard
    /// (squeekboard, and any on-screen keyboard using that protocol) sends it.
    #[test]
    fn an_ime_commit_of_a_word_is_accepted_whole() {
        use egui::{Event, ImeEvent};
        let evs = vec![
            Event::Ime(ImeEvent::Enabled),
            Event::Ime(ImeEvent::Preedit("hel".into())),
            Event::Ime(ImeEvent::Commit("hello".into())),
        ];
        assert_eq!(type_into(evs), "hello");
    }
}
}
