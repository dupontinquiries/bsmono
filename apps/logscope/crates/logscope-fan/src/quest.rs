//! quest.rs — a clarify-first pass. Have a (small, cheap) model enumerate the top open questions for the
//! current task, turn them into a fillable Q/A template — multiple-choice where context (including sibling
//! branches) suggests concrete options, open-ended otherwise — and fold the answers back into a focused
//! context block. Front-loading the ambiguity means the expensive generation that follows is targeted
//! instead of fanning branches at an under-specified prompt. That's the speed win: a handful of cheap
//! question tokens + one human (or cross-branch) answer pass replaces a pile of mis-aimed completions.
//!
//! Pure; the CLI/egui layer runs the model and does the I/O.

#[derive(Clone, Debug, PartialEq)]
pub enum QKind { Open, Choice(Vec<String>) }

#[derive(Clone, Debug, PartialEq)]
pub struct Question { pub text: String, pub kind: QKind }

/// Prompt a model for the `n` most important clarifying questions about `task`. If `context` snippets are
/// supplied (e.g. text from sibling branches), the model is told to draw candidate options from them.
pub fn quest_prompt(task: &str, n: usize, context: &[String]) -> String {
    let mut p = format!(
        "A developer asked: \"{task}\".\n\nBefore writing any code, list the {n} most important clarifying \
         questions whose answers would most change the implementation. Output one per line, numbered. When \
         a question has a small set of likely answers, append them in parentheses separated by ' / '."
    );
    if !context.is_empty() {
        p.push_str("\n\nWhen proposing options, consider these notes from related attempts:\n");
        for c in context.iter().take(6) {
            let s: String = c.chars().take(160).collect::<String>().replace('\n', " ");
            p.push_str(&format!("- {s}\n"));
        }
    }
    p
}

/// Parse a model's question list into structured questions. Recognizes lines led by `1.` / `1)` / `Q:` /
/// `Q1:` / `-` / `*`, an inline trailing `(a / b / c)` option group, and a following `Options:` line.
pub fn parse_questions(out: &str) -> Vec<Question> {
    let mut qs: Vec<Question> = Vec::new();
    for raw in out.lines() {
        let line = raw.trim();
        if line.is_empty() { continue; }
        if let Some(rest) = strip_q_lead(line) {
            let (text, choices) = split_inline_options(&rest);
            if text.is_empty() { continue; }
            qs.push(Question { text, kind: if choices.is_empty() { QKind::Open } else { QKind::Choice(choices) } });
        } else if let Some(opts) = strip_options_lead(line) {
            if let Some(last) = qs.last_mut() {
                let c = split_list(opts);
                if c.len() >= 2 { last.kind = QKind::Choice(c); }
            }
        }
    }
    qs
}

fn strip_q_lead(line: &str) -> Option<String> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() { i += 1; }
    if i > 0 && i < b.len() && (b[i] == b'.' || b[i] == b')') {
        return Some(line[i + 1..].trim().to_string());
    }
    for pre in ["Q:", "Q.", "- ", "* "] {
        if let Some(r) = line.strip_prefix(pre) { return Some(r.trim().to_string()); }
    }
    if line.starts_with('Q') {
        let rest = &line[1..];
        let rb = rest.as_bytes();
        let mut j = 0;
        while j < rb.len() && rb[j].is_ascii_digit() { j += 1; }
        if j < rb.len() && (rb[j] == b':' || rb[j] == b'.') { return Some(rest[j + 1..].trim().to_string()); }
    }
    None
}

fn strip_options_lead(line: &str) -> Option<&str> {
    for pre in ["Options:", "Choices:", "options:", "choices:"] {
        if let Some(r) = line.strip_prefix(pre) { return Some(r.trim()); }
    }
    None
}

/// Split a trailing "(a / b / c)" off the question text → (clean_text, options).
fn split_inline_options(text: &str) -> (String, Vec<String>) {
    let t = text.trim();
    if t.ends_with(')') {
        if let Some(open) = t.rfind('(') {
            let inner = &t[open + 1..t.len() - 1];
            if inner.contains('/') || inner.contains(',') {
                let opts = split_list(inner);
                if opts.len() >= 2 { return (t[..open].trim().to_string(), opts); }
            }
        }
    }
    (t.to_string(), Vec::new())
}

