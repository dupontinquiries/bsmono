//! New-mail notifications: a per-account poll that detects newly-arrived
//! messages, a bottom "notifier bar" QML snippet whose entries open each
//! message in a tab, and best-effort desktop notifications via `notify-send`
//! (the freedesktop/KDE notification daemon).
//!
//! v0.1.862: The design keeps the *detection* pure and testable (no clock,
//! no network, no Qt) and isolates the two side-effects — rendering QML and
//! firing a desktop notification — behind small functions the backend calls.
//!
//! Privacy: desktop notifications travel to the system notification daemon
//! (and on KDE may persist in history / show on a lock screen). So when
//! web2qml is in high-security mode ([`crate::security::is_high_security`]),
//! [`desktop_notify`] suppresses sender and subject and shows only a generic
//! "New mail" — the *fact* of mail without its content. Callers can also
//! disable desktop notifications entirely.

use std::collections::HashMap;

use crate::mail_account::AccountId;

/// One new-message event surfaced to the UI / notification layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailNotification {
    pub account: AccountId,
    pub message_id: String,
    pub sender: String,
    pub subject: String,
    pub time: i64,
}

/// Tracks the high-water mark per account so each poll only reports messages
/// newer than the last one seen. Cheap, in-memory, and resets per process —
/// the first poll of a session establishes the baseline without spamming the
/// user with their entire existing inbox (see [`Notifier::prime`]).
#[derive(Default)]
pub struct Notifier {
    last_seen_time: HashMap<AccountId, i64>,
    /// Accounts we've primed (established a baseline for) at least once.
    primed: HashMap<AccountId, ()>,
}

/// The minimal shape [`Notifier::poll`] needs from a message header, so the
/// poll is testable without `mail_proton`.
pub trait HeaderLike {
    fn id(&self) -> &str;
    fn time(&self) -> i64;
    fn sender_display(&self) -> String;
    fn subject(&self) -> &str;
    fn in_inbox(&self) -> bool;
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Establish the baseline for an account from its current inbox without
    /// emitting notifications. Call once after authenticating so the user
    /// isn't notified about mail that was already there.
    pub fn prime<H: HeaderLike>(&mut self, account: &AccountId, headers: &[H]) {
        let max = headers
            .iter()
            .filter(|h| h.in_inbox())
            .map(|h| h.time())
            .max()
            .unwrap_or(0);
        self.last_seen_time.insert(account.clone(), max);
        self.primed.insert(account.clone(), ());
    }

    /// Poll a fresh batch of inbox headers and return the ones newer than the
    /// account's high-water mark, advancing the mark. If the account was
    /// never primed, this primes it and returns nothing (so the first poll is
    /// a silent baseline).
    pub fn poll<H: HeaderLike>(
        &mut self,
        account: &AccountId,
        headers: &[H],
    ) -> Vec<MailNotification> {
        if !self.primed.contains_key(account) {
            self.prime(account, headers);
            return Vec::new();
        }
        let watermark = *self.last_seen_time.get(account).unwrap_or(&0);
        let mut out = Vec::new();
        let mut new_max = watermark;
        for h in headers {
            if !h.in_inbox() {
                continue;
            }
            if h.time() > watermark {
                out.push(MailNotification {
                    account: account.clone(),
                    message_id: h.id().to_string(),
                    sender: h.sender_display(),
                    subject: h.subject().to_string(),
                    time: h.time(),
                });
            }
            new_max = new_max.max(h.time());
        }
        self.last_seen_time.insert(account.clone(), new_max);
        // Newest first.
        out.sort_by(|a, b| b.time.cmp(&a.time));
        out
    }

    /// Forget an account (e.g. on logout/removal).
    pub fn forget(&mut self, account: &AccountId) {
        self.last_seen_time.remove(account);
        self.primed.remove(account);
    }
}

