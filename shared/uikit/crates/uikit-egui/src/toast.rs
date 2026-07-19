//! toast — the fire-and-forget status pill, ported from voiceterm's QML `toast` (used from ~205
//! call sites there: "Toggled fence", "Copied", "Saved", …). Bottom-center, above the app's
//! bottom chrome, one message at a time — `show` REPLACES the current toast and restarts its
//! timer, exactly as the QML did (no queue; the newest message wins).
//!
//! Visual contract, kept verbatim from the reference:
//! - dark neutral panel `rgba(0.1, 0.1, 0.12, 0.92)`, 1 px theme border, radius 8;
//! - a 4 px severity accent strip on the LEFT edge — success `#7ddc7d`, warning `#dcc97d`,
//!   error `#dc7d7d` — and NO strip for info (the legacy look), with the text nudged +2 px when
//!   the strip is present so both modes read centered;
//! - dwell 1500 ms for info/success, 2800 ms for warning/error (v0.1.151: problems dwell
//!   longer); 180 ms OutQuad opacity fade in/out.
//!
//! The expiry/replacement logic is pure (`Toasts::visible_at`) and unit-tested; only `ui`
//! touches egui.

use egui::{Align2, Color32, CornerRadius, FontId, Id, Order, Stroke, StrokeKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

impl Severity {
    /// The reference's exact accent hexes; `None` for info (no strip — legacy look).
    pub fn accent(self) -> Option<Color32> {
        match self {
            Severity::Info => None,
            Severity::Success => Some(Color32::from_rgb(0x7d, 0xdc, 0x7d)),
            Severity::Warning => Some(Color32::from_rgb(0xdc, 0xc9, 0x7d)),
            Severity::Error => Some(Color32::from_rgb(0xdc, 0x7d, 0x7d)),
        }
    }
    /// Dwell seconds before fade-out: problems linger (2.8 s), chatter doesn't (1.5 s).
    pub fn dwell(self) -> f64 {
        match self {
            Severity::Warning | Severity::Error => 2.8,
            _ => 1.5,
        }
    }
}

#[derive(Clone, Debug)]
struct Msg {
    text: String,
    severity: Severity,
    shown_at: f64,
}

/// The single-slot toast host. Hold one per app shell; call `ui` every frame.
#[derive(Default)]
pub struct Toasts {
    current: Option<Msg>,
}

impl Toasts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show a message (replaces any current toast and restarts its timer — newest wins).
    pub fn show(&mut self, text: impl Into<String>, severity: Severity, now: f64) {
        self.current = Some(Msg { text: text.into(), severity, shown_at: now });
    }
    pub fn info(&mut self, text: impl Into<String>, now: f64) {
        self.show(text, Severity::Info, now);
    }
    pub fn success(&mut self, text: impl Into<String>, now: f64) {
        self.show(text, Severity::Success, now);
    }
    pub fn warning(&mut self, text: impl Into<String>, now: f64) {
        self.show(text, Severity::Warning, now);
    }
    pub fn error(&mut self, text: impl Into<String>, now: f64) {
        self.show(text, Severity::Error, now);
    }

    /// The message that should be visible at `now`, if any — the pure expiry core. Expired
    /// messages are dropped on query.
    pub fn visible_at(&mut self, now: f64) -> Option<(&str, Severity)> {
        if let Some(m) = &self.current {
            if now - m.shown_at > m.severity.dwell() {
                self.current = None;
            }
        }
        self.current.as_ref().map(|m| (m.text.as_str(), m.severity))
    }

    /// Paint the toast bottom-center, `bottom_margin` px above the screen bottom (bind this to
    /// the actual bottom-chrome height, as the reference does, so the toast clears keycaps/bars
    /// without wasting space). Call every frame.
    pub fn ui(&mut self, ctx: &egui::Context, bottom_margin: f32) {
        let now = ctx.input(|i| i.time);
        let showing = self.visible_at(now).map(|(t, s)| (t.to_string(), s));
        // 180 ms fade, eased — both directions, as the QML Behavior on opacity did
        let opacity = crate::motion::animate_eased(ctx, Id::new("uikit-toast-fade"), showing.is_some(), 0.18, crate::motion::ease::out_quad);
        if opacity <= 0.01 {
            return;
        }
        // While fading out the message is gone from state; remember the last text for the tail
        // of the fade via egui temp memory.
        let mem_id = Id::new("uikit-toast-last");
        let (text, severity) = match showing {
            Some((t, s)) => {
                ctx.data_mut(|d| d.insert_temp(mem_id, (t.clone(), s as u8)));
                (t, s)
            }
            None => {
                let (t, s) = ctx.data_mut(|d| d.get_temp::<(String, u8)>(mem_id)).unwrap_or_default();
                let sev = match s {
                    1 => Severity::Success,
                    2 => Severity::Warning,
                    3 => Severity::Error,
                    _ => Severity::Info,
                };
                (t, sev)
            }
        };
        if text.is_empty() {
            return;
        }
        let screen = ctx.screen_rect();
        egui::Area::new(Id::new("uikit-toast"))
            .order(Order::Foreground)
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                let painter = ui.painter();
                let font = FontId::proportional(12.0);
                let galley = painter.layout_no_wrap(text.clone(), font.clone(), Color32::WHITE);
                let strip = severity.accent();
                let w = galley.size().x + 28.0 + if strip.is_some() { 4.0 } else { 0.0 };
                let h = galley.size().y + 16.0;
                let center_x = screen.center().x;
                let rect = egui::Rect::from_center_size(
                    egui::pos2(center_x, screen.bottom() - bottom_margin - h / 2.0),
                    egui::vec2(w, h),
                );
                let a = |c: Color32| c.gamma_multiply(opacity);
                // panel: the reference's dark neutral, NOT theme bg — toasts read the same on
                // every palette
                painter.rect_filled(rect, CornerRadius::same(8), a(Color32::from_rgba_unmultiplied(26, 26, 31, 235)));
                painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, a(ui.visuals().widgets.noninteractive.bg_stroke.color)), StrokeKind::Inside);
                if let Some(sc) = strip {
                    let s = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 1.0, rect.top() + 1.0),
                        egui::vec2(4.0, rect.height() - 2.0),
                    );
                    painter.rect_filled(s, CornerRadius::same(2), a(sc));
                }
                let dx = if strip.is_some() { 2.0 } else { 0.0 }; // the reference's centering nudge
                painter.text(
                    egui::pos2(rect.center().x + dx, rect.center().y),
                    Align2::CENTER_CENTER,
                    text,
                    font,
                    a(Color32::from_rgb(0xf0, 0xf0, 0xf0)),
                );
            });
        ctx.request_repaint(); // keep the fade/dwell timeline moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_contract_matches_the_reference() {
        assert_eq!(Severity::Info.accent(), None); // legacy look: no strip
        assert_eq!(Severity::Success.accent(), Some(Color32::from_rgb(0x7d, 0xdc, 0x7d)));
        assert_eq!(Severity::Warning.accent(), Some(Color32::from_rgb(0xdc, 0xc9, 0x7d)));
        assert_eq!(Severity::Error.accent(), Some(Color32::from_rgb(0xdc, 0x7d, 0x7d)));
        assert_eq!(Severity::Info.dwell(), 1.5);
        assert_eq!(Severity::Success.dwell(), 1.5);
        assert_eq!(Severity::Warning.dwell(), 2.8);
        assert_eq!(Severity::Error.dwell(), 2.8);
    }

    #[test]
    fn newest_wins_and_expiry_follows_the_dwell() {
        let mut t = Toasts::new();
        assert!(t.visible_at(0.0).is_none());
        t.info("copied", 10.0);
        assert_eq!(t.visible_at(11.0).map(|(s, _)| s.to_string()).as_deref(), Some("copied"));
        // expires after 1.5s of dwell
        assert!(t.visible_at(11.6).is_none());
        // show replaces + restarts: a warning at 20 shown over an info at 19.9 dwells from 20
        t.info("a", 19.9);
        t.warning("problem", 20.0);
        let vis = t.visible_at(22.5).map(|(s, v)| (s.to_string(), v));
        assert_eq!(vis, Some(("problem".into(), Severity::Warning))); // 2.5s < 2.8 dwell
        assert!(t.visible_at(22.9).is_none());
    }
}
