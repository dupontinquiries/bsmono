//! token_log — the JSONL record and window policy from `token_log.rs`.
//!
//! Token accounting for LLM calls: one JSON object per line, *"easy to grep /
//! tail / rm"*.
//!
//! ## The "does NOT do" list is part of the design
//!
//! The original states three exclusions, and each is a decision rather than an
//! omission. It *"doesn't know pricing"* — those fields live on the backend and
//! are user-entered text, so cost is computed at render time. It *"doesn't
//! enforce limits"*, deferred explicitly. And it *"doesn't aggregate across
//! sessions in memory"*: each query re-reads the file, because *"for typical
//! usage (hundreds of events) this is fine"*.
//!
//! Carrying those forward matters more than carrying the code. A port that
//! helpfully added a cost field would bake a pricing model into a log that was
//! deliberately kept free of one — and prices change while logs are forever.
//!
//! ## Rolling seven days, not a calendar week
//!
//! The comment gives the reason: *"power users care about 'how much did I spend
//! this week's-worth-of-work'"*. A calendar week resets on Monday morning,
//! which is exactly when the question stops being useful.
//!
//! ## An unknown window falls back rather than failing
//!
//! *"Falls back to AllTime when unknown so stale/typo URLs don't break the
//! page."* A stats page is what you open when something is already wrong; it
//! should not be the second thing that is.

/// What span of events a view covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimeWindow { Session, Today, Last7d, AllTime }

impl TimeWindow {
    pub const ALL: [TimeWindow; 4] =
        [TimeWindow::Session, TimeWindow::Today, TimeWindow::Last7d, TimeWindow::AllTime];

    /// Parse a `?window=` value, tolerantly.
    pub fn parse(s: &str) -> TimeWindow {
        match s.trim().to_ascii_lowercase().as_str() {
            "session" => TimeWindow::Session,
            "today" => TimeWindow::Today,
            "7d" | "week" | "7days" | "last7d" => TimeWindow::Last7d,
            _ => TimeWindow::AllTime,
        }
    }

    /// URL slug — the inverse of [`Self::parse`].
    pub fn slug(self) -> &'static str {
        match self {
            TimeWindow::Session => "session",
            TimeWindow::Today => "today",
            TimeWindow::Last7d => "7d",
            TimeWindow::AllTime => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeWindow::Session => "This session",
            TimeWindow::Today => "Today",
            TimeWindow::Last7d => "Last 7 days",
            TimeWindow::AllTime => "All time",
        }
    }

    /// Does `ts_ms` fall inside this window?
    ///
    /// `session_start_ms` is when the app launched. Rolling rather than
    /// calendar-aligned for Last7d — see the module docs.
    pub fn contains(self, ts_ms: u64, now_ms: u64, session_start_ms: u64) -> bool {
        const DAY: u64 = 86_400_000;
        match self {
            TimeWindow::AllTime => true,
            TimeWindow::Session => ts_ms >= session_start_ms,
            // Rolling 24h rather than midnight-to-now: a call at 23:50 should
            // still count at 00:10, when it is the thing you just did.
            TimeWindow::Today => now_ms.saturating_sub(ts_ms) < DAY,
            TimeWindow::Last7d => now_ms.saturating_sub(ts_ms) < DAY * 7,
        }
    }
}

/// One accounted call.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenEvent {
    pub ts_ms: u64,
    pub op: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Tokens served from a provider-side cache, billed differently.
    pub cached_input_tokens: u32,
}

impl TokenEvent {
    /// Tokens actually paid for, if a caller wants one number.
    ///
    /// Cached input is EXCLUDED rather than added: it is the part that was not
    /// re-processed, and counting it inflates every total on a backend that
    /// caches well — which would make the better-behaved provider look worse.
    pub fn billable_input(&self) -> u32 {
        self.input_tokens.saturating_sub(self.cached_input_tokens)
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens as u64 + self.output_tokens as u64
    }
}