/// Render the bottom notifier bar: a horizontal strip with one chip per new
/// message; tapping a chip opens that message in a tab and a "×" dismisses.
/// Returns an empty string when there is nothing to show (caller hides the
/// bar). `style` is reused from [`crate::mail_panes::PaneStyle`].
pub fn notifier_bar_qml(
    style: &crate::mail_panes::PaneStyle,
    notes: &[MailNotification],
) -> String {
    if notes.is_empty() {
        return String::new();
    }
    use crate::mail_panes::qml_escape as qe;
    let mut o = String::with_capacity(notes.len() * 256 + 256);
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Rectangle {{\nwidth: parent ? parent.width : 600\nimplicitHeight: 44\ncolor: \"{bg}\"\nborder.color: \"{accent}\"\nborder.width: 1\nRow {{\nanchors.fill: parent\nanchors.margins: 6\nspacing: 8\nText {{ text: \"{n} new\"; color: \"{accent}\"; font.bold: true; font.pixelSize: {fs}; font.family: \"{ff}\"; anchors.verticalCenter: parent.verticalCenter }}\n",
            bg = qe(&style.button_bg),
            accent = qe(&style.accent),
            n = notes.len(),
            fs = style.font_px,
            ff = qe(&style.font_family),
        ),
    );
    for note in notes.iter().take(5) {
        let who = if note.sender.is_empty() { "(unknown)" } else { &note.sender };
        let subj = if note.subject.is_empty() { "(no subject)" } else { &note.subject };
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Rectangle {{\nradius: 4\ncolor: \"transparent\"\nborder.color: \"{border}\"\nborder.width: 1\nheight: parent.height - 8\nanchors.verticalCenter: parent.verticalCenter\nimplicitWidth: _nt_{key}.width + 28\nText {{ id: _nt_{key}; anchors.centerIn: parent; text: \"{who}: {subj}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: 220 }}\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.mail_open_message(\"{id}\") }} }}\n}}\n",
                border = qe(&style.border),
                key = sanitize(&note.message_id),
                who = qe(who),
                subj = qe(subj),
                fg = qe(&style.fg),
                fs = style.font_px - 1,
                ff = qe(&style.font_family),
                id = qe(&note.message_id),
            ),
        );
    }
    // Dismiss-all.
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Item {{ Layout.fillWidth: true; width: 1 }}\nText {{ text: \"✕\"; color: \"{muted}\"; font.pixelSize: {fs}; anchors.verticalCenter: parent.verticalCenter; MouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.mail_dismiss_notifications() }} }} }}\n",
            muted = qe(&style.muted),
            fs = style.font_px,
        ),
    );
    o.push_str("}\n}\n"); // Row, Rectangle
    o
}

/// Fire a desktop notification via `notify-send` (freedesktop/KDE). Best
/// effort: if the binary is absent or the spawn fails, returns `false`
/// without error. In high-security mode the body is suppressed to just the
/// fact of new mail, so sender/subject never reach the notification daemon.
///
/// `enabled` lets the caller turn desktop notifications off entirely
/// (independent of the in-app bar).
pub fn desktop_notify(note: &MailNotification, enabled: bool) -> bool {
    if !enabled {
        return false;
    }
    let high_sec = crate::security::is_high_security();
    let (summary, body) = if high_sec {
        ("New mail".to_string(), "You have a new message.".to_string())
    } else {
        let who = if note.sender.is_empty() { "Someone".to_string() } else { note.sender.clone() };
        let subj = if note.subject.is_empty() { "(no subject)".to_string() } else { note.subject.clone() };
        (format!("Mail from {who}"), subj)
    };
    spawn_notify_send(&summary, &body)
}

