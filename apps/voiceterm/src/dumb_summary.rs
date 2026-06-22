// src/dumb_summary.rs
//
// Non-AI ("dumb") fallback for log summarization. Used when:
//   - AI summary feature is disabled outright
//   - AI is enabled but ollama isn't available (this module fills the
//     gap until a real summary lands)
//   - We want deterministic, instant summaries with no model dependency
//
// Two responsibilities:
//
//   1. SYNTHESIZE a one-line summary of a command + its output using
//      simple rule-based extraction. Recognizes a few common command
//      shapes (ls, dust, ps, git status, file paths) and falls back
//      to a generic line-count summary for anything else.
//
//   2. SCORE the relevance of a recent block to a voice utterance.
//      Returns 0..1 confidence based on keyword overlap. Used by the
//      QML side to expand/highlight blocks the user is talking about.
//
// Design philosophy: cheap, predictable, offline. No async, no
// background workers, no model load. Direct functions called on demand.

use std::collections::HashMap;

/// Output of the synthetic summary.
#[derive(Clone, Debug)]
pub struct DumbSummary {
    /// Short one-line plain-text summary, like "5 files: 2 .rs, 1 .toml,
    /// 1 .md, 1 dir". Fits in a single chip.
    pub text: String,
    /// Keywords extracted from the output, lowercased + de-duped. Used
    /// by score_relevance to compare against utterances.
    pub keywords: Vec<String>,
}

/// Generate a simple synthetic summary for a (command, output) pair.
/// Cheap (microseconds) and always returns something useful.
///
/// Recognized command shapes:
///   - ls / ls -l / ll / la → file count + extension breakdown
///   - dust / du → top 3 size consumers
///   - ps / top (one-shot, not alt-screen) → process count + heaviest
///   - git status → branch, dirty file count, untracked count
///   - cat / less / head / tail of a known file → file type + line count
///   - rg / grep → match count + top files
///   - find → match count
///   - everything else → "N lines, M unique words" + first significant words
pub fn summarize(command: &str, output: &str) -> DumbSummary {
    let cmd_lower = command.trim().to_lowercase();
    let first_word = cmd_lower.split_whitespace().next().unwrap_or("");
    let line_count = output.lines().filter(|l| !l.trim().is_empty()).count();
    if line_count == 0 {
        return DumbSummary { text: "(no output)".to_string(), keywords: vec![] };
    }

    // Per-command rule paths.
    match first_word {
        "ls" | "ll" | "la" | "dir" => return summarize_ls(output),
        "dust" => return summarize_dust(output),
        "du" => return summarize_du(output),
        "ps" | "pgrep" => return summarize_ps(output),
        _ if first_word.starts_with("git") || cmd_lower.starts_with("git ") => {
            return summarize_git(&cmd_lower, output);
        }
        "rg" | "grep" | "ag" | "ack" => return summarize_grep(output),
        "find" | "fd" => return summarize_find(output),
        "zip" => return summarize_zip(output),
        _ => {}
    }

    // Generic fallback: line count + first N keywords. Also scan for
    // ASCII progress-bar rows (e.g. `[██████░░░░░░] 47%` or
    // `│██████░░░░░│ 47/100`). When found, surface the parsed
    // percentage in BOTH the summary text AND the keywords list, so
    // (a) the user sees "47% complete" in the chip, (b) the relevance
    // scorer can match utterances like "what's the progress" against
    // the keyword "progress:47".
    let kws = extract_keywords(output, 12);
    let bars = detect_progress_bars(output);
    let preview: Vec<&str> = kws.iter().take(4).map(|s| s.as_str()).collect();
    let mut text = if !bars.is_empty() {
        // Latest progress bar wins — most recent line of streaming
        // output is usually the most recent bar update.
        let last = bars.last().unwrap();
        format!("{} lines · progress {}%", line_count, last.percent)
    } else if preview.is_empty() {
        format!("{} lines of output", line_count)
    } else {
        format!("{} lines · keywords: {}", line_count, preview.join(", "))
    };
    let _ = &mut text;  // silence unused-mut on cold path
    let mut all_keywords = kws;
    if !bars.is_empty() {
        // Embed each detected bar as a structured keyword. Format
        // `progress:N` for percentages, `progress:N/M` for fractional
        // forms. Multiple bars (e.g. tqdm with separate sub-tasks)
        // each get their own entry.
        for bar in &bars {
            if let Some((num, denom)) = bar.fraction {
                all_keywords.push(format!("progress:{}/{}", num, denom));
            } else {
                all_keywords.push(format!("progress:{}", bar.percent));
            }
        }
        // Generic "progress" word so utterances like "is it done yet"
        // / "what's the status" can fuzzy-match via the score_relevance
        // substring check.
        all_keywords.push("progress".to_string());
    }
    DumbSummary { text, keywords: all_keywords }
}

