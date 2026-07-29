//! stepview.rs — render a stepwise reply as the outline it already is.
//!
//! A coordinator run emits `⏸ STEP k/n — summary` between chunks of work. Presented as one blob that is
//! a wall of text; the markers are a table of contents nobody is reading.
//!
//! The observation that prompted this: a verbose reply that arrives FAST is not a problem to suppress.
//! Nine steps in two seconds is fine — the cost is not the tokens, it is that a reader has to scroll
//! through all of them to find the one that matters. Suppressing the verbosity would also destroy the
//! structure; rendering it keeps both.
//!
//! Splitting on markers the model already emits means no extra prompting, no second parse of the prose,
//! and it degrades to a single section when the model ignores the protocol.

/// One step's worth of a reply.
#[derive(Clone, PartialEq, Debug)]
pub struct Step {
    /// `k` from the marker, or None for text before the first one.
    pub index: Option<u32>,
    pub total: Option<u32>,
    /// The marker's one-line summary — what the model says it did.
    pub summary: String,
    /// Everything between this marker and the next.
    pub body: String,
}

impl Step {
    /// Header for a collapsed section.
    pub fn header(&self) -> String {
        match (self.index, self.total) {
            (Some(k), Some(n)) => format!("{k}/{n}  {}", self.summary),
            (Some(k), None) => format!("{k}  {}", self.summary),
            _ => self.summary.clone(),
        }
    }

    /// Lines of body, for deciding whether folding it saves anything.
    pub fn lines(&self) -> usize { self.body.lines().filter(|l| !l.trim().is_empty()).count() }
}

/// Parse `⏸ STEP k/n — summary` markers, in the form `stepwise::STEPWISE_PROMPT` asks for.
///
/// The marker sits AFTER the work it describes, so a marker's summary belongs to the text ABOVE it.
/// Getting that backwards labels every section with the summary of the previous one — which reads
/// plausibly and is wrong throughout, since consecutive step summaries are usually similar.
pub fn split_steps(text: &str) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    let mut body = String::new();
    for line in text.lines() {
        if let Some((k, n, summary)) = parse_marker(line) {
            out.push(Step { index: k, total: n, summary, body: std::mem::take(&mut body) });
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    // Trailing text after the last marker: the final answer, usually, and the part a reader wants.
    if !body.trim().is_empty() {
        out.push(Step { index: None, total: None, summary: String::new(), body });
    }
    out
}

fn parse_marker(line: &str) -> Option<(Option<u32>, Option<u32>, String)> {
    let t = line.trim();
    // Tolerate the pause glyph being absent: models drop it, and a marker without it is still a marker.
    let rest = t.strip_prefix('\u{23F8}').map(str::trim_start).unwrap_or(t);
    let rest = rest.strip_prefix("STEP")?.trim_start();
    let (nums, summary) = match rest.find(|c| c == '\u{2014}' || c == '-') {
        Some(i) => (&rest[..i], rest[i..].trim_start_matches(['\u{2014}', '-']).trim()),
        None => (rest, ""),
    };
    let mut it = nums.trim().split('/');
    let k = it.next().and_then(|v| v.trim().parse::<u32>().ok());
    let n = it.next().and_then(|v| v.trim().parse::<u32>().ok());
    k?;                                    // "STEP" with no number is prose, not a marker
    Some((k, n, summary.to_string()))
}

/// Should a step be folded by default?
///
/// The LAST step is never folded: it holds the final answer, and folding what someone is looking for
/// makes the outline worse than the wall of text it replaced.
pub fn fold_by_default(steps: &[Step], i: usize) -> bool {
    if i + 1 == steps.len() { return false; }
    steps[i].lines() > 3
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "\
Let me start with step 1.
Reading the input now.
\u{23F8} STEP 1/3 — Read and parse the user's message.
Generating the response.
\u{23F8} STEP 2/3 — Generate a response.
Here it is.
\u{23F8} STEP 3/3 — Output the response.
hi there";

    #[test]
    fn a_markers_summary_labels_the_text_ABOVE_it() {
        // The marker sits AFTER the work it describes. Getting this backwards labels every section with
        // the previous summary — which reads plausibly and is wrong throughout, because consecutive step
        // summaries are usually similar enough that nobody notices.
        let s = split_steps(REPLY);
        assert_eq!(s[0].index, Some(1));
        assert!(s[0].body.contains("Reading the input now"), "body is what came BEFORE the marker");
        assert!(!s[0].body.contains("Generating the response"));
        assert_eq!(s[1].summary, "Generate a response.");
        assert!(s[1].body.contains("Generating the response"));
    }

    #[test]
    fn the_final_answer_survives_as_its_own_section() {
        // Text after the last marker is the part a reader actually wants. Dropping it — easy, if the
        // loop only emits on a marker — would lose the answer and keep the narration.
        let s = split_steps(REPLY);
        let last = s.last().unwrap();
        assert_eq!(last.index, None);
        assert!(last.body.contains("hi there"));
    }

    #[test]
    fn the_last_section_is_never_folded() {
        // Folding what someone is looking for makes the outline worse than the wall of text it replaced.
        let s = split_steps(REPLY);
        assert!(!fold_by_default(&s, s.len() - 1));
    }

    #[test]
    fn short_steps_are_not_folded_either() {
        // A chevron over two lines costs a click and saves nothing.
        let s = split_steps(REPLY);
        assert!(!fold_by_default(&s, 0), "one line of body is not worth folding");
    }

    #[test]
    fn a_reply_with_no_markers_is_one_section_not_zero() {
        // Models ignore protocols. Degrading to a single section keeps the renderer working; returning
        // nothing would blank the message.
        let s = split_steps("just a normal answer\nwith two lines");
        assert_eq!(s.len(), 1);
        assert!(s[0].body.contains("normal answer"));
        assert_eq!(s[0].index, None);
    }

    #[test]
    fn a_missing_pause_glyph_still_parses() {
        // Models drop the glyph. A marker without it is still a marker, and refusing it would silently
        // turn the outline back into a wall of text.
        let s = split_steps("work\nSTEP 2/5 — did the thing\nmore");
        assert_eq!(s[0].index, Some(2));
        assert_eq!(s[0].total, Some(5));
        assert_eq!(s[0].summary, "did the thing");
    }

    #[test]
    fn the_word_STEP_in_prose_is_not_a_marker() {
        // "the next STEP is to..." must not split the message. A number is required.
        let s = split_steps("the next STEP is to compile\nthen run it");
        assert_eq!(s.len(), 1, "prose split into {} sections", s.len());
    }

    #[test]
    fn a_header_reads_usefully_without_the_body() {
        // The point of the outline: decide which section to open without opening any.
        let s = split_steps(REPLY);
        assert_eq!(s[0].header(), "1/3  Read and parse the user's message.");
    }
}
