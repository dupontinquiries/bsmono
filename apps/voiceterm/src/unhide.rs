// src/unhide.rs
//
// "Show me what's hidden" — pre-parse pass that strips visibility-
// suppressing attributes from elements before the qml_gen pipeline
// sees the document. Reveals content that was hidden by:
//
//   - inline `style="display:none"` / `style="visibility:hidden"` /
//     `style="opacity:0"` / `style="height:0"`
//   - `hidden=""` boolean attribute (HTML5)
//   - `aria-hidden="true"` (intended for screen readers — but we
//     deliberately reveal these too because users have explicitly
//     opted into the "show hidden" feature)
//
// We also detect class names that conventionally hide content
// (`hidden`, `is-hidden`, `d-none`, `sr-only`, `visually-hidden`,
// `screen-reader-only`) and strip them from the class list.
//
// What this DOESN'T do:
//   - Run JavaScript to undo dynamic hides (we don't have a JS engine)
//   - Override CSS rules from <style> blocks or external stylesheets
//     (we'd need a CSS engine; defer)
//   - Touch <script> or <noscript> tags
//
// What this means in practice: pages that hide content via inline
// styles or attributes will have it revealed. Pages that hide content
// via stylesheet rules (the more common modern approach) won't —
// those need a stylesheet pass which is bigger work.
//
// User control: opt-in via WEB2QML_UNHIDE=1 env var (settings UI
// next turn). Default OFF because un-hiding can reveal cookie banners,
// modal overlays, draft content the site author never meant to surface.
//
// Bonus: emits a small report log so the user can see what was
// revealed ("revealed 14 hidden elements: 8 by style, 4 by class, 2 by aria").

/// Result of an unhide pass — counts by reason, plus the modified HTML.
pub struct UnhideResult {
    pub html: String,
    pub revealed_inline_style: u32,
    pub revealed_hidden_attr: u32,
    pub revealed_aria_hidden: u32,
    pub revealed_class: u32,
}

impl UnhideResult {
    pub fn total(&self) -> u32 {
        self.revealed_inline_style + self.revealed_hidden_attr
            + self.revealed_aria_hidden + self.revealed_class
    }
}

/// CSS class names commonly used to hide content. Stripping these from
/// a class list typically restores visibility (assuming the underlying
/// stylesheet is the standard meaning).
const HIDING_CLASSES: &[&str] = &[
    "hidden",
    "is-hidden",
    "d-none",          // bootstrap
    "u-hidden",        // some atomic css
    "visually-hidden", // accessibility-friendly hide
    "sr-only",         // screen-reader only
    "screen-reader-only",
    "screen-reader-text",
    "invisible",
    "hide",
    "vh",              // some atomic
    "tw-hidden",       // some tailwind preset
];

/// Run the unhide pass on the input HTML. Returns the modified HTML
/// plus a report. The HTML is re-serialized via plain string passes
/// rather than scraper's serializer (which we'd need html5ever for) —
/// this keeps the pass minimal-cost. See "string passes" below for
/// rationale.
pub fn unhide_html(input: &str) -> UnhideResult {
    // Approach: we operate on the raw HTML string with carefully-scoped
    // regex-like passes. Doing this on the parsed DOM and then re-
    // serializing would mean importing html5ever (heavy) or writing
    // our own serializer (error-prone for edge cases like CDATA, raw
    // text, comments). Direct string passes are surgical: we look for
    // attribute literals inside open tags and rewrite them.
    //
    // Each pass walks the input ONCE and produces a new string. Total
    // work is ~O(n) per pass with 4 passes, so still linear overall.

    let mut out = input.to_string();
    let mut report = UnhideResult {
        html: String::new(),
        revealed_inline_style: 0,
        revealed_hidden_attr: 0,
        revealed_aria_hidden: 0,
        revealed_class: 0,
    };

    // Pass 1: inline style="display:none" et al.
    // We rewrite the style attribute value, removing offending
    // declarations. Other declarations (color, font-size, etc.) preserved.
    out = pass_inline_style(&out, &mut report.revealed_inline_style);

    // Pass 2: hiding class names in class="..." — MUST run before the
    // boolean hidden-attr pass. v0.1.614: "<div class='card hidden'>"
    // has a `hidden` TOKEN with space boundaries that the boolean-attr
    // pass would otherwise mistake for a `hidden` attribute, consuming it
    // and miscounting it as revealed_hidden_attr instead of revealed_class
    // (and the class pass would then find nothing). Running class first
    // removes+counts the class token correctly; what's left for the
    // hidden-attr pass is only genuine attributes.
    out = pass_hiding_class(&out, &mut report.revealed_class);

    // Pass 3: hidden="" boolean attribute on any tag.
    out = pass_hidden_attr(&out, &mut report.revealed_hidden_attr);

    // Pass 4: aria-hidden="true"
    out = pass_aria_hidden(&out, &mut report.revealed_aria_hidden);

    report.html = out;
    report
}

