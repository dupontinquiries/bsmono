//! callout — a message with a severity, built so it cannot be mistaken for the
//! text around it.
//!
//! The failure this replaces, seen in stoat's first Stack page: an error drawn as
//! a card like every other card — same dark fill, same hairline border — with its
//! title in flat red and its notes in the same small weak text as the location
//! line above it and the source below it. On a phone the three ran together into
//! one grey column with a red sentence in it. The most important thing on the
//! screen had the least separation from everything else.
//!
//! What makes a callout read as a different KIND of thing, all of it here so an
//! app does not re-derive it:
//!
//! * **a tinted surface**, the severity colour mixed into the card fill (mixed, not
//!   alpha-blended: the result is opaque, so its contrast does not depend on
//!   whatever backdrop is behind it, and a test can check it for every palette);
//! * **an accent stripe** down the leading edge, the same signal the kit's toasts
//!   use, so severity reads the same in both;
//! * **a border in the severity colour** rather than the page's hairline, and a
//!   soft shadow, so the card sits above the page instead of being ruled into it;
//! * **a chip** carrying the severity word and code, so the TITLE can be in the
//!   normal text colour at a size you can read — red body text on a dark ground is
//!   the lowest-contrast way to say "this matters";
//! * **tagged lines** (`note`, `help`) with the tag in its own colour and the text
//!   at body size, with real space between them.
//!
//! It draws; it does not decide. The app supplies the severity, the words and the
//! lines.

use crate::toast::Severity;
use crate::tokens::{FONT_BASE, FONT_LG, FONT_SM, RADIUS_MD, SPACE_MD, SPACE_SM, SPACE_XS};
use crate::PaletteColors;
use egui::{Color32, CornerRadius, RichText, Stroke};

/// Width of the leading accent stripe.
pub const STRIPE_W: f32 = 4.0;
/// How much of the severity colour goes into the surface.
pub const FILL_MIX: f32 = 0.13;
/// …and into the border.
pub const BORDER_MIX: f32 = 0.55;

/// One body line. `tag` is the short word in front ("note", "help", "hint").
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub tag: String,
    pub text: String,
    /// A line that tells you what to DO. Its tag takes the success colour, so the
    /// way out is findable at a glance among the explanations.
    pub actionable: bool,
}

impl Line {
    pub fn note(text: impl Into<String>) -> Line {
        Line { tag: "note".into(), text: text.into(), actionable: false }
    }

    pub fn help(text: impl Into<String>) -> Line {
        Line { tag: "help".into(), text: text.into(), actionable: true }
    }
}

/// Opaque linear mix of two colours. `t` = 0 is `a`, 1 is `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// The severity's colour in this palette. Info is the accent: it still needs to
/// read as a callout, just not as a problem.
pub fn accent(sev: Severity, c: &PaletteColors) -> Color32 {
    match sev {
        Severity::Error => c.danger,
        Severity::Warning => c.warning,
        Severity::Success => c.success,
        Severity::Info => c.accent,
    }
}

pub fn fill(sev: Severity, c: &PaletteColors) -> Color32 {
    mix(c.bg_card, accent(sev, c), FILL_MIX)
}

/// The border has ONE job - be seen against the surface - and a fixed mix does
/// not do it in every palette: in `earth` the danger colour and the card are so
/// close in luminance that a 55% mix came out at 1.28:1. So the mix is raised
/// until the edge is visible, and past the pure severity colour it leans on the
/// text colour, which is by construction far from the surface.
pub fn border(sev: Severity, c: &PaletteColors) -> Color32 {
    let (surface, tone) = (fill(sev, c), accent(sev, c));
    let candidates = [BORDER_MIX, 0.7, 0.85, 1.0].into_iter().map(|t| mix(c.bg_card, tone, t)).chain([0.25, 0.5, 0.75].into_iter().map(|t| mix(tone, c.text, t)));
    let mut last = tone;
    for b in candidates {
        last = b;
        if crate::uiverify::contrast(b, surface) >= MIN_EDGE_CONTRAST {
            break;
        }
    }
    last
}

/// What an edge needs against the surface it bounds to be seen at all.
pub const MIN_EDGE_CONTRAST: f32 = 1.5;

/// The most readable of the palette's own extremes (then plain black or white)
/// on top of `on`. A chip's text was `bg_base`, which is right on a dark palette
/// and unreadable on a light one, where the base is as pale as the chip.
pub fn ink_on(on: Color32, c: &PaletteColors) -> Color32 {
    [c.bg_base, c.text, Color32::BLACK, Color32::WHITE].into_iter().max_by(|a, b| crate::uiverify::contrast(*a, on).total_cmp(&crate::uiverify::contrast(*b, on))).unwrap_or(c.text)
}

