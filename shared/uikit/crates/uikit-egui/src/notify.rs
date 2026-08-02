//! notify — desktop notifications, with the privacy rule the fleet already had.
//!
//! Only voiceterm's mail could notify; snapclone is a messenger that could not
//! tell you a snap had arrived. This lifts the decisions from
//! `voiceterm/src/mail_notify.rs`, which had thought about them properly.
//!
//! ## The privacy rule, which is the reason not to write this from scratch
//!
//! A desktop notification travels to the system notification daemon, and on KDE
//! it may **persist in history and appear on a lock screen**. So in
//! high-security mode the sender and the content are suppressed and only the
//! *fact* is shown: "New message", not who from or what about.
//!
//! That is the whole point of the mode — a notification that leaks a name onto
//! a locked screen has defeated the encryption underneath it — and it is exactly
//! the clause a fresh implementation would omit, because the happy path works
//! perfectly without it.
//!
//! ## Coalescing
//!
//! Ten snaps arriving together should be one notification, not ten popups.
//! A burst is also when a user is *least* able to read them individually.

/// What to tell the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Who it is from. Suppressed in high-security mode.
    pub from: String,
    /// What it says or is. Suppressed in high-security mode.
    pub detail: String,
    /// The generic form, shown when the details are suppressed. Must carry no
    /// identifying information.
    pub generic: String,
}

/// A rendered notification: what would actually be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub summary: String,
    pub body: String,
}

/// Render one, honouring the privacy mode.
///
/// Returned rather than sent so the decision is testable without a notification
/// daemon — which is the only way the high-security clause can be verified at
/// all, since its failure is silent and off-machine.
pub fn render(note: &Note, high_security: bool) -> Rendered {
    if high_security {
        return Rendered {
            summary: note.generic.clone(),
            // Deliberately not "from {who}" — a name is the thing being
            // protected, and it is the first thing a lock screen shows.
            body: "Open the app to view.".to_string(),
        };
    }
    let who = if note.from.trim().is_empty() { "Someone".to_string() } else { note.from.clone() };
    let what = if note.detail.trim().is_empty() { note.generic.clone() } else { note.detail.clone() };
    Rendered { summary: who, body: what }
}

/// Render a batch as ONE notification.
///
/// A burst of ten popups is unreadable and is also when the user can least
/// afford to be interrupted ten times.
pub fn render_batch(notes: &[Note], high_security: bool) -> Option<Rendered> {
    match notes.len() {
        0 => None,
        1 => Some(render(&notes[0], high_security)),
        n => {
            let summary = format!("{n} new messages");
            if high_security {
                return Some(Rendered { summary, body: "Open the app to view.".into() });
            }
            // Names only, never content: a batch body listing what each said
            // would be a paragraph on a lock screen.
            let mut names: Vec<&str> = notes.iter()
                .map(|x| x.from.trim())
                .filter(|s| !s.is_empty())
                .collect();
            names.dedup();
            let body = if names.is_empty() { "From several people".to_string() }
                       else { format!("From {}", names.join(", ")) };
            Some(Rendered { summary, body })
        }
    }
}

/// Send via `notify-send`, best-effort.
///
/// A missing notification daemon is NOT an error worth surfacing: headless
/// machines and minimal window managers legitimately have none, and an app that
/// complains about it every time a message arrives is worse than one that is
/// quiet.
pub fn send(r: &Rendered, app_id: &str) -> bool {
    std::process::Command::new("notify-send")
        .arg("--app-name").arg(app_id)
        .arg(&r.summary)
        .arg(&r.body)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(from: &str, detail: &str) -> Note {
        Note { from: from.into(), detail: detail.into(), generic: "New message".into() }
    }

    #[test]
    fn high_security_leaks_neither_the_sender_nor_the_content() {
        // A notification persists in KDE's history and shows on a lock screen.
        // Leaking a name there defeats the encryption underneath it.
        let r = render(&note("Ada", "meet me at six"), true);
        assert!(!r.summary.contains("Ada"), "{r:?}");
        assert!(!r.body.contains("Ada"), "{r:?}");
        assert!(!r.body.contains("six"), "{r:?}");
        assert_eq!(r.summary, "New message");
    }

    #[test]
    fn normal_mode_says_who_and_what() {
        let r = render(&note("Ada", "meet me at six"), false);
        assert_eq!(r.summary, "Ada");
        assert_eq!(r.body, "meet me at six");
    }

    #[test]
    fn an_unnamed_sender_still_produces_a_readable_notification() {
        // A peer with no display name is common on first contact.
        let r = render(&note("", "hello"), false);
        assert_eq!(r.summary, "Someone");
        assert_eq!(r.body, "hello");
    }

    #[test]
    fn a_snap_with_no_text_falls_back_to_the_generic() {
        // The picture IS the message, so there is no detail to show — but the
        // notification still has to say something.
        let r = render(&note("Ada", ""), false);
        assert_eq!(r.body, "New message");
    }

    #[test]
    fn a_burst_becomes_one_notification() {
        // Ten popups is unreadable, and a burst is when the user can least
        // afford ten interruptions.
        let notes: Vec<Note> = (0..10).map(|i| note(&format!("p{i}"), "hi")).collect();
        let r = render_batch(&notes, false).unwrap();
        assert_eq!(r.summary, "10 new messages");
        assert!(r.body.starts_with("From "), "{r:?}");
    }

    #[test]
    fn a_batch_body_carries_names_and_never_content() {
        // Listing what each message said would be a paragraph on a lock screen.
        let notes = vec![note("Ada", "the safe combination is 1234"),
                         note("Bo", "and the door code")];
        let r = render_batch(&notes, false).unwrap();
        assert!(!r.body.contains("1234"), "{r:?}");
        assert!(!r.body.contains("door"), "{r:?}");
        assert!(r.body.contains("Ada") && r.body.contains("Bo"));
    }

    #[test]
    fn a_high_security_batch_leaks_nothing_but_the_count() {
        // The count alone is already a small disclosure and is the minimum a
        // batch notification can be.
        let notes = vec![note("Ada", "x"), note("Bo", "y")];
        let r = render_batch(&notes, true).unwrap();
        assert_eq!(r.summary, "2 new messages");
        assert!(!r.body.contains("Ada") && !r.body.contains("Bo"), "{r:?}");
    }

    #[test]
    fn one_note_in_a_batch_is_rendered_as_one_note() {
        // "1 new messages" is the kind of detail that makes an app feel
        // unfinished, and the single form is more useful anyway.
        let r = render_batch(&[note("Ada", "hi")], false).unwrap();
        assert_eq!(r.summary, "Ada");
    }

    #[test]
    fn an_empty_batch_sends_nothing() {
        assert_eq!(render_batch(&[], false), None);
        assert_eq!(render_batch(&[], true), None);
    }
}
