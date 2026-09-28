//! send_flow — sending a snap is FOUR STEPS, and the screen says which one you are on.
//!
//! The Qt predecessor made every step its own surface, so you always knew where
//! you were: `CameraWidget` (take) opened `SnapComposerDialog` (caption, colour,
//! timer, a big **Send ➤** button) which emitted `snapReady`; `MainWindow` then
//! opened `ContactPickerDialog` with "Send" on it; the send went through
//! `AppController::sendSnap`; the app switched to the inbox with *"Snap sent!"*
//! and a three-second `MediaSendSummary` receipt (metadata stripped, size,
//! recipients). If you had no contacts it said *"Add a friend first — swipe right
//! to Friends"* and took you there.
//!
//! The egui port had all the pieces and none of the sequence: composer controls
//! drawn under a LIVE viewfinder before anything was captured, the send hidden
//! in an upward swipe that jumped to the Friends page, a generic "send to N"
//! there, and a receipt computed for nobody. And the composed snap — caption,
//! colour, TIMER — never reached the wire: `SendSnap { to }` carried only the
//! recipients, and the bin wrote `duration_secs = 0`, which on this wire means
//! *never vanishes*. That is the exact inversion `composer.rs` warns leaks a photo.
//!
//! This is the sequence, as data. One state machine both pages read, so the
//! stepper strip at the top of each says the same thing, and the composed snap
//! travels with the recipients to the host instead of being dropped on the floor.
//!
//! Pure: no egui, no clock (times are passed in), no IO.

use crate::composer::Snap;

/// The four steps, in order. `Sent` is the receipt: the flow is over but still
/// on screen until the person starts another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Press the shutter. Nothing to compose yet.
    Capture,
    /// Caption, colour, timer over the captured frame; Next or swipe up.
    Compose,
    /// Pick who gets it; Send.
    Recipients,
    /// In flight: one row per recipient, each pending, delivered or failed.
    Sending,
    /// Everyone has answered (or failed). The receipt.
    Sent,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Capture, Step::Compose, Step::Recipients, Step::Sending];
    /// One-based, as the strip shows it: ① Take ② Compose ③ Choose ④ Send.
    pub fn number(self) -> u8 {
        match self {
            Step::Capture => 1,
            Step::Compose => 2,
            Step::Recipients => 3,
            Step::Sending | Step::Sent => 4,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Step::Capture => "Take",
            Step::Compose => "Compose",
            Step::Recipients => "Choose",
            Step::Sending => "Send",
            Step::Sent => "Sent",
        }
    }
    /// What to do here, for the line under the strip.
    pub fn prompt(self) -> &'static str {
        match self {
            Step::Capture => "press the shutter to take a snap",
            Step::Compose => "add a caption and a timer, then Next",
            Step::Recipients => "choose who gets it, then Send",
            Step::Sending => "sending…",
            Step::Sent => "sent — take another?",
        }
    }
}

/// What the shutter produced, as the host reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// The file the host wrote (its name is what the compose step shows).
    pub path: String,
    pub at_ms: u64,
}

/// One recipient's row in the send step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Delivered,
    Failed,
}

/// The flow. Advance it with what happened; read `step()`, `stepper()` and
/// `receipt()` to draw it.
#[derive(Debug, Clone, Default)]
pub struct SendFlow {
    captured: Option<Captured>,
    snap: Option<Snap>,
    recipients: Vec<(String, Outcome)>,
    sent_at_ms: Option<u64>,
    settled_at_ms: Option<u64>,
}

/// How long the receipt stays before the flow offers a new capture on its own.
/// Three seconds is the Qt summary's `QTimer::singleShot(3000, …)`.
pub const RECEIPT_MS: u64 = 3_000;

impl SendFlow {
    pub fn step(&self) -> Step {
        if self.settled_at_ms.is_some() {
            Step::Sent
        } else if self.sent_at_ms.is_some() {
            Step::Sending
        } else if self.snap.is_some() {
            Step::Recipients
        } else if self.captured.is_some() {
            Step::Compose
        } else {
            Step::Capture
        }
    }

    pub fn captured(&self) -> Option<&Captured> {
        self.captured.as_ref()
    }
    pub fn snap(&self) -> Option<&Snap> {
        self.snap.as_ref()
    }
    pub fn recipients(&self) -> &[(String, Outcome)] {
        &self.recipients
    }

    // ── advancing ──