/// One detected ASCII progress bar in command output.
#[derive(Clone, Debug)]
pub struct DetectedBar {
    /// Inferred completion percentage 0..100 (clamped). For bars with
    /// no explicit number alongside, derived from filled/total cell
    /// ratio.
    pub percent: u32,
    /// Optional explicit fraction (e.g. "47/100" displayed alongside
    /// the bar). When present, percent = 100*num/denom.
    pub fraction: Option<(u32, u32)>,
}

/// Scan output for ASCII progress-bar patterns. Returns one entry
/// per detected bar, in line order.
///
/// Recognized shapes:
///   - `[██████░░░░] 47%`    (Unicode block elements + percent)
///   - `[#####-----] 47%`    (ASCII hash + dash)
///   - `│██████░░░░│`        (box-drawing pipes + blocks)
///   - tqdm: `47%|████░░░░| 47/100 [00:23<00:25, ...]`
///   - bare percentage on its own line if surrounded by repeating
///     fill chars (e.g. cargo's `Compiling xxx` doesn't qualify;
///     `Building 47% [####....]` does).
///
/// Heuristic: a line counts as a bar if it contains a CONTIGUOUS run
/// of fill chars (`█▓▒░#=*-`) of length >= 6, optionally followed by
/// a percentage or N/M token. Without this minimum length, ordinary
/// text with one `=` would false-positive.
pub fn detect_progress_bars(output: &str) -> Vec<DetectedBar> {
    let mut out = Vec::new();
    // Filled chars (left/dark side of the bar).
    const FILLED: &[char] = &['█', '▓', '▒', '#', '=', '*'];
    // "Empty" / right-side chars.
    const EMPTY:  &[char] = &['░', '-', '.', ' '];

    for line in output.lines() {
        // Quick reject: must contain at least one filled char.
        if !line.chars().any(|c| FILLED.contains(&c)) { continue; }

        // Walk the line, finding the longest run of (FILLED|EMPTY) chars
        // that contains at least one FILLED. This is the bar's body.
        let chars: Vec<char> = line.chars().collect();
        let mut best_start = 0usize;
        let mut best_len = 0usize;
        let mut best_filled = 0usize;
        let mut i = 0usize;
        while i < chars.len() {
            if !FILLED.contains(&chars[i]) && !EMPTY.contains(&chars[i]) {
                i += 1; continue;
            }
            let start = i;
            let mut filled_count = 0usize;
            while i < chars.len()
                && (FILLED.contains(&chars[i]) || EMPTY.contains(&chars[i]))
            {
                if FILLED.contains(&chars[i]) { filled_count += 1; }
                i += 1;
            }
            let len = i - start;
            if len >= 6 && filled_count >= 1 && len > best_len {
                best_start = start;
                best_len = len;
                best_filled = filled_count;
            }
        }
        if best_len < 6 { continue; }

        // Look for an explicit percentage or N/M after the bar end.
        let after: String = chars[best_start + best_len ..].iter().collect();
        let mut explicit_percent: Option<u32> = None;
        let mut fraction: Option<(u32, u32)> = None;

        // Try percentage match first (e.g. " 47%" / "47.5%").
        if let Some(pct) = parse_first_percent(&after) {
            explicit_percent = Some(pct.min(100));
        }
        // Try fraction match (e.g. " 47/100" / "47 of 100").
        if let Some((n, d)) = parse_first_fraction(&after) {
            if d > 0 {
                fraction = Some((n, d));
                if explicit_percent.is_none() {
                    explicit_percent = Some(((n as u64 * 100) / d as u64).min(100) as u32);
                }
            }
        }
        // Also try BEFORE the bar — tqdm puts percent on the LEFT
        // (e.g. "47%|████░░░░|").
        if explicit_percent.is_none() {
            let before: String = chars[..best_start].iter().collect();
            if let Some(pct) = parse_last_percent(&before) {
                explicit_percent = Some(pct.min(100));
            }
        }

        let percent = explicit_percent.unwrap_or_else(|| {
            // Fallback: derive from fill ratio.
            ((best_filled as u64 * 100) / best_len as u64).min(100) as u32
        });

        out.push(DetectedBar { percent, fraction });
    }
    out
}

