//! qr — `QrScanDialog`: adding a contact by pointing the camera at their code.
//!
//! `camera.rs` already ports scan mode's frame throttle — the drop from ~30fps
//! to ~8fps that exists because a scanner does not need thirty frames a second
//! and a phone's battery says so. Nothing consumed those frames. This is the
//! surface that does.
//!
//! ## Four accepted payloads, one shape
//!
//! ```text
//! sc-aabbccdd                     sc-ID alone, default port 9876
//! sc-aabbccdd:9876                explicit port
//! snapclone://sc-aabbccdd         the URI form
//! snapclone://sc-aabbccdd:9876    URI with port
//! ```
//!
//! The port default is not cosmetic: a code that carries no port means "the
//! usual one", and treating a missing port as 0 would send the DHT a lookup for
//! a peer that cannot exist.
//!
//! ## The overflow rule, which the Qt code learned the hard way
//!
//! Its comment is worth carrying over: `toUShort` silently returns 0 on
//! overflow, so a QR claiming port 99999 became **port 0** — which is the
//! DHT-lookup sentinel elsewhere in this app. A malformed code would have been
//! read not as an error but as a different, valid instruction. Ports outside
//! 1..=65535 are rejected here for the same reason.
//!
//! ## Two things about the pattern that are worth knowing
//!
//! The Qt regex is **unanchored** — it finds an sc-ID anywhere inside a payload.
//! That is deliberate for QR codes carrying extra text around the id, and it
//! also means `not-sc-deadbeef` yields `sc-deadbeef`. Reproduced, with a test
//! naming it, because tightening it would reject real codes that wrap the id in
//! a URL or a label.
//!
//! The id length is likewise unbounded (`sc-[0-9a-fA-F]+`) even though the docs
//! say eight hex characters. Left as it is: the id is a routing key this view
//! does not own, and refusing a longer one would break the day the format grows.

/// The default port a bare sc-ID implies.
pub const DEFAULT_PORT: u16 = 9876;
/// Scan-mode frame rate. `camera.rs` throttles to this; the scanner assumes it.
pub const SCAN_FPS: u32 = 8;
/// How long the success state is held before the dialog reports and closes, so
/// the user sees WHAT was found rather than a flash and a dismissal.
pub const SUCCESS_HOLD_MS: f32 = 600.0;
/// The green viewfinder flash.
pub const FLASH_MS: f32 = 300.0;

/// A decoded contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedContact {
    pub sc_id: String,
    pub port: u16,
}

/// Parse a QR payload (or a pasted string — the manual fallback uses the same
/// function, so what the camera accepts and what typing accepts cannot drift).
pub fn parse_payload(raw: &str) -> Option<ScannedContact> {
    let s = raw.trim();
    // Unanchored, like the Qt regex: find `sc-` and take the hex run after it.
    let start = s.find("sc-")?;
    let after = &s[start + 3..];
    let hex_len = after.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(after.len());
    if hex_len == 0 { return None; }
    let sc_id = format!("sc-{}", &after[..hex_len]);

    let rest = &after[hex_len..];
    let port = match rest.strip_prefix(':') {
        None => DEFAULT_PORT,
        Some(tail) => {
            let digits_len = tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len());
            if digits_len == 0 {
                // A trailing colon with nothing usable after it. The Qt regex
                // simply does not capture the group and falls back to the
                // default, which is the friendlier reading of a truncated code.
                DEFAULT_PORT
            } else {
                // Parsed as u32 FIRST. Parsing straight into u16 wraps or fails
                // depending on the path, and the Qt bug was exactly this: a
                // silent 0 for an out-of-range port, which is the DHT-lookup
                // sentinel. Out of range is an error, not a different command.
                let v: u32 = tail[..digits_len].parse().ok()?;
                if v == 0 || v > 65535 { return None; }
                v as u16
            }
        }
    };
    Some(ScannedContact { sc_id, port })
}