/// Pass 1: rewrite inline style attributes to drop hiding declarations.
fn pass_inline_style(input: &str, count: &mut u32) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let bytes = input.as_bytes();
    while i < bytes.len() {
        // Look for ` style=` opening — the leading space prevents matching
        // arbitrary attribute names ending in "style" (e.g. "data-style").
        // Could be `style="..."` OR `style='...'` OR `style="..."` AT TAG
        // OPEN (ie. preceded by `<tag ` or another attribute).
        let remaining = &input[i..];
        if remaining.starts_with("style=\"") || remaining.starts_with("style='") {
            // Make sure we're inside a tag: scan backward from i looking
            // for `<` before any `>`. Cheap; bounded.
            if !inside_open_tag(input, i) {
                push_byte(&mut out, bytes[i]);
                i += 1;
                continue;
            }
            let quote = bytes[i + 6];  // " or '
            let val_start = i + 7;
            let val_end = match input[val_start..].find(quote as char) {
                Some(e) => val_start + e,
                None => {
                    push_byte(&mut out, bytes[i]);
                    i += 1;
                    continue;
                }
            };
            let style_value = &input[val_start..val_end];
            let (cleaned, removed) = clean_style_decls(style_value);
            *count += removed;
            // Write back: style="cleaned" (or omit attribute if empty)
            if cleaned.trim().is_empty() {
                // No declarations left — drop the attribute entirely.
                // We need to also strip the leading space if there is one.
                if !out.is_empty() && out.as_bytes().last() == Some(&b' ') {
                    out.pop();
                }
                i = val_end + 1;  // past closing quote
            } else {
                out.push_str("style=");
                out.push(quote as char);
                out.push_str(&cleaned);
                out.push(quote as char);
                i = val_end + 1;
            }
        } else {
            push_byte(&mut out, bytes[i]);
            i += 1;
        }
    }
    out
}

/// Strip hiding CSS declarations from a style attribute value.
/// Returns (cleaned_value, removed_count).
fn clean_style_decls(value: &str) -> (String, u32) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0u32;
    for decl in value.split(';') {
        let trimmed = decl.trim();
        if trimmed.is_empty() { continue; }
        if is_hiding_decl(trimmed) {
            removed += 1;
            continue;
        }
        kept.push(trimmed);
    }
    let joined = kept.join("; ");
    (joined, removed)
}

fn is_hiding_decl(decl: &str) -> bool {
    // Normalize: lowercase + strip whitespace around the colon.
    let lower = decl.to_ascii_lowercase();
    let (prop, val) = match lower.split_once(':') {
        Some((p, v)) => (p.trim(), v.trim()),
        None => return false,
    };
    match prop {
        "display"     => val == "none",
        "visibility"  => val == "hidden" || val == "collapse",
        "opacity"     => val == "0" || val == "0.0" || val == "0%",
        // Zero-dimension hides (common pattern with overflow:hidden)
        "height"      => val == "0" || val == "0px" || val == "0%",
        "max-height"  => val == "0" || val == "0px" || val == "0%",
        "width"       => val == "0" || val == "0px" || val == "0%",
        "max-width"   => val == "0" || val == "0px" || val == "0%",
        // Off-screen positioning patterns
        "position"    => false,  // need to check `left`/`top` separately; defer
        // text-indent: -9999px is a classic visually-hide
        "text-indent" => val.starts_with('-') && val.contains("9999"),
        _ => false,
    }
}

