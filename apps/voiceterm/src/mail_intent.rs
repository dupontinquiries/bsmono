//! Natural-language mail commands → structured intents → pane actions.
//!
//! v0.1.862: w2q already turns spoken/typed phrases into [`crate::
//! voice_intent::Intent`] values and routes them to panes. This module adds
//! the *mail* vocabulary in the same shape: a small, self-contained parser
//! that recognizes phrases like
//!
//!   * "open my last couple emails from bob"
//!   * "show me unread mail"
//!   * "open my drafts"
//!   * "compose an email to alice@proton.me"
//!   * "search my email for invoice"
//!
//! and produces a [`MailIntent`]. The intent is then *executed* by
//! [`crate::mail::MailHub::execute_intent`] (which has the live session to
//! actually resolve "from bob" into concrete message ids) and returns a list
//! of [`PaneAction`]s the UI layer opens.
//!
//! Parsing here is deliberately pure and dependency-free so it unit-tests
//! without a session or network. `voice_intent::parse` can delegate to
//! [`parse`] when its own matchers see a mail-shaped phrase (the seam is a
//! one-line call); keeping the mail grammar in the mail module avoids
//! bloating the global intent matcher and lets the two evolve independently.

/// Proton system label ids (stable; mirror hydroxide's constants).
pub mod label {
    pub const INBOX: &str = "0";
    pub const ALL_DRAFTS: &str = "1";
    pub const ALL_SENT: &str = "2";
    pub const TRASH: &str = "3";
    pub const SPAM: &str = "4";
    pub const ALL_MAIL: &str = "5";
    pub const ARCHIVE: &str = "6";
    pub const SENT: &str = "7";
    pub const DRAFTS: &str = "8";
    pub const STARRED: &str = "10";
}

/// A relative date window parsed from a phrase. The parser stays pure by
/// emitting this descriptor; the hub resolves it to concrete Unix-second
/// bounds against the current clock (see `MailHub`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    /// "in the last N days" / "past N days".
    LastDays(u32),
}

/// A selection of messages by recency, optional sender, optional subject,
/// and optional date window — shared by the open/action/reply/attachment
/// intents so they all resolve the same way.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Selector {
    pub count: usize,
    pub from: Option<String>,
    pub about: Option<String>,
    pub when: Option<When>,
    /// True when the phrase said "all" (open every match, capped) rather than
    /// a specific small number.
    pub all: bool,
}

/// A mutating action applied to selected messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailAction {
    MarkRead,
    MarkUnread,
    Archive,
    Trash,
    Star,
    Unstar,
}

impl MailAction {
    pub fn verb(&self) -> &'static str {
        match self {
            MailAction::MarkRead => "marked read",
            MailAction::MarkUnread => "marked unread",
            MailAction::Archive => "archived",
            MailAction::Trash => "trashed",
            MailAction::Star => "starred",
            MailAction::Unstar => "unstarred",
        }
    }
}

/// A recognized mail command. Resolution to concrete messages happens later,
/// in the hub, because it needs a live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailIntent {
    /// Open the inbox pane. `unread_only` filters to unread.
    OpenInbox { unread_only: bool },
    /// Open the drafts pane.
    OpenDrafts,
    /// Open the most recent matching messages (each in its own pane),
    /// narrowed by the selector's sender / subject / date window / count.
    OpenMessages(Selector),
    /// Open a search results pane for a free-text query.
    Search { query: String },
    /// Open a compose pane, optionally pre-addressed.
    Compose { to: Option<String> },
    /// Apply a mutating action to the selected messages.
    Act { action: MailAction, sel: Selector },
    /// Reply to the selected (usually single, most-recent) message.
    Reply { sel: Selector },
    /// Open the attachment graph/list for the selected messages.
    Attachments { sel: Selector },
}