/// Find the FIRST percentage in a string (e.g. " 47%" or "47.3%").
/// Decimal portion is dropped — we report integer percent.
fn parse_first_percent(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            // v0.1.466: remember where the integer part ends BEFORE
            // consuming any decimal portion. The prior code parsed
            // bytes[start..i] after advancing i past ".3", so the
            // slice was "47.3" which u32::parse REJECTS → the
            // `if let Ok` failed silently → None (the bug behind
            // parse_percent_handles_decimals). Parse the integer
            // part only; the decimal is for display, we truncate.
            let int_end = i;
            // Skip optional decimal portion (for position only).
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            }
            // Need a literal '%' immediately after (allowing one space).
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' { j += 1; }
            if j < bytes.len() && bytes[j] == b'%' {
                if let Ok(val) = std::str::from_utf8(&bytes[start..int_end])
                    .unwrap_or("0").parse::<u32>() {
                    return Some(val);
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Find the LAST percentage in a string. Used for tqdm-style "47%|██..."
/// where the percent is to the LEFT of the bar.
fn parse_last_percent(s: &str) -> Option<u32> {
    let mut last = None;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            }
            let end_digits = i;
            // Re-find the integer part end (before the '.').
            let int_end = (start..end_digits)
                .find(|&k| !bytes[k].is_ascii_digit())
                .unwrap_or(end_digits);
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' { j += 1; }
            if j < bytes.len() && bytes[j] == b'%' {
                if let Ok(val) = std::str::from_utf8(&bytes[start..int_end])
                    .unwrap_or("0").parse::<u32>() {
                    last = Some(val);
                }
            }
        } else {
            i += 1;
        }
    }
    last
}

/// Find the FIRST fraction "N/M" or "N of M". Returns (N, M).
fn parse_first_fraction(s: &str) -> Option<(u32, u32)> {
    // Pattern: digits, optional whitespace, '/' or "of", optional ws, digits.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1; continue;
        }
        let n_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
        let n_end = i;
        // Skip whitespace
        let mut j = i;
        while j < bytes.len() && bytes[j] == b' ' { j += 1; }
        // Either '/' or the literal "of"
        let denom_start = if j < bytes.len() && bytes[j] == b'/' {
            j + 1
        } else if j + 2 <= bytes.len() && &bytes[j..j+2] == b"of" {
            j + 2
        } else {
            i = n_end + 1; continue;
        };
        let mut k = denom_start;
        while k < bytes.len() && bytes[k] == b' ' { k += 1; }
        if k >= bytes.len() || !bytes[k].is_ascii_digit() {
            i = n_end + 1; continue;
        }
        let m_start = k;
        while k < bytes.len() && bytes[k].is_ascii_digit() { k += 1; }
        let n_str = std::str::from_utf8(&bytes[n_start..n_end]).unwrap_or("0");
        let m_str = std::str::from_utf8(&bytes[m_start..k]).unwrap_or("0");
        if let (Ok(n), Ok(m)) = (n_str.parse::<u32>(), m_str.parse::<u32>()) {
            // Sanity: don't accept fractions with denominator < numerator
            // (those are typically file paths like "/usr/bin").
            if m >= n && m > 0 && m < 1_000_000 {
                return Some((n, m));
            }
        }
        i = n_end + 1;
    }
    None
}

fn summarize_ls(output: &str) -> DumbSummary {
    // Count entries by extension. Lines may be in long-format ("-rw-r--r--
    // 1 user user 234 ...") or short-format ("file.rs  dir/  other.txt").
    // Detect format by leading char of first non-empty line.
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return DumbSummary { text: "Empty directory".to_string(), keywords: vec![] };
    }
    // v0.1.466: detect long-format by the first line that ISN'T
    // the "total N" header. The prior code checked lines[0]
    // directly, but for `ls -l` output lines[0] is "total 8",
    // which doesn't start with d/-/l → long_format=false → the
    // short-format path split every token of every row (29
    // "entries" for a 3-file listing — the bug behind
    // ls_long_format). Skip the header when sampling.
    let sample = lines.iter()
        .find(|l| !l.starts_with("total "))
        .copied()
        .unwrap_or(lines[0]);
    let long_format = sample.starts_with(|c: char| c == 'd' || c == '-' || c == 'l');
    let mut entries: Vec<&str> = Vec::new();
    if long_format {
        // Extract the last whitespace-separated token from each row
        // (skip "total N" header). Typical: "-rw-r--r-- 1 user user 234 Nov 3 10:12 README.md"
        for line in &lines {
            if line.starts_with("total ") { continue; }
            if let Some(last) = line.split_whitespace().last() {
                entries.push(last);
            }
        }
    } else {
        // Short format: split on whitespace, each token is an entry.
        for line in &lines {
            for tok in line.split_whitespace() {
                entries.push(tok);
            }
        }
    }
    let count = entries.len();
    // Bucket by extension.
    let mut ext_counts: HashMap<String, u32> = HashMap::new();
    let mut dirs = 0u32;
    for e in &entries {
        if e.ends_with('/') { dirs += 1; continue; }
        let ext = e.rsplit('.').next().unwrap_or("");
        if ext.is_empty() || ext == *e {
            *ext_counts.entry("(no ext)".to_string()).or_insert(0) += 1;
        } else if ext.len() > 8 {
            // Probably not a real extension (filename with dot), bucket as no-ext.
            *ext_counts.entry("(no ext)".to_string()).or_insert(0) += 1;
        } else {
            *ext_counts.entry(format!(".{}", ext)).or_insert(0) += 1;
        }
    }
    let mut top: Vec<(String, u32)> = ext_counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let top_str: Vec<String> = top.iter().take(4)
        .map(|(ext, n)| format!("{} {}", n, ext))
        .collect();
    let mut text = format!("{} entries", count);
    if dirs > 0 {
        text.push_str(&format!(" ({} dirs)", dirs));
    }
    if !top_str.is_empty() {
        text.push_str(": ");
        text.push_str(&top_str.join(", "));
    }
    DumbSummary {
        text,
        keywords: entries.iter().take(20).map(|s| s.to_lowercase()).collect(),
    }
}