fn split_list(s: &str) -> Vec<String> {
    let sep = if s.contains('/') { '/' } else { ',' };
    s.split(sep).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// From sibling-branch texts, propose up to `k` candidate options for `question` by keyword overlap
/// (stem + substring, so "sorting" connects to "quicksort").
pub fn harvest_options(question: &str, branch_texts: &[String], k: usize) -> Vec<String> {
    let qk: Vec<String> = keywords(question).into_iter().collect();
    let mut scored: Vec<(usize, String)> = Vec::new();
    for t in branch_texts {
        let bk: Vec<String> = keywords(t).into_iter().collect();
        let overlap = qk.iter().filter(|q| bk.iter().any(|b| related(q, b))).count();
        if overlap > 0 {
            let snip = first_clause(t);
            if !snip.is_empty() { scored.push((overlap, snip)); }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = Vec::new();
    for (_, s) in scored {
        if !out.contains(&s) { out.push(s); }
        if out.len() >= k { break; }
    }
    out
}

fn stem(w: &str) -> String {
    for suf in ["ing", "ed", "s"] {
        if w.len() > suf.len() + 2 && w.ends_with(suf) { return w[..w.len() - suf.len()].to_string(); }
    }
    w.to_string()
}
fn related(a: &str, b: &str) -> bool {
    let (a, b) = (stem(a), stem(b));
    a.len() >= 4 && b.len() >= 4 && (a.contains(&b) || b.contains(&a))
}

fn keywords(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 3).map(|w| w.to_string()).collect()
}
fn first_clause(s: &str) -> String {
    s.trim().split(|c| c == '.' || c == '\n' || c == ',').next().unwrap_or("").trim().chars().take(80).collect()
}

/// Render a fillable Q/A template. MC questions list lettered options; every question gets an `A:` line.
pub fn render_template(qs: &[Question]) -> String {
    let mut out = String::new();
    for q in qs {
        out.push_str(&format!("Q: {}\n", q.text));
        if let QKind::Choice(opts) = &q.kind {
            let labels: Vec<String> = opts.iter().enumerate().map(|(i, o)| format!("[{}] {o}", (b'a' + i as u8) as char)).collect();
            out.push_str(&format!("   {}\n", labels.join("   ")));
        }
        out.push_str("A: \n\n");
    }
    out
}

/// Parse a filled Q/A block back into (question, answer) pairs (only answers that were filled in).
pub fn parse_answers(filled: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut q: Option<String> = None;
    for line in filled.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("Q:") { q = Some(r.trim().to_string()); }
        else if let Some(r) = l.strip_prefix("A:") {
            let a = r.trim().to_string();
            if let Some(qq) = q.take() { if !a.is_empty() { pairs.push((qq, a)); } }
        }
    }
    pairs
}

/// Turn answered Q/A into a context block to prepend to the next, now-targeted generation.
pub fn clarified_context(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() { return String::new(); }
    let mut s = String::from("Resolved decisions:\n");
    for (q, a) in pairs { s.push_str(&format!("- {q} → {a}\n")); }
    s.push_str("\nImplement using exactly these decisions.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_inline_and_options_line() {
        let out = "Here are the questions:\n\
                   1. Which menu screen do we fix first? (Create World / Settings / Title)\n\
                   2. What should the render-distance range be?\n\
                   Q3: Seed input type?\n\
                   Options: numeric / text\n";
        let qs = parse_questions(out);
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0].text, "Which menu screen do we fix first?");
        assert_eq!(qs[0].kind, QKind::Choice(vec!["Create World".into(), "Settings".into(), "Title".into()]));
        assert_eq!(qs[1].kind, QKind::Open);
        assert_eq!(qs[2].kind, QKind::Choice(vec!["numeric".into(), "text".into()]));
    }

    #[test]
    fn harvest_options_ranks_by_overlap() {
        let q = "which sorting algorithm should we use";
        let branches = vec![
            "I implemented quicksort with a pivot.".to_string(),
            "This uses mergesort, stable and O(n log n).".to_string(),
            "Unrelated note about networking sockets.".to_string(),
        ];
        let opts = harvest_options(q, &branches, 3);
        assert!(opts.iter().any(|o| o.contains("quicksort")));
        assert!(opts.iter().any(|o| o.contains("mergesort")));
        assert!(!opts.iter().any(|o| o.contains("networking")));
    }

    #[test]
    fn render_and_parse_roundtrip() {
        let qs = vec![
            Question { text: "Fix which screen?".into(), kind: QKind::Choice(vec!["Create".into(), "Settings".into()]) },
            Question { text: "Slider range?".into(), kind: QKind::Open },
        ];
        let tmpl = render_template(&qs);
        assert!(tmpl.contains("[a] Create   [b] Settings"));
        // simulate a human filling answers
        let filled = tmpl.replace("Q: Fix which screen?\n   [a] Create   [b] Settings\nA: ",
                                  "Q: Fix which screen?\nA: Create World")
                         .replace("Q: Slider range?\nA: ", "Q: Slider range?\nA: 4 to 32 chunks");
        let pairs = parse_answers(&filled);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("Fix which screen?".into(), "Create World".into()));
        let ctx = clarified_context(&pairs);
        assert!(ctx.contains("Fix which screen? → Create World"));
        assert!(ctx.contains("Slider range? → 4 to 32 chunks"));
    }
}