/// The stash line: `ISO timestamp \t sc-id \t port`, append-only, so a scan can
/// be audited or re-used later even though nothing browses the file yet.
pub fn stash_line(timestamp: &str, c: &ScannedContact) -> String {
    format!("{}\t{}\t{}\n", timestamp, c.sc_id, c.port)
}

/// What the scanner is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Camera unavailable — the manual-entry fallback is the whole dialog.
    Manual,
    Scanning,
    /// Decoded, holding the success state before reporting.
    Found { contact: ScannedContact, held_ms: u32 },
    /// Reported and finished.
    Done(ScannedContact),
}

/// The dialog.
#[derive(Debug, Clone)]
pub struct QrScan {
    pub phase: Phase,
    /// The manual-entry field, always available: a camera that will not focus is
    /// not a reason to be unable to add a contact.
    pub manual: String,
    pub error: Option<String>,
    /// Frames seen since scanning began, for the throttle assertion below.
    pub frames: u32,
}

pub const MANUAL_HINT: &str = "sc-ID or snapclone:// URI";
pub const MANUAL_ERROR: &str = "Not a valid sc-ID. Format: sc-aabbccdd or sc-aabbccdd:9876";

impl QrScan {
    /// Open with a camera.
    pub fn scanning() -> Self {
        QrScan { phase: Phase::Scanning, manual: String::new(), error: None, frames: 0 }
    }
    /// Open without one.
    pub fn manual_only() -> Self {
        QrScan { phase: Phase::Manual, manual: String::new(), error: None, frames: 0 }
    }

    pub fn is_scanning(&self) -> bool { matches!(self.phase, Phase::Scanning) }

    /// Feed a decoded frame payload.
    ///
    /// Latches on the first success and stops scanning. The camera keeps
    /// delivering frames for a moment after — at 8fps the same code arrives
    /// several more times — and without the latch the dialog would report the
    /// same contact repeatedly and close mid-animation.
    pub fn frame(&mut self, payload: &str) -> bool {
        if !self.is_scanning() { return false; }
        self.frames += 1;
        match parse_payload(payload) {
            Some(c) => { self.phase = Phase::Found { contact: c, held_ms: 0 }; true }
            // A frame that does not decode is not an error worth showing: most
            // frames during a scan are of a table.
            None => false,
        }
    }

    /// Submit the manual field. Uses the SAME parser as the camera path.
    pub fn submit_manual(&mut self) -> bool {
        match parse_payload(&self.manual) {
            Some(c) => {
                self.error = None;
                // No hold on the manual path: the user typed it, so there is
                // nothing to show them that they do not already know.
                self.phase = Phase::Done(c);
                true
            }
            None => { self.error = Some(MANUAL_ERROR.to_string()); false }
        }
    }

    /// Advance the success hold. Returns the contact once, at the moment the
    /// dialog should report and close.
    pub fn tick(&mut self, dt_ms: f32) -> Option<ScannedContact> {
        let Phase::Found { contact, held_ms } = &mut self.phase else { return None };
        *held_ms += dt_ms as u32;
        if (*held_ms as f32) < SUCCESS_HOLD_MS { return None; }
        let c = contact.clone();
        self.phase = Phase::Done(c.clone());
        Some(c)
    }

    /// `✓ Found: sc-…`, or the scanning prompt.
    pub fn status(&self) -> String {
        match &self.phase {
            Phase::Manual => "Camera unavailable — paste a code instead".into(),
            Phase::Scanning => "Point at a SnapClone QR code".into(),
            Phase::Found { contact, .. } | Phase::Done(contact) =>
                format!("✓ Found: {}", contact.sc_id),
        }
    }