    /// The host wrote the shutter's frame. Only from Capture (or Sent: a new
    /// snap after a receipt starts over). A capture during Compose/Recipients
    /// is a RETAKE and replaces the frame but keeps the composition.
    pub fn on_captured(&mut self, cap: Captured) {
        if self.step() >= Step::Sending {
            self.reset();
        }
        self.captured = Some(cap);
    }

    /// Compose is done: caption / colour / timer decided. Only from Compose.
    pub fn on_composed(&mut self, snap: Snap) -> bool {
        if self.step() != Step::Compose {
            return false;
        }
        self.snap = Some(snap);
        true
    }

    /// Back from Recipients to Compose, keeping the frame.
    pub fn back_to_compose(&mut self) -> bool {
        if self.step() != Step::Recipients {
            return false;
        }
        self.snap = None;
        true
    }

    /// Recipients chosen and Send pressed. Only from Recipients, and only with
    /// someone to send to — a send to nobody is the case `send_summary` blocks.
    /// Returns what the host must be told: the recipients AND the composed snap.
    pub fn on_send(&mut self, to: &[String], now_ms: u64) -> Option<SendRequest> {
        if self.step() != Step::Recipients || to.is_empty() {
            return None;
        }
        let mut seen = Vec::new();
        for who in to {
            if !seen.contains(who) {
                seen.push(who.clone());
            }
        }
        self.recipients = seen.iter().map(|w| (w.clone(), Outcome::Pending)).collect();
        self.sent_at_ms = Some(now_ms);
        Some(SendRequest { to: seen, snap: self.snap.clone().expect("Recipients step has a snap") })
    }

    /// The host handed `who`'s copy to the socket.
    pub fn on_delivered(&mut self, who: &str, now_ms: u64) {
        self.settle(who, Outcome::Delivered, now_ms);
    }
    /// The host could not.
    pub fn on_failed(&mut self, who: &str, now_ms: u64) {
        self.settle(who, Outcome::Failed, now_ms);
    }

    fn settle(&mut self, who: &str, o: Outcome, now_ms: u64) {
        if self.step() != Step::Sending {
            return;
        }
        if let Some(r) = self.recipients.iter_mut().find(|(w, s)| w == who && *s == Outcome::Pending) {
            r.1 = o;
        }
        if self.recipients.iter().all(|(_, s)| *s != Outcome::Pending) {
            self.settled_at_ms = Some(now_ms);
        }
    }

    /// Cancel from anywhere before Sending: back to the shutter, frame dropped.
    pub fn cancel(&mut self) {
        if self.step() < Step::Sending {
            self.reset();
        }
    }

    /// Start over — the receipt's "take another".
    pub fn reset(&mut self) {
        *self = SendFlow::default();
    }

    /// Has the receipt been up long enough that the shutter should come back on its own?
    pub fn receipt_expired(&self, now_ms: u64) -> bool {
        matches!(self.settled_at_ms, Some(t) if now_ms.saturating_sub(t) >= RECEIPT_MS)
    }

    // ── reading ──

    /// The strip: every step with what was decided at it, and whether it is the
    /// current one. Decided steps carry their summary — the file name, the timer
    /// and caption, the recipient count — so the strip is also the record.
    pub fn stepper(&self) -> Vec<StepCell> {
        let cur = self.step();
        Step::ALL
            .iter()
            .map(|&s| StepCell {
                step: s,
                current: s == cur || (s == Step::Sending && cur == Step::Sent),
                done: s < cur.min(Step::Sending) || cur == Step::Sent,
                summary: self.summary_of(s),
            })
            .collect()
    }

    fn summary_of(&self, s: Step) -> Option<String> {
        match s {
            // "photo", not the file name: `snap-1788696974124.jpg` is a
            // sixty-pixel number nobody reads, and it pushed the strip onto a
            // second line at phone width. The path is still on `captured()`.
            Step::Capture => self.captured.as_ref().map(|c| {
                // Named by what was taken. A clip said "photo" in the
                // stepper until the recipient opened a video.
                let lower = c.path.to_ascii_lowercase();
                if lower.ends_with(".mkv") || lower.ends_with(".mp4") || lower.ends_with(".webm") {
                    "clip".to_string()
                } else {
                    "photo".to_string()
                }
            }),
            Step::Compose => self.snap.as_ref().map(|sn| {
                if sn.caption.is_empty() {
                    format!("{} · no caption", sn.duration.label())
                } else {
                    format!("{} · \u{201c}{}\u{201d}", sn.duration.label(), shorten(&sn.caption, 18))
                }
            }),
            Step::Recipients => (!self.recipients.is_empty()).then(|| match self.recipients.len() {
                1 => self.recipients[0].0.clone(),
                n => format!("{n} people"),
            }),
            Step::Sending | Step::Sent => self.settled_at_ms.map(|_| self.receipt().headline()),
        }
    }