/// Body-text readability is the test the tint is not allowed to fail (4.5:1, AA). The palette's own
/// text colour is used whenever it passes on the tinted surface, so a callout reads like the rest
/// of the screen; where it does not, the most readable of the palette's extremes takes over, the way
/// [`ink_on`] already does for the chip. `slate` is the case: its text is only 3.3:1 on its OWN card
/// (#4b4b4b on #8aa2bf, a palette-level shortfall), and the danger tint takes that to 2.9:1 — no
/// mix ratio recovers it, since the text is the problem, not the tint.
pub const MIN_BODY_CONTRAST: f32 = 4.5;

pub fn ink(sev: Severity, c: &PaletteColors) -> Color32 {
    let surface = fill(sev, c);
    if crate::uiverify::contrast(c.text, surface) >= MIN_BODY_CONTRAST { c.text } else { ink_on(surface, c) }
}

pub fn word(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "ERROR",
        Severity::Warning => "WARNING",
        Severity::Success => "OK",
        Severity::Info => "INFO",
    }
}

/// Draw it, full width, with `SPACE_SM` after. `code` goes in the chip beside the
/// severity word ("panic", "SIGSEGV", "E0308"); empty for none.
pub fn show(ui: &mut egui::Ui, c: &PaletteColors, sev: Severity, code: &str, title: &str, lines: &[Line]) -> egui::Response {
    draw(ui, c, sev, code, title, lines, None)
}

/// The same, with a body the person can fold away. A callout that explains a
/// stop is worth a third of a phone screen ONCE; after it has been read it is a
/// third of the screen between you and what you came to look at. Folded, it is
/// the chip and the title - still unmistakably there. Tapping the header toggles;
/// the app may also close it itself (stoat does, when you walk to another frame).
pub fn show_collapsible(ui: &mut egui::Ui, c: &PaletteColors, sev: Severity, code: &str, title: &str, lines: &[Line], open: &mut bool) -> egui::Response {
    draw(ui, c, sev, code, title, lines, Some(open))
}