/// Pass 2: remove `hidden=""` / `hidden` boolean attribute.
fn pass_hidden_attr(input: &str, count: &mut u32) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let remaining = &input[i..];
        // v0.1.614: require an ATTRIBUTE BOUNDARY before `hidden` so we
        // don't match the `hidden` SUBSTRING inside `aria-hidden` (or any
        // `*-hidden`/`xhidden` attribute). Without this, scanning
        // `<div aria-hidden="true">` hit the inner `hidden="true"` and
        // stripped it, corrupting the tag to `<div aria->` AND stealing
        // the match from pass_aria_hidden. The char before `hidden` must
        // be a tag/attr separator (space, '<', '"', "'", '/'), not a
        // letter/digit/hyphen.
        let boundary_before = i == 0
            || matches!(bytes[i - 1], b' ' | b'<' | b'"' | b'\'' | b'/' | b'\t' | b'\n');
        // `hidden="..."` or `hidden='...'` or just `hidden` (boolean)
        if boundary_before && (remaining.starts_with("hidden=\"")
            || remaining.starts_with("hidden='")
            || (remaining.starts_with("hidden")
                && (remaining.len() == 6
                    || matches!(bytes.get(i + 6), Some(b' ' | b'>' | b'/')))))
        {
            if !inside_open_tag(input, i) {
                push_byte(&mut out, bytes[i]);
                i += 1;
                continue;
            }
            // Determine end of attribute.
            if remaining.starts_with("hidden=\"") || remaining.starts_with("hidden='") {
                let quote = bytes[i + 7];
                let val_start = i + 8;
                let val_end = match input[val_start..].find(quote as char) {
                    Some(e) => val_start + e,
                    None => {
                        push_byte(&mut out, bytes[i]);
                        i += 1;
                        continue;
                    }
                };
                // Strip the leading space if present.
                if !out.is_empty() && out.as_bytes().last() == Some(&b' ') {
                    out.pop();
                }
                *count += 1;
                i = val_end + 1;
            } else {
                // boolean form
                if !out.is_empty() && out.as_bytes().last() == Some(&b' ') {
                    out.pop();
                }
                *count += 1;
                i += 6;
            }
        } else {
            push_byte(&mut out, bytes[i]);
            i += 1;
        }
    }
    out
}

/// Pass 3: rewrite `aria-hidden="true"` → `aria-hidden="false"`.
/// We don't drop the attribute, just flip it — this keeps the
/// document's a11y semantic structure intact while making the
/// content visible.
fn pass_aria_hidden(input: &str, count: &mut u32) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let remaining = &input[i..];
        if remaining.starts_with("aria-hidden=\"true\"")
            || remaining.starts_with("aria-hidden='true'")
        {
            if !inside_open_tag(input, i) {
                push_byte(&mut out, bytes[i]);
                i += 1;
                continue;
            }
            let quote_char = bytes[i + 12];  // " or '
            out.push_str("aria-hidden=");
            out.push(quote_char as char);
            out.push_str("false");
            out.push(quote_char as char);
            i += "aria-hidden=\"true\"".len();
            *count += 1;
        } else {
            push_byte(&mut out, bytes[i]);
            i += 1;
        }
    }
    out
}

