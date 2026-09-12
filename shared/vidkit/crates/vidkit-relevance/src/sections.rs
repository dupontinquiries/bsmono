//! Chapters and search over a transcript.
//!
//! A lecture is the case this is for: an hour of one speaker, no chapter marks,
//! and the thing you want is four minutes in the middle. Two jobs follow from
//! that — cut the transcript where the topic actually changes, and answer
//! "where did they talk about X" with a timestamp.
//!
//! Both are tier one in the sense [`crate`] uses: deterministic, instant, no
//! model. A model can improve the titles and would produce better boundaries,
//! and the [`crate::Analyzer`] queue is where that plugs in — but a lecture
//! index that needs a GPU to exist is one nobody gets.
//!
//! # Distinctive, not frequent
//!
//! The trap here is titling every chapter "so the okay right". Lecture
//! transcripts are dominated by filler and by the subject's own name, which
//! appear everywhere and therefore distinguish nothing. Terms are scored by how
//! much they stand out in ONE window against the whole transcript — an inverse
//! document frequency — so a word in every window scores zero however often it
//! is said.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::Window;

/// A detected topic.
#[derive(Clone, PartialEq, Debug)]
pub struct Chapter {
    pub start: Duration,
    pub end: Duration,
    /// The most distinctive terms, best first. Empty when nothing stood out.
    pub keywords: Vec<String>,
    /// Window indices this covers, for a caller that wants the text back.
    pub windows: std::ops::Range<usize>,
}

impl Chapter {
    /// A short label for a chapter list.
    pub fn title(&self) -> String {
        if self.keywords.is_empty() {
            return format!("{} – {}", fmt(self.start), fmt(self.end));
        }
        self.keywords
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn duration(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }
}

fn fmt(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Copy, Clone, Debug)]
pub struct ChapterPolicy {
    /// Below this vocabulary overlap, adjacent windows are different topics.
    ///
    /// Jaccard over content words. Lecture speech drifts, so this is low: a
    /// high threshold cuts every time the speaker changes example, and an
    /// hour of lecture becomes sixty chapters, which is the same as none.
    pub similarity: f32,
    /// No chapter shorter than this. A lecture does not change subject every
    /// forty seconds, and a chapter list nobody can click accurately is worse
    /// than a coarse one.
    pub min_len: Duration,
}

impl Default for ChapterPolicy {
    fn default() -> Self {
        Self {
            similarity: 0.18,
            min_len: Duration::from_secs(120),
        }
    }
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3)
        .map(str::to_string)
        .collect()
}

fn token_set(s: &str) -> HashSet<String> {
    tokens(s).into_iter().collect()
}

/// Vocabulary overlap of two windows, 0..1, counting every word.
///
/// Prefer [`similarity_ignoring`] with the transcript's ubiquitous terms: on a
/// lecture this measure says two windows on completely different subjects are
/// 23% similar because they share "okay", "right" and "basically", and no
/// threshold separates that from a real topic continuing. Filler is not just
/// bad in titles — it hides the boundaries.
pub fn similarity(a: &str, b: &str) -> f32 {
    similarity_ignoring(a, b, &HashSet::new())
}

/// Overlap counting only terms outside `ignore`.
pub fn similarity_ignoring(a: &str, b: &str, ignore: &HashSet<String>) -> f32 {
    let keep = |s: &str| -> HashSet<String> {
        token_set(s).into_iter().filter(|t| !ignore.contains(t)).collect()
    };
    let (x, y) = (keep(a), keep(b));
    if x.is_empty() && y.is_empty() {
        // Two silent windows are the same topic — nothing changed. Treating
        // them as different cuts a chapter at every pause.
        return 1.0;
    }
    let inter = x.intersection(&y).count() as f32;
    let union = x.union(&y).count() as f32;
    if union == 0.0 {
        1.0
    } else {
        inter / union
    }
}

