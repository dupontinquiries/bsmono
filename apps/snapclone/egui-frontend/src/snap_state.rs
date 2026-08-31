//! snap_state — which snaps have already been seen, across restarts.
//!
//! `MarkViewed` and `MarkExpired` were the last two host commands with no
//! store. The binary printed them and said so, and the consequence is worse
//! than a lost preference: **a snap was viewed until you quit and unviewed
//! afterwards.** In an app whose whole promise is that a snap is seen once
//! and gone, a restart handed it back. That is the promise failing in the
//! direction that matters — the sender believes it is gone.
//!
//! ## Two marks, not one flag
//!
//! `MarkViewed` is "they saw it". `MarkExpired` is "the window elapsed".
//! The command's own doc says storage does different things with each, and
//! it does:
//!
//! * **Viewed** is a receipt. It is what a read receipt would be derived
//!   from, and it means the recipient got the content.
//! * **Expired** is a deletion trigger. Nobody saw anything; the snap is
//!   simply over.
//!
//! Collapsing them into `seen: bool` would make an unopened snap that timed
//! out indistinguishable from one that was read, which is exactly the fact a
//! sender is asking about.
//!
//! ## Why ids and not the snaps
//!
//! This file records that an id is finished. It does not hold captions,
//! senders or bytes — those live in whatever received them. The store is
//! therefore small, boring, and cheap to consult before showing anything,
//! which is the property that matters: the filter runs on every feed load.
//!
//! ## Forgetting
//!
//! Entries are pruned by age ([`RETAIN_MS`]), because a list that only grows
//! is a file that eventually costs more to read than the feature is worth,
//! and because a permanent record of every snap ever received is itself a
//! privacy problem in an app built to not keep things. The retention is
//! deliberately longer than any snap's own life — forgetting an id while the
//! snap could still arrive again would let it reappear, which is the bug
//! this module exists to fix.

/// How long a mark is kept. Thirty days: comfortably past the 30-day expiry
/// the snap list itself uses, so a re-delivered snap is still recognised as
/// finished, and short enough that the file does not become a diary.
pub const RETAIN_MS: u64 = 31 * 24 * 60 * 60 * 1000;

pub const FILE: &str = "snap-state.conf";

/// How a snap ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The recipient saw it.
    Viewed,
    /// The window elapsed unopened.
    Expired,
}

impl Mark {
    pub fn as_str(self) -> &'static str {
        match self {
            Mark::Viewed => "viewed",
            Mark::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Mark> {
        match s.trim() {
            "viewed" => Some(Mark::Viewed),
            "expired" => Some(Mark::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub mark: Mark,
    /// Unix ms, for pruning.
    pub at_ms: u64,
}

/// The set of finished snaps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapState {
    entries: Vec<Entry>,
}

impl SnapState {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Entry> {
        self.entries.iter()
    }

    /// Is this snap finished, and how?
    pub fn mark_of(&self, id: &str) -> Option<Mark> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.mark)
    }

    /// Anything with a mark must not be shown again.
    pub fn is_finished(&self, id: &str) -> bool {
        self.mark_of(id).is_some()
    }

    /// Record a mark. Returns whether anything changed, so the caller only
    /// writes the file when it must.
    ///
    /// **Viewed wins over Expired, and a mark is never downgraded.** The two
    /// can race: a snap opened in the last moment of its window produces
    /// both, in either order depending on which timer fires first. Viewed is
    /// the stronger claim — somebody saw it — and letting a late Expired
    /// overwrite it would turn a delivered snap into one that timed out,
    /// which is precisely backwards for a read receipt.
    pub fn mark(&mut self, id: &str, mark: Mark, at_ms: u64) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                if e.mark == Mark::Expired && mark == Mark::Viewed {
                    e.mark = Mark::Viewed;
                    e.at_ms = at_ms;
                    return true;
                }
                false
            }
            None => {
                self.entries.push(Entry { id: id.to_string(), mark, at_ms });
                true
            }
        }
    }

    /// Drop marks older than [`RETAIN_MS`]. Returns how many went.
    pub fn prune(&mut self, now_ms: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| now_ms.saturating_sub(e.at_ms) < RETAIN_MS);
        before - self.entries.len()
    }

    /// Remove from `entries` everything already finished.
    ///
    /// The whole point of the store, applied at the one place it matters:
    /// the feed load. A snap that was viewed before the last restart must
    /// not come back as unopened.
    pub fn filter_unfinished(&self, entries: Vec<crate::snaps::FeedEntry>)
        -> Vec<crate::snaps::FeedEntry> {
        entries.into_iter().filter(|e| !self.is_finished(&e.id)).collect()
    }
}

/// `id = mark at_ms`, one per line. The same flat shape as the other stores
/// here, and hand-editable — which matters for the one recovery this file
/// has: deleting a line puts a snap back, and somebody will need that.
pub fn render(s: &SnapState) -> String {
    let mut out = String::from("# snapclone: snaps already seen or expired\n");
    for e in &s.entries {
        out.push_str(&format!("{} = {} {}\n", e.id, e.mark.as_str(), e.at_ms));
    }
    out
}

