//! palette — a command line for the things the buttons make slow.
//!
//! Every action here exists as a method already: join or leave the call,
//! send the captured snap to someone, flip the camera, go to a page, open
//! the help. What the palette adds is reaching them by name, and reaching
//! them LATER: "call at the hour", "send ada in 10 min". A phone user has
//! the tab bar; a keyboard user has this.
//!
//! ## Pure by construction
//!
//! [`suggest`] turns the typed text into ranked [`Suggestion`]s given the
//! clock and the contacts, and [`Schedule`] holds the deferred ones and
//! says which are due. Neither touches the app; the shell draws the list,
//! moves a cursor, and on Enter hands the chosen [`Action`] to the app's
//! own methods. So "at the hour" being the NEXT top of the hour, and
//! "in 90 min" being ninety minutes, are tests rather than beliefs.
//!
//! ## Times are relative to the clock given
//!
//! `at HH:MM` needs a local day, which is a timezone. The shell passes the
//! UTC offset it knows (zero if it does not), and the parser finds the next
//! occurrence of that wall time from `now`. "at :30" and "at the hour" need
//! no timezone at all — the minute hand is the same everywhere.

/// What a command does. Each maps to one app method.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// Join the group call (or leave it, if in one and `leave`).
    Call { leave: bool },
    /// Send the captured snap to this sc-ID.
    SendTo { id: String, name: String },
    FlipCamera,
    /// A page by index into `PAGES`.
    GoPage(usize),
    /// Take a photo.
    Snap,
    Help,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Suggestion {
    pub label: String,
    pub action: Action,
    /// When to do it; `None` is now.
    pub at_ms: Option<u64>,
}

/// A deferred command.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Scheduled {
    pub at_ms: u64,
    pub label: String,
    pub action: Action,
}

#[derive(Clone, Default, Debug)]
pub struct Schedule {
    items: Vec<Scheduled>,
}

impl Schedule {
    pub fn add(&mut self, s: Scheduled) {
        self.items.push(s);
        self.items.sort_by_key(|s| s.at_ms);
    }
    /// Everything whose time has come, removed. Sorted by time.
    pub fn due(&mut self, now_ms: u64) -> Vec<Scheduled> {
        let (due, later): (Vec<_>, Vec<_>) = self.items.drain(..).partition(|s| s.at_ms <= now_ms);
        self.items = later;
        due
    }
    /// The soonest pending one.
    pub fn next(&self) -> Option<&Scheduled> { self.items.first() }
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn clear(&mut self) { self.items.clear(); }
}

const HOUR_MS: u64 = 3_600_000;
const MIN_MS: u64 = 60_000;

/// Parse a time phrase. `offset_s` is the local UTC offset in seconds,
/// for `at HH:MM`. Returns an absolute time in ms, always after `now_ms`.
pub fn when(text: &str, now_ms: u64, offset_s: i64) -> Option<u64> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("at ").unwrap_or(&t).trim();
    let t = t.strip_prefix("the ").unwrap_or(t).trim();
    // "in 10 min", "in 2 h", "in 90m"
    if let Some(rest) = text.trim().to_lowercase().strip_prefix("in ") {
        let rest = rest.trim();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().ok()?;
        let unit = rest[digits.len()..].trim();
        let ms = if unit.starts_with('h') { n * HOUR_MS }
                 else if unit.starts_with('m') || unit.is_empty() { n * MIN_MS }
                 else if unit.starts_with('s') { n * 1000 }
                 else { return None };
        return Some(now_ms + ms);
    }
    // "hour", "on the hour", ":00", "top of the hour"
    if t == "hour" || t == "hour mark" || t == ":00" || t == "top of the hour" || t == "next hour" {
        return Some(now_ms - now_ms % HOUR_MS + HOUR_MS);
    }
    // ":30", ":15", ":45" — the next such minute mark
    if let Some(m) = t.strip_prefix(':') {
        let m: u64 = m.parse().ok()?;
        if m >= 60 { return None; }
        let hour_start = now_ms - now_ms % HOUR_MS;
        let this = hour_start + m * MIN_MS;
        return Some(if this > now_ms { this } else { this + HOUR_MS });
    }
    // "14:30", "9:05", "14h"
    let (h, m) = if let Some((h, m)) = t.split_once(':') {
        (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?)
    } else if let Some(h) = t.strip_suffix('h') {
        (h.trim().parse::<u64>().ok()?, 0)
    } else {
        return None;
    };
    if h >= 24 || m >= 60 { return None; }
    let day_ms = 24 * HOUR_MS;
    let local_now = (now_ms as i64 + offset_s * 1000) as u64;
    let local_day_start = local_now - local_now % day_ms;
    let mut local_at = local_day_start + h * HOUR_MS + m * MIN_MS;
    if local_at <= local_now { local_at += day_ms; }
    Some((local_at as i64 - offset_s * 1000) as u64)
}

