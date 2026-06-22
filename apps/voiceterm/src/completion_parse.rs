//! Parser for raw bytes captured during a tab-completion query.
//!
//! `PtyTerminal::query_completion` returns the bytes the shell emitted in
//! response to TAB. Different shells produce different formats:
//!
//! ## zsh (default config)
//!
//! Single match: shell types the completion inline. We see e.g.
//! `or_chooser.cpp\x1b[?2004l\r\r\n...`  — no list, just the rest of
//! the filename. Our `partial` was already on the line, so the captured
//! bytes are JUST the suffix.
//!
//! Multi-match: shell prints a list below the prompt and redraws the
//! prompt. Output looks like (with ANSI sequences stripped):
//!
//! ```text
//! <CR><LF>
//! colorchooser.cpp  color_chooser.cpp  colors.txt
//! <prompt>hx col
//! ```
//!
//! With menu-select enabled (`zstyle ':completion:*' menu select`), the
//! list is interactive and includes terminal positioning escapes — we
//! ignore those by stripping ANSI before splitting.
//!
//! ## bash
//!
//! Multi-match: similar to zsh, list below prompt then redraw.
//! Single match: same as zsh — completion fills in inline.
//!
//! ## Parsing strategy
//!
//! 1. Strip ALL ANSI escape sequences (CSI, OSC, etc).
//! 2. Strip the prompt redraw at the end (whatever follows the last
//!    newline that isn't a continuation of completion list lines).
//! 3. Split remaining content on whitespace into candidate tokens.
//! 4. Filter: each token must look like something a user could have
//!    typed at the prompt (no whitespace inside, length 1-200, no
//!    weird control bytes).
//! 5. Dedupe.
//!
//! The output is a Vec<String> of completion candidates. The caller
//! handles ranking against the voice text.

/// Strip ANSI/VT100 control sequences from bytes, returning UTF-8 string.
/// We strip:
///   - CSI sequences: ESC [ ... letter
///   - OSC sequences: ESC ] ... BEL or ESC ] ... ESC \
///   - Single-byte ESC [letter] (e.g. ESC = M for index)
///   - Bare control bytes 0x00-0x08, 0x0b-0x1f except CR/LF/TAB which we keep
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out_bytes = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // ESC sequences
        if b == 0x1b && i + 1 < bytes.len() {
            let next = bytes[i+1];
            match next {
                b'[' => {
                    // CSI: ESC [ <params> <final>. Final byte is in range 0x40..0x7e.
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() { i += 1; }  // consume the final byte
                    continue;
                }
                b']' => {
                    // OSC: ESC ] <params> BEL  OR  ESC ] <params> ESC \
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 { i += 1; break; }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i+1] == b'\\' {
                            i += 2; break;
                        }
                        i += 1;
                    }
                    continue;
                }
                _ => {
                    // Two-byte ESC sequences (ESC = M, ESC = D, etc.)
                    i += 2;
                    continue;
                }
            }
        }
        // Keep CR, LF, TAB. Drop other low control bytes.
        if b == b'\r' || b == b'\n' || b == b'\t' {
            out_bytes.push(b);
            i += 1;
            continue;
        }
        if b < 0x20 || b == 0x7f {
            i += 1;
            continue;
        }
        // Push the byte directly so multi-byte UTF-8 sequences pass through intact.
        out_bytes.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out_bytes).into_owned()
}

/// Parse captured completion bytes into a candidate list.
///
/// `partial` is what we typed — we use it to skip the prompt-redraw line
/// (which contains the prompt + the partial we sent).
pub fn parse(captured: &[u8], partial: &str) -> Vec<String> {
    let cleaned = strip_ansi(captured);
    if cleaned.is_empty() { return Vec::new(); }

    let mut candidates: Vec<String> = Vec::new();
    let partial_lc = partial.to_lowercase();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in cleaned.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let line_lc = line.to_lowercase();
        // Skip the prompt-redraw line. It contains our partial preceded
        // by prompt characters. Heuristic: if the line contains our
        // partial as the trailing substring (after a prompt char), skip.
        if !partial.is_empty() && line_lc.ends_with(&partial_lc) {
            continue;
        }
        // Otherwise this is a completion-list line. Split on whitespace.
        for tok in line.split_whitespace() {
            // Reject obvious non-completions.
            if tok.is_empty() || tok.len() > 200 { continue; }
            // Strip trailing markers zsh appends to indicate file type:
            //   /  for directory
            //   *  for executable
            //   @  for symlink
            //   =  for socket
            //   |  for FIFO
            // We keep the / on directories because that's useful info,
            // but strip the others.
            let cleaned_tok = tok.trim_end_matches(|c: char|
                c == '*' || c == '@' || c == '=' || c == '|');
            if cleaned_tok.is_empty() { continue; }
            // Reject tokens that are just punctuation
            if !cleaned_tok.chars().any(|c| c.is_ascii_alphanumeric()) { continue; }
            let key = cleaned_tok.to_lowercase();
            if seen.insert(key) {
                candidates.push(cleaned_tok.to_string());
            }
        }
        // Hard cap on candidates returned. zsh can list hundreds of
        // matches; the UI doesn't need them all and the resolver gets
        // slower with more entries to score.
        if candidates.len() >= 50 { break; }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi() {
        let input = b"\x1b[31mred text\x1b[0m";
        assert_eq!(strip_ansi(input), "red text");
    }

    #[test]
    fn strips_osc() {
        let input = b"\x1b]0;title\x07text";
        assert_eq!(strip_ansi(input), "text");
    }

    #[test]
    fn parses_zsh_multi_match() {
        // Simulated zsh multi-match output (simplified)
        let captured = b"\r\ncolorchooser.cpp  color_chooser.cpp  colors.txt\r\n\x1b[H\x1b[2K~/proj $ hx col";
        let candidates = parse(captured, "hx col");
        assert!(candidates.iter().any(|c| c == "colorchooser.cpp"),
            "got: {:?}", candidates);
        assert!(candidates.iter().any(|c| c == "color_chooser.cpp"),
            "got: {:?}", candidates);
    }

    #[test]
    fn skips_prompt_line() {
        let captured = b"\r\nfile1.txt  file2.txt\r\n$ open fil";
        let candidates = parse(captured, "open fil");
        assert!(!candidates.iter().any(|c| c == "fil"),
            "should not include the partial itself, got: {:?}", candidates);
    }
}