fn summarize_dust(output: &str) -> DumbSummary {
    // dust output: each non-tree-character line has a size + path. We just
    // grab the top 3 sized entries by parsing leading whitespace + size token.
    let mut sized: Vec<(String, String)> = Vec::new();  // (size, path)
    for line in output.lines() {
        let trimmed = line.trim();
        let mut tokens = trimmed.split_whitespace();
        let size = match tokens.next() { Some(s) => s, None => continue };
        // Quick size validation: starts with digit, ends with size suffix
        if !size.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { continue; }
        let last_char = size.chars().last().unwrap_or(' ');
        if !matches!(last_char, 'K'|'k'|'M'|'m'|'G'|'g'|'T'|'t'|'B'|'b') { continue; }
        // Find a path-like token in the rest of the line
        for tok in tokens {
            if tok.contains('/') || tok.starts_with('.') {
                sized.push((size.to_string(), tok.to_string()));
                break;
            }
        }
    }
    if sized.is_empty() {
        return DumbSummary { text: "(unparseable dust output)".to_string(), keywords: vec![] };
    }
    // Skip the synthetic total row at the end (path=".") — already-noted bug.
    sized.retain(|(_, p)| p != "." && p != "./");
    // dust output is sorted ascending; take last 3 (largest).
    let top: Vec<&(String, String)> = sized.iter().rev().take(3).collect();
    let parts: Vec<String> = top.iter().map(|(s, p)| {
        let basename = p.rsplit('/').next().unwrap_or(p);
        format!("{} ({})", basename, s)
    }).collect();
    let kws: Vec<String> = sized.iter()
        .map(|(_, p)| p.rsplit('/').next().unwrap_or(p).to_lowercase())
        .collect();
    DumbSummary {
        text: format!("Top space: {}", parts.join(", ")),
        keywords: kws,
    }
}

fn summarize_du(output: &str) -> DumbSummary {
    // du output is line-by-line "size\tpath". Total at end if -c was used.
    let mut sized: Vec<(String, String)> = Vec::new();
    for line in output.lines() {
        let mut parts = line.split('\t');
        let size = match parts.next() { Some(s) => s.trim(), None => continue };
        let path = match parts.next() { Some(p) => p.trim(), None => continue };
        if size.is_empty() || path.is_empty() { continue; }
        sized.push((size.to_string(), path.to_string()));
    }
    if sized.is_empty() {
        // fallback: first line preview
        return DumbSummary {
            text: format!("{} lines", output.lines().count()),
            keywords: extract_keywords(output, 8),
        };
    }
    let count = sized.len();
    let last = &sized[sized.len() - 1];
    DumbSummary {
        text: format!("{} entries; total {} ({})", count, last.0, last.1),
        keywords: sized.iter().map(|(_, p)|
            p.rsplit('/').next().unwrap_or(p).to_lowercase()).collect(),
    }
}

fn summarize_ps(output: &str) -> DumbSummary {
    // First line is the header. Remaining are processes.
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= 1 {
        return DumbSummary { text: "(no processes)".to_string(), keywords: vec![] };
    }
    let count = lines.len() - 1;
    // Try to find the COMMAND column index from the header.
    let header = lines[0];
    let words: Vec<&str> = header.split_whitespace().collect();
    let cmd_col = words.iter().position(|w|
        w.eq_ignore_ascii_case("COMMAND") || w.eq_ignore_ascii_case("CMD"));
    let mut commands: Vec<String> = Vec::new();
    for line in &lines[1..] {
        let row: Vec<&str> = line.split_whitespace().collect();
        if let Some(col) = cmd_col {
            if let Some(cmd) = row.get(col) {
                let basename = cmd.rsplit('/').next().unwrap_or(cmd);
                commands.push(basename.to_string());
            }
        }
    }
    let preview: Vec<&str> = commands.iter().take(3).map(|s| s.as_str()).collect();
    let text = if preview.is_empty() {
        format!("{} processes", count)
    } else {
        format!("{} processes incl. {}", count, preview.join(", "))
    };
    DumbSummary {
        text,
        keywords: commands.iter().map(|s| s.to_lowercase()).collect(),
    }
}