/// How distinctive each term is: frequency here, divided by how many windows
/// use it at all.
fn distinctive(text: &str, doc_freq: &HashMap<String, usize>, n_windows: usize) -> Vec<String> {
    let mut score: HashMap<String, f32> = HashMap::new();
    for t in tokens(text) {
        let df = *doc_freq.get(&t).unwrap_or(&1) as f32;
        // A term in every window scores zero however often it is said, which
        // is what keeps filler and the subject's own name out of the titles.
        let idf = (n_windows as f32 / df).ln();
        if idf > 0.0 {
            *score.entry(t).or_insert(0.0) += idf;
        }
    }
    let mut v: Vec<(String, f32)> = score.into_iter().collect();
    // Sorted by score, then alphabetically: an unstable tie order makes the
    // chapter list shuffle between runs on the same input.
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    v.into_iter().take(5).map(|(t, _)| t).collect()
}

fn doc_frequencies(windows: &[Window]) -> HashMap<String, usize> {
    let mut df: HashMap<String, usize> = HashMap::new();
    for w in windows {
        for t in token_set(&w.text) {
            *df.entry(t).or_insert(0) += 1;
        }
    }
    df
}

/// Cut a transcript into chapters.
pub fn chapters(windows: &[Window], policy: &ChapterPolicy) -> Vec<Chapter> {
    if windows.is_empty() {
        return Vec::new();
    }
    let df = doc_frequencies(windows);

    // Terms in most windows are the speaker's habits and the subject's own
    // name. They are excluded from the boundary test as well as from the
    // titles: leaving them in makes two windows on unrelated subjects look
    // related, which is how an entire lecture comes back as one chapter.
    let ubiquitous: HashSet<String> = df
        .iter()
        .filter(|(_, &n)| windows.len() >= 3 && n * 2 >= windows.len())
        .map(|(t, _)| t.clone())
        .collect();

    // Cut where adjacent windows stop sharing vocabulary.
    let mut cuts: Vec<usize> = vec![0];
    for i in 1..windows.len() {
        if similarity_ignoring(&windows[i - 1].text, &windows[i].text, &ubiquitous)
            < policy.similarity
        {
            cuts.push(i);
        }
    }
    cuts.push(windows.len());

    let mut out: Vec<Chapter> = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b {
            continue;
        }
        let start = windows[a].start;
        let end = windows[b - 1].end;
        let text: String = windows[a..b]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        // Too short to be its own chapter: fold into the previous one rather
        // than dropping it, or the timeline develops gaps.
        if let Some(last) = out.last_mut() {
            if end.saturating_sub(start) < policy.min_len {
                last.end = end;
                last.windows.end = b;
                let merged: String = windows[last.windows.clone()]
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                last.keywords = distinctive(&merged, &df, windows.len());
                continue;
            }
        }

        out.push(Chapter {
            start,
            end,
            keywords: distinctive(&text, &df, windows.len()),
            windows: a..b,
        });
    }

    // A leading chapter can also be too short; it has no previous to fold
    // into, so it absorbs the next instead.
    if out.len() > 1 && out[0].duration() < policy.min_len {
        let second = out.remove(1);
        out[0].end = second.end;
        out[0].windows.end = second.windows.end;
        let merged: String = windows[out[0].windows.clone()]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        out[0].keywords = distinctive(&merged, &df, windows.len());
    }
    out
}

/// A hit from searching a transcript.
#[derive(Clone, PartialEq, Debug)]
pub struct Hit {
    pub at: Duration,
    pub window: usize,
    /// How well this window matched, higher is better.
    pub score: f32,
    /// The window's text, for a result list.
    pub text: String,
}