/// Coalesce several notifications into one desktop notification (avoids a
/// burst of popups). Honors the same privacy/enable rules.
pub fn desktop_notify_batch(notes: &[MailNotification], enabled: bool) -> bool {
    if !enabled || notes.is_empty() {
        return false;
    }
    if notes.len() == 1 {
        return desktop_notify(&notes[0], enabled);
    }
    let high_sec = crate::security::is_high_security();
    let summary = format!("{} new messages", notes.len());
    let body = if high_sec {
        "You have new mail.".to_string()
    } else {
        // First few senders, deduped, comma-joined.
        let mut seen = std::collections::BTreeSet::new();
        let mut who: Vec<String> = Vec::new();
        for n in notes {
            let s = if n.sender.is_empty() { "(unknown)".to_string() } else { n.sender.clone() };
            if seen.insert(s.clone()) {
                who.push(s);
            }
            if who.len() >= 3 {
                break;
            }
        }
        format!("From {}", who.join(", "))
    };
    spawn_notify_send(&summary, &body)
}

fn spawn_notify_send(summary: &str, body: &str) -> bool {
    use std::process::{Command, Stdio};
    Command::new("notify-send")
        .arg("--app-name=web2qml")
        .arg("--icon=mail-unread")
        .arg("--")
        .arg(summary)
        .arg(body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap asynchronously-ish: don't block the UI thread on the
            // daemon. A quick try_wait is enough; if it's still running we
            // detach (notify-send exits promptly in practice).
            let _ = child.try_wait();
            true
        })
        .unwrap_or(false)
}

fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct H {
        id: String,
        time: i64,
        from: String,
        subj: String,
        inbox: bool,
    }
    impl HeaderLike for H {
        fn id(&self) -> &str {
            &self.id
        }
        fn time(&self) -> i64 {
            self.time
        }
        fn sender_display(&self) -> String {
            self.from.clone()
        }
        fn subject(&self) -> &str {
            &self.subj
        }
        fn in_inbox(&self) -> bool {
            self.inbox
        }
    }
    fn h(id: &str, time: i64) -> H {
        H { id: id.into(), time, from: "Bob".into(), subj: "Hi".into(), inbox: true }
    }

    #[test]
    fn first_poll_is_silent_baseline() {
        let mut n = Notifier::new();
        let acc = AccountId::new("a");
        let got = n.poll(&acc, &[h("m1", 100), h("m2", 200)]);
        assert!(got.is_empty(), "first poll must not notify");
    }

    #[test]
    fn second_poll_reports_only_new() {
        let mut n = Notifier::new();
        let acc = AccountId::new("a");
        n.poll(&acc, &[h("m1", 100), h("m2", 200)]); // baseline at 200
        let got = n.poll(&acc, &[h("m3", 250), h("m2", 200), h("m4", 300)]);
        let ids: Vec<&str> = got.iter().map(|x| x.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m4", "m3"], "newest first, only > watermark");
        // Third poll with nothing new → empty.
        assert!(n.poll(&acc, &[h("m4", 300)]).is_empty());
    }

    #[test]
    fn non_inbox_headers_ignored() {
        let mut n = Notifier::new();
        let acc = AccountId::new("a");
        n.prime(&acc, &[h("m1", 100)]);
        let mut spam = h("s1", 999);
        spam.inbox = false;
        assert!(n.poll(&acc, &[spam]).is_empty());
    }

    #[test]
    fn forget_resets_baseline() {
        let mut n = Notifier::new();
        let acc = AccountId::new("a");
        n.poll(&acc, &[h("m1", 100)]); // baseline
        n.forget(&acc);
        // After forget, next poll re-primes silently.
        assert!(n.poll(&acc, &[h("m2", 500)]).is_empty());
    }

    #[test]
    fn notifier_bar_empty_when_no_notes() {
        let bar = notifier_bar_qml(&crate::mail_panes::PaneStyle::dark_default(), &[]);
        assert!(bar.is_empty());
    }

    #[test]
    fn notifier_bar_balanced_and_links() {
        let notes = vec![MailNotification {
            account: AccountId::new("a"),
            message_id: "m9".into(),
            sender: "Bob".into(),
            subject: "Lunch".into(),
            time: 1,
        }];
        let bar = notifier_bar_qml(&crate::mail_panes::PaneStyle::dark_default(), &notes);
        assert!(bar.contains("mail_open_message(\"m9\")"));
        assert_eq!(bar.matches('{').count(), bar.matches('}').count(), "{bar}");
    }
}