/// An action the UI layer performs in response to an executed intent. The
/// hub fills these with concrete ids once it has queried the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneAction {
    /// Open the inbox list pane (optionally filtered to unread).
    OpenInbox { unread_only: bool },
    /// Open the drafts list pane.
    OpenDrafts,
    /// Open a single message view pane by Proton message id, with a
    /// human-friendly title for the tab.
    OpenMessage { id: String, title: String },
    /// Open a search-results pane for the query (already executed; the hub
    /// passes the matched ids so the pane can render without re-querying).
    OpenSearch { query: String, ids: Vec<String> },
    /// Open a compose pane, optionally pre-addressed.
    OpenCompose { to: Option<String> },
    /// Open an attachments pane (graph + list) for one or more messages.
    OpenAttachments { ids: Vec<String>, title: String },
    /// Re-render the current list pane (after a mutating action).
    Refresh,
    /// Nothing matched the query (e.g. "from bob" but no mail from bob). The
    /// UI shows this as a transient notice rather than an empty pane.
    Notice(String),
}

impl MailIntent {
    /// The base [`crate::mail_proton::MessageFilter`] for list-style intents.
    /// Sender/subject substrings are *not* expressed here (Proton's list
    /// endpoint filters by label/address, not arbitrary substrings); the hub
    /// applies those as a post-filter over the returned headers.
    pub fn base_filter(&self) -> crate::mail_proton::MessageFilter {
        use crate::mail_proton::MessageFilter;
        let mut f = MessageFilter::default();
        match self {
            MailIntent::OpenInbox { .. } => f.label = label::INBOX.to_string(),
            MailIntent::OpenDrafts => f.label = label::DRAFTS.to_string(),
            MailIntent::OpenMessages(sel) => {
                f.label = label::ALL_MAIL.to_string();
                f.page_size = (sel.count as i64 * 5).clamp(15, 50);
            }
            MailIntent::Search { .. } => f.label = label::ALL_MAIL.to_string(),
            MailIntent::Compose { .. } => {}
            MailIntent::Act { sel, .. }
            | MailIntent::Reply { sel }
            | MailIntent::Attachments { sel } => {
                f.label = label::ALL_MAIL.to_string();
                f.page_size = (sel.count as i64 * 5).clamp(15, 50);
            }
        }
        f
    }
}