fn json_escape(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// One line of the log.
pub fn serialize(e: &TokenEvent) -> String {
    let mut out = String::with_capacity(128);
    out.push_str("{\"ts\":");
    out.push_str(&e.ts_ms.to_string());
    out.push_str(",\"op\":");
    json_escape(&mut out, &e.op);
    out.push_str(",\"model\":");
    json_escape(&mut out, &e.model);
    out.push_str(&format!(",\"in\":{}", e.input_tokens));
    out.push_str(&format!(",\"out\":{}", e.output_tokens));
    out.push_str(&format!(",\"cached_in\":{}}}", e.cached_input_tokens));
    out
}

fn field_str(line: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn field_num(line: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if end == 0 { return None; }
    rest[..end].parse().ok()
}

/// Parse a line, or `None` when it is not a usable record.
///
/// A malformed line is skipped rather than aborting the read: the log is
/// append-only and a torn final write is the normal way it ends after a crash.
/// Losing the whole history to one bad tail would defeat the point of keeping
/// it.
pub fn parse(line: &str) -> Option<TokenEvent> {
    let ts_ms = field_num(line, "ts")?;
    Some(TokenEvent {
        ts_ms,
        op: field_str(line, "op")?,
        model: field_str(line, "model")?,
        input_tokens: field_num(line, "in")? as u32,
        output_tokens: field_num(line, "out")? as u32,
        // Absent on records written before caching existed — zero rather than
        // a parse failure, so an older log still reads.
        cached_input_tokens: field_num(line, "cached_in").unwrap_or(0) as u32,
    })
}

/// Totals for one op.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpTotals {
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

/// Per-op totals within a window, sorted by total tokens descending.
///
/// Sorted by cost rather than alphabetically: a stats page is opened to find
/// what is expensive, and an alphabetical list makes that a search.
pub fn totals_by_op(events: &[TokenEvent], w: TimeWindow, now_ms: u64, session_ms: u64)
    -> Vec<(String, OpTotals)> {
    let mut map: std::collections::HashMap<String, OpTotals> = Default::default();
    for e in events.iter().filter(|e| w.contains(e.ts_ms, now_ms, session_ms)) {
        let t = map.entry(e.op.clone()).or_default();
        t.calls += 1;
        t.input += e.input_tokens as u64;
        t.output += e.output_tokens as u64;
        t.cached += e.cached_input_tokens as u64;
    }
    let mut v: Vec<(String, OpTotals)> = map.into_iter().collect();
    v.sort_by(|a, b| (b.1.input + b.1.output).cmp(&(a.1.input + a.1.output))
        .then_with(|| a.0.cmp(&b.0)));
    v
}

/// The `n` largest calls in a window — the "spikes" view.
pub fn top_spikes(events: &[TokenEvent], w: TimeWindow, now_ms: u64, session_ms: u64, n: usize)
    -> Vec<&TokenEvent> {
    let mut v: Vec<&TokenEvent> = events.iter()
        .filter(|e| w.contains(e.ts_ms, now_ms, session_ms)).collect();
    // Newest first among equals, so a repeated spike shows the one you can
    // still remember making.
    v.sort_by(|a, b| b.total_tokens().cmp(&a.total_tokens())
        .then_with(|| b.ts_ms.cmp(&a.ts_ms)));
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: u64, op: &str, inp: u32, out: u32) -> TokenEvent {
        TokenEvent { ts_ms: ts, op: op.into(), model: "gemma".into(),
                     input_tokens: inp, output_tokens: out, cached_input_tokens: 0 }
    }

    const DAY: u64 = 86_400_000;

    #[test]
    fn a_record_round_trips() {
        let e = TokenEvent { ts_ms: 1_700_000_000_000, op: "summary".into(),
                             model: "gemma-4-e2b".into(), input_tokens: 1500,
                             output_tokens: 50, cached_input_tokens: 1200 };
        assert_eq!(parse(&serialize(&e)), Some(e));
    }

    #[test]
    fn one_record_is_one_line() {
        // The format's whole claim is "easy to grep / tail". A record spanning
        // two lines breaks every one of those tools.
        let mut e = ev(1, "summary", 10, 5);
        e.model = "weird\nmodel\tname".into();
        let line = serialize(&e);
        assert_eq!(line.lines().count(), 1, "{line}");
        assert_eq!(parse(&line).unwrap().model, "weird\nmodel\tname");
    }

    #[test]
    fn a_quote_in_a_field_cannot_forge_a_record() {
        let mut e = ev(1, "summary", 10, 5);
        e.op = "sum\",\"in\":999999,\"x\":\"".into();
        let parsed = parse(&serialize(&e)).unwrap();
        assert_eq!(parsed.input_tokens, 10, "the injected field did not win");
        assert_eq!(parsed.op, e.op);
    }

    #[test]
    fn an_older_record_without_the_cached_field_still_reads() {
        // Written before caching existed. Zero rather than a parse failure, or
        // upgrading the app would appear to erase the history.
        let line = "{\"ts\":5,\"op\":\"chat\",\"model\":\"m\",\"in\":10,\"out\":2}";
        let e = parse(line).expect("parses");
        assert_eq!(e.cached_input_tokens, 0);
        assert_eq!(e.input_tokens, 10);
    }

    #[test]
    fn a_torn_final_line_is_skipped_rather_than_losing_the_log() {
        // Append-only files end this way after a crash, and losing the whole
        // history to one bad tail would defeat keeping it.
        assert_eq!(parse("{\"ts\":5,\"op\":\"ch"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("not json at all"), None);
    }

    #[test]
    fn cached_input_is_excluded_from_the_billable_count() {
        // It is the part that was NOT re-processed; counting it inflates every
        // total on a backend that caches well, making the better-behaved
        // provider look worse.
        let e = TokenEvent { cached_input_tokens: 900, ..ev(1, "chat", 1000, 50) };
        assert_eq!(e.billable_input(), 100);
    }

    #[test]
    fn a_cache_larger_than_the_input_does_not_underflow() {
        // A provider reporting inconsistent numbers must not produce four
        // billion billable tokens on a stats page.
        let e = TokenEvent { cached_input_tokens: 5000, ..ev(1, "chat", 100, 5) };
        assert_eq!(e.billable_input(), 0);
    }

    #[test]
    fn an_unknown_window_falls_back_rather_than_failing() {
        // "So stale/typo URLs don't break the page" — a stats page is what you
        // open when something is already wrong.
        assert_eq!(TimeWindow::parse("nonsense"), TimeWindow::AllTime);
        assert_eq!(TimeWindow::parse(""), TimeWindow::AllTime);
    }

    #[test]
    fn window_parsing_tolerates_case_and_alternate_spellings() {
        for s in ["7d", "7D", " week ", "7days", "last7d"] {
            assert_eq!(TimeWindow::parse(s), TimeWindow::Last7d, "{s}");
        }
    }

    #[test]
    fn every_window_round_trips_through_its_slug() {
        // The slug is the inverse of parse; a mismatch makes a tab link open a
        // different tab than the one it highlights.
        for w in TimeWindow::ALL {
            assert_eq!(TimeWindow::parse(w.slug()), w, "{w:?}");
        }
    }

    #[test]
    fn last_seven_days_rolls_rather_than_aligning_to_a_calendar_week() {
        // "Power users care about 'how much did I spend this week's-worth-of-
        // work'". A calendar week resets on Monday morning, exactly when the
        // question stops being useful.
        let now = DAY * 100;
        assert!(TimeWindow::Last7d.contains(now - DAY * 6, now, 0));
        assert!(!TimeWindow::Last7d.contains(now - DAY * 8, now, 0));
    }

    #[test]
    fn today_rolls_too_so_a_late_night_call_still_counts_after_midnight() {
        // A call at 23:50 is the thing you just did at 00:10.
        let now = DAY * 100;
        assert!(TimeWindow::Today.contains(now - 20 * 60 * 1000, now, 0));
    }

    #[test]
    fn the_session_window_starts_at_launch() {
        let start = DAY * 100;
        assert!(!TimeWindow::Session.contains(start - 1, start + 5, start));
        assert!(TimeWindow::Session.contains(start + 1, start + 5, start));
    }

    #[test]
    fn a_clock_correction_does_not_empty_a_window() {
        // Saturating: an event timestamped after `now` would otherwise wrap
        // into an enormous age and vanish from every view.
        let now = DAY * 100;
        assert!(TimeWindow::Today.contains(now + DAY, now, 0));
    }

    #[test]
    fn totals_are_sorted_by_cost_rather_than_alphabetically() {
        // A stats page is opened to find what is expensive; alphabetical makes
        // that a search.
        let evs = vec![ev(1, "aaa", 10, 5), ev(2, "zzz", 900, 100), ev(3, "aaa", 10, 5)];
        let t = totals_by_op(&evs, TimeWindow::AllTime, 10, 0);
        assert_eq!(t[0].0, "zzz");
        assert_eq!(t[1].1.calls, 2, "and the two aaa calls aggregated");
    }

    #[test]
    fn spikes_are_the_largest_calls_newest_first_among_equals() {
        // A repeated spike should show the one you can still remember making.
        let evs = vec![ev(1, "a", 100, 0), ev(2, "b", 900, 0), ev(3, "c", 900, 0)];
        let s = top_spikes(&evs, TimeWindow::AllTime, 10, 0, 2);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].ts_ms, 3, "newest of the tied largest");
    }

    #[test]
    fn an_empty_log_produces_empty_views_rather_than_panicking() {
        assert!(totals_by_op(&[], TimeWindow::AllTime, 0, 0).is_empty());
        assert!(top_spikes(&[], TimeWindow::AllTime, 0, 0, 5).is_empty());
    }
}