pub fn parse(text: &str) -> SnapState {
    let mut s = SnapState::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((id, rest)) = line.split_once('=') else { continue };
        let mut parts = rest.split_whitespace();
        let Some(mark) = parts.next().and_then(Mark::parse) else { continue };
        // A missing or unparseable timestamp becomes 0, which means the entry
        // prunes on the next pass rather than being dropped now. Losing a
        // mark silently is how a snap comes back; losing it on a schedule at
        // least matches what the file said.
        let at_ms = parts.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        s.entries.push(Entry { id: id.trim().to_string(), mark, at_ms });
    }
    s
}

pub fn load() -> SnapState {
    config_kit::load(crate::settings_store::APP, FILE)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

pub fn store(s: &SnapState) -> std::io::Result<()> {
    config_kit::save(crate::settings_store::APP, FILE, &render(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> crate::snaps::FeedEntry {
        crate::snaps::FeedEntry {
            id: id.to_string(),
            sender_name: "someone".into(),
            sent_at: 0,
            media: crate::snaps::Media::Image,
            thumb_path: String::new(),
        }
    }

    #[test]
    fn a_viewed_snap_does_not_come_back_after_a_restart() {
        // The bug in one test. Before this store, a snap was viewed until
        // you quit and unviewed afterwards — in an app whose promise is that
        // a snap is seen once and gone, and whose SENDER believes it is
        // gone.
        let mut s = SnapState::default();
        s.mark("snap-1", Mark::Viewed, 1_000);
        let reloaded = parse(&render(&s));
        assert_eq!(reloaded, s, "the mark did not survive the round trip");
        let feed = reloaded.filter_unfinished(vec![entry("snap-1"), entry("snap-2")]);
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].id, "snap-2");
    }

    #[test]
    fn viewed_and_expired_stay_distinguishable() {
        // Collapsing them into one flag makes an unopened snap that timed
        // out indistinguishable from one that was read — which is exactly
        // the fact a sender is asking about.
        let mut s = SnapState::default();
        s.mark("seen", Mark::Viewed, 1);
        s.mark("timed-out", Mark::Expired, 2);
        let r = parse(&render(&s));
        assert_eq!(r.mark_of("seen"), Some(Mark::Viewed));
        assert_eq!(r.mark_of("timed-out"), Some(Mark::Expired));
        assert!(r.is_finished("seen") && r.is_finished("timed-out"),
                "both are finished, however they ended");
    }

    #[test]
    fn a_late_expiry_cannot_downgrade_a_view() {
        // A snap opened in the last moment of its window produces both
        // marks, in whichever order the timers fire. Viewed is the stronger
        // claim; letting Expired land on top would turn a delivered snap
        // into one that timed out.
        let mut s = SnapState::default();
        assert!(s.mark("snap-1", Mark::Viewed, 10));
        assert!(!s.mark("snap-1", Mark::Expired, 11), "a no-op reported a change");
        assert_eq!(s.mark_of("snap-1"), Some(Mark::Viewed));

        // The other order upgrades, and reports it so the file is written.
        let mut s = SnapState::default();
        s.mark("snap-2", Mark::Expired, 10);
        assert!(s.mark("snap-2", Mark::Viewed, 11), "the upgrade was not persisted");
        assert_eq!(s.mark_of("snap-2"), Some(Mark::Viewed));
    }

    #[test]
    fn marking_the_same_snap_twice_does_not_ask_for_a_write() {
        // `mark` returning false is what keeps the file from being rewritten
        // on every frame that re-reports an id.
        let mut s = SnapState::default();
        assert!(s.mark("snap-1", Mark::Viewed, 1));
        assert!(!s.mark("snap-1", Mark::Viewed, 2));
        assert_eq!(s.len(), 1, "a duplicate became a second entry");
    }

    #[test]
    fn marks_are_forgotten_eventually_but_not_before_the_snap_is() {
        // Forgetting an id while the snap could still be re-delivered would
        // let it reappear, which is the bug this module exists to fix.
        let mut s = SnapState::default();
        s.mark("old", Mark::Viewed, 0);
        s.mark("recent", Mark::Viewed, RETAIN_MS);
        assert_eq!(s.prune(RETAIN_MS - 1), 0, "pruned inside the retention window");
        assert_eq!(s.prune(RETAIN_MS + 1), 1);
        assert!(s.is_finished("recent") && !s.is_finished("old"));
    }

    #[test]
    fn a_damaged_line_costs_that_line_and_not_the_file() {
        // The recovery this file has is hand-editing, so a typo must not
        // take every other mark with it — that would put a page of viewed
        // snaps back.
        let text = "\
# comment
snap-1 = viewed 1000
this line is nonsense
snap-2 = wat 2000
snap-3 = expired
snap-4 = viewed 4000
";
        let s = parse(text);
        assert_eq!(s.len(), 3, "{:?}", s.iter().collect::<Vec<_>>());
        assert!(s.is_finished("snap-1") && s.is_finished("snap-4"));
        assert!(!s.is_finished("snap-2"), "an unknown mark was accepted");
        // A missing timestamp is kept with 0, so it prunes on schedule
        // rather than silently letting the snap come back now.
        assert_eq!(s.mark_of("snap-3"), Some(Mark::Expired));
    }
}
