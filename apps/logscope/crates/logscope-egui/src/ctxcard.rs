//! ctxcard — the two facts a context card owes the reader.
//!
//! ## What the references show and ours did not
//!
//!
//! (The reference images are GENERATED design studies, not captures of this app. They are a
//! legitimate source of intent — the operator chose them to say what the interface should feel like
//! — and they are not evidence about behaviour. Nothing visible in one should be read as something
//! this program did; see the correction at the top of `nodelabel`.)
//! Every context card in the atlantis and mansion-garden references carries a size-and-age line
//! ("12.4 KB · Modified 2h ago") and a relevance figure in the corner (0.98, 0.86). Ours showed the
//! file name, an excerpt and some keyword chips — nothing about how big the file is, and nothing
//! about why it is in context.
//!
//! The second is the one worth having. A context card exists to answer "what is the model looking
//! at"; without a relevance figure it answers only "something".
//!
//! ## A number with a stated meaning, not a confidence
//!
//! The reference's 0.98 is a worked example's flourish. Reproducing it would mean inventing a
//! confidence — and a fabricated score in the one panel whose job is explaining the context would be
//! actively harmful, because it looks like evidence.
//!
//! So the figure here is a COUNT, rendered as a ratio and always shown with its denominator: how
//! many of the conversation's own topic words appear in the document. "0.67 · 4/6 topics" can be
//! checked by reading the file, which is the property that separates a measurement from a decoration.
//! When the conversation has no topic words yet there is nothing to match against, and the card says
//! so rather than showing 0.00 — an absence of evidence displayed as a low score is a lie.
//!
//! Pure functions over a document and a word list, so both halves are tested without a frame.

/// Human file size. Binary units, because that is what the file managers next to this app use.
pub fn size_label(bytes: usize) -> String {
    const K: f32 = 1024.0;
    let b = bytes as f32;
    if bytes < 1024 { format!("{bytes} B") }
    else if b < K * K { format!("{:.1} KB", b / K) }
    else { format!("{:.1} MB", b / (K * K)) }
}

/// How many of `topics` occur in `lines`, and how many were looked for.
///
/// Case-insensitive substring match on the whole document text. Substring rather than word-boundary
/// on purpose: a log file's topic words appear inside identifiers and paths (`pool_size`,
/// `/var/log/db_pool.log`) and requiring a boundary would report a file that is entirely about the
/// subject as unrelated.
pub fn topic_hits(lines: &[String], topics: &[String]) -> (usize, usize) {
    if topics.is_empty() { return (0, 0); }
    let hay = lines.join("\n").to_lowercase();
    let hit = topics.iter()
        .filter(|t| !t.trim().is_empty())
        .filter(|t| hay.contains(&t.to_lowercase()))
        .count();
    (hit, topics.iter().filter(|t| !t.trim().is_empty()).count())
}

/// The corner figure, or None when there is nothing to measure.
///
/// None rather than 0.0 for an empty topic set: the card must not show a low score for "we have not
/// asked anything yet", because that reads as "this file is irrelevant" and it is not a claim the
/// data supports.
pub fn topic_match(lines: &[String], topics: &[String]) -> Option<(f32, usize, usize)> {
    let (hit, total) = topic_hits(lines, topics);
    if total == 0 { return None; }
    Some((hit as f32 / total as f32, hit, total))
}

/// `0.67 · 4/6 topics` — the ratio AND its denominator, always.
///
/// The denominator is not decoration. A bare 0.67 is indistinguishable from a model's confidence;
/// with "4/6 topics" beside it the reader knows exactly what was counted and can check it.
pub fn match_label(hit: usize, total: usize) -> String {
    let r = if total == 0 { 0.0 } else { hit as f32 / total as f32 };
    format!("{r:.2} · {hit}/{total} topics")
}

/// `12.4 KB · 180 lines`, with the placeholder case called out.
///
/// The bundled `sample.log` is attached to every prompt on a fresh session, which is how "hi there"
/// once came back as a discussion of a worker exiting on signal 11 — the card should say when the
/// context is the demo file rather than something the user chose.
pub fn size_line(lines: &[String], placeholder: bool) -> String {
    let bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
    let base = format!("{} · {} lines", size_label(bytes), lines.len());
    if placeholder { format!("{base} · sample, not your data") } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> { xs.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn sizes_read_the_way_a_file_manager_shows_them() {
        assert_eq!(size_label(0), "0 B");
        assert_eq!(size_label(999), "999 B");
        assert_eq!(size_label(1024), "1.0 KB");
        assert_eq!(size_label(12_698), "12.4 KB");
        assert_eq!(size_label(2_202_010), "2.1 MB");
    }

    #[test]
    fn topics_are_counted_against_the_document() {
        let doc = v(&["2026-05-07 pool_size=42 active=41 wait=120ms",
                      "GET /health 200", "worker exited on signal 11"]);
        let (hit, total) = topic_hits(&doc, &v(&["pool", "worker", "latency", "retry"]));
        assert_eq!((hit, total), (2, 4), "pool and worker are present; latency and retry are not");
    }

    /// Substring rather than word-boundary: a log's topic words live inside identifiers and paths,
    /// and requiring a boundary reports a file that is entirely about the subject as unrelated.
    #[test]
    fn a_topic_inside_an_identifier_still_counts() {
        let doc = v(&["db_pool_metrics.log", "/var/log/db_pool.log"]);
        assert_eq!(topic_hits(&doc, &v(&["pool"])), (1, 1));
        assert_eq!(topic_hits(&doc, &v(&["POOL"])), (1, 1), "case-insensitive");
    }

    /// An absence of evidence must not render as a low score.
    #[test]
    fn no_topics_yet_is_none_rather_than_zero() {
        let doc = v(&["anything at all"]);
        assert_eq!(topic_match(&doc, &[]), None);
        assert_eq!(topic_match(&doc, &v(&["   ", ""])), None, "blank topics are not topics");
        // and a real miss IS zero, which is a claim the data supports
        let m = topic_match(&doc, &v(&["absent"])).expect("one topic was looked for");
        assert_eq!((m.1, m.2), (0, 1));
        assert!(m.0 < f32::EPSILON);
    }

    /// The denominator is the difference between a measurement and a decoration.
    #[test]
    fn the_label_always_carries_what_was_counted() {
        assert_eq!(match_label(4, 6), "0.67 · 4/6 topics");
        assert_eq!(match_label(0, 3), "0.00 · 0/3 topics");
        assert_eq!(match_label(3, 3), "1.00 · 3/3 topics");
        // never a bare ratio, which would read as a model's confidence
        for (h, t) in [(1, 2), (5, 5), (0, 1)] {
            assert!(match_label(h, t).contains(&format!("{h}/{t}")));
        }
    }

    #[test]
    fn the_size_line_names_the_demo_document() {
        let doc = v(&["one", "two", "three"]);
        let real = size_line(&doc, false);
        assert!(real.contains("3 lines") && real.contains("B"), "{real}");
        assert!(!real.contains("sample"));
        let demo = size_line(&doc, true);
        assert!(demo.contains("sample, not your data"),
                "the bundled log is attached to every prompt; the card must say so: {demo}");
    }

    #[test]
    fn an_empty_document_does_not_divide_by_zero() {
        assert_eq!(size_line(&[], false), "0 B · 0 lines");
        assert_eq!(topic_hits(&[], &v(&["x"])), (0, 1));
        assert_eq!(match_label(0, 0), "0.00 · 0/0 topics");
    }
}