/// Pass 4: strip hiding class names from class="..." values.
fn pass_hiding_class(input: &str, count: &mut u32) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let remaining = &input[i..];
        if remaining.starts_with("class=\"") || remaining.starts_with("class='") {
            if !inside_open_tag(input, i) {
                push_byte(&mut out, bytes[i]);
                i += 1;
                continue;
            }
            let quote = bytes[i + 6];
            let val_start = i + 7;
            let val_end = match input[val_start..].find(quote as char) {
                Some(e) => val_start + e,
                None => {
                    push_byte(&mut out, bytes[i]);
                    i += 1;
                    continue;
                }
            };
            let class_value = &input[val_start..val_end];
            let (cleaned, removed) = clean_class_list(class_value);
            *count += removed;
            if cleaned.is_empty() {
                if !out.is_empty() && out.as_bytes().last() == Some(&b' ') {
                    out.pop();
                }
                i = val_end + 1;
            } else {
                out.push_str("class=");
                out.push(quote as char);
                out.push_str(&cleaned);
                out.push(quote as char);
                i = val_end + 1;
            }
        } else {
            push_byte(&mut out, bytes[i]);
            i += 1;
        }
    }
    out
}

/// Strip hiding class names from a space-separated class list.
fn clean_class_list(value: &str) -> (String, u32) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0u32;
    for cls in value.split_whitespace() {
        if HIDING_CLASSES.iter().any(|h| h.eq_ignore_ascii_case(cls)) {
            removed += 1;
            continue;
        }
        kept.push(cls);
    }
    (kept.join(" "), removed)
}

/// Helper: scan backward from position to determine if we're inside an
/// open tag (between `<tagname` and `>`). Returns false if we hit a `>`
/// before a `<`. Bounded scan distance — most tags fit in <200 chars.
fn inside_open_tag(haystack: &str, pos: usize) -> bool {
    let bytes = haystack.as_bytes();
    let limit = pos.saturating_sub(2048);  // don't scan forever
    let mut i = pos;
    while i > limit {
        i -= 1;
        match bytes[i] {
            b'>' => return false,
            b'<' => return true,
            _ => continue,
        }
    }
    false
}

fn push_byte(out: &mut String, b: u8) {
    // ASCII-fast path; non-ASCII bytes are part of UTF-8 multibytes.
    // We treat the buffer as UTF-8 and trust the input's structure.
    // Since we only branch on ASCII patterns, multibyte sequences
    // pass through intact.
    if b < 0x80 {
        out.push(b as char);
    } else {
        // Need to push the byte verbatim. We're working in bytes but
        // String requires valid UTF-8 boundaries. Use a Vec<u8> and
        // convert at end? For now, push as-is via unsafe — input is
        // valid UTF-8 (came from a String) and we only INSERT ASCII
        // changes, so the result remains valid UTF-8.
        unsafe {
            out.as_mut_vec().push(b);
        }
    }
}

// ─── Empty-page heuristic ──────────────────────────────────────────
//
// Detects pages that are MOSTLY hidden content (e.g. a 200-byte body
// gated by a 5KB JS bundle that wasn't run). Useful to surface a
// "this page needs JS — open externally?" hint when our rendered
// output would be near-empty.
//
// Returns true when:
//   - body has < 500 chars of visible text after our render pass
//   - AND the document had > 5KB of script content
//   - AND there's at least one `<noscript>` or "JavaScript required" string