/// A rendering of "when" for a label: "in 12 min", "in 2 h 05".
pub fn until(at_ms: u64, now_ms: u64) -> String {
    let d = at_ms.saturating_sub(now_ms);
    let mins = (d + MIN_MS / 2) / MIN_MS;
    if mins < 1 { "now".into() }
    else if mins < 60 { format!("in {mins} min") }
    else { format!("in {} h {:02}", mins / 60, mins % 60) }
}

/// Split "send ada at the hour" into ("send ada", Some("at the hour")).
fn split_when(input: &str) -> (&str, Option<&str>) {
    let lower = input.to_lowercase();
    for key in [" at ", " in "] {
        if let Some(i) = lower.rfind(key) {
            return (input[..i].trim(), Some(input[i + 1..].trim()));
        }
    }
    (input.trim(), None)
}

/// Does `name` match `q` — a prefix of a word, or a subsequence?
fn matches(name: &str, q: &str) -> bool {
    let (n, q) = (name.to_lowercase(), q.to_lowercase());
    if q.is_empty() { return true; }
    if n.split_whitespace().any(|w| w.starts_with(&q)) || n.starts_with(&q) { return true; }
    let mut it = n.chars();
    q.chars().all(|c| it.any(|x| x == c))
}

/// Rank what the input could mean. `contacts` are `(sc_id, name)`.
/// Empty input lists every verb, so the palette teaches itself.
pub fn suggest(input: &str, now_ms: u64, offset_s: i64, contacts: &[(String, String)],
               in_call: bool, pages: &[&str]) -> Vec<Suggestion> {
    let (verb_part, when_part) = split_when(input);
    let at_ms = when_part.and_then(|w| when(w, now_ms, offset_s));
    // A time phrase that did not parse is not silently "now".
    if when_part.is_some() && at_ms.is_none() { return Vec::new(); }
    let suffix = at_ms.map(|t| format!(" · {}", until(t, now_ms))).unwrap_or_default();
    let mut words = verb_part.split_whitespace();
    let verb = words.next().unwrap_or("").to_lowercase();
    let rest: Vec<&str> = words.collect();
    let arg = rest.join(" ");
    let mut out = Vec::new();
    let push = |out: &mut Vec<Suggestion>, label: String, action: Action| {
        out.push(Suggestion { label: format!("{label}{suffix}"), action, at_ms });
    };

    if verb.is_empty() || "call".starts_with(&verb) || verb == "join" {
        if in_call && at_ms.is_none() {
            push(&mut out, "leave the call".into(), Action::Call { leave: true });
        } else {
            push(&mut out, "call".into(), Action::Call { leave: false });
        }
    }
    if ("hang".starts_with(&verb) || "leave".starts_with(&verb) || "end".starts_with(&verb)) && !verb.is_empty() {
        push(&mut out, "leave the call".into(), Action::Call { leave: true });
    }
    if verb.is_empty() || "send".starts_with(&verb) {
        for (id, name) in contacts {
            if matches(name, &arg) || matches(id, &arg) {
                push(&mut out, format!("send to {name}"), Action::SendTo { id: id.clone(), name: name.clone() });
            }
        }
    }
    if verb.is_empty() || "flip".starts_with(&verb) {
        push(&mut out, "flip camera".into(), Action::FlipCamera);
    }
    if verb.is_empty() || "snap".starts_with(&verb) || "photo".starts_with(&verb) {
        push(&mut out, "take a photo".into(), Action::Snap);
    }
    if verb.is_empty() || "help".starts_with(&verb) {
        push(&mut out, "help".into(), Action::Help);
    }
    if verb.is_empty() || "go".starts_with(&verb) || pages.iter().any(|p| p.to_lowercase().starts_with(&verb)) {
        let q = if "go".starts_with(&verb) && !verb.is_empty() { arg.to_lowercase() } else { verb.clone() };
        for (i, p) in pages.iter().enumerate() {
            if q.is_empty() || p.to_lowercase().starts_with(&q) {
                push(&mut out, format!("go to {p}"), Action::GoPage(i));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGES: [&str; 5] = ["Camera", "Inbox", "Friends", "Wayfinder", "Settings"];
    fn contacts() -> Vec<(String, String)> {
        vec![("sc-ada".into(), "Ada Lovelace".into()), ("sc-bob".into(), "Bob".into())]
    }
    // 2026-09-01 14:17:00 UTC
    const NOW: u64 = 1_788_531_420_000;

    #[test]
    fn the_hour_mark_is_the_next_top_of_the_hour() {
        let at = when("at the hour", NOW, 0).unwrap();
        assert_eq!(at % HOUR_MS, 0);
        assert!(at > NOW && at - NOW <= HOUR_MS);
        assert_eq!(when("the hour mark", NOW, 0), Some(at));
        assert_eq!(when(":00", NOW, 0), Some(at));
        assert_eq!(when("on the hour", NOW, 0), None, "\"on\" is not a phrase we parse; say so rather than guess");
        // :30 is later this hour (it is :17), :15 is next hour's
        assert_eq!(when(":30", NOW, 0).unwrap() - (NOW - NOW % HOUR_MS), 30 * MIN_MS);
        assert_eq!(when(":15", NOW, 0).unwrap() - (NOW - NOW % HOUR_MS), HOUR_MS + 15 * MIN_MS);
    }

    #[test]
    fn relative_and_wall_clock_times() {
        assert_eq!(when("in 10 min", NOW, 0), Some(NOW + 10 * MIN_MS));
        assert_eq!(when("in 90m", NOW, 0), Some(NOW + 90 * MIN_MS));
        assert_eq!(when("in 2 h", NOW, 0), Some(NOW + 2 * HOUR_MS));
        assert_eq!(when("in 45", NOW, 0), Some(NOW + 45 * MIN_MS), "bare number is minutes");
        assert_eq!(when("in soon", NOW, 0), None);
        // 14:30 UTC today is 13 minutes away; 09:00 is tomorrow's.
        assert_eq!(when("at 14:30", NOW, 0), Some(NOW + 13 * MIN_MS));
        assert_eq!(when("9:00", NOW, 0), Some(NOW + 18 * HOUR_MS + 43 * MIN_MS));
        // With a +2 h offset the local clock reads 16:17, so 16:30 is 13 min away.
        assert_eq!(when("at 16:30", NOW, 7200), Some(NOW + 13 * MIN_MS));
        assert_eq!(when("at 25:00", NOW, 0), None);
        assert_eq!(until(NOW + 13 * MIN_MS, NOW), "in 13 min");
        assert_eq!(until(NOW + 125 * MIN_MS, NOW), "in 2 h 05");
    }

    #[test]
    fn call_at_the_hour_is_one_scheduled_call() {
        let s = suggest("call at the hour", NOW, 0, &contacts(), false, &PAGES);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].action, Action::Call { leave: false });
        assert!(s[0].at_ms.is_some());
        assert!(s[0].label.starts_with("call · in "), "{}", s[0].label);
        // A time that does not parse is not "now".
        assert!(suggest("call at whenever", NOW, 0, &contacts(), false, &PAGES).is_empty());
    }

    #[test]
    fn send_finds_a_contact_by_prefix_and_by_subsequence() {
        let s = suggest("send ada", NOW, 0, &contacts(), false, &PAGES);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].action, Action::SendTo { id: "sc-ada".into(), name: "Ada Lovelace".into() });
        let s = suggest("send lvlc in 10 min", NOW, 0, &contacts(), false, &PAGES);
        assert_eq!(s.len(), 1, "subsequence of Lovelace: {s:?}");
        assert_eq!(s[0].at_ms, Some(NOW + 10 * MIN_MS));
        assert_eq!(suggest("send zed", NOW, 0, &contacts(), false, &PAGES).len(), 0);
        assert_eq!(suggest("send", NOW, 0, &contacts(), false, &PAGES).len(), 2, "no name lists everyone");
    }

    #[test]
    fn in_a_call_the_verb_becomes_leave_and_pages_are_reachable_by_name() {
        let s = suggest("call", NOW, 0, &[], true, &PAGES);
        assert_eq!(s[0].action, Action::Call { leave: true });
        let s = suggest("hang", NOW, 0, &[], true, &PAGES);
        assert_eq!(s[0].action, Action::Call { leave: true });
        let s = suggest("inb", NOW, 0, &[], false, &PAGES);
        assert_eq!(s, vec![Suggestion { label: "go to Inbox".into(), action: Action::GoPage(1), at_ms: None }]);
        let s = suggest("go way", NOW, 0, &[], false, &PAGES);
        assert_eq!(s[0].action, Action::GoPage(3));
        assert!(!suggest("", NOW, 0, &contacts(), false, &PAGES).is_empty(), "empty input teaches the verbs");
    }

    #[test]
    fn the_schedule_fires_in_order_and_reports_the_next() {
        let mut sch = Schedule::default();
        sch.add(Scheduled { at_ms: NOW + 20, label: "b".into(), action: Action::Snap });
        sch.add(Scheduled { at_ms: NOW + 10, label: "a".into(), action: Action::Help });
        assert_eq!(sch.next().unwrap().label, "a");
        assert!(sch.due(NOW + 5).is_empty());
        let d = sch.due(NOW + 15);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].label, "a");
        assert_eq!(sch.len(), 1);
        assert_eq!(sch.due(NOW + 100).len(), 1);
        assert!(sch.is_empty());
    }
}