fn draw(ui: &mut egui::Ui, c: &PaletteColors, sev: Severity, code: &str, title: &str, lines: &[Line], mut open: Option<&mut bool>) -> egui::Response {
    let tone = accent(sev, c);
    let text_ink = ink(sev, c);
    let expanded = open.as_deref().copied().unwrap_or(true);
    let lines = if expanded { lines } else { &[] };
    let folds = open.is_some();
    let frame = egui::Frame::new()
        .fill(fill(sev, c))
        .stroke(Stroke::new(1.0_f32, border(sev, c)))
        .corner_radius(CornerRadius::same(RADIUS_MD as u8))
        .shadow(egui::Shadow { offset: [0, 3], blur: 10, spread: 0, color: Color32::from_black_alpha(70) })
        // Room for the stripe on the leading edge, then the same breathing space
        // the kit's cards have.
        .inner_margin(egui::Margin { left: (SPACE_MD as f32 + STRIPE_W) as i8, right: SPACE_MD as i8, top: SPACE_SM as i8 + 2, bottom: SPACE_SM as i8 + 2 });
    let r = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = SPACE_XS as f32;

        // The chip: severity word and code, dark on the severity colour.
        let chip_text = if code.is_empty() { word(sev).to_string() } else { format!("{}  {code}", word(sev)) };
        let top = ui.cursor().top();
        // A row is at least `interact_size.y` tall, and on a touch palette that is
        // 44pt: the chip came out as a 40pt slab (seen on screen). A chip is a
        // label, not a target - the whole header is the target. The row's height is
        // fixed when the row is CREATED, so this has to be set on the parent first;
        // setting it inside the row changed nothing (also seen on screen).
        let row_floor = std::mem::replace(&mut ui.spacing_mut().interact_size.y, 0.0);
        ui.horizontal(|ui| {
            egui::Frame::new().fill(tone).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(chip_text).size(FONT_SM as f32).strong().color(ink_on(tone, c)));
            });
            if folds {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(if expanded { "less" } else { "more" }).size(FONT_SM as f32).color(tone));
                });
            }
        });
        ui.spacing_mut().interact_size.y = row_floor;
        ui.add(egui::Label::new(RichText::new(title).size(FONT_LG as f32).strong().color(text_ink)).wrap());
        if let Some(open) = open.as_deref_mut() {
            // The whole header is the target, not the word: a thumb aims at the message.
            let header = egui::Rect::from_min_max(egui::pos2(ui.min_rect().left(), top), egui::pos2(ui.min_rect().right(), ui.cursor().top()));
            if ui.interact(header, ui.id().with("callout-fold"), egui::Sense::click()).clicked() {
                *open = !*open;
            }
        }

        if !lines.is_empty() {
            ui.add_space(2.0);
            let w = ui.available_width();
            let (rule, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
            ui.painter().rect_filled(rule, 0.0, border(sev, c));
            ui.add_space(2.0);
        }
        for line in lines {
            // One label, two formats: the tag and the text wrap together, and two
            // labels in a wrapping row would each claim the row from its left edge.
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = ui.available_width();
            let tag_color = if line.actionable { c.success } else { tone };
            job.append(&line.tag, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(FONT_SM as f32), color: tag_color, ..Default::default() });
            job.append(&line.text, 8.0, egui::TextFormat { font_id: egui::FontId::proportional(FONT_BASE as f32), color: text_ink, line_height: Some(FONT_BASE as f32 * 1.35), ..Default::default() });
            ui.add(egui::Label::new(job).wrap());
        }
    });
    // The stripe, over the frame's own leading edge, inset so it follows the corners.
    let rect = r.response.rect;
    let stripe = egui::Rect::from_min_max(egui::pos2(rect.left() + 5.0, rect.top() + 9.0), egui::pos2(rect.left() + 5.0 + STRIPE_W, rect.bottom() - 9.0));
    ui.painter().rect_filled(stripe, CornerRadius::same(2), tone);
    ui.add_space(SPACE_SM as f32);
    r.response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uiverify;

    fn all_palettes() -> Vec<(String, PaletteColors)> {
        snapclone_theme::PaletteId::all().iter().map(|id| (id.as_str().to_string(), PaletteColors::from_palette(snapclone_theme::palette_for(*id)))).collect()
    }

    const SEVS: [Severity; 4] = [Severity::Error, Severity::Warning, Severity::Success, Severity::Info];

    /// The tint must never cost legibility, in ANY palette the fleet ships - a
    /// light theme is where a "subtle tint" quietly becomes grey text on pink.
    #[test]
    fn body_text_stays_readable_on_every_tinted_surface_in_every_palette() {
        let palettes = all_palettes();
        assert!(palettes.len() >= 2, "the scan found palettes to check");
        let (mut own, mut fallback) = (0, 0);
        for (name, c) in &palettes {
            for sev in SEVS {
                let f = fill(sev, c);
                let text = uiverify::contrast(ink(sev, c), f);
                assert!(text >= MIN_BODY_CONTRAST, "{name}/{sev:?}: body text {text:.2}:1 on the tinted surface");
                if ink(sev, c) == c.text { own += 1 } else { fallback += 1 }
                let chip = uiverify::contrast(ink_on(accent(sev, c), c), accent(sev, c));
                assert!(chip >= 3.0, "{name}/{sev:?}: chip text {chip:.2}:1 on the severity colour");
            }
        }
        // The fallback is for the palettes that need it, not a new default: most surfaces keep the
        // palette's own text (so a callout reads like the screen it is on), and at least one does
        // not (slate/Error at 2.9:1 is why `ink` exists; if every palette passes with its own text
        // some day, the fallback has nothing to do and this floor is the thing to delete).
        assert!(own > fallback, "{own} surfaces use the palette text, {fallback} the fallback");
        assert!(fallback >= 1, "no palette needed the fallback — slate's text used to be 2.93:1 on its error tint");
        let slate = palettes.iter().find(|(n, _)| n == "slate").expect("slate is a fleet palette");
        assert_ne!(ink(Severity::Error, &slate.1), slate.1.text, "slate/Error is the case the fallback was written for");
    }

    /// "Not flat" as numbers: the surface is visibly not the card it sits among,
    /// the border is visibly not the surface, and the four severities differ.
    #[test]
    fn a_callout_is_distinguishable_from_a_card_and_the_severities_from_each_other() {
        for (name, c) in all_palettes() {
            let mut fills = Vec::new();
            for sev in SEVS {
                let (f, b) = (fill(sev, &c), border(sev, &c));
                assert_ne!(f, c.bg_card, "{name}/{sev:?}: the tint changed nothing");
                assert!(uiverify::contrast(b, f) >= MIN_EDGE_CONTRAST, "{name}/{sev:?}: the border disappears into the surface ({:.2}:1)", uiverify::contrast(b, f));
                fills.push(f);
            }
            // Error vs warning vs ok is the distinction that matters; info may
            // legitimately sit near another when a palette's accent does.
            assert!(fills[0] != fills[1] && fills[1] != fills[2] && fills[0] != fills[2], "{name}: error/warning/ok surfaces collide: {fills:?}");
        }
    }

    /// Folded, the header is still all there and the body is gone; a click on the
    /// header (not only on the word) unfolds it.
    #[test]
    fn folded_it_keeps_the_chip_and_title_and_a_tap_on_the_header_unfolds_it() {
        let c = crate::default_palette();
        let lines = [Line::note("a note that only shows when unfolded")];
        let ctx = egui::Context::default();
        let _ = crate::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 400.0));
        let mut open = false;
        let mut t = 0.0;
        let mut frame = |open: &mut bool, events: Vec<egui::Event>| {
            t += 0.05;
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() };
            let mut rect = egui::Rect::NOTHING;
            #[allow(deprecated)]
            let out = ctx.run(input, |ctx| {
                #[allow(deprecated)]
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = show_collapsible(ui, &c, Severity::Error, "SIGSEGV", "segmentation fault in `depth3`", &lines, open).rect;
                });
            });
            (rect, crate::layout_probe::texts(&out.shapes).into_iter().map(|t| (t.text, t.rect)).collect::<Vec<_>>())
        };
        frame(&mut open, vec![]);
        let (folded, texts) = frame(&mut open, vec![]);
        let has = |texts: &[(String, egui::Rect)], s: &str| texts.iter().any(|(t, _)| t.contains(s));
        assert!(has(&texts, "ERROR  SIGSEGV") && has(&texts, "segmentation fault") && has(&texts, "more"));
        assert!(!has(&texts, "only shows when unfolded"));

        // Tap the TITLE, not the word "more".
        let pos = texts.iter().find(|(t, _)| t.contains("segmentation fault")).unwrap().1.center();
        let click = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut open, vec![egui::Event::PointerMoved(pos)]);
        frame(&mut open, vec![click(true), click(false)]);
        assert!(open, "a tap on the header unfolded it");
        let (unfolded, texts) = frame(&mut open, vec![]);
        assert!(has(&texts, "only shows when unfolded") && has(&texts, "less"));
        assert!(unfolded.height() > folded.height() + 10.0, "{} vs {}", unfolded.height(), folded.height());
    }

    #[test]
    fn it_draws_a_stripe_a_tinted_surface_and_legible_lines_at_phone_width() {
        let c = crate::default_palette();
        let lines = [Line::note("the panic message is in the program output above; the process is stopped before unwinding, so every frame is still intact"), Line::help("rerun with the panic breakpoint armed to stop where it happens")];
        let size = egui::vec2(360.0, 320.0);
        let mut rect = egui::Rect::NOTHING;
        let frame = uiverify::render(size, c.bg_base, |ctx| {
            let _ = crate::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::new().fill(c.bg_base).inner_margin(10.0)).show(ctx, |ui| {
                rect = show(ui, &c, Severity::Error, "panic", "Rust panic in `tile_viewer::load`", &lines).rect;
            });
        });
        assert!(rect.width() > 300.0 && rect.height() > 80.0, "{rect:?}");
        // The chip is a chip: under apply_palette's touch-sized rows it was 40pt tall.
        let chip_h = {
            let mut h = 0.0;
            let _ = crate::layout_probe::probe(size, 2, true, |ctx| {
                let _ = crate::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
                #[allow(deprecated)]
                egui::CentralPanel::default().show(ctx, |ui| {
                    h = show(ui, &c, Severity::Error, "panic", "T", &[]).rect.height();
                });
            });
            h
        };
        assert!(chip_h < 90.0, "a callout with a chip and a one-word title is {chip_h}pt tall; the chip row is inflating it");
        // The stripe: a column of the severity colour at the leading edge.
        let stripe = frame.get((rect.left() + 6.5) as usize, rect.center().y as usize);
        assert_eq!(stripe, c.danger, "accent stripe");
        // The surface, sampled where no text is: tinted, not the page and not a plain card.
        // (Away from the corners: they are rounded, and a corner pixel is the page.)
        let surface = frame.get((rect.right() - 24.0) as usize, (rect.top() + 16.0) as usize);
        assert_eq!(surface, fill(Severity::Error, &c));
        assert_ne!(surface, c.bg_base);
        // And something readable is drawn inside it.
        frame.assert_legible("callout body", rect.shrink(12.0), fill(Severity::Error, &c));

        let report = crate::layout_probe::probe(size, 3, true, |ctx| {
            let _ = crate::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(10.0)).show(ctx, |ui| {
                show(ui, &c, Severity::Error, "panic", "Rust panic in `tile_viewer::load`", &lines);
            });
        });
        assert!(report.clean(), "{}", report.describe());
    }
}