/// Parse a phrase into a [`MailIntent`], or `None` if it isn't a mail
/// command. Case-insensitive; tolerant of light politeness/filler since the
/// global voice pipeline already strips most of it before we see it.
pub fn parse(text: &str) -> Option<MailIntent> {
    let t = normalize(text);
    if t.is_empty() {
        return None;
    }

    // Gate: only treat as mail if a mail noun is present, so we don't
    // hijack unrelated phrases. Reply/attachment/compose verbs also qualify.
    let has_mail_verb = t.starts_with("compose")
        || t.starts_with("draft")
        || t.starts_with("reply")
        || t.contains("attachment");
    if !mentions_mail(&t) && !has_mail_verb {
        return None;
    }

    // Order matters: specific list/read verbs are checked before the
    // catch-all compose, because words like "email" appear in every mail
    // phrase and must not hijack "search ... for X" or "open ... from Y".

    // Resolve sender and date-window up front — they decide whether an
    // ambiguous phrase ("find the email from alice yesterday") is a keyword
    // search or an open-these-messages query, and they let question forms
    // ("what is the email alice sent me yesterday") route to open without an
    // explicit open-verb.
    let sender = extract_sender(&t);
    let window = extract_when(&t);

    // Drafts.
    if t.contains("draft") {
        return Some(MailIntent::OpenDrafts);
    }

    // Attachments ("show attachments from bob", "open the attachment...").
    if t.contains("attachment") {
        return Some(MailIntent::Attachments { sel: selector(&t, 1) });
    }

    // Reply.
    if t.starts_with("reply") || t.contains("reply to") {
        return Some(MailIntent::Reply { sel: selector(&t, 1) });
    }

    // Mutating actions. Checked before the open/read branch because phrases
    // like "mark ... read" contain "read". Default applies to all matched
    // when a sender is named without an explicit count.
    if let Some(action) = detect_action(&t) {
        let default = if sender.is_some() { 25 } else { 1 };
        return Some(MailIntent::Act {
            action,
            sel: selector(&t, default),
        });
    }

    // Search — but only a *pure* keyword search (no sender, no date window).
    // "find the email from alice yesterday" has both, so it falls through to
    // the open-these-messages query below instead.
    if (t.contains("search") || t.contains("find")) && sender.is_none() && window.is_none() {
        if let Some(rest) = first_match(&t, &["search", "find"]) {
            if let Some(q) = extract_after(rest, &["for", "about", "containing", "with"]) {
                return Some(MailIntent::Search { query: strip_time_words(&trim_trailing_nouns(&q)) });
            }
            let q = strip_mail_nouns(rest);
            if !q.is_empty() {
                return Some(MailIntent::Search { query: q });
            }
        }
    }

    // Inbox / unread. Note: "new email"/"new mail" mean *compose*, so the
    // unread cue is "unread" or "new message(s)", never bare "new mail".
    let unread = t.contains("unread") || t.contains("new message");
    if t.contains("inbox")
        || (unread && sender.is_none() && window.is_none() && !t.contains("last"))
    {
        return Some(MailIntent::OpenInbox { unread_only: unread });
    }

    // Open / read these messages. Fires on an explicit verb OR when the
    // phrase named a sender or a date window — which covers question forms
    // ("what is the email that alice sent me yesterday?") and bare
    // references ("all my emails today about fire") with no command verb.
    let open_verb = t.contains("open")
        || t.contains("read")
        || t.contains("show")
        || t.contains("last")
        || t.contains("recent")
        || t.contains("display")
        || t.contains("pull up")
        || t.contains("get my")
        || t.contains("what")
        || t.contains("which");
    if open_verb || sender.is_some() || window.is_some() {
        return Some(MailIntent::OpenMessages(selector(&t, 1)));
    }

    // Compose / new mail — last, and only on an explicit compose verb (or a
    // bare "email <recipient>" with no list/read verb above having fired).
    if let Some(rest) = first_match(&t, &["compose", "write", "new email", "new mail", "email"]) {
        let to = extract_after(rest, &["to", "for"]).or_else(|| {
            rest.split_whitespace()
                .find(|w| looks_like_recipient(w))
                .map(|s| s.to_string())
        });
        return Some(MailIntent::Compose {
            to: to.map(|s| trim_trailing_nouns(&s)),
        });
    }

    None
}

/// Detect a mutating action verb in the phrase.
fn detect_action(t: &str) -> Option<MailAction> {
    if t.contains("mark") && (t.contains("unread")) {
        return Some(MailAction::MarkUnread);
    }
    if t.contains("mark") && t.contains("read") {
        return Some(MailAction::MarkRead);
    }
    if t.contains("archive") {
        return Some(MailAction::Archive);
    }
    if t.contains("trash") || t.contains("delete") {
        return Some(MailAction::Trash);
    }
    if t.contains("unstar") {
        return Some(MailAction::Unstar);
    }
    if t.contains("star") || t.contains("flag") {
        return Some(MailAction::Star);
    }
    None
}

/// Cap on how many tabs an "all" query opens, to avoid a tab explosion.
pub const ALL_CAP: usize = 25;

/// Build a [`Selector`] from a phrase, with `default_count` when no quantity
/// is stated. Populates the date window, the sender, the subject term, and
/// the `all` flag.
fn selector(t: &str, default_count: usize) -> Selector {
    let when = extract_when(t);
    let from = extract_sender(t);
    let about = extract_after(t, &["about", "regarding", "re", "containing", "mentioning"])
        .map(|s| strip_time_words(&trim_trailing_nouns(&s)))
        .filter(|s| !s.is_empty());
    let plural = t.contains("emails") || t.contains("messages");
    let all_word = has_word(t, "all") || has_word(t, "every") || t.contains("everything");
    // "all"/"every", or a plural noun scoped by a date/subject, means "open
    // every match" (capped) rather than a single most-recent message.
    let all = all_word || (plural && (when.is_some() || about.is_some()));
    let count = extract_count(t).unwrap_or(if all { ALL_CAP } else { default_count });
    Selector {
        count,
        from,
        about,
        when,
        all,
    }
}