pub fn looks_js_gated(html: &str, rendered_text_len: usize) -> bool {
    if rendered_text_len > 500 { return false; }
    let lower = html.to_lowercase();
    let script_size: usize = lower.matches("<script").count() * 100;  // rough proxy
    let total_lower_len = lower.len();
    let estimated_script_bytes = if lower.contains("</script>") {
        // sum lengths between <script> and </script>
        let mut total = 0usize;
        let mut search_from = 0;
        while let Some(start) = lower[search_from..].find("<script") {
            let abs_start = search_from + start;
            if let Some(end_rel) = lower[abs_start..].find("</script>") {
                total += end_rel;
                search_from = abs_start + end_rel + 9;
            } else {
                break;
            }
        }
        total
    } else {
        script_size
    };
    if estimated_script_bytes < 5000 { return false; }
    // Soft confirmation
    let has_indicator = lower.contains("<noscript")
        || lower.contains("javascript is required")
        || lower.contains("javascript required")
        || lower.contains("enable javascript")
        || lower.contains("anubis")
        || lower.contains("cf-browser-verification");
    has_indicator || (total_lower_len > 3000 && rendered_text_len < 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unhide_inline_display_none() {
        let input = r#"<div style="display:none;color:red">hi</div>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_inline_style, 1);
        assert!(!r.html.contains("display:none"));
        assert!(r.html.contains("color"));
    }

    #[test]
    fn unhide_visibility_hidden() {
        let input = r#"<span style="visibility:hidden">secret</span>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_inline_style, 1);
    }

    #[test]
    fn unhide_aria_hidden() {
        let input = r#"<div aria-hidden="true">x</div>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_aria_hidden, 1);
        assert!(r.html.contains("aria-hidden=\"false\""));
    }

    #[test]
    fn unhide_hidden_attr() {
        let input = r#"<p hidden="">secret</p>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_hidden_attr, 1);
        assert!(!r.html.contains("hidden="));
    }

    #[test]
    fn unhide_hidden_class() {
        let input = r#"<div class="card hidden text-bold">x</div>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_class, 1);
        assert!(r.html.contains("card"));
        assert!(r.html.contains("text-bold"));
        assert!(!r.html.contains(" hidden "));
    }

    #[test]
    fn aria_hidden_not_corrupted_by_attr_pass() {
        // v0.1.614 regression: the boolean hidden-attr pass used to match
        // the `hidden` SUBSTRING inside `aria-hidden`, corrupting the tag
        // to "<div aria->" and stealing the match from the aria pass.
        let r = unhide_html(r#"<div aria-hidden="true">x</div>"#);
        assert_eq!(r.revealed_aria_hidden, 1);
        assert_eq!(r.revealed_hidden_attr, 0, "must NOT count aria-hidden as a hidden attr");
        assert!(r.html.contains("aria-hidden=\"false\""));
        assert!(!r.html.contains("aria->"), "tag must not be corrupted");
    }

    #[test]
    fn hidden_class_token_not_miscounted_as_attr() {
        // v0.1.614 regression: a `hidden` CLASS token (space-bounded
        // inside class="...") used to be eaten by the boolean hidden-attr
        // pass and counted as revealed_hidden_attr instead of
        // revealed_class. Pass order (class before attr) fixes it.
        let r = unhide_html(r#"<div class="card hidden">x</div>"#);
        assert_eq!(r.revealed_class, 1, "hidden class must count as a class reveal");
        assert_eq!(r.revealed_hidden_attr, 0, "not as a hidden attribute");
    }

    #[test]
    fn genuine_hidden_attr_still_works_alongside_class() {
        // a REAL hidden attribute next to a class must still be caught.
        let r = unhide_html(r#"<div class="card" hidden>x</div>"#);
        assert_eq!(r.revealed_hidden_attr, 1, "real boolean hidden attr still revealed");
    }

    #[test]
    fn preserves_other_styles() {
        let input = r#"<div style="display:none; color:red; font-size:14px">x</div>"#;
        let r = unhide_html(input);
        assert_eq!(r.revealed_inline_style, 1);
        assert!(r.html.contains("color"));
        assert!(r.html.contains("font-size"));
    }

    #[test]
    fn ignores_attribute_in_text_content() {
        // Not inside an open tag — should pass through untouched.
        let input = "Read about <b>style</b> tags. The text 'style=\"display:none\"' should not match.";
        let r = unhide_html(input);
        assert_eq!(r.total(), 0);
    }

    #[test]
    fn js_gated_anubis_match() {
        let html = r#"<html><head><script>"#.to_string()
            + &"x".repeat(6000)
            + r#"</script><body>Anubis loading…</body></html>"#;
        assert!(looks_js_gated(&html, 50));
    }

    #[test]
    fn js_gated_normal_page_no_match() {
        let html = "<html><body>".to_string()
            + &"normal article content. ".repeat(100)
            + "</body></html>";
        assert!(!looks_js_gated(&html, 2400));
    }
}