fn summarize_git(cmd: &str, output: &str) -> DumbSummary {
    if cmd.contains("status") {
        // Look for branch, modified, untracked counts.
        let mut branch = String::new();
        let mut modified = 0u32;
        let mut untracked = 0u32;
        let mut staged = 0u32;
        for line in output.lines() {
            let l = line.trim();
            if l.starts_with("On branch ") {
                branch = l.strip_prefix("On branch ").unwrap_or("").to_string();
            } else if l.starts_with("modified:") {
                modified += 1;
            } else if l.starts_with("new file:") || l.starts_with("renamed:")
                || l.starts_with("deleted:") {
                staged += 1;
            } else if !l.is_empty() && !l.starts_with('#')
                && !l.contains(':') && !l.starts_with("Untracked")
                && !l.starts_with("Changes") && !l.starts_with("nothing")
                && !l.starts_with("(use") && !l.starts_with("On ") {
                // Heuristic: a bare-path line in untracked section.
                // Conservative — false positives are fine for dumb mode.
                if l.split_whitespace().count() == 1 {
                    untracked += 1;
                }
            }
        }
        let mut parts = Vec::new();
        if !branch.is_empty() { parts.push(format!("on {}", branch)); }
        if modified > 0 { parts.push(format!("{} modified", modified)); }
        if staged > 0 { parts.push(format!("{} staged", staged)); }
        if untracked > 0 { parts.push(format!("{} untracked", untracked)); }
        if parts.is_empty() && output.contains("nothing to commit") {
            parts.push("clean".to_string());
        }
        let text = if parts.is_empty() { "git status".to_string() }
            else { parts.join(", ") };
        return DumbSummary {
            text,
            keywords: vec![branch.to_lowercase()],
        };
    }
    if cmd.contains("log") {
        let count = output.lines().filter(|l| l.starts_with("commit ")).count();
        return DumbSummary {
            text: format!("{} commits", count),
            keywords: extract_keywords(output, 8),
        };
    }
    if cmd.contains("branch") {
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        let current = lines.iter()
            .find(|l| l.starts_with('*'))
            .map(|l| l.trim_start_matches(['*', ' ']).to_string());
        let mut text = format!("{} branches", lines.len());
        if let Some(c) = current {
            text.push_str(&format!("; current: {}", c));
        }
        return DumbSummary {
            text,
            keywords: lines.iter().map(|l|
                l.trim_start_matches(['*', ' ']).to_lowercase()).collect(),
        };
    }
    DumbSummary {
        text: format!("git output ({} lines)", output.lines().count()),
        keywords: extract_keywords(output, 8),
    }
}