impl When {
    /// Resolve to a half-open `[begin, end)` Unix-second range relative to
    /// `now`. Day boundaries use UTC (no timezone dependency); week/month
    /// are calendar-aligned (week starts Monday).
    pub fn resolve(&self, now: i64) -> (i64, i64) {
        let day = now.div_euclid(86_400);
        let day_start = day * 86_400;
        match self {
            When::Today => (day_start, day_start + 86_400),
            When::Yesterday => (day_start - 86_400, day_start),
            When::ThisWeek => {
                let wk = week_start(day) * 86_400;
                (wk, now + 1)
            }
            When::LastWeek => {
                let wk = week_start(day) * 86_400;
                (wk - 7 * 86_400, wk)
            }
            When::ThisMonth => {
                let (y, m, _) = civil_from_days(day);
                let first = days_from_civil(y, m, 1) * 86_400;
                (first, now + 1)
            }
            When::LastDays(n) => (now - (*n as i64) * 86_400, now + 1),
        }
    }
}

/// Monday-based start-of-week day number for a given day number.
fn week_start(day: i64) -> i64 {
    // 1970-01-01 (day 0) was a Thursday. weekday with Sunday=0:
    let dow_sun0 = (day.rem_euclid(7) + 4).rem_euclid(7); // 0=Sun..6=Sat
    let since_monday = (dow_sun0 + 6).rem_euclid(7); // 0 if Monday
    day - since_monday
}

/// Civil date from a day number (proleptic Gregorian; Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day number from a civil date (inverse of [`civil_from_days`]).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Detect a date window from natural phrasing.
fn extract_when(t: &str) -> Option<When> {
    if t.contains("yesterday") {
        return Some(When::Yesterday);
    }
    if t.contains("today") || t.contains("this morning") || t.contains("tonight") {
        return Some(When::Today);
    }
    // "last N days" / "past N days" / "in the last N days".
    if let Some(n) = last_n_days(t) {
        return Some(When::LastDays(n));
    }
    if t.contains("this week") {
        return Some(When::ThisWeek);
    }
    if t.contains("last week") {
        return Some(When::LastWeek);
    }
    if t.contains("past week") {
        return Some(When::LastDays(7));
    }
    if t.contains("this month") {
        return Some(When::ThisMonth);
    }
    if t.contains("last month") || t.contains("past month") {
        return Some(When::LastDays(30));
    }
    None
}