    /// How far through the flash, 0..=1, for the viewfinder animation.
    pub fn flash(&self) -> f32 {
        match &self.phase {
            Phase::Found { held_ms, .. } => ((*held_ms as f32) / FLASH_MS).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// Draw the scanner sheet. Returns a contact once one is confirmed.
pub fn qr_ui(ui: &mut egui::Ui, s: &mut QrScan,
             c: &uikit_egui::PaletteColors) -> Option<ScannedContact> {
    qr_ui_diag(ui, s, c, crate::qr_scan_status::Status::Idle, None)
}

/// The scanner well's side, in points: as wide as the dialog allows up to
/// a size a code fills comfortably at arm's length.
pub const WELL_MAX: f32 = 260.0;
pub const WELL_MIN: f32 = 160.0;

/// The centre square of a `w × h` frame, as texture coordinates. The
/// scanner shows the middle of the picture because that is where a code
/// held up to the lens is, and a letterboxed 16:9 frame in a square well
/// would make the code a third smaller than it needs to be.
pub fn centre_square_uv(w: usize, h: usize) -> egui::Rect {
    if w == 0 || h == 0 { return egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)); }
    if w > h {
        let f = h as f32 / w as f32;
        let x0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x0 + f, 1.0))
    } else {
        let f = w as f32 / h as f32;
        let y0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, y0), egui::pos2(1.0, y0 + f))
    }
}