fn summarize_grep(output: &str) -> DumbSummary {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let count = lines.len();
    // Try to extract unique source files (prefix before colon).
    let mut files: HashMap<String, u32> = HashMap::new();
    for line in &lines {
        if let Some(idx) = line.find(':') {
            let f = &line[..idx];
            // Only count if it looks like a file path
            if f.contains('/') || f.contains('.') {
                *files.entry(f.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut top: Vec<(String, u32)> = files.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let top_str: Vec<String> = top.iter().take(3)
        .map(|(f, n)| {
            let basename = f.rsplit('/').next().unwrap_or(f);
            format!("{} ({}x)", basename, n)
        }).collect();
    let text = if top_str.is_empty() {
        format!("{} matches", count)
    } else {
        format!("{} matches in {}", count, top_str.join(", "))
    };
    DumbSummary {
        text,
        keywords: top.iter().map(|(f, _)|
            f.rsplit('/').next().unwrap_or(f).to_lowercase()).collect(),
    }
}

fn summarize_find(output: &str) -> DumbSummary {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let count = lines.len();
    // Group by extension for context.
    let mut ext_counts: HashMap<String, u32> = HashMap::new();
    for line in &lines {
        let p = line.trim();
        let basename = p.rsplit('/').next().unwrap_or(p);
        let ext = basename.rsplit('.').next().unwrap_or("");
        if !ext.is_empty() && ext != basename && ext.len() <= 8 {
            *ext_counts.entry(format!(".{}", ext)).or_insert(0) += 1;
        }
    }
    let mut top: Vec<(String, u32)> = ext_counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let top_str: Vec<String> = top.iter().take(3)
        .map(|(ext, n)| format!("{} {}", n, ext))
        .collect();
    let text = if top_str.is_empty() {
        format!("{} matches", count)
    } else {
        format!("{} matches: {}", count, top_str.join(", "))
    };
    DumbSummary {
        text,
        keywords: lines.iter().take(20).map(|l|
            l.rsplit('/').next().unwrap_or(l).to_lowercase()).collect(),
    }
}

/// One entry parsed from a `zip` run's `adding: <path> (deflated N%)` /
/// `(stored 0%)` lines.
#[derive(Debug, Clone, PartialEq)]
pub struct ZipEntry {
    pub path: String,
    /// compression ratio percent (0 for stored), None if the line had no %.
    pub deflated_pct: Option<u32>,
    pub stored: bool,
    pub is_dir: bool,
}

/// A structured report of a zip run — the "interactive/previewed zip with
/// summaries" core. Pure: parse the textual zip output into something the
/// terminal can render as a preview AND serialize for a peer handoff.
#[derive(Debug, Clone, PartialEq)]
pub struct ZipReport {
    pub entries: Vec<ZipEntry>,
    pub file_count: usize,
    pub dir_count: usize,
    /// average deflate % across compressed (non-stored, non-dir) entries.
    pub avg_deflate: u32,
    /// the archive name, if it could be recovered from the command.
    pub archive: Option<String>,
}

/// Parse `zip` output lines into a ZipReport. Tolerant: ignores non-`adding:`
/// lines (zip banners, the command echo, blank lines).
pub fn parse_zip_report(output: &str) -> ZipReport {
    let mut entries = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        // lines look like: "adding: dir/file.ext (deflated 70%)" or
        // "adding: dir/ (stored 0%)".
        let rest = match line.strip_prefix("adding:").or_else(|| line.strip_prefix("updating:")) {
            Some(r) => r.trim(),
            None => continue,
        };
        // split off the trailing "(... N%)" annotation.
        let (path_part, annot) = match rest.rfind('(') {
            Some(idx) => (rest[..idx].trim(), &rest[idx..]),
            None => (rest, ""),
        };
        if path_part.is_empty() { continue; }
        let is_dir = path_part.ends_with('/');
        let stored = annot.contains("stored");
        let deflated_pct = if annot.contains("deflated") {
            // pull the integer before '%'
            annot.chars().filter(|c| c.is_ascii_digit() || *c == '%')
                .collect::<String>()
                .trim_end_matches('%')
                .parse::<u32>().ok()
        } else { None };
        entries.push(ZipEntry {
            path: path_part.to_string(),
            deflated_pct, stored, is_dir,
        });
    }
    let dir_count = entries.iter().filter(|e| e.is_dir).count();
    let file_count = entries.len() - dir_count;
    let deflates: Vec<u32> = entries.iter()
        .filter(|e| !e.is_dir && !e.stored)
        .filter_map(|e| e.deflated_pct)
        .collect();
    let avg_deflate = if deflates.is_empty() { 0 }
        else { (deflates.iter().sum::<u32>() as usize / deflates.len()) as u32 };
    ZipReport { entries, file_count, dir_count, avg_deflate, archive: None }
}

fn summarize_zip(output: &str) -> DumbSummary {
    let r = parse_zip_report(output);
    if r.entries.is_empty() {
        return DumbSummary { text: "(no archive entries)".to_string(), keywords: vec![] };
    }
    // extension breakdown of files (not dirs).
    let mut ext_counts: HashMap<String, u32> = HashMap::new();
    for e in r.entries.iter().filter(|e| !e.is_dir) {
        let base = e.path.rsplit('/').next().unwrap_or(&e.path);
        let ext = base.rsplit('.').next().unwrap_or("");
        if !ext.is_empty() && ext != base && ext.len() <= 8 {
            *ext_counts.entry(format!(".{}", ext)).or_insert(0) += 1;
        }
    }
    let mut top: Vec<(String, u32)> = ext_counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top_str: Vec<String> = top.iter().take(3).map(|(x, n)| format!("{} {}", n, x)).collect();
    let text = if top_str.is_empty() {
        format!("zipped {} files in {} dirs · avg {}% deflate", r.file_count, r.dir_count, r.avg_deflate)
    } else {
        format!("zipped {} files ({}) in {} dirs · avg {}% deflate",
            r.file_count, top_str.join(", "), r.dir_count, r.avg_deflate)
    };
    let mut keywords: Vec<String> = r.entries.iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.path.rsplit('/').next().unwrap_or(&e.path).to_lowercase())
        .take(20).collect();
    keywords.push("zip".into());
    keywords.push("archive".into());
    DumbSummary { text, keywords }
}

/// Generic keyword extractor: scans output for distinctive tokens
/// (filenames, paths, identifiers) ignoring filler. Returns up to N
/// unique lowercased keywords sorted by frequency.
fn extract_keywords(output: &str, max: usize) -> Vec<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for line in output.lines() {
        for raw in line.split_whitespace() {
            let tok = raw.trim_matches(|c: char|
                !c.is_alphanumeric() && c != '_' && c != '-' && c != '/' && c != '.');
            // v0.1.567: strip a word-final '.' that leaked through the
            // trim above (it allows interior '.' for paths/versions, but
            // a trailing sentence period produced "performance." /
            // "license." in summaries). trim_end only — interior dots
            // (v0.1.567, src/main.rs) and a leading "." are preserved.
            let tok = tok.trim_end_matches('.');
            if tok.len() < 3 || tok.len() > 40 { continue; }
            // Skip pure numbers and common shell noise.
            if tok.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') {
                continue;
            }
            let lower = tok.to_lowercase();
            if STOPWORDS.contains(&lower.as_str()) { continue; }
            *counts.entry(lower).or_insert(0) += 1;
        }
    }
    let mut sorted: Vec<(String, u32)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.into_iter().take(max).map(|(k, _)| k).collect()
}

