//! IRC channel-log search for the search bar — the pure ranker bridge.
//!
//! Same shape as `note_search.rs`: the host supplies loaded documents, this
//! ranks and builds the dropdown rows, and everything here is testable with
//! no filesystem. The difference is what a hit *is*. A note match is a line
//! in a file you wrote; an IRC match is one message in a conversation, and a
//! single line out of an exchange is usually unreadable on its own —
//! "yeah that's the one" tells you nothing without the two lines above it.
//!
//! So a row here is a **run**, not a line: the ranker finds the best-matching
//! message, then the row carries a few lines of surrounding context, which is
//! what makes the result answer a question rather than just prove a word was
//! said.
//!
//! ## The consent gate is upstream, not here
//!
//! This module never opens a file. The host loads logs through
//! `irc_kit::sources`, which admits only channels the operator has explicitly
//! granted and only lines this machine captured while joined. By the time
//! anything reaches [`search`] the decision has been made; searching an
//! ungranted log is not prevented here because it is not reachable here.

use irc_kit::log::Line;
use irc_kit::search as ranker;

/// A ranked result: one matched message plus the exchange around it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub network: String,
    pub channel: String,
    /// Unix seconds of the matched message.
    pub ts: u64,
    /// Who said the matched line.
    pub nick: String,
    /// The matched message, windowed.
    pub snippet: String,
    /// Lines before and after, oldest first, each already `nick: text`.
    pub context: Vec<String>,
    pub score: i64,
    /// Index into the slice searched, so a caller can open the full log there.
    pub index: usize,
}

/// How many lines of context to carry either side of a hit.
pub const CONTEXT_LINES: usize = 2;

/// The silence that ends a conversation.
///
/// Adjacency in the log is not adjacency in time: a channel goes quiet
/// overnight and resumes, and the line before the first morning message can be
/// eight hours older. Treating those as one exchange produces context that
/// reads as a reply to something nobody was answering. Fifteen minutes is the
/// point where an IRC exchange has plainly ended — long enough to survive
/// someone thinking before they type, short enough to split a resumed topic.
pub const MAX_GAP_SECS: u64 = 15 * 60;

fn render(l: &Line) -> String {
    if l.action {
        format!("* {} {}", l.nick, l.text)
    } else if l.notice {
        format!("-{}- {}", l.nick, l.text)
    } else {
        format!("{}: {}", l.nick, l.text)
    }
}

/// Rank `lines` against `query` and build dropdown rows with context.
///
/// Results are deduplicated by run: if three adjacent messages all match, the
/// best one becomes the row and the others are absorbed into its context
/// rather than producing three near-identical rows.
pub fn search(query: &str, lines: &[Line], max_results: usize) -> Vec<Hit> {
    // Over-fetch so clustering has something to collapse; otherwise a tight
    // run of matches eats the whole result budget.
    let raw = ranker::search(query, lines, max_results.saturating_mul(4).max(max_results));
    if raw.is_empty() {
        return Vec::new();
    }

    // Cluster by index first, then split each run wherever the log went
    // quiet: two hits three lines apart but three hours apart are separate
    // conversations and deserve separate rows.
    let runs: Vec<(usize, usize)> = ranker::cluster(&raw, CONTEXT_LINES)
        .into_iter()
        .flat_map(|(s, e)| split_on_silence(lines, s, e))
        .collect();
    let mut out: Vec<Hit> = Vec::new();

    for (start, end) in runs {
        // The best-scoring hit inside this run represents it.
        let Some(best) = raw
            .iter()
            .filter(|h| h.index >= start && h.index <= end)
            .max_by_key(|h| h.score)
        else {
            continue;
        };
        let l = &lines[best.index];

        // Context stops at a silence too, for the same reason.
        let ctx_lo = walk_back(lines, best.index, CONTEXT_LINES);
        let ctx_hi = walk_forward(lines, best.index, CONTEXT_LINES);
        let context = lines[ctx_lo..ctx_hi]
            .iter()
            .enumerate()
            .filter(|(i, _)| ctx_lo + i != best.index)
            .map(|(_, c)| render(c))
            .collect();

        out.push(Hit {
            network: l.network.clone(),
            channel: l.channel.clone(),
            ts: l.ts,
            nick: l.nick.clone(),
            snippet: best.snippet.clone(),
            context,
            score: best.score,
            index: best.index,
        });
    }

    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| b.ts.cmp(&a.ts)));
    out.truncate(max_results);
    out
}