/// [`qr_ui`] with the scanner's own diagnosis of why it has not found anything.
///
/// Separate so every existing call site draws what it drew. The diagnosis needs counters the
/// camera loop keeps, and threading them through the one signature would have made the
/// scanner's UI depend on state it has no business owning.
///
/// `frame` is the live camera texture, when the camera is running: the
/// well shows it, centre-cropped, with corner marks — a popup ABOVE the
/// camera page's controls, so the code being aimed at is the biggest thing
/// on screen rather than a picture under a shutter, a dropdown and a step
/// strip. Without a frame the well is the empty hole it always was, and
/// the diagnosis line says why.
pub fn qr_ui_diag(ui: &mut egui::Ui, s: &mut QrScan,
                  c: &uikit_egui::PaletteColors,
                  diag: crate::qr_scan_status::Status,
                  frame: Option<&egui::TextureHandle>) -> Option<ScannedContact> {
    let dt = ui.input(|i| i.stable_dt) * 1000.0;
    let mut out = s.tick(dt);

    // Air around the whole pane and between its rows. Without this every
    // row sat at egui's default 3-pt item spacing: the heading touched the
    // viewfinder, the status touched the frame, and the manual field and
    // its button ran together into one band.
    let pad = uikit_egui::tokens::SPACE_MD as f32;
    let gap = uikit_egui::tokens::SPACE_SM as f32;
    ui.add_space(pad);
    ui.spacing_mut().item_spacing.y = gap;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        ui.colored_label(c.text_weak, "SCAN A CODE");
        if s.is_scanning() || matches!(s.phase, Phase::Found { .. }) {
            // The viewfinder: the live picture, flashing on a hit. Nothing
            // decodes here — frames arrive from the camera source.
            let side = (ui.available_width() - 2.0 * pad).clamp(WELL_MIN, WELL_MAX);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
            let lit = s.flash();
            let border = if lit > 0.0 {
                let g = c.success;
                egui::Color32::from_rgba_unmultiplied(g.r(), g.g(), g.b(), (255.0 * lit) as u8)
            } else { c.border };
            // The viewfinder gets its own filled well, so the frame reads as
            // a hole to look through rather than an outline floating on the
            // page — and so the status line under it has an edge to sit off.
            ui.painter().rect_filled(rect, 8.0, c.bg_base);
            if let Some(t) = frame {
                let [w, h] = t.size();
                ui.painter().image(t.id(), rect.shrink(2.0), centre_square_uv(w, h), egui::Color32::WHITE);
                // Corner marks: where to put the code. Four short L's rather
                // than a full frame, so the picture underneath stays visible.
                let (l, m) = (side * 0.12, 2.0);
                let col = if lit > 0.0 { c.success } else { c.accent };
                let r = rect.shrink(10.0);
                for (cx, cy, sx, sy) in [(r.left(), r.top(), 1.0, 1.0), (r.right(), r.top(), -1.0, 1.0),
                                          (r.left(), r.bottom(), 1.0, -1.0), (r.right(), r.bottom(), -1.0, -1.0)] {
                    ui.painter().line_segment([egui::pos2(cx, cy), egui::pos2(cx + sx * l, cy)], egui::Stroke::new(m, col));
                    ui.painter().line_segment([egui::pos2(cx, cy), egui::pos2(cx, cy + sy * l)], egui::Stroke::new(m, col));
                }
            }
            ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(2.0_f32, border),
                                     egui::StrokeKind::Inside);
            // WHY IT HAS NOT FOUND ANYTHING. A scanner that is looking and one that is broken
            // draw the same thing — a viewfinder and no result — so the person moves the phone
            // closer, turns it, and gives up on a feature that was never running.
            //
            // A fault is coloured; "looking" is not. "Looking" in warning colour every second
            // teaches people to ignore this line, which costs exactly when there is a fault.
            if !matches!(diag, crate::qr_scan_status::Status::Idle) {
                let col = if diag.is_fault() { c.warning } else { c.text_weak };
                ui.colored_label(col, egui::RichText::new(diag.line()).small());
            }
        }
        let status_col = if matches!(s.phase, Phase::Found { .. } | Phase::Done(_)) {
            c.success
        } else { c.text_weak };
        ui.colored_label(status_col, s.status());

        // A rule with real air either side, not egui's flush separator: this
        // is the seam between "point the camera" and "type it instead", and
        // the two halves were running together.
        ui.add_space(gap);
        ui.separator();
        ui.add_space(gap);
        // The manual field is present in BOTH phases, not only the fallback: a
        // camera that will not focus should not stop you adding a contact.
        ui.colored_label(c.text_weak, "OR TYPE THE CODE");
        ui.add(egui::TextEdit::singleline(&mut s.manual).id_salt("qr-s_manual")
            .hint_text(MANUAL_HINT).desired_width(240.0));
        // The button is its own row at touch height, clear of the field it
        // submits — they were adjacent at default spacing.
        if ui.add_sized([240.0, uikit_egui::tokens::TOUCH_MIN as f32],
                        egui::Button::new("Add")).clicked() && s.submit_manual() {
            if let Phase::Done(cc) = &s.phase { out = Some(cc.clone()); }
        }
        if let Some(e) = &s.error { ui.colored_label(c.danger, e); }
    });
    ui.add_space(pad);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<ScannedContact> { parse_payload(s) }

    #[test]
    fn the_scanner_well_shows_the_centre_square_of_the_frame() {
        // A 16:9 frame in a square well: the middle 9/16 of the width, the
        // whole height. A code held up to the lens is in the middle.
        let uv = centre_square_uv(1280, 720);
        assert!((uv.width() - 0.5625).abs() < 1e-4, "{uv:?}");
        assert!((uv.min.x - 0.21875).abs() < 1e-4);
        assert_eq!((uv.min.y, uv.max.y), (0.0, 1.0));
        let tall = centre_square_uv(720, 1280);
        assert_eq!((tall.min.x, tall.max.x), (0.0, 1.0));
        assert!((tall.height() - 0.5625).abs() < 1e-4);
        assert_eq!(centre_square_uv(0, 0).max, egui::pos2(1.0, 1.0), "degenerate: the whole frame");
    }

    #[test]
    fn all_four_documented_payload_forms_parse() {
        assert_eq!(p("sc-aabbccdd"),
            Some(ScannedContact { sc_id: "sc-aabbccdd".into(), port: 9876 }));
        assert_eq!(p("sc-aabbccdd:9000"),
            Some(ScannedContact { sc_id: "sc-aabbccdd".into(), port: 9000 }));
        assert_eq!(p("snapclone://sc-aabbccdd"),
            Some(ScannedContact { sc_id: "sc-aabbccdd".into(), port: 9876 }));
        assert_eq!(p("snapclone://sc-aabbccdd:9000"),
            Some(ScannedContact { sc_id: "sc-aabbccdd".into(), port: 9000 }));
    }

    #[test]
    fn a_missing_port_means_the_usual_one_not_zero() {
        // Port 0 is the DHT-lookup sentinel elsewhere in this app, so defaulting
        // to it would turn "no port given" into a different instruction.
        assert_eq!(p("sc-abcd").unwrap().port, DEFAULT_PORT);
    }

    #[test]
    fn an_out_of_range_port_is_rejected_rather_than_silently_becoming_zero() {
        // THE bug the Qt comment records: toUShort returns 0 on overflow, so a
        // code claiming port 99999 was read as port 0 — a malformed code
        // arriving not as an error but as a valid, different command.
        assert_eq!(p("sc-abcd:99999"), None);
        assert_eq!(p("sc-abcd:65536"), None);
        assert_eq!(p("sc-abcd:0"), None);
        assert_eq!(p("sc-abcd:65535").unwrap().port, 65535);
        assert_eq!(p("sc-abcd:1").unwrap().port, 1);
    }

    #[test]
    fn hex_is_case_insensitive_because_qr_encoders_are_not_consistent() {
        assert_eq!(p("sc-AABBCCDD").unwrap().sc_id, "sc-AABBCCDD");
        assert_eq!(p("sc-AaBbCcDd").unwrap().sc_id, "sc-AaBbCcDd");
    }

    #[test]
    fn a_non_hex_id_is_not_an_id() {
        assert_eq!(p("sc-"), None);
        assert_eq!(p("sc-zzzz"), None);
        assert_eq!(p("hello world"), None);
        assert_eq!(p(""), None);
    }

    #[test]
    fn the_id_stops_at_the_first_non_hex_character() {
        // "sc-abcd/extra" is an id followed by something else, not a longer id.
        assert_eq!(p("sc-abcd/extra").unwrap().sc_id, "sc-abcd");
        assert_eq!(p("sc-abcd-more").unwrap().sc_id, "sc-abcd");
    }

    #[test]
    fn payloads_are_trimmed_because_scanners_add_whitespace() {
        assert_eq!(p("  sc-abcd\n").unwrap().sc_id, "sc-abcd");
    }

    #[test]
    fn the_pattern_is_unanchored_and_that_is_deliberate() {
        // A code that wraps the id in a label or a URL still scans, which is
        // why the Qt regex is not anchored. The cost is that "not-sc-deadbeef"
        // yields sc-deadbeef. Named here so tightening it later is a decision
        // rather than a surprise.
        assert_eq!(p("Add me: sc-deadbeef thanks").unwrap().sc_id, "sc-deadbeef");
        assert_eq!(p("not-sc-deadbeef").unwrap().sc_id, "sc-deadbeef");
    }

    #[test]
    fn a_truncated_port_falls_back_rather_than_failing() {
        // A partially-read code is better answered with the default port than
        // with a refusal the user cannot act on.
        assert_eq!(p("sc-abcd:").unwrap().port, DEFAULT_PORT);
        assert_eq!(p("sc-abcd:x").unwrap().port, DEFAULT_PORT);
    }

    #[test]
    fn the_first_decode_latches_and_later_frames_are_ignored() {
        // At 8fps the same code arrives several more times before the camera
        // stops. Without the latch the dialog reports repeatedly and closes
        // mid-animation.
        let mut s = QrScan::scanning();
        assert!(s.frame("sc-abcd"));
        assert!(!s.is_scanning());
        assert!(!s.frame("sc-99999999"), "a later frame must not replace it");
        match &s.phase {
            Phase::Found { contact, .. } => assert_eq!(contact.sc_id, "sc-abcd"),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn frames_that_do_not_decode_are_not_errors() {
        // Most frames during a scan are of a table.
        let mut s = QrScan::scanning();
        for _ in 0..20 { assert!(!s.frame("just a table")); }
        assert!(s.is_scanning());
        assert!(s.error.is_none(), "a scanner does not nag while it scans");
        assert_eq!(s.frames, 20);
    }

    #[test]
    fn the_success_state_is_held_before_the_dialog_reports() {
        // So the user sees WHAT was found rather than a flash and a dismissal.
        let mut s = QrScan::scanning();
        s.frame("sc-abcd:9000");
        assert_eq!(s.tick(500.0), None, "still showing the result");
        let got = s.tick(200.0).expect("reports after the hold");
        assert_eq!(got, ScannedContact { sc_id: "sc-abcd".into(), port: 9000 });
        assert_eq!(s.tick(1000.0), None, "and reports exactly once");
    }

    #[test]
    fn the_flash_runs_over_its_own_shorter_window() {
        let mut s = QrScan::scanning();
        s.frame("sc-abcd");
        assert_eq!(s.flash(), 0.0);
        s.tick(FLASH_MS / 2.0);
        assert!((s.flash() - 0.5).abs() < 0.05);
        s.tick(FLASH_MS);
        assert_eq!(s.flash(), 1.0, "clamped, and the hold outlasts it");
    }

    #[test]
    fn manual_entry_accepts_exactly_what_the_camera_accepts() {
        // Same parser, so the two paths cannot drift into accepting different
        // things — which would be reported as "the scanner works but typing it
        // does not".
        for payload in ["sc-abcd", "snapclone://sc-abcd:9000", "  sc-ABCD  "] {
            let mut s = QrScan::manual_only();
            s.manual = payload.into();
            assert!(s.submit_manual(), "manual rejected {payload:?}");
        }
    }

    #[test]
    fn a_bad_manual_entry_explains_the_format_and_keeps_the_text() {
        let mut s = QrScan::manual_only();
        s.manual = "nonsense".into();
        assert!(!s.submit_manual());
        assert_eq!(s.error.as_deref(), Some(MANUAL_ERROR));
        assert_eq!(s.manual, "nonsense", "retyping a long id is the worst outcome here");
        assert!(MANUAL_ERROR.contains("sc-aabbccdd"), "the message shows the shape");
    }

    #[test]
    fn a_manual_submission_does_not_wait() {
        // The hold exists to show the user something they did not know. They
        // typed this one.
        let mut s = QrScan::manual_only();
        s.manual = "sc-abcd".into();
        s.submit_manual();
        assert!(matches!(s.phase, Phase::Done(_)));
    }

    #[test]
    fn a_second_manual_attempt_clears_the_previous_error() {
        let mut s = QrScan::manual_only();
        s.manual = "bad".into();
        s.submit_manual();
        s.manual = "sc-abcd".into();
        assert!(s.submit_manual());
        assert!(s.error.is_none());
    }

    #[test]
    fn the_stash_line_is_tab_separated_and_newline_terminated() {
        // Append-only, so a scan can be audited later even though nothing
        // browses the file yet.
        let c = ScannedContact { sc_id: "sc-abcd".into(), port: 9000 };
        assert_eq!(stash_line("2026-08-02T12:00:00Z", &c),
                   "2026-08-02T12:00:00Z\tsc-abcd\t9000\n");
    }

    #[test]
    fn the_status_line_names_what_was_found() {
        let mut s = QrScan::scanning();
        assert!(s.status().contains("Point at"));
        s.frame("sc-abcd");
        assert_eq!(s.status(), "✓ Found: sc-abcd");
        assert!(QrScan::manual_only().status().contains("Camera unavailable"));
    }

    #[test]
    fn the_scan_rate_matches_what_the_camera_throttles_to() {
        // camera.rs drops to scan mode for battery; a scanner assuming 30fps
        // would size its own timeouts wrong.
        assert_eq!(SCAN_FPS, 8);
    }
}

#[cfg(test)]
mod input_reality_check {
    use super::*;
    use egui::{Context, Event, RawInput};

    /// Drive the REAL page for several frames, typing one character per frame.
    ///
    /// The primitive works in isolation; this asks whether the page as written
    /// does. A field that takes one character and then stops is a field whose
    /// widget id moved, or whose focus was taken, between frames — and neither
    /// shows up in a unit test of the state machine.
    #[test]
    fn the_manual_field_accepts_more_than_one_character() {
        let ctx = Context::default();
        let colors = uikit_egui::palette_colors("midnight");
        let mut s = QrScan::manual_only();

        // Frame one: draw, so the widgets exist to be focused.
        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                qr_ui(ui, &mut s, &colors);
            });
        });

        // Tab to the field rather than guessing its id. next_auto_id() before
        // the call is NOT the field — several widgets precede it — and a probe
        // that focuses the wrong widget reports a bug that is its own.
        let tab = |ctx: &Context, s: &mut QrScan| {
            let input = RawInput {
                events: vec![Event::Key { key: egui::Key::Tab, physical_key: None,
                                          pressed: true, repeat: false,
                                          modifiers: Default::default() }],
                ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { qr_ui(ui, s, &colors); });
            });
        };
        // The manual field is the first focusable widget on the manual-only
        // sheet, so one Tab reaches it.
        tab(&ctx, &mut s);

        for ch in "sc-ab".chars() {
            let input = RawInput { events: vec![Event::Text(ch.to_string())],
                                   ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    qr_ui(ui, &mut s, &colors);
                });
            });
        }
        assert_eq!(s.manual, "sc-ab", "the field kept only {:?}", s.manual);
    }
}