    /// The receipt: what the Qt `MediaSendSummary` + status bar said after a send.
    pub fn receipt(&self) -> Receipt {
        let delivered = self.recipients.iter().filter(|(_, s)| *s == Outcome::Delivered).count();
        let failed = self.recipients.iter().filter(|(_, s)| *s == Outcome::Failed).count();
        let pending = self.recipients.len() - delivered - failed;
        Receipt { delivered, failed, pending }
    }
}

/// What `on_send` hands the host: WHO and WHAT. The old command carried only
/// the who; the what — the timer above all — was rebuilt by the bin as zero.
#[derive(Debug, Clone, PartialEq)]
pub struct SendRequest {
    pub to: Vec<String>,
    pub snap: Snap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCell {
    pub step: Step,
    pub current: bool,
    pub done: bool,
    pub summary: Option<String>,
}

impl StepCell {
    /// `1 Take` / `2 Compose · 5s · "hi"`. Plain digits: the circled ones
    /// (①②③④) drew as tofu in the app's font under Xvfb.
    pub fn label(&self) -> String {
        let n = self.step.number();
        match &self.summary {
            Some(s) => format!("{n} {} · {s}", self.step.title()),
            None => format!("{n} {}", self.step.title()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub delivered: usize,
    pub failed: usize,
    pub pending: usize,
}

impl Receipt {
    /// "Snap sent!" when everything went; the honest count otherwise.
    pub fn headline(&self) -> String {
        match (self.delivered, self.failed, self.pending) {
            (_, 0, 0) if self.delivered > 0 => "Snap sent!".to_string(),
            (d, f, 0) => format!("sent to {d}, {f} failed"),
            (d, f, p) => format!("{d} sent · {f} failed · {p} sending"),
        }
    }
    pub fn all_failed(&self) -> bool {
        self.delivered == 0 && self.pending == 0 && self.failed > 0
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::Duration;

    fn snap(caption: &str, secs: u32) -> Snap {
        Snap { caption: caption.into(), color: [255, 255, 255], caption_y: 0.5, duration: Duration::from_secs(secs) }
    }

    #[test]
    fn the_steps_go_in_order_and_each_gate_holds() {
        let mut f = SendFlow::default();
        assert_eq!(f.step(), Step::Capture);
        assert!(!f.on_composed(snap("x", 5)), "nothing captured: nothing to compose");
        assert!(f.on_send(&["ada".into()], 0).is_none(), "nothing composed: nothing to send");
        f.on_captured(Captured { path: "/tmp/snaps/img_001.jpg".into(), at_ms: 10 });
        assert_eq!(f.step(), Step::Compose);
        assert!(f.on_send(&["ada".into()], 0).is_none(), "still nothing composed");
        assert!(f.on_composed(snap("hello", 5)));
        assert_eq!(f.step(), Step::Recipients);
        assert!(f.on_send(&[], 20).is_none(), "a send to nobody is not a send");
        let req = f.on_send(&["ada".into(), "bob".into(), "ada".into()], 20).unwrap();
        assert_eq!(req.to, vec!["ada".to_string(), "bob".to_string()], "recipients deduplicated");
        assert_eq!(req.snap.duration, Duration::Seconds(5), "THE TIMER TRAVELS WITH THE SEND");
        assert_eq!(req.snap.caption, "hello");
        assert_eq!(f.step(), Step::Sending);
        f.on_delivered("ada", 30);
        assert_eq!(f.step(), Step::Sending, "bob is still pending");
        f.on_failed("bob", 40);
        assert_eq!(f.step(), Step::Sent);
        assert_eq!(f.receipt(), Receipt { delivered: 1, failed: 1, pending: 0 });
        assert_eq!(f.receipt().headline(), "sent to 1, 1 failed");
    }

    #[test]
    fn a_clean_send_says_snap_sent_and_the_receipt_expires_into_a_new_capture() {
        let mut f = SendFlow::default();
        f.on_captured(Captured { path: "a.jpg".into(), at_ms: 0 });
        f.on_composed(snap("", 0));
        f.on_send(&["ada".into()], 100);
        f.on_delivered("ada", 200);
        assert_eq!(f.receipt().headline(), "Snap sent!");
        assert!(!f.receipt_expired(200 + RECEIPT_MS - 1));
        assert!(f.receipt_expired(200 + RECEIPT_MS));
        // a fresh shutter press after the receipt starts a new flow
        f.on_captured(Captured { path: "b.jpg".into(), at_ms: 300 });
        assert_eq!(f.step(), Step::Compose);
        assert!(f.recipients().is_empty() && f.snap().is_none(), "the old send is gone");
    }

    #[test]
    fn a_retake_keeps_nothing_but_the_step_and_back_keeps_the_frame() {
        let mut f = SendFlow::default();
        f.on_captured(Captured { path: "a.jpg".into(), at_ms: 0 });
        f.on_captured(Captured { path: "b.jpg".into(), at_ms: 1 });
        assert_eq!(f.captured().unwrap().path, "b.jpg", "a second shutter press is a retake");
        f.on_composed(snap("hi", 10));
        assert!(f.back_to_compose());
        assert_eq!(f.step(), Step::Compose);
        assert_eq!(f.captured().unwrap().path, "b.jpg", "back keeps the frame");
        assert!(!f.back_to_compose(), "already there");
        f.cancel();
        assert_eq!(f.step(), Step::Capture);
    }

    #[test]
    fn nothing_after_send_can_be_cancelled_or_settled_twice() {
        let mut f = SendFlow::default();
        f.on_captured(Captured { path: "a.jpg".into(), at_ms: 0 });
        f.on_composed(snap("", 5));
        f.on_send(&["ada".into()], 0);
        f.cancel();
        assert_eq!(f.step(), Step::Sending, "a send in flight is not cancelled from the screen");
        f.on_delivered("ada", 1);
        f.on_failed("ada", 2);
        assert_eq!(f.receipt(), Receipt { delivered: 1, failed: 0, pending: 0 }, "the first answer stands");
        f.on_delivered("stranger", 3);
        assert_eq!(f.recipients().len(), 1, "an answer for nobody adds nobody");
    }

    #[test]
    fn the_strip_records_what_was_decided_and_marks_the_current_step() {
        let mut f = SendFlow::default();
        let labels = |f: &SendFlow| f.stepper().iter().map(|c| c.label()).collect::<Vec<_>>();
        assert_eq!(labels(&f), vec!["1 Take", "2 Compose", "3 Choose", "4 Send"]);
        assert!(f.stepper()[0].current && !f.stepper()[1].current);
        f.on_captured(Captured { path: "/data/snaps/img_017.jpg".into(), at_ms: 0 });
        assert_eq!(labels(&f)[0], "1 Take · photo");
        assert!(f.stepper()[0].done && f.stepper()[1].current);
        f.on_composed(snap("a very long caption that will not fit", 10));
        assert_eq!(labels(&f)[1], "2 Compose · 10s · \u{201c}a very long capti…\u{201d}");
        f.on_send(&["ada".into(), "bob".into()], 0);
        assert_eq!(labels(&f)[2], "3 Choose · 2 people");
        assert!(f.stepper()[3].current, "sending is the current step");
        f.on_delivered("ada", 1);
        f.on_delivered("bob", 2);
        assert_eq!(labels(&f)[3], "4 Send · Snap sent!");
        assert!(f.stepper().iter().all(|c| c.done), "after the receipt every step is done");
        let mut one = SendFlow::default();
        one.on_captured(Captured { path: "x".into(), at_ms: 0 });
        one.on_composed(snap("", 0));
        one.on_send(&["ada".into()], 0);
        assert_eq!(labels(&one)[1], "2 Compose · ∞ · no caption");
        assert_eq!(labels(&one)[2], "3 Choose · ada");
    }

    #[test]
    fn the_receipt_is_honest_about_a_total_failure() {
        let mut f = SendFlow::default();
        f.on_captured(Captured { path: "x".into(), at_ms: 0 });
        f.on_composed(snap("", 5));
        f.on_send(&["ada".into(), "bob".into()], 0);
        f.on_failed("ada", 1);
        assert_eq!(f.receipt().headline(), "0 sent · 1 failed · 1 sending");
        f.on_failed("bob", 2);
        assert!(f.receipt().all_failed());
        assert_eq!(f.receipt().headline(), "sent to 0, 2 failed");
    }
}