/// Find where a term or phrase was discussed, best first.
///
/// Scored by distinctiveness rather than raw count, for the same reason the
/// titles are: searching a lecture for its own subject should not return every
/// window in order.
pub fn search(windows: &[Window], query: &str, limit: usize) -> Vec<Hit> {
    let terms = token_set(query);
    if terms.is_empty() || windows.is_empty() {
        return Vec::new();
    }
    let df = doc_frequencies(windows);
    let n = windows.len() as f32;

    let mut hits: Vec<Hit> = windows
        .iter()
        .filter_map(|w| {
            let have = tokens(&w.text);
            let mut score = 0.0;
            for t in &terms {
                let count = have.iter().filter(|h| *h == t).count();
                if count == 0 {
                    continue;
                }
                let idf = (n / *df.get(t).unwrap_or(&1) as f32).ln().max(0.01);
                score += count as f32 * idf;
            }
            (score > 0.0).then(|| Hit {
                at: w.start,
                window: w.index,
                score,
                text: w.text.clone(),
            })
        })
        .collect();

    // Ties break on time, so the earliest mention wins: for "when was X
    // introduced", the first occurrence is the answer.
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.at.cmp(&b.at))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(i: usize, text: &str) -> Window {
        Window {
            index: i,
            start: Duration::from_secs(i as u64 * 60),
            end: Duration::from_secs((i as u64 + 1) * 60),
            text: text.into(),
        }
    }

    /// A lecture: filler everywhere, three subjects.
    fn lecture() -> Vec<Window> {
        let filler = "okay right so basically";
        let a = "matrices matrix determinant eigenvalue linear";
        let b = "probability distribution bayes posterior random";
        let c = "gradient descent optimisation convex learning";
        let mut v = Vec::new();
        for (i, topic) in [a, a, a, b, b, b, c, c, c].iter().enumerate() {
            v.push(win(i, &format!("{filler} {topic}")));
        }
        v
    }

    #[test]
    fn a_lecture_is_cut_where_the_subject_changes() {
        let ch = chapters(&lecture(), &ChapterPolicy::default());
        assert_eq!(ch.len(), 3, "expected three subjects, got {:?}", ch.iter().map(|c| c.title()).collect::<Vec<_>>());
        assert_eq!(ch[0].start, Duration::ZERO);
        assert_eq!(ch[2].end, Duration::from_secs(9 * 60));
    }

    /// THE trap. Lecture speech is mostly filler and the subject's own name,
    /// which appear everywhere and distinguish nothing — a title of "okay
    /// right so" is worse than no title.
    #[test]
    fn filler_never_reaches_a_title() {
        let ch = chapters(&lecture(), &ChapterPolicy::default());
        for c in &ch {
            for bad in ["okay", "right", "basically"] {
                assert!(
                    !c.keywords.iter().any(|k| k == bad),
                    "{bad:?} got into a title: {:?}",
                    c.keywords
                );
            }
        }
        assert!(ch[0].keywords.iter().any(|k| k.contains("matri") || k == "eigenvalue"));
        assert!(ch[1].keywords.iter().any(|k| k == "bayes" || k == "probability"));
    }

    /// A lecture does not change subject every forty seconds. A chapter list
    /// nobody can click accurately is worse than a coarse one.
    #[test]
    fn short_topics_are_folded_into_their_neighbour() {
        let mut w = lecture();
        // One stray window in the middle, on its own subject.
        w.insert(4, win(4, "aside anecdote unrelated tangent story"));
        for (i, win) in w.iter_mut().enumerate() {
            win.index = i;
            win.start = Duration::from_secs(i as u64 * 60);
            win.end = Duration::from_secs((i as u64 + 1) * 60);
        }
        let ch = chapters(&w, &ChapterPolicy::default());
        assert!(
            ch.iter().all(|c| c.duration() >= Duration::from_secs(120)),
            "a chapter shorter than the floor survived: {:?}",
            ch.iter().map(|c| c.duration()).collect::<Vec<_>>()
        );
    }

    /// Folding must not leave holes: the chapters have to tile the transcript.
    #[test]
    fn chapters_cover_the_whole_transcript_without_gaps() {
        let w = lecture();
        let ch = chapters(&w, &ChapterPolicy::default());
        assert_eq!(ch.first().unwrap().start, w.first().unwrap().start);
        assert_eq!(ch.last().unwrap().end, w.last().unwrap().end);
        for pair in ch.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "gap between chapters");
        }
    }

    /// A talk on one subject is one chapter, not nine.
    #[test]
    fn a_single_subject_stays_a_single_chapter() {
        let w: Vec<Window> = (0..9)
            .map(|i| win(i, "matrices matrix determinant eigenvalue linear algebra"))
            .collect();
        assert_eq!(chapters(&w, &ChapterPolicy::default()).len(), 1);
    }

    /// The measure that made a whole lecture one chapter: filler shared by
    /// every window makes unrelated subjects look related.
    #[test]
    fn filler_hides_boundaries_unless_it_is_excluded() {
        let a = "okay right basically matrices matrix determinant eigenvalue linear";
        let b = "okay right basically probability distribution bayes posterior random";
        let naive = similarity(a, b);
        assert!(naive > 0.18, "the naive measure should look similar: {naive}");

        let filler: HashSet<String> = ["okay", "right", "basically"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let fair = similarity_ignoring(a, b, &filler);
        assert_eq!(fair, 0.0, "with filler out these share nothing");
    }

    /// Silence is not a topic change; treating it as one cuts at every pause.
    #[test]
    fn silent_windows_do_not_start_a_chapter() {
        assert_eq!(similarity("", ""), 1.0);
        let w = vec![
            win(0, "matrices matrix determinant eigenvalue"),
            win(1, ""),
            win(2, ""),
            win(3, "matrices matrix determinant eigenvalue"),
        ];
        let ch = chapters(&w, &ChapterPolicy::default());
        assert_eq!(ch.len(), 1, "a pause split the chapter: {ch:?}");
    }

    #[test]
    fn an_empty_transcript_is_no_chapters_rather_than_a_panic() {
        assert!(chapters(&[], &ChapterPolicy::default()).is_empty());
    }

    /// The chapter list must not shuffle between runs on identical input.
    #[test]
    fn titles_are_stable() {
        let a: Vec<String> = chapters(&lecture(), &ChapterPolicy::default())
            .iter()
            .map(|c| c.title())
            .collect();
        let b: Vec<String> = chapters(&lecture(), &ChapterPolicy::default())
            .iter()
            .map(|c| c.title())
            .collect();
        assert_eq!(a, b);
    }

    // ---- search -----------------------------------------------------------

    #[test]
    fn search_finds_the_window_that_discussed_a_term() {
        let hits = search(&lecture(), "bayes", 5);
        assert!(!hits.is_empty(), "found nothing");
        assert_eq!(hits[0].window, 3, "wrong window: {hits:?}");
        assert_eq!(hits[0].at, Duration::from_secs(180));
    }

    /// Searching a lecture for its own subject should not return every window
    /// in order — a term everywhere carries no information about where.
    #[test]
    fn a_term_in_every_window_scores_near_nothing() {
        let common = search(&lecture(), "okay", 20);
        let rare = search(&lecture(), "eigenvalue", 20);
        let top_common = common.first().map(|h| h.score).unwrap_or(0.0);
        let top_rare = rare.first().map(|h| h.score).unwrap_or(0.0);
        assert!(
            top_rare > top_common * 5.0,
            "a distinctive term scored {top_rare} against filler's {top_common}"
        );
    }

    /// "When was X introduced" wants the FIRST mention, so equal scores break
    /// on time.
    #[test]
    fn equal_scores_prefer_the_earliest_mention() {
        let w = vec![
            win(0, "alpha beta gamma"),
            win(1, "delta epsilon zeta"),
            win(2, "alpha beta gamma"),
        ];
        let hits = search(&w, "alpha", 5);
        assert_eq!(hits[0].window, 0, "the later mention won: {hits:?}");
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        assert!(search(&lecture(), "zzzz", 5).is_empty());
        assert!(search(&lecture(), "", 5).is_empty());
        assert!(search(&[], "anything", 5).is_empty());
    }

    #[test]
    fn the_limit_is_respected() {
        let hits = search(&lecture(), "matrices probability gradient", 2);
        assert_eq!(hits.len(), 2);
    }

    /// A chapter with nothing distinctive still needs a label, or the list has
    /// a blank row.
    #[test]
    fn a_chapter_without_keywords_falls_back_to_its_times() {
        let c = Chapter {
            start: Duration::from_secs(65),
            end: Duration::from_secs(190),
            keywords: Vec::new(),
            windows: 0..2,
        };
        assert_eq!(c.title(), "1:05 – 3:10");
    }
}