/// Parse "<n> days"/"<n> day" appearing in the phrase (number word or digit).
fn last_n_days(t: &str) -> Option<u32> {
    let w: Vec<&str> = t.split_whitespace().collect();
    for i in 0..w.len() {
        if (w[i] == "days" || w[i] == "day") && i > 0 {
            if let Some(n) = word_to_num(w[i - 1]) {
                if n >= 1 {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn word_to_num(w: &str) -> Option<u32> {
    match w {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "ten" => Some(10),
        "fourteen" => Some(14),
        "thirty" => Some(30),
        _ => w.parse::<u32>().ok(),
    }
}

/// Extract the sender from "from X", "sent by X", or "X sent (me)".
fn extract_sender(t: &str) -> Option<String> {
    if let Some(raw) = extract_after_word(t, "from") {
        // "from the last 3 days" / "from yesterday" is a *time* expression,
        // not a sender — cleaning it of date words leaves nothing.
        let cleaned = clean_sender(&raw);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    if t.contains("sent by") {
        if let Some(s) = extract_after(t, &["by"]) {
            let first = s.split_whitespace().next().unwrap_or("").to_string();
            if !first.is_empty() {
                return Some(first);
            }
        }
    }
    let w: Vec<&str> = t.split_whitespace().collect();
    if let Some(i) = w.iter().position(|x| *x == "sent" || *x == "emailed") {
        if i > 0 {
            let cand = w[i - 1];
            if !is_function_word(cand) {
                return Some(cand.to_string());
            }
        }
    }
    None
}

/// Strip date words, articles, and bare numbers from a candidate sender. An
/// empty result means the phrase was temporal ("the last 3 days"), not a name.
fn clean_sender(s: &str) -> String {
    strip_time_words(s)
        .split_whitespace()
        .filter(|w| !matches!(*w, "the" | "a" | "an") && w.parse::<u64>().is_err())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn is_function_word(w: &str) -> bool {
    matches!(
        w,
        "that" | "who" | "you" | "i" | "we" | "they" | "it" | "this" | "a" | "an"
            | "someone" | "somebody" | "and" | "has" | "have" | "had" | "just" | "recently"
    )
}

/// Remove date/time tokens so a captured sender/subject doesn't trail them
/// (e.g. "alice yesterday" → "alice", "fire today" → "fire").
fn strip_time_words(s: &str) -> String {
    s.split_whitespace()
        .filter(|w| {
            !matches!(
                *w,
                "today" | "yesterday" | "tonight" | "week" | "weeks" | "month" | "months"
                    | "day" | "days" | "this" | "last" | "past" | "recent" | "recently"
                    | "morning" | "afternoon" | "evening"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// Whole-word containment test.
fn has_word(t: &str, word: &str) -> bool {
    t.split_whitespace().any(|w| w == word)
}

// ---------------------------------------------------------------------
// helpers (all pure)
// ---------------------------------------------------------------------

fn normalize(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let trimmed = lower
        .trim_start_matches("hey ")
        .trim_start_matches("ok ")
        .trim_start_matches("please ")
        .trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ';') || c.is_whitespace());
    // Collapse "my"/"the" possessives that don't affect meaning.
    trimmed
        .split_whitespace()
        .filter(|w| !matches!(*w, "my" | "the" | "me" | "some" | "any"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn mentions_mail(t: &str) -> bool {
    const NOUNS: [&str; 7] = ["email", "emails", "mail", "inbox", "message", "messages", "draft"];
    NOUNS.iter().any(|n| t.split_whitespace().any(|w| w == *n || w.starts_with(n)))
}

fn strip_mail_nouns(t: &str) -> String {
    t.split_whitespace()
        .filter(|w| !matches!(*w, "email" | "emails" | "mail" | "message" | "messages" | "inbox"))
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// Drop trailing mail nouns from an extracted value, e.g. "bob emails" →
/// "bob", "invoice email" → "invoice".
fn trim_trailing_nouns(s: &str) -> String {
    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| !matches!(*w, "email" | "emails" | "mail" | "message" | "messages"))
        .collect();
    words.join(" ").trim().to_string()
}

/// If `t` contains any of `verbs`, return the remainder after the first one.
fn first_match<'a>(t: &'a str, verbs: &[&str]) -> Option<&'a str> {
    for v in verbs {
        if let Some(pos) = t.find(v) {
            return Some(t[pos + v.len()..].trim_start());
        }
    }
    None
}

/// Return the text following the first of `markers` (as whole words).
fn extract_after(t: &str, markers: &[&str]) -> Option<String> {
    let words: Vec<&str> = t.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if markers.contains(w) {
            let rest = words[i + 1..].join(" ");
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Return the single word (or short phrase up to the next marker) after a
/// given marker word — used for "from bob".
fn extract_after_word(t: &str, marker: &str) -> Option<String> {
    let words: Vec<&str> = t.split_whitespace().collect();
    let stops = ["about", "regarding", "re", "from", "to", "with", "containing"];
    for (i, w) in words.iter().enumerate() {
        if *w == marker {
            let mut out = Vec::new();
            for nw in &words[i + 1..] {
                if stops.contains(nw) {
                    break;
                }
                out.push(*nw);
            }
            let s = out.join(" ").trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Parse a count from number words or digits: "a"/"one"=1, "couple"=2,
/// "few"/"several"=3, or a literal integer. Returns None if no quantity.
fn extract_count(t: &str) -> Option<usize> {
    // Two passes so the indefinite article ("a"/"an") never overrides an
    // explicit quantity later in the phrase: "a couple" is 2, not 1. Numbers
    // that quantify a *duration* ("last 3 days") are skipped — those belong
    // to the date window, not the message count.
    const TIME_UNITS: [&str; 8] = [
        "day", "days", "week", "weeks", "month", "months", "hour", "hours",
    ];
    let words: Vec<&str> = t.split_whitespace().collect();
    let mut article: Option<usize> = None;
    for (i, w) in words.iter().enumerate() {
        let next_is_time = words.get(i + 1).map(|n| TIME_UNITS.contains(n)).unwrap_or(false);
        if next_is_time {
            continue;
        }
        match *w {
            "a" | "an" | "single" => article = article.or(Some(1)),
            "one" => return Some(1),
            "couple" | "two" => return Some(2),
            "few" | "several" | "three" => return Some(3),
            "four" => return Some(4),
            "five" => return Some(5),
            "ten" => return Some(10),
            _ => {
                if let Ok(n) = w.parse::<usize>() {
                    if n >= 1 && n <= 100 {
                        return Some(n);
                    }
                }
            }
        }
    }
    article
}

fn looks_like_recipient(w: &str) -> bool {
    w.contains('@') || (w.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) && w.len() > 1)
}

/// v0.1.866: the Proton domains this bridge supports. The "add my email" command
/// accepts these and denies everything else with a message.
pub fn is_proton_address(email: &str) -> bool {
    let e = email.trim().trim_matches(|c| c == '<' || c == '>').to_lowercase();
    match e.rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() => {
            matches!(domain, "proton.me" | "protonmail.com" | "protonmail.ch" | "pm.me")
        }
        _ => false,
    }
}

/// v0.1.866: recognize a pre-login "add/register my email" command. Returns
/// `Some(addr_opt)` when the phrase IS such a command — `addr_opt` is the email
/// mentioned, if any — and `None` when it is not a register command at all (so
/// the caller can fall through to other intents).
pub fn parse_register_email(text: &str) -> Option<Option<String>> {
    let t = text.trim().to_lowercase();
    const TRIGGERS: &[&str] = &[
        "register email", "register my email", "register an email", "register account",
        "add my email", "add an email", "add email", "add my account", "add account",
        "connect my email", "connect email", "sign in to email", "log in to email",
        "set up my email", "set up email",
    ];
    if !TRIGGERS.iter().any(|tr| t.contains(tr)) {
        return None;
    }
    Some(first_email(&t))
}

/// First email-looking token in a phrase: a word with a non-empty local part, an
/// `@`, and a dotted domain. Punctuation/brackets trimmed.
fn first_email(t: &str) -> Option<String> {
    for raw in t.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let w = raw.trim_matches(|c: char| "<>()\"'.".contains(c));
        if let Some((local, domain)) = w.split_once('@') {
            if !local.is_empty() && domain.contains('.') && !domain.starts_with('.') {
                return Some(w.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proton_address_detection() {
        assert!(is_proton_address("alice@proton.me"));
        assert!(is_proton_address("Bob@ProtonMail.com"));
        assert!(is_proton_address("x@pm.me"));
        assert!(is_proton_address("y@protonmail.ch"));
        assert!(!is_proton_address("alice@gmail.com"));
        assert!(!is_proton_address("nope"));
        assert!(!is_proton_address("@proton.me")); // empty local part
    }

    #[test]
    fn register_email_command() {
        assert_eq!(parse_register_email("add my email alice@proton.me"),
                   Some(Some("alice@proton.me".into())));
        assert_eq!(parse_register_email("register email"), Some(None));
        assert_eq!(parse_register_email("add account bob@gmail.com"),
                   Some(Some("bob@gmail.com".into())));
        assert_eq!(parse_register_email("connect my email to alice@pm.me"),
                   Some(Some("alice@pm.me".into())));
        // not register commands → None (fall through)
        assert_eq!(parse_register_email("open my inbox"), None);
        assert_eq!(parse_register_email("what is the weather"), None);
    }

    fn sel(count: usize, from: Option<&str>, about: Option<&str>, when: Option<When>, all: bool) -> Selector {
        Selector {
            count,
            from: from.map(|s| s.to_string()),
            about: about.map(|s| s.to_string()),
            when,
            all,
        }
    }

    #[test]
    fn hero_phrase_last_couple_from_bob() {
        assert_eq!(
            parse("open my last couple emails from bob").unwrap(),
            MailIntent::OpenMessages(sel(2, Some("bob"), None, None, false))
        );
    }

    #[test]
    fn last_email_from_sender_about_subject() {
        assert_eq!(
            parse("read the last email from alice about the invoice").unwrap(),
            MailIntent::OpenMessages(sel(1, Some("alice"), Some("invoice"), None, false))
        );
    }

    #[test]
    fn question_form_with_sender_and_yesterday() {
        // The hero case from the request: no command verb, just a question.
        assert_eq!(
            parse("what is the email that alice sent me yesterday?").unwrap(),
            MailIntent::OpenMessages(sel(1, Some("alice"), None, Some(When::Yesterday), false))
        );
    }

    #[test]
    fn all_today_about_fire_opens_many() {
        let i = parse("show all my emails today about fire").unwrap();
        assert_eq!(
            i,
            MailIntent::OpenMessages(sel(ALL_CAP, None, Some("fire"), Some(When::Today), true))
        );
    }

    #[test]
    fn plural_with_date_implies_all() {
        // "my emails from this week" — plural + window, no verb, no count.
        let i = parse("my emails from this week").unwrap();
        match i {
            MailIntent::OpenMessages(s) => {
                assert_eq!(s.when, Some(When::ThisWeek));
                assert!(s.all);
                assert_eq!(s.count, ALL_CAP);
            }
            other => panic!("expected OpenMessages, got {other:?}"),
        }
    }

    #[test]
    fn from_last_n_days_is_window_not_sender() {
        let i = parse("open my emails from the last 3 days").unwrap();
        match i {
            MailIntent::OpenMessages(s) => {
                assert_eq!(s.when, Some(When::LastDays(3)));
                assert_eq!(s.from, None, "'from the last 3 days' is temporal");
            }
            other => panic!("expected OpenMessages, got {other:?}"),
        }
    }

    #[test]
    fn sent_by_form() {
        let i = parse("open the email sent by bob today").unwrap();
        match i {
            MailIntent::OpenMessages(s) => {
                assert_eq!(s.from.as_deref(), Some("bob"));
                assert_eq!(s.when, Some(When::Today));
            }
            other => panic!("expected OpenMessages, got {other:?}"),
        }
    }

    #[test]
    fn inbox_and_unread() {
        assert_eq!(parse("show my inbox").unwrap(), MailIntent::OpenInbox { unread_only: false });
        assert_eq!(parse("show unread mail").unwrap(), MailIntent::OpenInbox { unread_only: true });
    }

    #[test]
    fn drafts() {
        assert_eq!(parse("open my drafts").unwrap(), MailIntent::OpenDrafts);
        assert_eq!(parse("show draft emails").unwrap(), MailIntent::OpenDrafts);
    }

    #[test]
    fn compose_to() {
        assert_eq!(
            parse("compose an email to alice@proton.me").unwrap(),
            MailIntent::Compose { to: Some("alice@proton.me".into()) }
        );
        assert_eq!(parse("new email").unwrap(), MailIntent::Compose { to: None });
    }

    #[test]
    fn search() {
        assert_eq!(
            parse("search my email for invoice").unwrap(),
            MailIntent::Search { query: "invoice".into() }
        );
        assert_eq!(
            parse("find emails about the quarterly report").unwrap(),
            MailIntent::Search { query: "quarterly report".into() }
        );
    }

    #[test]
    fn find_with_sender_routes_to_open_not_search() {
        // "find" + sender + date → open those messages, not a keyword search.
        let i = parse("find the email from alice yesterday").unwrap();
        assert!(matches!(i, MailIntent::OpenMessages(_)), "got {i:?}");
    }

    #[test]
    fn ignores_non_mail() {
        assert!(parse("scroll down").is_none());
        assert!(parse("open google.com").is_none());
        assert!(parse("what's the weather").is_none());
    }

    #[test]
    fn count_words() {
        assert_eq!(extract_count("last five messages"), Some(5));
        assert_eq!(extract_count("a couple"), Some(2));
        assert_eq!(extract_count("open 7 emails"), Some(7));
        assert_eq!(extract_count("open emails"), None);
        // duration numbers are not message counts
        assert_eq!(extract_count("emails from the last 3 days"), None);
    }

    #[test]
    fn when_resolution() {
        // 2023-11-15 12:00:00 UTC = 1_700_049_600
        let now = 1_700_049_600;
        let (b, e) = When::Today.resolve(now);
        assert_eq!(b, 1_700_006_400); // 2023-11-15 00:00 UTC
        assert_eq!(e, 1_700_006_400 + 86_400);
        let (yb, ye) = When::Yesterday.resolve(now);
        assert_eq!(ye, b);
        assert_eq!(yb, b - 86_400);
        let (lb, le) = When::LastDays(3).resolve(now);
        assert_eq!(lb, now - 3 * 86_400);
        assert_eq!(le, now + 1);
        // this-week begins on a Monday (2023-11-13).
        let (wb, _) = When::ThisWeek.resolve(now);
        assert_eq!(wb, super::days_from_civil(2023, 11, 13) * 86_400);
    }

    #[test]
    fn base_filter_labels() {
        assert_eq!(
            MailIntent::OpenInbox { unread_only: true }.base_filter().label,
            label::INBOX
        );
        assert_eq!(MailIntent::OpenDrafts.base_filter().label, label::DRAFTS);
        let f = MailIntent::OpenMessages(sel(2, None, None, None, false)).base_filter();
        assert!(f.page_size >= 15);
    }

    #[test]
    fn actions_parse() {
        assert_eq!(
            parse("archive the last email from bob").unwrap(),
            MailIntent::Act { action: MailAction::Archive, sel: sel(25, Some("bob"), None, None, false) }
        );
        assert!(matches!(
            parse("mark this email as read"),
            Some(MailIntent::Act { action: MailAction::MarkRead, .. })
        ));
        assert!(matches!(
            parse("mark the last email from alice unread"),
            Some(MailIntent::Act { action: MailAction::MarkUnread, .. })
        ));
        assert!(matches!(
            parse("delete the last email from spam"),
            Some(MailIntent::Act { action: MailAction::Trash, .. })
        ));
        assert!(matches!(
            parse("star the email from bob"),
            Some(MailIntent::Act { action: MailAction::Star, .. })
        ));
    }

    #[test]
    fn reply_parses() {
        assert!(matches!(
            parse("reply to the last email from bob"),
            Some(MailIntent::Reply { .. })
        ));
    }

    #[test]
    fn attachments_parse() {
        assert_eq!(
            parse("show me the attachments from bob").unwrap(),
            MailIntent::Attachments { sel: sel(1, Some("bob"), None, None, false) }
        );
    }

    #[test]
    fn mark_read_not_treated_as_open() {
        assert!(matches!(parse("mark all my email read"), Some(MailIntent::Act { action: MailAction::MarkRead, .. })));
    }
}