/// Draw the local identity as a scannable-looking square.
///
/// The other half of the QR flow: `qr_ui` reads somebody else's code, this
/// shows yours so they can. Without it the exchange only works in one
/// direction and both people need a third device.
///
/// ## It says what it is
///
/// `qr_render`'s own docs are blunt — "a real QR reader will not decode it",
/// because there is no Reed-Solomon encoder, no mask evaluation and no format
/// information. So the caption says so. A square that LOOKS scannable and is
/// not would have people holding phones at each other until one of them gives
/// up, and blaming their camera.
pub fn my_code_ui(ui: &mut egui::Ui, id: &str, c: &uikit_egui::PaletteColors, revealed_at: Option<f64>, tilt: Option<uikit_egui::qr::Tilt>) -> bool {
    use crate::qr_render::layout_for;
    let mut tapped = false;
    ui.vertical_centered(|ui| {
        ui.colored_label(c.text_weak, "MY CODE");
        // THE KIT'S CODE, not a white square with black cells: theme colours
        // behind the WCAG guard, an integer cell at the device pixel ratio,
        // rounded eyes and dotted modules on the same glass card as the rest
        // of the chrome. A tap opens it full-screen (the C++ showExpandedQr).
        // The card leans with the phone (the sensor's tilt) or under the pointer.
        let (rect, _) = (ui.available_rect_before_wrap(), ());
        let side = 168.0;
        let probe = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + side * 0.5), egui::vec2(side, side));
        let tilt = tilt.unwrap_or_else(|| {
            ui.input(|i| i.pointer.hover_pos()).map(|p| uikit_egui::qr::Tilt::from_pointer(p, probe)).unwrap_or_default()
        });
        let r = uikit_egui::qr::qr_ui_tilted(ui, &layout_for(id), side, c, revealed_at, tilt).on_hover_text("tap to enlarge");
        if r.clicked() { tapped = true; }
        ui.colored_label(c.text_weak, egui::RichText::new(id).monospace().small());
        // The honest caption. Not a standard QR (see the kit's module docs:
        // no Reed-Solomon, no format bits), so a phone's stock reader will
        // not decode it — but another snapclone's camera does, through the
        // kit's own decoder, which is how the scan in the friends flow
        // works. The old caption said "not scannable", which was true of
        // strangers' readers and false of this app's.
        ui.colored_label(c.text_weak,
            egui::RichText::new("another snapclone's camera can scan this — or read the code aloud")
                .small());
    });
    tapped
}