/// Split an index run wherever consecutive lines are more than [`MAX_GAP_SECS`]
/// apart. Returns at least one span.
fn split_on_silence(lines: &[Line], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut lo = start;
    for i in start..end {
        if lines[i + 1].ts.saturating_sub(lines[i].ts) > MAX_GAP_SECS {
            out.push((lo, i));
            lo = i + 1;
        }
    }
    out.push((lo, end));
    out
}

/// First index at or before `from` reachable within `n` lines without
/// crossing a silence.
fn walk_back(lines: &[Line], from: usize, n: usize) -> usize {
    let mut i = from;
    for _ in 0..n {
        if i == 0 {
            break;
        }
        if lines[i].ts.saturating_sub(lines[i - 1].ts) > MAX_GAP_SECS {
            break;
        }
        i -= 1;
    }
    i
}

/// One past the last index at or after `from` reachable within `n` lines
/// without crossing a silence.
fn walk_forward(lines: &[Line], from: usize, n: usize) -> usize {
    let mut i = from;
    for _ in 0..n {
        if i + 1 >= lines.len() {
            break;
        }
        if lines[i + 1].ts.saturating_sub(lines[i].ts) > MAX_GAP_SECS {
            break;
        }
        i += 1;
    }
    i + 1
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM` UTC.
///
/// Hand-rolled for the same reason `irc_kit::transport` hand-rolls the
/// inverse: voiceterm has no date crate and this is one civil-calendar
/// conversion, not a formatting library.
pub fn stamp(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs_of_day = ts % 86_400;
    // Inverse of days_from_civil (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// Wire format for the QML dropdown, one row per line:
/// `network/channel\tYYYY-MM-DD HH:MM\tnick\tsnippet\tcontext`
///
/// Context lines are joined with ` ⏎ ` rather than a real newline because the
/// wire format is tab/newline delimited — embedding either would split one row
/// into several and corrupt the list.
pub fn to_wire(hits: &[Hit]) -> String {
    hits.iter()
        .map(|h| {
            let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
            format!(
                "{}/{}\t{}\t{}\t{}\t{}",
                clean(&h.network),
                clean(&h.channel),
                stamp(h.ts),
                clean(&h.nick),
                clean(&h.snippet),
                h.context.iter().map(|c| clean(c)).collect::<Vec<_>>().join(" ⏎ "),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use irc_kit::log::Origin;

    fn l(ts: u64, nick: &str, text: &str) -> Line {
        Line {
            ts,
            server_time: None,
            network: "oftc".into(),
            channel: "#asahi-dev".into(),
            nick: nick.into(),
            text: text.into(),
            notice: false,
            action: false,
            origin: Origin::Participated,
        }
    }

    fn convo() -> Vec<Line> {
        vec![
            l(100, "alice", "morning"),
            l(110, "bob", "quick question about the decoder"),
            l(120, "carol", "avd was staged stateless for vulkan video"),
            l(130, "bob", "ah that explains the v4l2 shape"),
            l(140, "alice", "lunch?"),
            l(900, "dave", "unrelated chatter about lunch"),
        ]
    }

    #[test]
    fn a_hit_carries_the_exchange_around_it() {
        let hits = search("avd vulkan", &convo(), 5);
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.nick, "carol");
        // Two either side, and the matched line itself is not duplicated in
        // its own context.
        assert_eq!(h.context.len(), 4);
        assert!(h.context.iter().any(|c| c.starts_with("bob: quick question")));
        assert!(h.context.iter().any(|c| c.starts_with("bob: ah that explains")));
        assert!(!h.context.iter().any(|c| c.contains("staged stateless")));
    }

    #[test]
    fn context_is_clamped_at_the_edges_of_the_log() {
        let hits = search("morning", &convo(), 5);
        assert_eq!(hits[0].index, 0);
        // Nothing before line 0, so only the two after it.
        assert_eq!(hits[0].context.len(), 2);
    }

    #[test]
    fn a_run_of_adjacent_matches_becomes_one_row_not_three() {
        let lines = vec![
            l(10, "a", "vulkan here"),
            l(20, "b", "vulkan there"),
            l(30, "c", "vulkan everywhere"),
        ];
        let hits = search("vulkan", &lines, 10);
        assert_eq!(hits.len(), 1, "one row for the run");
    }

    #[test]
    fn a_silence_splits_a_run_even_when_the_lines_are_adjacent() {
        // The regression: these four are indices 0..3, so index clustering
        // alone made them one exchange despite the last being hours later.
        let lines = vec![
            l(10, "a", "vulkan here"),
            l(20, "b", "vulkan there"),
            l(30, "c", "vulkan everywhere"),
            l(30 + MAX_GAP_SECS + 1, "d", "vulkan much later"),
        ];
        let hits = search("vulkan", &lines, 10);
        assert_eq!(hits.len(), 2, "the silence ends the conversation");
    }

    #[test]
    fn context_does_not_reach_across_a_silence() {
        let lines = vec![
            l(0, "old", "yesterday's topic"),
            l(MAX_GAP_SECS + 100, "carol", "avd vulkan video"),
            l(MAX_GAP_SECS + 110, "bob", "makes sense"),
        ];
        let hits = search("avd vulkan", &lines, 5);
        assert_eq!(hits.len(), 1);
        let ctx = &hits[0].context;
        assert!(
            !ctx.iter().any(|c| c.contains("yesterday")),
            "a line from before the silence is not context: {ctx:?}"
        );
        assert!(ctx.iter().any(|c| c.contains("makes sense")));
    }

    #[test]
    fn nothing_matching_yields_no_rows() {
        assert!(search("kubernetes", &convo(), 5).is_empty());
        assert!(search("", &convo(), 5).is_empty());
    }

    #[test]
    fn actions_and_notices_render_distinguishably() {
        let mut a = l(10, "n", "waves");
        a.action = true;
        let mut b = l(20, "bot", "build failed");
        b.notice = true;
        assert_eq!(render(&a), "* n waves");
        assert_eq!(render(&b), "-bot- build failed");
        assert_eq!(render(&l(30, "n", "hi")), "n: hi");
    }

    #[test]
    fn stamp_matches_the_epoch_anchors_transport_uses() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(1_787_414_062), "2026-08-22 15:54");
        assert_eq!(stamp(951_868_800), "2000-03-01 00:00");
        assert_eq!(stamp(1_709_208_000), "2024-02-29 12:00");
    }

    #[test]
    fn the_wire_row_has_exactly_five_fields() {
        let hits = search("avd vulkan", &convo(), 5);
        let wire = to_wire(&hits);
        assert_eq!(wire.lines().count(), 1);
        assert_eq!(wire.split('\t').count(), 5);
    }

    #[test]
    fn tabs_and_newlines_in_a_message_cannot_forge_extra_rows() {
        // A channel member typing a tab must not be able to inject columns,
        // or a newline to inject rows, into the dropdown.
        let lines = vec![l(10, "evil\tnick", "text\twith\ttabs\nand a newline")];
        let hits = search("tabs", &lines, 5);
        let wire = to_wire(&hits);
        assert_eq!(wire.lines().count(), 1, "no injected rows");
        assert_eq!(wire.split('\t').count(), 5, "no injected columns");
    }

    #[test]
    fn context_lines_are_joined_without_a_real_newline() {
        let hits = search("avd vulkan", &convo(), 5);
        let wire = to_wire(&hits);
        let ctx = wire.split('\t').nth(4).unwrap();
        assert!(ctx.contains(" ⏎ "));
        assert!(!ctx.contains('\n'));
    }
}