/// Common filler / structural words that aren't useful as keywords.
/// Short list — bigger sets tend to over-prune. The goal is to
/// prevent words like "the" / "a" from dominating frequency counts.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "for", "of", "to", "in", "on",
    "at", "is", "was", "are", "were", "be", "by", "with", "this", "that",
    "from", "as", "it", "its", "if", "then", "else", "than",
];

/// Score the relevance of a block's keywords to a voice utterance.
/// Returns 0..1 confidence: 0 = no overlap, 1 = strong match.
///
/// Strategy: count utterance words that appear in the block's keyword
/// list (or vice versa for short blocks). Apply a saturation curve so
/// 1-2 hits = moderate confidence (~0.5), 3+ hits = high confidence.
///
/// This is the "linear slider based on token confidence" the user
/// described — QML reads this score per block and can render a thin
/// progress bar to show "this block is N% relevant to what you just
/// said."
pub fn score_relevance(utterance: &str, block_keywords: &[String]) -> f32 {
    if block_keywords.is_empty() { return 0.0; }
    let utt_lower = utterance.to_lowercase();
    let utt_tokens: Vec<&str> = utt_lower.split_whitespace()
        .filter(|t| t.len() >= 3 && !STOPWORDS.contains(t))
        .collect();
    if utt_tokens.is_empty() { return 0.0; }

    let mut hits = 0u32;
    for token in &utt_tokens {
        // Direct keyword match
        if block_keywords.iter().any(|kw| kw == token || kw.contains(token)) {
            hits += 1;
            continue;
        }
        // Substring match the other way (utterance contains keyword)
        if block_keywords.iter().any(|kw| kw.len() >= 3 && token.contains(kw.as_str())) {
            hits += 1;
        }
    }
    if hits == 0 { return 0.0; }
    // Saturation curve: 1 hit = ~0.5, 2 hits = ~0.75, 3+ hits = ~0.9+.
    // Formula: 1 - 0.5^hits, clamped to 0.95 max so the slider never
    // hits exactly 1.0 (visual reservation for "definitive match"
    // states like a literal command-name match).
    let raw = 1.0 - 0.5_f32.powi(hits as i32);
    raw.min(0.95)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_strip_trailing_period_keep_versions_paths() {
        // v0.1.567: a word-final sentence period used to leak into
        // keywords ("performance." / "license."). Verify it's stripped
        // while interior dots (versions, paths) survive.
        let kws = extract_keywords(
            "Focused on performance. Edit src/main.rs and bump v0.1.567 now.", 12);
        assert!(kws.contains(&"performance".to_string()), "got: {:?}", kws);
        assert!(!kws.iter().any(|k| k.ends_with('.')), "no trailing dots: {:?}", kws);
        assert!(kws.contains(&"src/main.rs".to_string()), "path kept: {:?}", kws);
        assert!(kws.contains(&"v0.1.567".to_string()), "version kept: {:?}", kws);
    }

    #[test]
    fn ls_long_format() {
        let out = "total 8\n\
                   -rw-r--r-- 1 alice alice 100 Nov 3 10:00 README.md\n\
                   -rw-r--r-- 1 alice alice 200 Nov 3 10:00 main.rs\n\
                   -rw-r--r-- 1 alice alice 300 Nov 3 10:00 Cargo.toml\n";
        let s = summarize("ls -la", out);
        assert!(s.text.contains("3"));
    }

    #[test]
    fn relevance_substring() {
        let kws = vec!["main.rs".to_string(), "cargo.toml".to_string()];
        // utterance "open main" should match "main.rs" via substring
        assert!(score_relevance("open main", &kws) > 0.4);
    }

    #[test]
    fn relevance_no_match() {
        let kws = vec!["foo.rs".to_string()];
        assert_eq!(score_relevance("xyz qwerty", &kws), 0.0);
    }

    #[test]
    fn progress_bar_with_explicit_percent() {
        let out = "Building: [██████░░░░░░░░] 47%\n";
        let bars = detect_progress_bars(out);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].percent, 47);
    }

    #[test]
    fn progress_bar_tqdm_style() {
        // tqdm output: percent on the LEFT of the bar
        let out = "47%|████████████░░░░░░░░| 47/100 [00:23<00:25, 2.1it/s]\n";
        let bars = detect_progress_bars(out);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].percent, 47);
        assert_eq!(bars[0].fraction, Some((47, 100)));
    }

    #[test]
    fn progress_bar_ratio_only() {
        // No percentage shown — derive from fill ratio.
        // 6 filled out of 12 total = 50%
        let out = "[██████░░░░░░]\n";
        let bars = detect_progress_bars(out);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].percent, 50);
    }

    #[test]
    fn progress_bar_short_line_rejected() {
        // 4-char run — too short, should not detect.
        let out = "use of [###]\n";
        let bars = detect_progress_bars(out);
        assert_eq!(bars.len(), 0);
    }

    #[test]
    fn progress_in_summary_text() {
        let out = "Working...\nBuilding: [██████░░░░] 60%\n";
        let s = summarize("make", out);
        assert!(s.text.contains("60"), "summary text was: {}", s.text);
        // Keyword should also be present for relevance scoring.
        assert!(s.keywords.iter().any(|k| k.starts_with("progress:")));
    }

    #[test]
    fn parse_percent_handles_decimals() {
        assert_eq!(parse_first_percent(" 47.3%"), Some(47));
    }

    #[test]
    fn parse_fraction_rejects_paths() {
        // "/usr/bin" shouldn't trigger fraction parser.
        assert_eq!(parse_first_fraction("/usr/bin"), None);
    }

    // v0.1.734: zip preview/summary — built from the exact dogfood output.
    const ZIP_OUT: &str = "\
adding: plamo--camapp/ (stored 0%)
adding: plamo--camapp/.flatpak-manifest.json (deflated 70%)
adding: plamo--camapp/logo.png (deflated 1%)
adding: plamo--camapp/.gitlab-ci.yml (deflated 41%)
adding: plamo--camapp/logo.png.license (stored 0%)
adding: plamo--camapp/AndroidManifest.xml (deflated 69%)
adding: plamo--camapp/org.kde.plasma.camera.json (deflated 70%)
adding: plamo--camapp/Messages.sh (deflated 29%)
adding: plamo--camapp/.git/ (stored 0%)
adding: plamo--camapp/.git/objects/ (stored 0%)
adding: plamo--camapp/.git/objects/pack/pack-b9ef.pack (deflated 4%)";

    #[test]
    fn parse_zip_counts_and_dirs() {
        let r = parse_zip_report(ZIP_OUT);
        // 3 dirs (camapp/, .git/, objects/), rest files
        assert_eq!(r.dir_count, 3);
        assert_eq!(r.file_count, r.entries.len() - 3);
        // avg deflate over the compressed files (70,1,41,69,70,29,4) = 40
        assert_eq!(r.avg_deflate, (70+1+41+69+70+29+4)/7);
    }

    #[test]
    fn zip_summary_text_has_counts_and_exts() {
        let s = summarize(&"zip -r libcameraexample.zip ./plamo--camapp".to_string(), ZIP_OUT);
        assert!(s.text.starts_with("zipped "), "got: {}", s.text);
        assert!(s.text.contains("deflate"));
        // keyword surface includes filenames + the "zip" tag
        assert!(s.keywords.iter().any(|k| k == "zip"));
        assert!(s.keywords.iter().any(|k| k.contains("androidmanifest.xml")));
    }

    #[test]
    fn zip_stored_vs_deflated_classified() {
        let r = parse_zip_report(ZIP_OUT);
        let logo = r.entries.iter().find(|e| e.path.ends_with("logo.png")).unwrap();
        assert_eq!(logo.deflated_pct, Some(1));
        assert!(!logo.stored);
        let license = r.entries.iter().find(|e| e.path.ends_with("logo.png.license")).unwrap();
        assert!(license.stored);
        assert_eq!(license.deflated_pct, None);
    }

    #[test]
    fn zip_handles_no_entries() {
        let s = summarize(&"zip x.zip".to_string(), "zip warning: nothing to do");
        assert!(s.text.contains("no") || s.text.contains("0"), "got: {}", s.text);
    }
}
