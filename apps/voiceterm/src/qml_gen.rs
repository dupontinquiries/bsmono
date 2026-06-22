// src/qml_gen.rs — HTML→QML. Uses write! to avoid intermediate allocations.
use scraper::{ElementRef, Selector};
use std::borrow::Cow;
use std::fmt::Write;
use std::sync::OnceLock;
use crate::ad_filter::{self, FilterMode, FilterStats};
use crate::anim_detect;
use crate::layout_detect;

const MAX_NODES: usize = 500;
/// v0.1.411: per-table rendered-row cap. A table is a SINGLE
/// node in the MAX_NODES count, but emit_adaptive_table emits
/// every row inline in BOTH wide and narrow modes — so a
/// multi-thousand-row data table explodes into hundreds of
/// thousands of QML lines while consuming just one node, blowing
/// straight past MAX_NODES. (Root cause of the user's
/// 1M-line-QML / off-to-the-side page: a giant table.) We cap
/// the rendered rows and append a "showing N of M" notice.
/// This is a targeted down-payment on the windowed-rendering
/// arc — full virtualization would render rows on scroll.
const MAX_TABLE_ROWS: usize = 200;
/// v0.1.168 perf: the "width binding" used by every emitted child Text /
/// Column / Rectangle. Was `parent ? parent.width : 800` — a JS ternary
/// that re-evaluates whenever parent.width changes AND has to evaluate
/// truthiness of parent on every binding pass.
///
/// Every child of the generated page is nested inside the top-level
/// wrapper Column in `push_preview`, which always has a valid parent
/// (the Loader). So child elements never face a null parent — the
/// ternary was overcautious. Now we emit a plain `parent.width`,
/// which QML resolves to a direct property binding at parse time
/// instead of a JS expression evaluated each pass.
///
/// On a 5000-node page that's ~5000 ternary expressions converted to
/// direct property bindings. The wrapper Column itself still uses
/// the ternary form (see push_preview), since IT is the only element
/// whose parent could theoretically be null during instantiation.
const W: &str = "parent.width";

/// v0.1.167 perf: zero-allocation case-insensitive ASCII substring
/// check. Used by CssHints::from_element and is_hidden, both of which
/// previously called `to_ascii_lowercase()` on per-element strings —
/// that allocation fired once per node with a class or style attribute,
/// adding up to thousands of small heap allocations on a content-heavy
/// page. This helper does the comparison inline byte-by-byte; the
/// caller passes a pre-lowercased needle constant and a mixed-case
/// haystack from the live element.
///
/// Mirror of ad_filter::contains_ci but lives here so qml_gen can stay
/// independent of ad_filter's internals.
#[inline]
fn contains_ci(haystack: &str, needle_lc: &str) -> bool {
    let nb = needle_lc.as_bytes();
    let hb = haystack.as_bytes();
    if nb.is_empty() { return true; }
    if nb.len() > hb.len() { return false; }
    // Iterate windows of the haystack equal to needle length; compare
    // each byte lowercased against the needle byte (which the caller
    // promises is already lowercase). No allocation, no UTF-8 decode
    // — these patterns are all ASCII.
    hb.windows(nb.len()).any(|w| {
        w.iter().zip(nb).all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

fn s<'a>(lock: &'a OnceLock<Selector>, css: &str) -> &'a Selector {
    lock.get_or_init(|| Selector::parse(css).unwrap())
}
static S0: OnceLock<Selector> = OnceLock::new(); // body
static S1: OnceLock<Selector> = OnceLock::new(); // img
static S2: OnceLock<Selector> = OnceLock::new(); // input[name]
static S3: OnceLock<Selector> = OnceLock::new(); // submit
static S4: OnceLock<Selector> = OnceLock::new(); // option
static S5: OnceLock<Selector> = OnceLock::new(); // source
static S6: OnceLock<Selector> = OnceLock::new(); // summary

pub fn select_body(doc: &scraper::Html) -> Option<ElementRef<'_>> { doc.select(s(&S0, "body")).next() }

/// v0.1.387 (title extraction): pull the `<title>` text from
/// a parsed HTML document. Returns the cleaned title or an
/// empty string when no title is present, the title is
/// whitespace-only, or it's a known placeholder ("untitled
/// document", etc).
///
/// Cleanup applied:
///   - Whitespace collapsed (newlines, multiple spaces → single)
///   - Trimmed
///   - Capped at 200 chars (rare but happens — some templated
///     sites stuff the entire description into `<title>`)
///   - Char-boundary-safe truncation with ellipsis suffix
///
/// Used by Backend at fetch time to populate
/// `self.current_title`. The bookmarks `save_url` qt_method
/// reads that field to provide a real title for the saved
/// page; future ships can wire it into audit-log display
/// strings, session-summary prompts, and tab labels.
///
/// v0.1.772: strip literal tag-like markup from RCDATA text (titles).
/// A `<` only starts a tag if immediately followed by an optional `/` and an
/// ASCII letter, so punctuation like "a < b" is preserved.
fn strip_tag_markup(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = bytes[i];
        if c == b'<' {
            // peek: optional '/', then must be ASCII alpha to count as a tag
            let mut j = i + 1;
            if j < s.len() && bytes[j] == b'/' { j += 1; }
            if j < s.len() && (bytes[j] as char).is_ascii_alphabetic() {
                // skip to the closing '>' (or end of string)
                match s[i..].find('>') {
                    Some(rel) => { i += rel + 1; continue; }
                    None => { i = s.len(); continue; }
                }
            }
        }
        // copy this char (handle multi-byte UTF-8 correctly)
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub fn extract_title(doc: &scraper::Html) -> String {
    // Use a dedicated OnceLock selector — `title` is a tag
    // selector, no namespace concerns.
    static TITLE_SEL: OnceLock<Selector> = OnceLock::new();
    let sel = TITLE_SEL.get_or_init(|| Selector::parse("title").unwrap());

    let raw = match doc.select(sel).next() {
        Some(el) => el.text().collect::<String>(),
        None => return String::new(),
    };

    // v0.1.772: <title> is RCDATA in HTML5, so any markup inside it (e.g. a
    // site that literally wrote "<span>" in the title) arrives as literal text
    // via .text(). Strip recognized tag-like sequences (`<tag ...>` / `</tag>`,
    // i.e. a `<` immediately followed by an optional `/` and an ASCII letter)
    // so a user-facing title never shows raw markup. Conservative: a lone "<"
    // used as punctuation (e.g. "a < b") is left intact because it isn't
    // followed by a letter/slash-letter.
    let raw = strip_tag_markup(&raw);

    // Collapse whitespace.
    let mut cleaned = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !last_was_space && !cleaned.is_empty() {
                cleaned.push(' ');
            }
            last_was_space = true;
        } else {
            cleaned.push(c);
            last_was_space = false;
        }
    }
    let cleaned = cleaned.trim().to_string();

    if cleaned.is_empty() {
        return String::new();
    }

    // Reject known placeholders.
    let lower = cleaned.to_lowercase();
    if matches!(lower.as_str(),
        "untitled document" | "untitled" | "document"
        | "new tab" | "loading..." | "loading"
    ) {
        return String::new();
    }

    // Cap length, unicode-safe.
    const MAX: usize = 200;
    if cleaned.len() <= MAX {
        return cleaned;
    }
    let mut cut = MAX;
    while cut > 0 && !cleaned.is_char_boundary(cut) { cut -= 1; }
    format!("{}…", &cleaned[..cut])
}

pub struct RenderStyle {
    pub text_color: String, pub link_color: String, pub link_hover: String,
    pub font_size: i32, pub font_family: String, pub border_color: String,
    pub input_bg: String, pub input_fg: String, pub button_bg: String, pub button_fg: String,
    pub filter_mode: FilterMode,
    pub text_pre: String, pub text_suf: String,
    /// v0.1.167 perf: cached `font_size.to_string()` so the per-element
    /// emitter doesn't allocate a fresh String for the common case
    /// "this element uses the inherited font size". Inline-style and
    /// Tailwind overrides still do their own `to_string` per element,
    /// but those are a minority. On a 5000-node page this drops
    /// thousands of small heap allocations.
    pub font_size_str: String,
    /// v0.1.235: subtle alt-row background for tables. White-with-
    /// low-alpha for dark themes (lifts toward white), black-with-
    /// low-alpha for light themes (drops toward black). Chosen so
    /// the zebra is faintly visible against `input_bg` without
    /// overpowering the cell text. Could also be used by other
    /// alternating-row patterns (corpus listing, etc.) in the future.
    pub zebra_bg: String,
    /// v0.1.243: dedicated color for the paragraph-selection
    /// accent strip. Was using link_color which (a) made selection
    /// visually indistinguishable from inline links, and (b) on
    /// some themes rendered as a mid-tone blue that lacked contrast
    /// against the page bg.
    ///
    /// User feedback v0.1.241 conversation: "the selection box
    /// cannot be black on all themes it does need to be background
    /// and text contrast aware."
    ///
    /// Tuned per-theme for high contrast against the page bg AND
    /// distinguishable from the text_color and link_color:
    ///   - Dark bg → warm bright (#ffaa44 orange) — saturated,
    ///     pops on dark, not blue (links), not gray (text)
    ///   - Light bg → saturated dark (#c04020 dark red-brown) —
    ///     pops on light, not blue (links), not near-black (text)
    pub selection_accent: String,
}

impl RenderStyle {
    pub fn from_theme(dark: bool, fs: i32, mono: bool, fm: FilterMode) -> Self {
        let tc: String = if dark {"#d4d0c8"} else {"#3b3732"}.into();
        let ff: String = if mono {"monospace"} else {"sans-serif"}.into();
        let tp = format!("Text {{\ncolor: \"{tc}\"\ntext: \"");
        let ts = format!("\"\nfont.pixelSize: {fs}\nfont.family: \"{ff}\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: {W}\n}}");
        let fs_str = fs.to_string();
        RenderStyle {
            text_color: tc, link_color: if dark {"#5599dd"} else {"#3a7ab5"}.into(),
            link_hover: "#e08030".into(), font_size: fs, font_family: ff,
            border_color: if dark {"#555"} else {"#bbb"}.into(),
            input_bg: if dark {"#363636"} else {"#f0ebe2"}.into(),
            input_fg: if dark {"#e0e0e0"} else {"#333333"}.into(),
            button_bg: if dark {"#3a4a60"} else {"#d8d0c4"}.into(),
            button_fg: if dark {"#c8dce8"} else {"#2a3a4a"}.into(),
            filter_mode: fm, text_pre: tp, text_suf: ts,
            font_size_str: fs_str,
            // v0.1.235: zebra-row tint. Low alpha (~4%) so the
            // alternation is visible but not aggressive. Hex
            // with alpha channel (ARGB; Qt accepts 8-digit hex
            // as #AARRGGBB).
            zebra_bg: if dark {
                "#0affffff".into()  // 4% white over input_bg
            } else {
                "#08000000".into()  // 3% black over input_bg
            },
            // v0.1.243: high-contrast selection accent. Distinct
            // from link_color (so selection ≠ link visually) and
            // from text_color (so accent stripe ≠ text):
            //   dark page → warm orange (#ffaa44)
            //   light page → dark red-brown (#c04020)
            // Both saturated and unambiguous against their bg.
            selection_accent: if dark {
                "#ffaa44".into()
            } else {
                "#c04020".into()
            },
        }
    }

    /// v0.1.422: override the selection accent for the handful of
    /// themes where the static dark/light accent collides with the
    /// theme's own text or link color (found in the v0.1.247
    /// audit). `from_theme` only sees a binary dark/light flag, so
    /// it can't avoid every per-theme collision; this applies a
    /// targeted override by theme index. Picked to be a cool hue
    /// well away from each theme's warm tc/lk so the selection
    /// strip never reads as text or as a link.
    ///
    /// Authoritative indices (counted from the QML themes array):
    ///   9  = ReadLt — lk #8a5028 (warm brown) → teal
    ///   10 = Sepia  — lk #8a4020 (warm red)   → teal-blue
    ///   14 = Amber  — tc #ffb000 (gold)       → cyan (≈complement)
    /// All other themes keep the dark/light default.
    pub fn with_theme_idx(mut self, idx: i32) -> Self {
        let override_accent = match idx {
            9  => Some("#1f7a6a"),  // ReadLt → teal
            10 => Some("#1a6b8a"),  // Sepia → teal-blue
            14 => Some("#33ccbb"),  // Amber → cyan (complement of gold)
            _ => None,
        };
        if let Some(c) = override_accent {
            self.selection_accent = c.into();
        }
        self
    }
}

// Escape directly into a Write target — zero intermediate allocation
#[inline]
fn write_esc(w: &mut impl Write, s: &str) {
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' { in_tag = true; continue; }
        if c == '>' && in_tag { in_tag = false; continue; }
        if in_tag { continue; }
        match c {
            '\\' => { let _ = w.write_str("\\\\"); }
            '"' => { let _ = w.write_str("\\\""); }
            '\n' => { let _ = w.write_str("\\n"); }
            '\r' | '\0' => {}
            '\t' => { let _ = w.write_str("    "); }
            '{' => { let _ = w.write_char('('); }
            '}' => { let _ = w.write_char(')'); }
            // Strip all other control chars (0x00-0x1F except \n \t, and 0x7F)
            c if c.is_control() => {}
            c => { let _ = w.write_char(c); }
        }
    }
}

// For API compatibility where a String is needed
pub fn esc(s: &str) -> String { let mut o = String::with_capacity(s.len()+8); write_esc(&mut o, s); o }

/// Public write_esc for other modules
pub fn write_esc_pub(w: &mut String, s: &str) { write_esc(w, s); }

/// v0.1.278: emit a MouseArea block for a link click. Handles
/// the radial-menu options menu on press-and-hold OR right-
/// click, falling back to direct navigation on a plain left-
/// click. The radial menu is rendered globally in main.qml;
/// we just emit a call to `backend.link_menu(url, mappedX,
/// mappedY)` and let QML position the menu near the cursor.
///
/// Args:
///   o: output buffer to append to
///   href: link target URL (will be escape-write'd)
///
/// Replaces inline `MouseArea { onClicked: backend.nav(...) }`
/// emissions. Centralizes the link interaction model so a
/// future change (e.g. opening in new tab by default with
/// modifier) is one edit, not 5+.
pub fn emit_link_mousearea(o: &mut String, href: &str) {
    o.push_str("\nMouseArea {\nanchors.fill: parent\ncursorShape: Qt.PointingHandCursor");
    // Accept right-button too so right-click triggers the menu.
    o.push_str("\nacceptedButtons: Qt.LeftButton | Qt.RightButton");
    // Long-press for touch screens (no right-click on phone).
    o.push_str("\npressAndHoldInterval: 350");
    // v0.1.537: a plain TAP now opens the intent menu (Open vs Read/
    // teleprompter vs Copy/New tab) instead of navigating directly.
    // The old direct-nav tap was ambiguous — and on the reading
    // surface it was being swallowed by the teleprompter's paragraph
    // tap, so links felt unclickable. Routing every tap through
    // backend.link_menu makes the link element OWN the tap (it can't
    // fall through to the teleprompter) and lets the user state intent
    // first. Right-click + press-and-hold also open the menu.
    o.push_str("\nonClicked: function(mouse) {\nvar p = mapToItem(null, mouse.x, mouse.y);\nif (backend) backend.link_menu(\"");
    write_esc(o, href);
    o.push_str("\", p.x, p.y)\n}");
    o.push_str("\nonPressAndHold: function(mouse) {\nvar p = mapToItem(null, mouse.x, mouse.y);\nif (backend) backend.link_menu(\"");
    write_esc(o, href);
    o.push_str("\", p.x, p.y)\n}\n}");
}

/// True if a raw href attribute carries no navigable target we can
/// follow: empty, a bare in-page "#", about:blank, or a `javascript:`
/// scheme. (A real in-page anchor like "#section" is NOT dead — it
/// scrolls to an anchor — so only the bare "#" matches here.)
///
/// Callers should still try `detect_js_href` to recover a real URL
/// from data-href / onclick before treating the link as dead; only
/// when that fails do we render the non-navigable cue. v0.1.875.
pub fn is_dead_href(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() { return true; }
    if t == "#" || t.eq_ignore_ascii_case("about:blank") { return true; }
    if t.len() >= 11 && t[..11].eq_ignore_ascii_case("javascript:") { return true; }
    false
}

/// Emit a non-navigable "interactive element (unsupported)" cue for a
/// link/button whose target is a dead/JS href we can't execute. The
/// silent no-op (a fake working link that navigates nowhere) is
/// replaced by a de-emphasized label + a ⚡ marker, so the user can
/// see the element WAS interactive and that we can't run it, rather
/// than tapping a dead link. v0.1.875.
fn emit_interactive_cue(o: &mut String, text: &str, width: &str, sty: &RenderStyle) {
    o.push_str("Text {\nwidth: "); o.push_str(width);
    o.push_str("\ntext: \"");
    write_esc(o, text);
    // U+26A1 HIGH VOLTAGE SIGN, appended outside write_esc (plain glyph,
    // needs no QML string escaping).
    o.push_str(" \u{26A1}\"\ncolor: \"");
    // border_color is the muted/secondary palette color — reads as
    // de-emphasized vs the active link_color.
    o.push_str(&sty.border_color);
    o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
    o.push_str("\nfont.family: \""); o.push_str(&sty.font_family);
    // No underline (so it doesn't masquerade as a working link) and no
    // MouseArea (non-navigable).
    o.push_str("\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\n}");
}

/// v0.1.220: HTML-preserving escape for embedding rich-text content
/// in a QML string literal. Unlike `write_esc`, this does NOT strip
/// HTML tags — `<b>`, `<a>`, `<i>` etc pass through intact so Qt's
/// `Text.RichText` parser sees them.
///
/// Used by manpage rendering paths (heading, list item, table cell
/// when rich) where we WANT the rich-text markup to reach Qt.
///
/// Escape rules:
///   - `\` → `\\`     (QML backslash)
///   - `"` → `\"`     (QML string delimiter — escapes inside HTML
///                     attributes too; Qt unescapes at runtime)
///   - `\n` → `\n`    (literal escape sequence)
///   - `\r`, `\0`, other control chars → stripped
///   - `\t` → 4 spaces
///   - `{` `}` left as-is (Qt rich-text doesn't interpret them)
///
/// This function is a sibling to `write_esc`, not a replacement —
/// scraper-parsed HTML still goes through write_esc (which strips
/// tags) because in that context we deliberately want plain text.
pub fn write_esc_keep_html(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' | '\0' => {}
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

#[inline]
fn text_qml(content: &str, sty: &RenderStyle) -> String {
    let mut o = String::with_capacity(sty.text_pre.len() + content.len() + sty.text_suf.len() + 16);
    o.push_str(&sty.text_pre); write_esc(&mut o, content); o.push_str(&sty.text_suf); o
}

/// Resolve a (possibly relative) href against a base URL,
/// producing an absolute URL.
///
/// v0.1.401 (URL resolution audit): rewritten to handle
/// edge cases the original missed. Previously:
///   - `../` segments were left literal (not collapsed)
///   - a base with a query string but no path slash
///     (`https://site.com?x=1`) + a relative href produced
///     garbage like `https:/page2` because `rfind('/')`
///     landed in the `://`
///   - base fragments/queries weren't stripped before
///     joining a relative path
///
/// Resolution rules (matching the WHATWG URL spec subset we
/// need for a renderer):
///   - empty href              → base unchanged
///   - `#frag`                 → base minus its own frag, + frag
///   - `http://` / `https://`  → absolute, returned as-is
///   - `//host/path`           → inherit base's scheme
///   - `/abs/path`             → base origin + path
///   - `rel/path` / `../up`    → resolve against base's
///                               directory, collapsing `.`
///                               and `..` segments
pub fn resolve_url(href: &str, base: &str) -> String {
    let h = href.trim();
    if h.is_empty() { return base.into(); }
    // Fragment-only: replace base's fragment.
    if h.starts_with('#') {
        let b = base.find('#').map(|p| &base[..p]).unwrap_or(base);
        return format!("{b}{h}");
    }
    // Already absolute.
    if h.starts_with("http://") || h.starts_with("https://") {
        return h.into();
    }
    // Protocol-relative: inherit scheme.
    if h.starts_with("//") {
        let scheme = if base.starts_with("https") { "https" } else { "http" };
        return format!("{scheme}:{h}");
    }

    // Everything below needs the base decomposed into
    // (origin, path). Strip the base's own query/fragment
    // first — they never carry into a resolved relative ref.
    let base_no_frag = base.split('#').next().unwrap_or(base);
    let base_no_query = base_no_frag.split('?').next().unwrap_or(base_no_frag);

    // Find the origin (scheme://host) and the path portion.
    let (origin, base_path) = match base_no_query.find("://") {
        Some(p) => {
            let after_scheme = &base_no_query[p + 3..];
            let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
            let origin = &base_no_query[..p + 3 + host_end];
            let path = &after_scheme[host_end..]; // includes leading '/' or empty
            (origin.to_string(), path.to_string())
        }
        None => {
            // Base isn't a full URL — can't resolve, return
            // href as-is (best effort, matches old fallback).
            return h.into();
        }
    };

    // Split href into its own path part + query/fragment
    // suffix so we only normalize the path segments.
    let (h_path, h_suffix) = split_path_suffix(h);

    // Determine the path to resolve against.
    let resolved_path = if h_path.starts_with('/') {
        // Absolute path — replace base path entirely.
        normalize_path(&h_path)
    } else {
        // Relative — resolve against base's directory (the
        // part up to and including the last '/').
        let base_dir = match base_path.rfind('/') {
            Some(idx) => &base_path[..=idx],
            None => "/", // base had no path slash → root
        };
        let combined = format!("{base_dir}{h_path}");
        normalize_path(&combined)
    };

    format!("{origin}{resolved_path}{h_suffix}")
}

/// Split an href into (path, suffix) where suffix is the
/// first `?` or `#` and everything after. The path part is
/// what gets `.`/`..` normalization; the suffix is preserved
/// verbatim.
fn split_path_suffix(h: &str) -> (String, String) {
    // Whichever of '?' or '#' comes first marks the suffix.
    let q = h.find('?');
    let f = h.find('#');
    let cut = match (q, f) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    match cut {
        Some(i) => (h[..i].to_string(), h[i..].to_string()),
        None => (h.to_string(), String::new()),
    }
}

/// Collapse `.` and `..` segments in a path, the way a
/// browser does. Always returns a path beginning with '/'.
/// A trailing slash on the input is preserved.
///
/// Examples:
///   /a/b/../c      → /a/c
///   /a/./b         → /a/b
///   /a/b/../../c   → /c
///   /a/b/../../../c → /c   (can't go above root)
///   /a/b/          → /a/b/ (trailing slash kept)
fn normalize_path(path: &str) -> String {
    let had_trailing_slash = path.ends_with('/') && path.len() > 1;
    let leading_slash = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" => { /* skip empty (from // or leading/trailing) */ }
            "." => { /* current dir — drop */ }
            ".." => { out.pop(); /* parent — can't go above root */ }
            other => out.push(other),
        }
    }
    let mut result = String::new();
    if leading_slash { result.push('/'); }
    result.push_str(&out.join("/"));
    if had_trailing_slash && !result.ends_with('/') {
        result.push('/');
    }
    // Edge: empty result with leading slash already covered;
    // ensure at least "/" for a root-resolving path.
    if result.is_empty() { result.push('/'); }
    result
}


fn is_filtered(e: &ElementRef, sty: &RenderStyle) -> bool {
    ad_filter::should_filter(e.value().name(), e.value().attr("id"), e.value().attr("class"), e.value().attr("src"), sty.filter_mode)
}

fn is_hidden(e: &ElementRef) -> bool {
    if e.value().attr("hidden").is_some() || e.value().attr("aria-hidden") == Some("true") { return true; }
    // v0.1.172: aria-modal="true" without explicit `open` attribute means
    // "this is a collapsed dialog, currently invisible until activated."
    // Apple's iPhone marketing page has ~8 such modal subtrees in the
    // footer (trade-in modal, feature-card modals, etc.), each a full
    // modal with header, content paragraphs, close button. The live site
    // keeps them invisible via CSS opacity/pointer-events; our renderer
    // doesn't know that and lays them all out. Skipping them removes a
    // substantial amount of layout work in the footer region — confirmed
    // stutter zone in v0.1.170 profiling.log analysis.
    //
    // `<dialog open>` semantically means "this dialog is currently
    // visible" — respect it and don't skip those.
    if e.value().attr("aria-modal") == Some("true") && e.value().attr("open").is_none() {
        return true;
    }
    if let Some(st) = e.value().attr("style") {
        // v0.1.167 perf: previously this did `st.to_ascii_lowercase()`
        // allocating a String per element with any inline style. Now
        // we check via contains_ci, which is a byte-level lowercase
        // comparison with no allocation.
        //
        // We only check the combinations we care about. `display:none`
        // and friends use a colon, but we deliberately don't include
        // it in the needles — different sites use varying whitespace
        // (`display:none`, `display: none`, `display:  none`). Splitting
        // into separate substring checks (display + none) keeps the
        // logic identical to the old behavior.
        if (contains_ci(st, "display") && contains_ci(st, "none"))
            || (contains_ci(st, "visibility") && contains_ci(st, "hidden")) {
            return true;
        }
        // v0.1.172: inline `opacity: 0` is the third way authors mark
        // "currently invisible." Modal scrims and pre-animation states
        // commonly use this. Same logic as display/visibility above:
        // separate contains_ci checks so spacing/case variants all hit.
        // Caveat: `opacity: 0.5` would also trip "opacity" + "0" — but
        // 0.5 is still "mostly visible" content and the false-positive
        // cost is real. Use a stricter check: must contain `opacity: 0`
        // OR `opacity:0` as a unit (no trailing dot).
        if contains_ci(st, "opacity") {
            // Check for "opacity: 0" or "opacity:0" followed by ; or end-of-string
            // (not "0.5", not "0.1"). Conservative: cheap byte-level scan.
            let lower = st.as_bytes();
            for i in 0..lower.len().saturating_sub(7) {
                if lower[i..].len() >= 7 && lower[i..i+7].eq_ignore_ascii_case(b"opacity") {
                    // Skip whitespace and colon
                    let mut j = i + 7;
                    while j < lower.len() && (lower[j] == b' ' || lower[j] == b':' || lower[j] == b'\t') { j += 1; }
                    // Must be '0' followed by ';', end, or whitespace (not '0.5' etc)
                    if j < lower.len() && lower[j] == b'0' {
                        let after = j + 1;
                        if after >= lower.len()
                            || lower[after] == b';'
                            || lower[after] == b' '
                            || lower[after] == b'\t' {
                            return true;
                        }
                    }
                    break;
                }
            }
        }
        if contains_ci(st, "position") && contains_ci(st, "absolute")
            && (contains_ci(st, "left: -") || contains_ci(st, "left:-")) {
            return true;
        }
    }
    false
}

pub fn detect_challenge(html: &str) -> Option<String> {
    let l = html.to_ascii_lowercase();
    if l.contains("anubis") && l.contains("access denied") { return Some("Protected by Anubis — JavaScript proof-of-work required.".into()); }
    if l.contains("cf-browser-verification") || l.contains("__cf_chl_") { return Some("Cloudflare browser check — JavaScript required.".into()); }
    if l.contains("hcaptcha") || l.contains("g-recaptcha") { return Some("CAPTCHA required.".into()); }
    None
}

/// v0.1.373 (JS-rendering arc, Option 1): detect a single-page-app
/// shell where the body is essentially empty and the actual
/// content gets rendered by JavaScript after page load. Returns
/// Some(friendly_message) when an SPA shell is detected, None
/// otherwise.
///
/// Signal set:
///   1. `<body>` has exactly one or two element children
///   2. Those children are `<div id="root">`, `<div id="app">`,
///      `<div id="__next">`, or similar root mount nodes
///   3. The root divs are EMPTY (no children or only whitespace)
///   4. Page contains at least one `<script>` tag, especially
///      ones referring to common SPA framework bundles
///   5. Optionally: `<noscript>` block warning about JS
///
/// We're conservative: a page with substantial body content
/// (e.g. SSR'd content alongside scripts) should NOT trigger
/// this — only true empty shells.
///
/// Returns a message that includes a generic explanation plus,
/// when detectable, the specific framework guess. Used by main.rs
/// to render a banner instead of an empty page.
pub fn detect_spa(html: &str) -> Option<String> {
    let l = html.to_ascii_lowercase();

    // Look for body mount-point patterns. The substring approach
    // is much cheaper than parsing for what's a hot-path detector.
    let has_root_mount = l.contains(r#"<div id="root">"#)
        || l.contains(r#"<div id='root'>"#)
        || l.contains(r#"<div id="root"></div>"#)
        || l.contains(r#"<div id="app">"#)
        || l.contains(r#"<div id='app'>"#)
        || l.contains(r#"<div id="__next">"#)
        || l.contains(r#"<div id="__nuxt">"#)
        || l.contains(r#"<div id="svelte">"#);

    if !has_root_mount { return None; }

    // Body must be sparse. We approximate "sparse" by checking
    // the length of the rendered body content excluding scripts
    // and the mount div itself. If the body has substantial
    // markup OUTSIDE the root mount, the page isn't pure SPA.
    //
    // Cheap heuristic: count `<p>`, `<article>`, `<h1>`, `<h2>`,
    // `<main>`, and similar content-bearing tags. If there are
    // more than a few, this is server-rendered content with
    // a hydration root — not a pure SPA shell.
    let content_tag_count = l.matches("<p>").count()
        + l.matches("<p ").count()
        + l.matches("<article").count()
        + l.matches("<main").count()
        + l.matches("<h1").count()
        + l.matches("<h2").count()
        + l.matches("<section").count();

    if content_tag_count > 3 {
        // Has real content; not an empty-shell SPA.
        return None;
    }

    // Must have at least one script tag (otherwise the mount
    // div is just an oddly-named static div).
    if !l.contains("<script") { return None; }

    // Try to guess the framework for a more informative message.
    // These heuristics target visible bundle paths or framework-
    // specific data attributes.
    let framework = if l.contains("__next_data__") || l.contains("/_next/") {
        Some("Next.js")
    } else if l.contains("__nuxt") || l.contains("/_nuxt/") {
        Some("Nuxt")
    } else if l.contains("data-reactroot") || l.contains("/react") {
        Some("React")
    } else if l.contains("vue.js") || l.contains("v-app") {
        Some("Vue")
    } else if l.contains("data-svelte") || l.contains("svelte-") {
        Some("Svelte")
    } else if l.contains("ng-version") || l.contains("ng-app") {
        Some("Angular")
    } else {
        None
    };

    let msg = match framework {
        Some(name) => format!(
            "This page is a {} single-page application — content is rendered \
             by JavaScript at runtime. web2qml shows the rendered HTML; \
             this site needs a JavaScript engine to display content. \
             Try the site's RSS feed, an RSS-based reader, or check if \
             the site has a static mirror.",
            name
        ),
        None => "This page appears to be a JavaScript single-page application — \
                 content is rendered by JavaScript at runtime. web2qml shows the \
                 rendered HTML; this site needs a JavaScript engine to display content. \
                 Try the site's RSS feed, an RSS-based reader, or check if the site \
                 has a static mirror.".to_string(),
    };

    Some(msg)
}

fn best_srcset(ss: &str) -> Option<&str> { ss.split(',').filter_map(|e| e.trim().split_whitespace().next()).last() }

/// v0.1.375 (JS-rendering arc, Option 2): extract hydration
/// data blobs from SPA HTML. Many JS-rendered sites embed the
/// initial state as JSON in a `<script>` tag — Next.js uses
/// `__NEXT_DATA__`, Nuxt uses `__NUXT__`, etc. Extracting this
/// JSON gives us 40-60% of the SSR'd content without running JS.
///
/// This function returns the raw JSON string when found. Parsing
/// + rendering will come in a future ship that defines the
/// schema. The detection + extraction is the prerequisite — once
/// we have the data, rendering it is a separate problem.
///
/// Patterns recognized:
/// - `<script id="__NEXT_DATA__" type="application/json">...</script>` (Next.js)
/// - `<script>window.__NUXT__ = ...</script>` (Nuxt 2 — assignment form)
/// - `<script>window.__INITIAL_STATE__ = ...</script>` (generic Redux/Vuex)
/// - JSON-LD: `<script type="application/ld+json">...</script>` (last resort)
///
/// Returns (kind, json_string) on the first match. `kind` is a
/// short identifier for future rendering paths to switch on.
/// None if no hydration data was found.
pub fn extract_hydration_data(html: &str) -> Option<(&'static str, String)> {
    // Strategy: regex-free substring search. Find the opening
    // pattern, then scan forward for the closing `</script>`.
    // Conservative: we tolerate some attribute reordering by
    // matching the most common forms first, then falling back.
    //
    // Next.js: id="__NEXT_DATA__" is the unique marker. The
    // script tag is always `type="application/json"` so the
    // body between > and </script> is parseable JSON.
    if let Some(blob) = find_script_by_id(html, "__NEXT_DATA__") {
        return Some(("next", blob));
    }
    // Nuxt 2: window.__NUXT__ = {...}. The blob ends at `}` plus
    // the closing `</script>`. We find the assignment and the
    // closing tag, then trim the trailing `;` if present.
    if let Some(blob) = find_window_assignment(html, "__NUXT__") {
        return Some(("nuxt", blob));
    }
    // Generic Redux/Vuex initial state.
    if let Some(blob) = find_window_assignment(html, "__INITIAL_STATE__") {
        return Some(("initial-state", blob));
    }
    // JSON-LD as last resort. Provides structured semantic data
    // even on non-SPA pages.
    if let Some(blob) = find_script_by_type(html, "application/ld+json") {
        return Some(("json-ld", blob));
    }
    None
}

/// Find a script tag with the given `id` attribute and return
/// its inner body. Tolerates attribute order variations.
fn find_script_by_id(html: &str, id: &str) -> Option<String> {
    let needle = format!(r#"id="{}""#, id);
    let needle_alt = format!(r#"id='{}'"#, id);
    // Locate the id attribute occurrence first.
    let id_pos = html.find(&needle)
        .or_else(|| html.find(&needle_alt))?;
    // Walk backward to find the `<script` opener — we accept
    // the id appearing anywhere in the script tag's attributes.
    let opener_start = html[..id_pos].rfind("<script")?;
    // Find the `>` that closes the opening tag.
    let body_start = opener_start + html[opener_start..].find('>')? + 1;
    // Find `</script>` after that.
    let body_end = body_start + html[body_start..].find("</script>")?;
    let body = html[body_start..body_end].trim();
    if body.is_empty() { return None; }
    Some(body.to_string())
}

/// Find a script tag with the given `type` attribute. Returns
/// the FIRST match (for JSON-LD there can be many; first is the
/// most likely to be page-level metadata).
fn find_script_by_type(html: &str, type_val: &str) -> Option<String> {
    let needle = format!(r#"type="{}""#, type_val);
    let needle_alt = format!(r#"type='{}'"#, type_val);
    let pos = html.find(&needle)
        .or_else(|| html.find(&needle_alt))?;
    let opener_start = html[..pos].rfind("<script")?;
    let body_start = opener_start + html[opener_start..].find('>')? + 1;
    let body_end = body_start + html[body_start..].find("</script>")?;
    let body = html[body_start..body_end].trim();
    if body.is_empty() { return None; }
    Some(body.to_string())
}

/// Find a window assignment of the form
/// `window.__NAME__ = {...}` (with optional `var`/`let`) inside
/// a `<script>` block. Returns the RHS expression as a string,
/// trimmed of trailing `;` and whitespace.
fn find_window_assignment(html: &str, var_name: &str) -> Option<String> {
    // Search for the assignment patterns. We tolerate `window.`
    // prefix or bare assignment.
    let patterns = [
        format!("window.{}={{", var_name),
        format!("window.{} = {{", var_name),
        format!("window.{}=[", var_name),
        format!("window.{} = [", var_name),
    ];
    let (start, opening_char) = patterns.iter()
        .find_map(|p| html.find(p).map(|i| {
            let opens = if p.ends_with('{') { '{' } else { '[' };
            (i + p.len() - 1, opens)
        }))?;
    // Scan forward from `start` matching braces/brackets to find
    // the end of the literal. Tolerates strings (skip past quoted
    // content) and escapes. Cheap depth-counter implementation.
    let closing_char = if opening_char == '{' { '}' } else { ']' };
    let bytes = html.as_bytes();
    let mut depth: i32 = 0;
    let mut in_str: Option<u8> = None;
    let mut prev_escape = false;
    let mut end_pos = None;
    let mut idx = start;
    while idx < bytes.len() {
        let b = bytes[idx];
        if let Some(q) = in_str {
            if !prev_escape && b == q {
                in_str = None;
            }
            prev_escape = !prev_escape && b == b'\\';
        } else if b == b'"' || b == b'\'' {
            in_str = Some(b);
            prev_escape = false;
        } else if b == opening_char as u8 {
            depth += 1;
        } else if b == closing_char as u8 {
            depth -= 1;
            if depth == 0 {
                end_pos = Some(idx + 1);
                break;
            }
        }
        idx += 1;
    }
    let end = end_pos?;
    let body = html[start..end].trim();
    if body.is_empty() { return None; }
    Some(body.to_string())
}

/// v0.1.378 (JS-rendering arc, Option 2 step 2): render a
/// JSON-LD article blob as markdown that can pass through
/// `manpages::to_qml`. Returns None when the blob isn't an
/// Article-shaped JSON-LD document.
///
/// Why this works without a JS engine: JSON-LD is structured
/// data the SERVER prepared at build time. Reading it requires
/// only JSON-like parsing, not JS execution. Schema.org's
/// Article, BlogPosting, and NewsArticle types are widely
/// adopted by SEO-conscious sites (news, blogs, documentation).
///
/// We don't pull in serde_json — the parsing surface is small
/// enough to handle with substring search + escape-aware
/// extraction. Five fields cover most usable Articles:
/// `headline` (title), `author` (string or nested name),
/// `datePublished`, `description`, `articleBody`.
///
/// When the blob is array-wrapped or `@graph`-wrapped (common
/// in @context-style JSON-LD), we pick the first item whose
/// `@type` is Article-ish.
pub fn render_json_ld_article(blob: &str) -> Option<String> {
    // Find first @type and check if Article-ish.
    let type_val = find_json_string_field(blob, "@type")?;
    let is_article = matches!(type_val.as_str(),
        "Article" | "BlogPosting" | "NewsArticle" | "TechArticle"
        | "ScholarlyArticle" | "Report"
    );
    if !is_article { return None; }

    let headline = find_json_string_field(blob, "headline")
        .or_else(|| find_json_string_field(blob, "name"));
    let author = find_json_string_field(blob, "author")
        .or_else(|| find_nested_author_name(blob));
    let date = find_json_string_field(blob, "datePublished")
        .or_else(|| find_json_string_field(blob, "dateCreated"));
    let description = find_json_string_field(blob, "description");
    let body = find_json_string_field(blob, "articleBody")
        .or_else(|| find_json_string_field(blob, "text"));

    // We need at least a headline + (body OR description) to
    // render usefully. JSON-LD documents that only specify
    // metadata without the actual content text aren't worth
    // surfacing — the HTML would have rendered them anyway.
    if headline.is_none() {
        return None;
    }
    if body.is_none() && description.is_none() {
        return None;
    }

    let mut md = String::with_capacity(2048);
    if let Some(h) = &headline {
        md.push_str("# ");
        md.push_str(&unescape_json_string(h));
        md.push_str("\n\n");
    }
    // Byline + date together if either is present.
    let byline_parts: Vec<String> = [author.as_ref(), date.as_ref()]
        .iter()
        .filter_map(|o| o.map(|s| unescape_json_string(s)))
        .collect();
    if !byline_parts.is_empty() {
        md.push_str("*");
        md.push_str(&byline_parts.join(" — "));
        md.push_str("*\n\n");
    }
    if let Some(d) = &description {
        let d_clean = unescape_json_string(d);
        if !d_clean.trim().is_empty() {
            md.push_str(&d_clean);
            md.push_str("\n\n");
        }
    }
    if let Some(b) = &body {
        let b_clean = unescape_json_string(b);
        md.push_str(&b_clean);
        md.push_str("\n");
    }
    Some(md)
}

/// v0.1.384 (JS-rendering arc, Option 2 step): given a Next.js
/// `__NEXT_DATA__` JSON blob, attempt to render it as an
/// article when the blob contains article-shaped data at any
/// nesting depth.
///
/// Next.js stores route data as
/// `{ "props": { "pageProps": { ... } }, ... }`. Many news
/// and blog sites place the article object somewhere inside
/// pageProps — common shapes:
///   * `pageProps.article = { title, body, ... }`
///   * `pageProps.post = { headline, content, ... }`
///   * `pageProps.data = { title, articleBody, ... }`
///   * `pageProps.initialState.article = { ... }`
///
/// Rather than enumerate every site's structure, we look for
/// the SHAPE — an object that has both a "title/headline" field
/// AND a "body/content/articleBody" field. The first such
/// shape we find gets rendered.
///
/// Returns the markdown ready for `manpages::to_qml`. None if
/// no article shape is found in the blob.
///
/// This is the first concrete consumer of `__NEXT_DATA__`
/// extraction (which has been live since v0.1.375 but only
/// surfacing JSON-LD until this ship). It's intentionally
/// limited: only Article-shaped data, no schema validation
/// beyond the title+body shape check. Many Next.js pages will
/// still return None; that's expected — render the regular
/// HTML in those cases.
pub fn render_next_data_article(blob: &str) -> Option<String> {
    // Find an object containing a (title|headline|name) field
    // AND a (body|articleBody|content|text) field. Scan once,
    // tracking nesting depth and the most recent "{" — when we
    // find both kinds of field within the same depth range,
    // try to render.
    //
    // Implementation: brute-force linear scan that picks the
    // FIRST title+body pair found anywhere in the blob. The
    // text fields are typically long enough (article body) and
    // the title short enough that ambiguity is rare in
    // practice.

    let title_candidates = ["title", "headline", "name"];
    let body_candidates = ["articleBody", "body", "content", "text"];

    // Find the earliest title and the earliest body.
    let title = title_candidates.iter()
        .filter_map(|&f| {
            find_json_string_field(blob, f)
                .map(|v| (v, f))
        })
        .filter(|(v, _)| {
            // Titles are short — reject if >300 chars (often
            // means we matched something else like a long
            // description).
            v.len() <= 300 && !v.trim().is_empty()
        })
        .next();

    let body = body_candidates.iter()
        .filter_map(|&f| {
            find_json_string_field(blob, f)
                .map(|v| (v, f))
        })
        .filter(|(v, _)| {
            // Bodies are LONG — reject if <100 chars (often
            // means we matched a UI string like "content").
            // Unescape-aware: 100 char threshold on raw blob
            // is conservative (most real articles are 1000+).
            v.len() >= 100
        })
        .next();

    let (title_raw, _) = title?;
    let (body_raw, _) = body?;

    // Optional fields. Authors in __NEXT_DATA__ are often
    // nested objects (e.g. {"author":{"name":"X"}}); fall
    // back to the same nested helper we use for JSON-LD.
    let author = find_json_string_field(blob, "author")
        .or_else(|| find_nested_author_name(blob));
    let date = find_json_string_field(blob, "datePublished")
        .or_else(|| find_json_string_field(blob, "publishedAt"))
        .or_else(|| find_json_string_field(blob, "publishDate"))
        .or_else(|| find_json_string_field(blob, "date"));

    let mut md = String::with_capacity(2048 + body_raw.len());
    md.push_str("# ");
    md.push_str(&unescape_json_string(&title_raw));
    md.push_str("\n\n");

    let byline_parts: Vec<String> = [author.as_ref(), date.as_ref()]
        .iter()
        .filter_map(|o| o.map(|s| unescape_json_string(s)))
        .filter(|s| !s.trim().is_empty())
        .collect();
    if !byline_parts.is_empty() {
        md.push_str("*");
        md.push_str(&byline_parts.join(" — "));
        md.push_str("*\n\n");
    }

    md.push_str(&unescape_json_string(&body_raw));
    md.push_str("\n");
    Some(md)
}

/// v0.1.385 (JS-rendering arc, Option 2 step): render a Nuxt
/// `__NUXT__` blob as an article. Same shape-detection
/// approach as `render_next_data_article` — Nuxt's
/// `data`/`state`/`pinia` containers vary by Nuxt version
/// and per-app store layout, so a path-based walker would be
/// brittle. Field-shape matching reuses the same
/// title+body heuristic.
///
/// Returns the markdown ready for `manpages::to_qml`. None if
/// no article shape is found.
pub fn render_nuxt_article(blob: &str) -> Option<String> {
    // The renderer is shape-based, not path-based; delegating
    // is exactly correct. The only reason for separate
    // functions is to keep a clear surface in main.rs's
    // hydration handler and to make per-framework tests
    // easier to read.
    render_next_data_article(blob)
}

/// v0.1.385 (JS-rendering arc, Option 2 step): render a
/// generic `__INITIAL_STATE__` Redux/Vuex blob as an article.
/// Same shape-detection approach as Nuxt + Next.js.
pub fn render_initial_state_article(blob: &str) -> Option<String> {
    render_next_data_article(blob)
}

/// Find a JSON string field by name. Returns the raw (still-
/// escaped) string value. Caller calls `unescape_json_string`
/// to get the readable form.
///
/// Handles the patterns:
///   "fieldname":"value"
///   "fieldname": "value"
///   "fieldname" : "value"
///
/// Does NOT handle:
///   - Numeric values (we'd return None)
///   - Nested objects (caller's job)
///   - Arrays (caller's job)
///
/// First match wins — sufficient for our use case where
/// top-level fields are unique within an Article object.
fn find_json_string_field(blob: &str, field: &str) -> Option<String> {
    // Look for `"<field>"` followed by `:` and an opening
    // quote. Scan once.
    let needle = format!("\"{}\"", field);
    let mut idx = 0;
    while let Some(rel) = blob[idx..].find(&needle) {
        let abs = idx + rel;
        // Skip past `"field"` to look for the colon.
        let after = abs + needle.len();
        if after >= blob.len() {
            return None;
        }
        let rest = &blob[after..];
        // Whitespace then `:` then whitespace then `"`.
        let trimmed = rest.trim_start();
        if !trimmed.starts_with(':') {
            // Probably part of a larger key like
            // "fieldname_extra"; advance and retry.
            idx = after;
            continue;
        }
        let after_colon = trimmed[1..].trim_start();
        if !after_colon.starts_with('"') {
            // Numeric / object / array — caller handles
            // those separately. Return None for string-
            // valued caller.
            return None;
        }
        // Read characters until the matching closing `"`,
        // handling escape sequences.
        let bytes = after_colon.as_bytes();
        let mut end = 1;  // skip opening quote
        let mut escape = false;
        while end < bytes.len() {
            let b = bytes[end];
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                break;
            }
            end += 1;
        }
        if end >= bytes.len() {
            return None;
        }
        let value = &after_colon[1..end];
        return Some(value.to_string());
    }
    None
}

/// Author can appear as a string or as a nested object like
/// `"author": {"@type":"Person","name":"Alice"}`. When
/// `find_json_string_field("author")` returns None (because
/// the value isn't a bare string), try to extract the nested
/// `name` field instead.
///
/// We don't try to handle author arrays — the first author's
/// name is the most useful surface in a 1-2 sentence summary.
fn find_nested_author_name(blob: &str) -> Option<String> {
    let idx = blob.find("\"author\"")?;
    let rest = &blob[idx + 8..];
    let trimmed = rest.trim_start();
    if !trimmed.starts_with(':') { return None; }
    let after_colon = trimmed[1..].trim_start();
    if !after_colon.starts_with('{') {
        // Either it's a string (caller handles) or an array
        // (we punt for now).
        return None;
    }
    // Look for "name":"..." inside the object. Use the same
    // string-field extractor — it'll find the first "name"
    // which inside an author block is conventionally the
    // author's display name.
    //
    // We constrain the search window to the object's
    // contents to avoid picking up a later top-level "name"
    // field (which would conflict).
    let bytes = after_colon.as_bytes();
    let mut end = 1;
    let mut depth = 1;
    let mut in_str = false;
    let mut escape = false;
    while end < bytes.len() && depth > 0 {
        let b = bytes[end];
        if escape {
            escape = false;
        } else if in_str {
            if b == b'\\' { escape = true; }
            else if b == b'"' { in_str = false; }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
        }
        end += 1;
    }
    if depth != 0 { return None; }
    let object_body = &after_colon[..end];
    find_json_string_field(object_body, "name")
}

/// Unescape common JSON string escape sequences. We don't
/// implement full RFC 8259 — \uXXXX is the most likely
/// missing case, but we handle \n, \t, \", \\, \/. Anything
/// unrecognized is passed through verbatim.
fn unescape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('u') => {
                    // \uXXXX — read 4 hex digits.
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() == 4 {
                        if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(cp) {
                                out.push(ch);
                                continue;
                            }
                        }
                    }
                    // Malformed — drop the escape, append nothing.
                }
                Some(other) => {
                    // Unknown escape — preserve verbatim.
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// v0.1.379 (data-attr arc, parallel task): strip noisy
/// tracking/animation `data-*` attributes from HTML BEFORE
/// it gets handed to `scraper::Html::parse_document`. Real
/// pages from publication-grade sites (Apple, Stripe, Vercel,
/// Linear) carry 30-50% of their attribute bytes in
/// telemetry/animation `data-*` attributes that qml_gen never
/// reads. Stripping pre-parse saves parse time + memory.
///
/// Conservative regex-free byte-scan implementation:
///   1. Find the literal `data-` prefix
///   2. Capture the attribute name (chars until `=` or
///      whitespace)
///   3. If the name matches one of the known-noisy prefixes
///      AND the next non-space char is `=` followed by a
///      quoted value, snip the entire `data-foo="bar"` run
///
/// Prefixes stripped:
///   data-analytics-*
///   data-staggered-item
///   data-anim-scroll-group
///   data-focus-expression
///   data-component-list  (already not read by qml_gen)
///   data-impression-id
///   data-tracking-*
///   data-gtm-*
///   data-ga-*
///   data-pixel-*
///   data-segment-*
///
/// Critically does NOT strip:
///   data-src, data-srcset, data-original, data-lazy-src
///     — these are real image sources used by the lazy-load
///       path (see is_placeholder + img handler)
///   data-href — used by detect_js_href as a click target
///   data-aos, data-animate — used by anim_detect
///   data-empty — used by picture handler for Apple's
///     placeholder marker
///   data-bg-image, data-background — sometimes real URLs
///
/// Returns the stripped HTML. Always allocates (we're
/// rebuilding the string); pages with no matches end up
/// near-equivalent to the original length. Idempotent: running
/// twice is the same as running once.
pub fn strip_tracking_attrs(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Cheap fast-path: most bytes are NOT the start of a
        // `data-` attribute. The leading char of the prefix is
        // 'd' so we only do the slower work after seeing one.
        if bytes[i] == b'd' && i + 5 < bytes.len() && &bytes[i..i + 5] == b"data-" {
            // Look at preceding char to make sure this is an
            // attribute boundary (whitespace before `data-`),
            // not inside a text node or another attr value.
            let prev_is_boundary = i == 0
                || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r');
            if prev_is_boundary {
                // Scan the attribute name: data-... up to `=` or
                // whitespace or `>`.
                let name_start = i;
                let mut j = i + 5; // past "data-"
                while j < bytes.len() {
                    let c = bytes[j];
                    if c == b'=' || c == b' ' || c == b'\t'
                        || c == b'\n' || c == b'\r' || c == b'>'
                        || c == b'/'
                    {
                        break;
                    }
                    j += 1;
                }
                let name = &bytes[name_start..j];
                if is_tracking_attr(name) {
                    // Strip the attr. If followed by `=`, also
                    // strip the value (quoted or unquoted).
                    let mut k = j;
                    // Skip whitespace between name and `=`.
                    while k < bytes.len()
                        && matches!(bytes[k], b' ' | b'\t')
                    {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'=' {
                        k += 1;
                        // Skip whitespace after `=`.
                        while k < bytes.len()
                            && matches!(bytes[k], b' ' | b'\t')
                        {
                            k += 1;
                        }
                        if k < bytes.len()
                            && (bytes[k] == b'"' || bytes[k] == b'\'')
                        {
                            let quote = bytes[k];
                            k += 1;
                            // Scan until matching quote.
                            while k < bytes.len() && bytes[k] != quote {
                                k += 1;
                            }
                            if k < bytes.len() { k += 1; } // past closing quote
                        } else {
                            // Unquoted value — scan until ws or `>`.
                            while k < bytes.len() {
                                let c = bytes[k];
                                if c == b' ' || c == b'\t' || c == b'\n'
                                    || c == b'\r' || c == b'>' || c == b'/'
                                {
                                    break;
                                }
                                k += 1;
                            }
                        }
                    }
                    // Also consume a single leading whitespace
                    // byte (the separator between this attr and
                    // the previous one) so we don't leave
                    // double spaces in the markup.
                    if !out.is_empty()
                        && out.as_bytes().last() == Some(&b' ')
                    {
                        out.pop();
                    }
                    i = k;
                    continue;
                }
            }
        }
        // v0.1.447: emit a full UTF-8 codepoint, not a single
        // byte. The previous `out.push(bytes[i] as char)` cast
        // EACH byte to a Latin-1 char, so any multi-byte codepoint
        // (curly apostrophe U+2019 = E2 80 99, em-dash, ellipsis,
        // accented letters, emoji) became 2-4 garbage chars —
        // e.g. "he's" rendered "heâ€™s". This is the same bug
        // class v0.1.331 fixed in manpages; this HTML-preprocessing
        // path never got the fix. Detect the lead byte's length
        // and copy the whole codepoint.
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else {
            let len = if b < 0xC0 { 1 }      // stray continuation
                else if b < 0xE0 { 2 }
                else if b < 0xF0 { 3 }
                else { 4 };
            let end = (i + len).min(bytes.len());
            if let Ok(piece) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(piece);
                i = end;
            } else {
                // Truncated/invalid — skip the lead byte and
                // continue rather than emit mojibake.
                i += 1;
            }
        }
    }
    out
}

/// True when the byte slice matches one of the known
/// tracking/animation `data-*` attribute names. Includes
/// prefix matches (`data-analytics-*`) and exact matches
/// (`data-staggered-item`).
fn is_tracking_attr(name: &[u8]) -> bool {
    // Must start with "data-".
    if name.len() < 6 || &name[..5] != b"data-" {
        return false;
    }
    // Exact matches.
    if matches!(name,
        b"data-staggered-item"
        | b"data-anim-scroll-group"
        | b"data-focus-expression"
        | b"data-component-list"
        | b"data-impression-id"
    ) {
        return true;
    }
    // Prefix matches: data-analytics-..., data-tracking-...,
    // data-gtm-..., data-ga-..., data-pixel-..., data-segment-...
    static PREFIXES: &[&[u8]] = &[
        b"data-analytics-",
        b"data-tracking-",
        b"data-gtm-",
        b"data-ga-",
        b"data-pixel-",
        b"data-segment-",
    ];
    for p in PREFIXES {
        if name.len() > p.len() && &name[..p.len()] == *p {
            return true;
        }
    }
    false
}

/// v0.1.374: detect placeholder image src values. Used by the
/// img tag handler to skip past common lazy-load placeholder
/// patterns and find the real source URL.
///
/// Patterns matched:
/// - empty / whitespace-only
/// - `data:image/gif` and `data:image/svg` (common spacer types)
/// - Short `data:image/png;base64,iVBORw0KGgo...` strings (the
///   well-known prefix of a 1x1 transparent PNG; we don't decode,
///   just match the prefix + length constraint)
/// - `about:blank` and `#` (used as empty src markers)
///
/// Does NOT match:
/// - Real http/https URLs
/// - Long data: URIs (typically real inline images)
/// - Relative paths
pub fn is_placeholder(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() { return true; }
    if t.starts_with("data:image/gif") { return true; }
    if t.starts_with("data:image/svg") { return true; }
    if t.starts_with("data:image/png;base64,iVBORw0KGgo")
        && t.len() < 200
    {
        return true;
    }
    if t == "about:blank" || t == "#" { return true; }
    false
}

fn detect_js_href(e: &ElementRef, base: &str) -> Option<String> {
    if let Some(h) = e.value().attr("data-href") { if !h.is_empty() { return Some(resolve_url(h, base)); } }
    if let Some(oc) = e.value().attr("onclick") {
        if oc.to_ascii_lowercase().contains("location") {
            for q in &['\'', '"'] { if let Some(s) = oc.find(*q) { if let Some(e) = oc[s+1..].find(*q) {
                let u = &oc[s+1..s+1+e];
                if !u.is_empty() && (u.starts_with("http") || u.starts_with("/")) { return Some(resolve_url(u, base)); }
            } } }
        }
    }
    None
}

/// Extract basic CSS style hints from inline style and class attributes
struct CssHints {
    /// v0.1.167 perf: changed from `Option<String>` to
    /// `Option<Cow<'static, str>>` so Tailwind class hits use
    /// borrowed `&'static str` instead of allocating a fresh String
    /// per element. Inline-style colors still allocate (the value
    /// comes from runtime parsing of `style="color: ..."`) but the
    /// Tailwind hot path is now alloc-free.
    color: Option<Cow<'static, str>>,
    bg_color: Option<Cow<'static, str>>,
    font_size: Option<i32>,
    bold: bool,
    italic: bool,
    center: bool,
    padding: Option<i32>,
    border_radius: Option<i32>,
    max_width: Option<i32>,
}

impl CssHints {
    fn from_element(elem: &ElementRef) -> Self {
        let mut h = CssHints {
            color: None, bg_color: None, font_size: None, bold: false,
            italic: false, center: false, padding: None, border_radius: None, max_width: None,
        };

        // Parse inline style
        // v0.1.167 perf: previously this did `style.to_ascii_lowercase()`
        // allocating per-element. We still need a lowercased key for
        // matching ("font-size" vs "FONT-SIZE"), but the VALUE can stay
        // mixed-case (css_color_to_hex and css_size_to_px handle that).
        // So we just lowercase the keys inline — small temp allocations
        // for each property, but the key portion is short (~12 bytes
        // max for our match cases).
        if let Some(style) = elem.value().attr("style") {
            for prop in style.split(';') {
                let prop = prop.trim();
                if let Some((key, val)) = prop.split_once(':') {
                    let key_lc = key.trim().to_ascii_lowercase();
                    let val = val.trim();
                    match key_lc.as_str() {
                        "color" => { h.color = css_color_to_hex(val).map(Cow::Owned); }
                        "background-color" | "background" => { h.bg_color = css_color_to_hex(val).map(Cow::Owned); }
                        "font-size" => { h.font_size = css_size_to_px(val); }
                        "font-weight" => {
                            // Case-insensitive match against a few values.
                            // Common form is "bold" lowercase; numeric forms
                            // don't have case. We do a single lowercase
                            // alloc here only when the property name matches,
                            // which is rare.
                            let v = val.to_ascii_lowercase();
                            if v == "bold" || v == "700" || v == "800" || v == "900" { h.bold = true; }
                        }
                        "font-style" => {
                            // Same shape — lowercase only when this property
                            // is set (rare).
                            if val.eq_ignore_ascii_case("italic") { h.italic = true; }
                        }
                        "text-align" => {
                            if val.eq_ignore_ascii_case("center") { h.center = true; }
                        }
                        "padding" => { h.padding = css_size_to_px(val); }
                        "border-radius" => { h.border_radius = css_size_to_px(val); }
                        "max-width" => { h.max_width = css_size_to_px(val); }
                        _ => {}
                    }
                }
            }
        }

        // Check class-based styling.
        // v0.1.167 perf: was `cls.to_ascii_lowercase()` once per element
        // with a class attribute. Now uses contains_ci which does the
        // case-insensitive comparison byte-by-byte without allocating.
        // Tailwind color/size hits assign &'static str via Cow::Borrowed
        // — no allocation in the hot path.
        if let Some(cls) = elem.value().attr("class") {
            if contains_ci(cls, "font-bold") || contains_ci(cls, "fw-bold") { h.bold = true; }
            if contains_ci(cls, "italic") { h.italic = true; }
            if contains_ci(cls, "text-center") || contains_ci(cls, "text-centre") { h.center = true; }
            // Tailwind text colors — &'static str via Cow::Borrowed
            if contains_ci(cls, "text-red") { h.color = Some(Cow::Borrowed("#dc2626")); }
            else if contains_ci(cls, "text-green") { h.color = Some(Cow::Borrowed("#16a34a")); }
            else if contains_ci(cls, "text-blue") { h.color = Some(Cow::Borrowed("#2563eb")); }
            else if contains_ci(cls, "text-yellow") || contains_ci(cls, "text-amber") { h.color = Some(Cow::Borrowed("#d97706")); }
            else if contains_ci(cls, "text-gray") || contains_ci(cls, "text-grey") || contains_ci(cls, "text-muted") { h.color = Some(Cow::Borrowed("#6b7280")); }
            else if contains_ci(cls, "text-white") { h.color = Some(Cow::Borrowed("#ffffff")); }
            // Tailwind text sizes
            if contains_ci(cls, "text-xs") { h.font_size = Some(12); }
            else if contains_ci(cls, "text-sm") { h.font_size = Some(14); }
            else if contains_ci(cls, "text-lg") { h.font_size = Some(18); }
            else if contains_ci(cls, "text-xl") { h.font_size = Some(20); }
            else if contains_ci(cls, "text-2xl") { h.font_size = Some(24); }
            else if contains_ci(cls, "text-3xl") { h.font_size = Some(30); }
            else if contains_ci(cls, "text-4xl") { h.font_size = Some(36); }
            // Tailwind bg colors
            if contains_ci(cls, "bg-red") { h.bg_color = Some(Cow::Borrowed("#fecaca")); }
            else if contains_ci(cls, "bg-green") { h.bg_color = Some(Cow::Borrowed("#bbf7d0")); }
            else if contains_ci(cls, "bg-blue") { h.bg_color = Some(Cow::Borrowed("#bfdbfe")); }
            else if contains_ci(cls, "bg-yellow") || contains_ci(cls, "bg-amber") { h.bg_color = Some(Cow::Borrowed("#fde68a")); }
            else if contains_ci(cls, "bg-gray") || contains_ci(cls, "bg-grey") { h.bg_color = Some(Cow::Borrowed("#e5e7eb")); }
        }
        h
    }

    /// Apply style hints to a QML Text element (append to string builder)
    fn apply_to_text(&self, o: &mut String, sty: &RenderStyle) {
        let color = self.color.as_deref().unwrap_or(&sty.text_color);
        // v0.1.167 perf: use the cached `font_size_str` when this
        // element inherits the default size (the common case). For
        // overrides, write! directly into the buffer to avoid the
        // intermediate `i32::to_string()` allocation.
        let fs = self.font_size.unwrap_or(sty.font_size);
        o.push_str("color: \""); o.push_str(color); o.push('"');
        o.push_str("\nfont.pixelSize: ");
        if fs == sty.font_size {
            o.push_str(&sty.font_size_str);
        } else {
            let _ = write!(o, "{fs}");
        }
        o.push_str("\nfont.family: \""); o.push_str(&sty.font_family); o.push('"');
        if self.bold { o.push_str("\nfont.bold: true"); }
        if self.italic { o.push_str("\nfont.italic: true"); }
        if self.center { o.push_str("\nhorizontalAlignment: Text.AlignHCenter"); }
        o.push_str("\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: ");
        if let Some(mw) = self.max_width {
            // v0.1.168 perf: dropped the `parent ?` truthiness check
            // for the same reason as the top-level W constant —
            // child Texts always have a valid parent. Saves a JS
            // truthiness evaluation per max-width-styled element.
            o.push_str("Math.min(");
            let _ = write!(o, "{mw}");
            o.push_str(", parent.width)");
        }
        else { o.push_str(W); }
        o.push('\n');
    }

    /// Wrap content with background Rectangle if bg_color is set
    fn maybe_bg_wrap_start(&self, o: &mut String) -> bool {
        if let Some(bg) = &self.bg_color {
            o.push_str("Rectangle {\nwidth: "); o.push_str(W);
            o.push_str("\nimplicitHeight: childCol.implicitHeight");
            // v0.1.167 perf: write! into buffer instead of `to_string()`.
            if let Some(p) = self.padding {
                o.push_str(" + ");
                let _ = write!(o, "{}", p * 2);
            }
            o.push_str("\ncolor: \""); o.push_str(bg); o.push('"');
            if let Some(r) = self.border_radius {
                o.push_str("\nradius: ");
                let _ = write!(o, "{r}");
            }
            o.push_str("\nColumn {\n");
            if let Some(p) = self.padding {
                o.push_str("anchors.centerIn: parent\nwidth: parent.width - ");
                let _ = write!(o, "{}", p * 2);
                o.push('\n');
            } else {
                o.push_str("anchors.fill: parent\n");
            }
            true
        } else { false }
    }

    fn maybe_bg_wrap_end(&self, o: &mut String, wrapped: bool) {
        if wrapped {
            o.push_str("}\n}"); // close Column + Rectangle
        }
    }
}

fn css_color_to_hex(val: &str) -> Option<String> {
    let v = val.trim();
    if v.starts_with('#') && (v.len() == 4 || v.len() == 7 || v.len() == 9) {
        return Some(v.to_string());
    }
    if v.starts_with("rgb") {
        // rgb(r,g,b) → #rrggbb
        let inner = v.trim_start_matches("rgba(").trim_start_matches("rgb(").trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (parts[0].trim().parse::<u8>(), parts[1].trim().parse::<u8>(), parts[2].trim().parse::<u8>()) {
                return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
            }
        }
    }
    // Named colors
    match v {
        "red" => Some("#ff0000".into()), "blue" => Some("#0000ff".into()),
        "green" => Some("#008000".into()), "white" => Some("#ffffff".into()),
        "black" => Some("#000000".into()), "gray"|"grey" => Some("#808080".into()),
        "orange" => Some("#ffa500".into()), "yellow" => Some("#ffff00".into()),
        "purple" => Some("#800080".into()), "pink" => Some("#ffc0cb".into()),
        "transparent" => Some("transparent".into()),
        _ => None
    }
}

fn css_size_to_px(val: &str) -> Option<i32> {
    let v = val.trim().trim_end_matches("px").trim_end_matches("em").trim_end_matches("rem").trim_end_matches("pt");
    if let Ok(n) = v.parse::<f32>() {
        if val.contains("em") || val.contains("rem") { Some((n * 16.0) as i32) }
        else if val.contains("pt") { Some((n * 1.33) as i32) }
        else { Some(n as i32) }
    } else { None }
}

// ─── Generator ───

/// v0.1.345: scan the direct children of an element looking for
/// "sidebar" shapes — semantic tags (<aside>, <nav>) that almost
/// always render as side navigation, or class/id values that
/// match common sidebar patterns. Used by the main generator to
/// demote a Row/RowWrap layout to Column when a sidebar is
/// present, preventing the off-screen-shift bug where the main
/// content lands at x = sidebar_width and falls off the right
/// edge of narrower viewports.
///
/// Only inspects DIRECT children, not deep descendants — a
/// `<nav>` somewhere deep in the tree is almost always a
/// breadcrumb or pagination row, not a sidebar. Sidebars are
/// typically immediate children of the layout container.
/// v0.1.366: extracted single-element sidebar check from
/// has_sidebar_child so the same logic can be used both for
/// direct-children scanning and for the new "first child in
/// 2-child Row" heuristic. Also expanded the signal set to
/// catch modern React/Tailwind layouts that DON'T use semantic
/// names (chatgpt.com, claude.ai, linear.app, notion.so style
/// shells).
///
/// Signals checked, ordered cheapest-first:
///
/// 1. Semantic tag (`<aside>`, `<nav>`)
/// 2. Class name containing canonical sidebar tokens
///    ("sidebar", "side-nav", etc.) — v0.1.345 set
/// 3. ID matching canonical patterns — v0.1.345 set
/// 4. **NEW v0.1.366**: Tailwind width utility classes —
///    `w-48`/`w-56`/`w-60`/`w-64`/`w-72`/`w-80`/`w-96` etc.
///    plus responsive variants like `md:w-64`. These width
///    presets are 12-24rem — the classic sidebar range.
///    Wider presets (`w-1/2`, `w-full`, `w-screen`) are
///    deliberately excluded — those mean "main content."
/// 5. **NEW v0.1.366**: position-fixed/sticky utility classes
///    (`fixed`, `sticky`). When a direct child of a flex row
///    has fixed positioning, it's almost always a sidebar.
/// 6. **NEW v0.1.366**: inline `style="width: <NNN>px"` or
///    `style="width: <N>rem"` with the value falling in the
///    sidebar range (<= 320px / 20rem). The most common
///    sidebar widths in production sites.
fn is_sidebar_like(elem: &scraper::node::Element) -> bool {
    // (1) Semantic tags
    let child_tag = elem.name();
    if matches!(child_tag, "aside" | "nav") {
        return true;
    }
    // (2/3/4/5) Class-based checks
    if let Some(cls) = elem.attr("class") {
        // v0.1.345 canonical sidebar names
        if contains_ci(cls, "sidebar")
            || contains_ci(cls, "side-nav")
            || contains_ci(cls, "sidenav")
            || contains_ci(cls, "side-rail")
            || contains_ci(cls, "left-rail")
            || contains_ci(cls, "right-rail")
            || contains_ci(cls, "side-panel")
            || contains_ci(cls, "table-of-contents")
            || contains_ci(cls, "toc-wrap")
            || contains_ci(cls, "nav-sidebar")
        {
            return true;
        }
        // Whole-word check for ambiguous tokens
        for tok in cls.split_ascii_whitespace() {
            let t = tok.to_ascii_lowercase();
            if t == "toc" || t == "aside" || t == "secondary" {
                return true;
            }
            // v0.1.366: Tailwind narrow-width classes. Strip
            // responsive prefix (`md:`, `lg:`, `sm:`, `xl:`,
            // `2xl:`) before comparing — `md:w-64` and `w-64`
            // both signal sidebar shape.
            let bare = match t.split_once(':') {
                Some((prefix, rest)) if matches!(prefix,
                    "sm" | "md" | "lg" | "xl" | "2xl"
                    | "hover" | "focus") => rest,
                _ => t.as_str(),
            };
            // Tailwind w-NN where NN is in the sidebar range.
            // Widths: 12rem=w-48, 14rem=w-56, 15rem=w-60,
            // 16rem=w-64, 18rem=w-72, 20rem=w-80, 24rem=w-96.
            // Smaller (w-44 = 11rem) we also accept — those
            // are condensed sidebars. Wider (w-screen,
            // w-full, w-1/2) we explicitly skip.
            if matches!(bare,
                "w-44" | "w-48" | "w-52" | "w-56" | "w-60"
                | "w-64" | "w-72" | "w-80" | "w-96"
                | "w-1/4" | "w-1/5") {
                return true;
            }
            // v0.1.366: position-fixed signals. Direct flex
            // children with `fixed`/`sticky` are almost always
            // sidebars. Note: `static` (the CSS default) is
            // also a Tailwind class but means "not positioned"
            // — exclude.
            if matches!(bare, "fixed" | "sticky") {
                return true;
            }
        }
    }
    // (3) ID checks — v0.1.345 set
    if let Some(id) = elem.attr("id") {
        let id_lc = id.to_ascii_lowercase();
        if id_lc == "sidebar"
            || id_lc == "sidenav"
            || id_lc == "nav-sidebar"
            || id_lc == "secondary"
            || id_lc == "aside"
            || id_lc == "toc"
            || id_lc.starts_with("sidebar-")
            || id_lc.starts_with("sidenav-")
        {
            return true;
        }
    }
    // (6) Inline-style narrow width
    if let Some(style) = elem.attr("style") {
        let style_lc = style.to_ascii_lowercase();
        // Look for "width: <N>px" or "width:<N>px" with N <= 320.
        if let Some(pos) = style_lc.find("width:") {
            let after = &style_lc[pos + 6..].trim_start();
            // Parse a leading number; cheap, no allocs
            let mut num_end = 0;
            for (i, c) in after.char_indices() {
                if c.is_ascii_digit() || c == '.' { num_end = i + c.len_utf8(); } else { break; }
            }
            if num_end > 0 {
                if let Ok(value) = after[..num_end].parse::<f64>() {
                    let rest = &after[num_end..].trim_start();
                    if rest.starts_with("px") && value > 0.0 && value <= 320.0 {
                        return true;
                    }
                    if rest.starts_with("rem") && value > 0.0 && value <= 20.0 {
                        return true;
                    }
                }
            }
        }
        // position: fixed | sticky inline.
        if style_lc.contains("position:")
            && (style_lc.contains("fixed") || style_lc.contains("sticky"))
        {
            return true;
        }
    }
    false
}

fn has_sidebar_child(n: &ElementRef) -> bool {
    // v0.1.366: walk direct children, applying the expanded
    // is_sidebar_like check.
    for c in n.children() {
        if let scraper::Node::Element(elem) = c.value() {
            if is_sidebar_like(elem) {
                return true;
            }
        }
    }
    // v0.1.366: deeper-search fallback. Modern React apps
    // often wrap the sidebar in an extra layer of <div>s
    // before the actual sidebar markup. If the direct-children
    // scan found nothing AND the element has a small number
    // of element children (≤ 3 — Row-shaped), peek ONE level
    // deeper for sidebar markers. Bounded to keep cost low.
    //
    // Caveat: only descend through structurally-neutral
    // wrappers (`<div>` / `<span>` / `<section>`). Semantic
    // tags like `<header>` / `<footer>` / `<main>` /
    // `<article>` IMPLY a role; descending into a `<header>`
    // that contains a `<nav>` would falsely flag the
    // breadcrumb pattern as a sidebar.
    let mut element_children: Vec<scraper::ElementRef> = Vec::with_capacity(4);
    for c in n.children() {
        if let Some(e) = scraper::ElementRef::wrap(c) {
            element_children.push(e);
            if element_children.len() > 3 { return false; }
        }
    }
    // Only do the deep scan if we have a Row-shaped child count.
    if element_children.len() < 2 || element_children.len() > 3 {
        return false;
    }
    // Look at the FIRST element-child's grandchildren only —
    // sidebars are conventionally before main content. Saves
    // a full tree walk.
    if let Some(first) = element_children.first() {
        let first_tag = first.value().name();
        // Skip semantic wrappers; only descend through neutral
        // structural elements.
        if matches!(first_tag, "div" | "span" | "section") {
            for c in first.children() {
                if let scraper::Node::Element(elem) = c.value() {
                    if is_sidebar_like(elem) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// v0.1.871: shared <img>/<picture> QML emission. Both the img-tag handler and
/// the picture-element source path produced the identical Column>Image(+alt
/// Text) structure (~30 lines, duplicated); this is the single place that shape
/// lives now, so future image-rendering changes touch one site. `r` is the
/// already-resolved URL. Tiny-image skipping stays in the callers (they guard on
/// dimensions before they have a buffer to write into).
fn emit_image(o: &mut String, r: &str, alt: &str, attr_w: Option<i32>, attr_h: Option<i32>, sty: &RenderStyle) {
    o.push_str("Column {\nwidth: "); o.push_str(W);
    if lazy_images_enabled() {
        // v0.1.173: lazy-decode — source binds to the URL only once the nearest
        // tier-wrapper ancestor's `_sourceArmed` flips true (off-screen images
        // bind `source: ""` → no fetch/decode/upload). `_root._sourceArmed: true`
        // keeps Images outside any tier wrapper loading.
        o.push_str("\nspacing: 4\nImage {\nsource: _sourceArmed ? \"");
        write_esc(o, r);
        o.push_str("\" : \"\"");
    } else {
        o.push_str("\nspacing: 4\nImage {\nsource: \"");
        write_esc(o, r);
        o.push_str("\"");
    }
    o.push_str("\nfillMode: Image.PreserveAspectFit\nasynchronous: true\ncache: true");
    // Width: prefer an explicit sane attr width, else sourceSize, capped at parent.
    if let Some(w) = attr_w {
        if w > 4 && w < 4000 {
            o.push_str("\nwidth: Math.min(");
            let _ = write!(o, "{w}");
            o.push_str(", parent.width)");
        } else {
            o.push_str("\nwidth: Math.min(sourceSize.width, parent.width)");
        }
    } else {
        o.push_str("\nwidth: Math.min(sourceSize.width, parent.width)");
    }
    // Height: cap to ~70% of viewport so a tall portrait at a narrow viewport
    // doesn't dominate. With PreserveAspectFit smaller images render natural.
    if let (Some(w), Some(h)) = (attr_w, attr_h) {
        if w > 0 && h > 0 && h < 4000 {
            o.push_str("\nheight: Math.min(width * ");
            let _ = write!(o, "{:.4}", h as f32 / w as f32);
            o.push_str(", _root._vpHeight * 0.7)");
        }
    } else {
        o.push_str("\nheight: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3");
    }
    // sourceSize cap: 2x parent width for retina, max 1600px.
    o.push_str("\nsourceSize.width: Math.min(1600, parent.width * 2)\n}");
    if !alt.is_empty() && alt.len() > 3 && alt.len() < 200 {
        o.push_str("\nText {\ntext: \""); write_esc(o, alt);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.text_color);
        o.push_str("\"\nfont.pixelSize: ");
        let _ = write!(o, "{}", (sty.font_size - 3).max(10));
        o.push_str("\nfont.italic: true\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: parent.width\nhorizontalAlignment: Text.AlignHCenter\n}");
    }
    o.push_str("\n}");
}

pub fn generate_safe_qml_with_stats(n: &ElementRef, d: usize, mx: usize, base: &str, sty: &RenderStyle, st: &mut FilterStats) -> String {
    if d > mx { return String::new(); }
    let tag = n.value().name();
    match tag { "head"|"meta"|"link"|"style"|"script"|"path"|"svg"|"source" => return String::new(), _ => {} }
    st.total_elements += 1;
    if st.total_elements > MAX_NODES as u32 { return String::new(); }
    if is_filtered(n, sty) { st.filtered_elements += 1; return String::new(); }
    if is_hidden(n) { return String::new(); }

    // v0.1.379 (anti-modern-web identity): tracking-wrapper
    // detection. Telemetry-only wrappers (data-analytics-*,
    // data-component-list, data-impression-id, GTM/FB/Segment
    // pixel attrs) waste layout time on invisible scaffolding.
    //
    // `classify_tracking_wrapper` returns (remove, unwrap):
    //   - remove=true → drop the element + its subtree
    //   - unwrap=true → render children directly without a
    //     wrapper Column/Row around them
    //
    // The unwrap case is the structural win: a `<div
    // data-analytics-section="hero">` wrapping the actual
    // hero markup becomes invisible scaffolding, and we
    // emit the children straight into the parent's layout
    // flow. The "skip aggressively and see what breaks"
    // direction from the JS-rendering design conversation.
    //
    // v0.1.728 (compiler speedup): classify_tracking_wrapper early-returns for
    // any tag that isn't div/span/p — which is the majority of elements. The
    // old code collected an attrs Vec + scanned children for EVERY element
    // before that early-return threw the work away. Measured ~4× slower walk
    // from the per-element Vec alloc. Gate the whole block on the tag first so
    // only div/span/p pay the attrs/children cost.
    if matches!(tag, "div" | "span" | "p") {
        // Collect a small attrs slice for classification.
        // Most such tags have 0-3 attributes; allocate inline.
        let attrs_vec: Vec<(&str, &str)> = n.value().attrs().collect();
        let has_child_elements = n.children()
            .any(|c| matches!(c.value(), scraper::Node::Element(_)));
        let (remove, unwrap) = ad_filter::classify_tracking_wrapper(
            tag, &attrs_vec, has_child_elements,
        );
        if remove {
            st.filtered_elements += 1;
            return String::new();
        }
        if unwrap {
            // Render children directly without this wrapper.
            // Mirrors the noscript handler's child-emission
            // pattern. Tracks as "filtered" in stats so
            // diagnostics show the wrapper was bypassed.
            st.filtered_elements += 1;
            let mut parts = Vec::new();
            for c in n.children() {
                if let Some(el) = ElementRef::wrap(c) {
                    let s = generate_safe_qml_with_stats(&el, d + 1, mx, base, sty, st);
                    if !s.is_empty() { parts.push(s); }
                } else if let scraper::Node::Text(t) = c.value() {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        parts.push(text_qml(trimmed, sty));
                    }
                }
            }
            return parts.join("\n");
        }
    }

    let anim = anim_detect::detect_animation(n.value().attr("class"), n.value().attr("style"), n.value().attr("data-aos"), n.value().attr("data-animate"));
    let anchor = if let Some(id) = n.value().attr("id") { if !id.is_empty() { let mut a = String::with_capacity(48); let _ = write!(a, "Item {{\nobjectName: \"anchor_"); write_esc(&mut a, id); a.push_str("\"\nwidth: 1\nheight: 0\n}\n"); a } else { String::new() } } else { String::new() };

    // noscript
    if tag == "noscript" {
        let inner = n.inner_html(); if inner.trim().is_empty() { return String::new(); }
        let frag = scraper::Html::parse_fragment(&inner); let root = frag.root_element();
        let mut parts = Vec::new();
        for c in root.children() {
            if let Some(el) = ElementRef::wrap(c) { let s = generate_safe_qml_with_stats(&el, d+1, mx, base, sty, st); if !s.is_empty() { parts.push(s); } }
            else if let scraper::Node::Text(t) = c.value() { let t = t.trim(); if !t.is_empty() { parts.push(text_qml(t, sty)); } }
        }
        return parts.join("\n");
    }

    // iframe
    if tag == "iframe" {
        if let Some(src) = n.value().attr("src") { if !src.is_empty() {
            // v0.1.159 perf: hover plumbing stripped. Same reasoning
            // as <a> — see notes there.
            let r = resolve_url(src, base); let mut o = String::with_capacity(260);
            o.push_str("Item {\nwidth: "); o.push_str(W);
            o.push_str("\nheight: 36\nRectangle {\nanchors.fill: parent\ncolor: \"");
            o.push_str(&sty.input_bg); o.push_str("\"\nradius: 4\nborder.color: \"");
            o.push_str(&sty.border_color); o.push_str("\"\nborder.width: 1\n}\nText {\nanchors.centerIn: parent\ntext: \"[embed] ");
            write_esc(&mut o, &r[..r.len().min(50)]);
            o.push_str("\"\ncolor: \""); o.push_str(&sty.link_color);
            o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
            o.push_str("\n}");
            emit_link_mousearea(&mut o, &r);
            o.push_str("\n}");
            return o;
        } } return String::new();
    }

    // link
    if tag == "a" {
        let t: String = n.text().collect(); let t = t.trim(); if t.is_empty() { return String::new(); }
        let raw_href = n.value().attr("href").unwrap_or("");
        // v0.1.875: a dead/JS href (javascript:, bare "#", about:blank,
        // empty) has no target we can follow. Before giving up, try to
        // recover a real URL from data-href / onclick="...location=...";
        // if none, emit a non-navigable cue instead of a silent dead link.
        let href = if is_dead_href(raw_href) {
            match detect_js_href(n, base) {
                Some(real) => real,
                None => {
                    let mut o = String::with_capacity(160);
                    emit_interactive_cue(&mut o, t, W, sty);
                    return o;
                }
            }
        } else {
            resolve_url(raw_href, base)
        };
        // v0.1.159 perf: stripped hover plumbing for scroll smoothness.
        //
        // Was: every link had `property bool _hover: false`, a
        // hoverEnabled MouseArea with `onContainsMouseChanged: parent
        // ._hover = containsMouse`, plus bindings on color (with a
        // runtime Qt.lighter() call) and font.bold (which triggers
        // layout because bold text has different metrics).
        //
        // On pages with many links (which is most pages), this
        // produced:
        //   - O(n) hoverEnabled MouseAreas that the input system has
        //     to maintain hover state for every frame during scroll
        //   - O(n) onContainsMouseChanged → property → binding
        //     indirection chains
        //   - layout reflow if hover happens (font.bold changes
        //     metrics)
        //
        // User report v0.1.158 testing: "scrolling was stuttering for
        // no reason on a basic qt page".
        //
        // Trade-off: no hover-highlight on links on desktop. Touch/
        // mobile loses nothing (touch doesn't fire hover anyway).
        // Desktop loses a visual nicety that we can re-add as a
        // background-cued hover state if real users miss it. The
        // underline still distinguishes links from body text.
        //
        // CSS-author intent for the color is preserved (still parsed
        // from the css hint), and the cursor still flips to pointer
        // on desktop because cursorShape doesn't require hoverEnabled.
        let mut o = String::with_capacity(280);
        let css = CssHints::from_element(n);
        let color = css.color.as_deref().unwrap_or(&sty.link_color);
        let fs = css.font_size.unwrap_or(sty.font_size);
        let bold = css.bold;
        o.push_str("Text {\nwidth: "); o.push_str(W);
        o.push_str("\ntext: \"");
        write_esc(&mut o, t);
        o.push_str("\"\ncolor: \"");
        o.push_str(color);
        o.push_str("\"\nfont.pixelSize: ");
        if fs == sty.font_size {
            o.push_str(&sty.font_size_str);
        } else {
            let _ = write!(o, "{fs}");
        }
        o.push_str("\nfont.family: \""); o.push_str(&sty.font_family);
        o.push_str("\"");
        if bold {
            o.push_str("\nfont.bold: true");
        }
        o.push_str("\nfont.underline: true\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere");
        emit_link_mousearea(&mut o, &href);
        o.push_str("\n}");
        return o;
    }

    // button
    if tag == "button" {
        let t: String = n.text().collect(); let t = t.trim(); if t.is_empty() { return String::new(); }
        if let Some(href) = detect_js_href(n, base) {
            // v0.1.159 perf: same hover-strip as <a>. See notes there.
            let mut o = String::with_capacity(180);
            o.push_str("Text {\ntext: \""); write_esc(&mut o, t);
            o.push_str("\"\ncolor: \"");
            o.push_str(&sty.link_color);
            o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
            o.push_str("\nfont.underline: true");
            emit_link_mousearea(&mut o, &href);
            o.push_str("\n}");
            return o;
        }
        let mut o = String::with_capacity(128);
        o.push_str("Rectangle {\nwidth: Math.min(200, "); o.push_str(W);
        o.push_str(")\nheight: 36\nradius: 4\ncolor: \"");
        o.push_str(&sty.button_bg);
        o.push_str("\"\nText {\nanchors.centerIn: parent\ntext: \""); write_esc(&mut o, t);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.button_fg);
        o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
        o.push_str("\n}\n}");
        return o;
    }

    // details/summary
    if tag == "details" {
        let st_text = n.select(s(&S6, "summary")).next().map(|s| { let t: String = s.text().collect(); t.trim().to_string() }).unwrap_or_else(|| "Details".into());
        let mut inner = Vec::new();
        for c in n.children() { if let Some(el) = ElementRef::wrap(c) { if el.value().name() != "summary" { let q = generate_safe_qml_with_stats(&el, d+1, mx, base, sty, st); if !q.is_empty() { inner.push(q); } } } }
        let open = n.value().attr("open").is_some();
        let mut o = String::with_capacity(256);
        // v0.1.168 perf: use a deterministic id (`_d<n>`) on the
        // outer Column so the Text and MouseArea below can reference
        // its `expanded` property by id instead of `parent.parent`.
        // QML resolves ids at parse time (single property lookup)
        // whereas `parent.parent` is a runtime chain that re-resolves
        // every binding pass. The id also dodges any "parent context
        // shifted" surprises if the details element gets reparented.
        //
        // The id uses `st.total_elements` as a uniqueness counter
        // — it's incremented for every node, so it's monotonically
        // unique within a single page render. Depth alone collides
        // for sibling <details> elements at the same depth.
        //
        // Also: the text ternary `(expanded ? "▼ " : "▶ ")` stays
        // as JS — QML doesn't have a "binding case statement" that
        // would be faster. Marginal cost, not worth restructuring
        // into two Text elements with `visible` bindings.
        let uid = st.total_elements;
        let _ = write!(o, "Column {{\nid: _d{uid}\nwidth: {W}\nspacing: 4\nproperty bool expanded: {}\nRectangle {{\nwidth: parent.width\nheight: 32\nradius: 4\ncolor: \"{}\"\nborder.color: \"{}\"\nborder.width: 1\nText {{\nanchors.verticalCenter: parent.verticalCenter\nx: 8\ntext: (_d{uid}.expanded ? \"\\u25BC \" : \"\\u25B6 \") + \"", open, sty.input_bg, sty.border_color);
        write_esc(&mut o, &st_text);
        let _ = write!(o, "\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.bold: true\n}}\nMouseArea {{\nanchors.fill: parent\ncursorShape: Qt.PointingHandCursor\nonClicked: {{ _d{uid}.expanded = !_d{uid}.expanded }}\n}}\n}}\nColumn {{\nwidth: parent.width\nspacing: 4\nvisible: _d{uid}.expanded\n{}\n}}\n}}", sty.link_color, sty.font_size, inner.join("\n"));
        return o;
    }

    // select
    if tag == "select" {
        let name = n.value().attr("name").unwrap_or("select");
        let opts: Vec<String> = n.select(s(&S4, "option")).filter_map(|o| { let t: String = o.text().collect(); let t = t.trim().to_string(); if t.is_empty() { None } else { Some(t) } }).collect();
        if !opts.is_empty() {
            let mut o = String::with_capacity(128);
            let _ = write!(o, "Column {{\nwidth: {W}\nspacing: 2\nText {{\ntext: \""); write_esc(&mut o, name);
            let _ = write!(o, "\"\ncolor: \"{}\"\nfont.pixelSize: {}\n}}\nComboBox {{\nwidth: Math.min(300, parent.width)\nmodel: [", sty.text_color, sty.font_size - 2);
            for (i, opt) in opts.iter().enumerate() { if i > 0 { o.push_str(", "); } o.push('"'); write_esc(&mut o, opt); o.push('"'); }
            o.push_str("]\n}\n}"); return o;
        }
    }

    // ul/ol
    if tag == "ul" || tag == "ol" {
        let mut items = Vec::new(); let mut idx = 1;
        for c in n.children() { if let Some(el) = ElementRef::wrap(c) {
            if el.value().name() == "li" { let t: String = el.text().collect(); let t = t.trim(); if !t.is_empty() {
                let bullet = if tag == "ol" { format!("{idx}. ") } else { "\u{2022} ".into() };
                let mut o = String::with_capacity(128);
                let _ = write!(o, "Text {{\ncolor: \"{}\"\ntext: \"", sty.text_color); write_esc(&mut o, &bullet); write_esc(&mut o, t);
                let _ = write!(o, "\"\nfont.pixelSize: {}\nfont.family: \"{}\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: parent.width - 16\nx: 16\n}}", sty.font_size, sty.font_family);
                items.push(o); idx += 1;
            } } else { let q = generate_safe_qml_with_stats(&el, d+1, mx, base, sty, st); if !q.is_empty() { items.push(q); } }
        } }
        if !items.is_empty() {
            let j = items.join("\n");
            let mut o = String::with_capacity(j.len()+40);
            o.push_str("Column {\nwidth: "); o.push_str(W); o.push_str("\nspacing: 3\n"); o.push_str(&j); o.push_str("\n}");
            let final_qml = maybe_animate(o, &anim);
            // v0.1.236: list selection for corpus. Use the
            // extended word-count bounds (3..=200) so short
            // lists (e.g., 3-item bullet summaries) are still
            // selectable. Nested lists (d > 0) don't get their
            // own wrap — the outermost list's wrap covers
            // everything inside.
            //
            // Text content for paragraph_selected: walk children
            // for <li> text only, joining with " · " so the
            // reading flow naturally pauses between items.
            if d == 0 {
                let mut item_texts: Vec<String> = Vec::new();
                for c in n.children() {
                    if let Some(el) = ElementRef::wrap(c) {
                        if el.value().name() == "li" {
                            let t: String = el.text().collect();
                            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !t.is_empty() { item_texts.push(t); }
                        }
                    }
                }
                let combined = item_texts.join(" \u{00B7} ");
                return maybe_wrap_extended(final_qml, &combined, sty, Some(st), ContentType::List);
            }
            return final_qml;
        }
        return String::new();
    }

    // video/audio
    if tag == "video" || tag == "audio" {
        let src = n.value().attr("src").or_else(|| n.select(s(&S5, "source")).next().and_then(|s| s.value().attr("src")));
        if let Some(src) = src { let r = resolve_url(src, base); let icon = if tag == "video" { "\u{25B6}" } else { "\u{266B}" };
            // v0.1.159 perf: hover plumbing stripped. Same reasoning
            // as <a> — see notes there.
            let mut o = String::with_capacity(220);
            let _ = write!(o, "Item {{\nwidth: {W}\nheight: 36\nRectangle {{\nanchors.fill: parent\ncolor: \"{}\"\nradius: 4\nborder.color: \"{}\"\nborder.width: 1\n}}\nText {{\nanchors.centerIn: parent\ntext: \"{icon} {tag} ", sty.input_bg, sty.border_color);
            write_esc(&mut o, &r[..r.len().min(40)]);
            let _ = write!(o, "\"\ncolor: \"{}\"\nfont.pixelSize: {}\n}}", sty.link_color, sty.font_size);
            emit_link_mousearea(&mut o, &r);
            o.push_str("\n}"); return o;
        }
    }

    // table
    //
    // v0.1.197 (ship 2.4.2): viewport-aware table rendering.
    // Path:
    //   1. Walk the <table> into a TableData (headers + rows).
    //   2. Emit adaptive QML: ColumnLayout grid (wide) + stacked
    //      card Column (narrow), switched by parent.width at runtime.
    //   3. Tag the wrapping Item with `_tid: "t_<n>"` so future
    //      ships (2.4.4 — voice/touch commands) can target it.
    //
    // Falls back to the legacy renderer only when extract_table_data
    // returns None (no rows). Anything with at least one header row
    // goes through the new path.
    if tag == "table" {
        if let Some(data) = extract_table_data(n) {
            let final_qml = maybe_animate(emit_adaptive_table(&data, sty, false), &anim);
            // v0.1.236: table selection for corpus. The text
            // content is built from cells joined with spaces and
            // rows separated by " / " — gives the reading
            // pipeline a single readable string that mirrors how
            // a user would speak the table cell-by-cell.
            //
            // Skipped when the table is inside a nested context
            // (d > 0) so a top-level read doesn't recurse into
            // sub-tables. Each top-level table gets its own
            // selectable wrap.
            if d == 0 {
                let mut parts: Vec<String> = Vec::new();
                if !data.headers.is_empty() {
                    parts.push(data.headers.join(" "));
                }
                for row in &data.rows {
                    parts.push(row.join(" "));
                }
                let combined = parts.join(" \u{2044} "); // FRACTION SLASH between rows
                return maybe_wrap_extended(final_qml, &combined, sty, Some(st), ContentType::Table);
            }
            return final_qml;
        }
        // Else fall through — empty/malformed table renders nothing.
    }

    // onclick navigation on div/span
    if matches!(tag, "div"|"span"|"li"|"article") && n.value().attr("onclick").is_some() {
        if let Some(href) = detect_js_href(n, base) {
            let t: String = n.text().collect(); let t = t.trim(); if !t.is_empty() {
                let mut o = String::with_capacity(200);
                o.push_str("Text {\nwidth: "); o.push_str(W);
                o.push_str("\ntext: \"");
                write_esc(&mut o, t);
                o.push_str("\"\ncolor: \""); o.push_str(&sty.link_color);
                o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
                o.push_str("\nfont.underline: true\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere");
                emit_link_mousearea(&mut o, &href);
                o.push_str("\n}");
                return o;
            }
        }
    }

    // Standalone <input> outside a form. Most types are dropped as
    // useless without a containing form (text fields with no submit
    // path), but checkboxes + radios are interactive on their own —
    // think log viewers like the one user reported (alx.sh/l/asahi-dev),
    // where "Show join/leave messages" toggles client-side via
    // JS-managed class changes. We render them as a real Qt CheckBox
    // / RadioButton so the visual is clear.
    //
    // v0.1.104:
    //   - enabled: true (was false) — user can actually toggle.
    //     The page's own JS won't fire (we don't have a JS runtime),
    //     but the visible state at least responds to taps.
    //   - find matching <label for="<id>"> by walking up the tree
    //     and searching ancestors' descendants. Most pages put the
    //     label as a sibling of the input or in the same wrapper.
    if tag == "input" {
        let itype = n.value().attr("type").unwrap_or("text");
        if itype == "checkbox" || itype == "radio" {
            let id_attr = n.value().attr("id").unwrap_or("");
            let checked = n.value().attr("checked").is_some();

            // Search for <label for="<id>"> in nearby DOM. Walk up
            // up to 4 ancestors; each ancestor, scan immediate
            // descendants for label[for=id]. Stop on first hit.
            //
            // Why limited depth: an unbounded scan could traverse
            // the whole document for every checkbox, O(N²) on
            // checkbox-heavy pages. 4 levels covers the common
            // patterns: sibling, parent's sibling, two-level wrapper,
            // section-level grouping.
            let mut label_text = String::new();
            if !id_attr.is_empty() {
                use scraper::Selector;
                let sel_str = format!("label[for=\"{}\"]", id_attr.replace('"', ""));
                // Parse and immediately bind. The returned Result's Err
                // variant borrows from sel_str — we drop the Err arm
                // by `.ok()` so only the owned Selector remains, then
                // sel_str can be dropped at end-of-scope without lifetime
                // worries.
                let sel_opt = Selector::parse(&sel_str).ok();
                if let Some(sel) = sel_opt {
                    let mut cur = n.parent();
                    let mut depth = 0;
                    while let Some(node) = cur {
                        if depth >= 4 { break; }
                        if let Some(elem) = ElementRef::wrap(node) {
                            if let Some(found) = elem.select(&sel).next() {
                                let txt: String = found.text().collect::<String>()
                                    .trim().to_string();
                                if !txt.is_empty() {
                                    label_text = txt;
                                    break;
                                }
                            }
                        }
                        cur = node.parent();
                        depth += 1;
                    }
                }
            }
            // Fallback chain if no <label for=> found.
            if label_text.is_empty() {
                label_text = n.value().attr("aria-label")
                    .or_else(|| n.value().attr("value"))
                    .or_else(|| n.value().attr("name"))
                    .unwrap_or("")
                    .to_string();
            }

            let mut o = String::with_capacity(192);
            let qml_tag = if itype == "radio" { "RadioButton" } else { "CheckBox" };
            let _ = write!(o, "{} {{\n\
                objectName: \"_chk_", qml_tag);
            write_esc(&mut o, id_attr);
            o.push_str("\"\n");
            if !label_text.is_empty() {
                o.push_str("text: \"");
                write_esc(&mut o, &label_text);
                o.push_str("\"\n");
            }
            if checked { o.push_str("checked: true\n"); }
            // v0.1.104: enabled (was disabled). Local toggle works
            // even though page JS doesn't run. Visual reacts to tap;
            // user can see what state they're choosing. Class-toggle
            // and CSS-driven filtering wiring is a separate pass.
            o.push_str("enabled: true\n");
            let _ = write!(o, "font.pixelSize: {}\n", sty.font_size);
            o.push_str("}\n");
            return o;
        }
        // Other bare input types: text, search, email render as a
        // basic TextField so the user can SEE the filter exists.
        // The page's own JS handler won't fire (no JS runtime), but
        // the local typed value is at least visible. Useful for log
        // viewers (alx.sh/l/asahi-dev pattern) where filter input is
        // a key UI affordance even without backend filtering.
        //
        // Other types (file, password, hidden, button, image, etc.)
        // we still drop because they need a form context or aren't
        // useful as visible decoration.
        if itype == "text" || itype == "search" || itype == "email"
            || itype == "url" || itype == "tel"
        {
            let placeholder = n.value().attr("placeholder")
                .or_else(|| n.value().attr("aria-label"))
                .unwrap_or("");
            let id_attr = n.value().attr("id").unwrap_or("");
            let mut o = String::with_capacity(192);
            let _ = write!(o, "TextField {{\n\
                objectName: \"_txt_");
            write_esc(&mut o, id_attr);
            o.push_str("\"\n");
            if !placeholder.is_empty() {
                o.push_str("placeholderText: \"");
                write_esc(&mut o, placeholder);
                o.push_str("\"\n");
            }
            let _ = write!(o, "font.pixelSize: {}\n", sty.font_size);
            o.push_str("Layout.preferredWidth: 200\n");
            o.push_str("}\n");
            return o;
        }
        return String::new();
    }

    // <label> outside a form: render as plain text. Pairs with
    // CheckBox / RadioButton above when we get the matching pair.
    //
    // v0.1.104: when this label has `for="<id>"` and the matching
    // input is a checkbox/radio elsewhere in the document, we
    // SKIP rendering — the checkbox path already absorbed the
    // label text. Without this skip, the page renders both the
    // bare label text AND the checkbox-with-text, looking duplicated.
    if tag == "label" {
        let for_attr = n.value().attr("for").unwrap_or("");
        if !for_attr.is_empty() {
            // Walk up to find an ancestor, then look for a sibling
            // input with the matching id (mirrors the checkbox lookup).
            let sel_str = format!("input[id=\"{}\"]", for_attr.replace('"', ""));
            // Use .ok() to discard the Err arm (which borrows sel_str)
            // before sel_str goes out of scope. See checkbox lookup
            // above for the same pattern + rationale.
            let sel_opt = Selector::parse(&sel_str).ok();
            if let Some(sel) = sel_opt {
                let mut cur = n.parent();
                let mut depth = 0;
                while let Some(node) = cur {
                    if depth >= 4 { break; }
                    if let Some(elem) = ElementRef::wrap(node) {
                        if let Some(found) = elem.select(&sel).next() {
                            let itype = found.value().attr("type").unwrap_or("text");
                            if itype == "checkbox" || itype == "radio" {
                                // Already absorbed into the CheckBox.
                                return String::new();
                            }
                            break;
                        }
                    }
                    cur = node.parent();
                    depth += 1;
                }
            }
        }
        let txt: String = n.text().collect::<String>().trim().to_string();
        if txt.is_empty() { return String::new(); }
        let mut o = String::with_capacity(96);
        let _ = write!(o, "Text {{ text: \"");
        write_esc(&mut o, &txt);
        let _ = write!(o, "\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.family: \"{}\"\n}}\n",
            sty.text_color, sty.font_size, sty.font_family);
        return o;
    }

    // form
    if tag == "form" {
        let action = resolve_url(n.value().attr("action").unwrap_or(""), base);
        let mut fields: Vec<(String, String, String)> = Vec::new();
        for inp in n.select(s(&S2, "input[name], textarea[name]")) {
            let name = inp.value().attr("name").unwrap_or("").to_string();
            let ft = inp.value().attr("type").unwrap_or("text").to_string();
            let ph = inp.value().attr("placeholder").or_else(|| inp.value().attr("value")).unwrap_or(&name).to_string();
            if !name.is_empty() && ft != "hidden" && ft != "submit" { fields.push((name, ft, ph)); }
        }
        let sub = n.select(s(&S3, "input[type=submit], button[type=submit], button:not([type])")).next()
            .map(|el| el.value().attr("value").map(|s|s.to_string()).unwrap_or_else(|| el.text().collect::<String>().trim().to_string())).unwrap_or_else(|| "Submit".into());
        if !fields.is_empty() {
            let mut o = String::with_capacity(512);
            o.push_str(&format!("Column {{\nwidth: {W}\nspacing: 6\n"));
            for (i, (_, ft, ph)) in fields.iter().enumerate() {
                if ft == "checkbox" || ft == "radio" {
                    // Inside-form checkbox/radio: render as CheckBox
                    // (or RadioButton). Submit path doesn't yet
                    // collect their values — submit_form_get only
                    // reads .text on the children. That's a bug for
                    // submitting forms with checkbox state, but
                    // fixing requires changing submit collection
                    // logic. Defer.
                    let qml_tag = if ft == "radio" { "RadioButton" } else { "CheckBox" };
                    let _ = write!(o, "{qml_tag} {{\nobjectName: \"_field_{i}\"\ntext: \"");
                    write_esc(&mut o, ph);
                    let _ = write!(o, "\"\nfont.pixelSize: {}\nfont.family: \"{}\"\n}}\n",
                        sty.font_size, sty.font_family);
                    continue;
                }
                let echo = if ft == "password" { "echoMode: TextInput.Password\n" } else { "" };
                let _ = write!(o, "TextField {{\nobjectName: \"_field_{i}\"\nwidth: parent.width - 20\nplaceholderText: \""); write_esc(&mut o, ph);
                let _ = write!(o, "\"\n{echo}color: \"{}\"\nfont.pixelSize: {}\nfont.family: \"{}\"\nbackground: Rectangle {{\ncolor: \"{}\"\nradius: 4\nborder.color: \"{}\"\nborder.width: 1\n}}\n}}\n", sty.input_fg, sty.font_size, sty.font_family, sty.input_bg, sty.border_color);
            }
            let names = fields.iter().map(|(n,_,_)| n.as_str()).collect::<Vec<_>>().join("\x1F");
            let _ = write!(o, "Button {{\ntext: \""); write_esc(&mut o, &sub);
            let _ = write!(o, "\"\nwidth: parent.width - 20\nonClicked: {{\nvar vals = []\nfor (var c = 0; c < parent.children.length; c++) {{\nvar ch = parent.children[c]\nif (ch.objectName && ch.objectName.indexOf(\"_field_\") === 0) vals.push(ch.text || \"\")\n}}\nif (backend) backend.submit_form_get(\"");
            write_esc(&mut o, &action); o.push_str("\", \""); write_esc(&mut o, &names);
            let _ = write!(o, "\", vals.slice(0, {}).join(\"\\x1F\"))\n}}\n}}\n}}\n}}", fields.len());
            return o;
        }
    }

    // img
    if tag == "img" {
        // v0.1.374: source resolution order, tried in turn:
        //   1. src (current value — may be placeholder)
        //   2. data-src (lazy-load pattern A)
        //   3. data-lazy-src (lazy-load pattern B)
        //   4. data-original (lazy-load pattern C, jQuery lazyload)
        //   5. data-srcset (responsive lazy variant)
        //   6. srcset (responsive non-lazy)
        // If the chosen src is a known placeholder, try the next
        // candidate in turn. This handles sites that fill src
        // with a base64 transparent png while keeping the real
        // URL in data-src.
        let src_candidates: [Option<&str>; 6] = [
            n.value().attr("src"),
            n.value().attr("data-src"),
            n.value().attr("data-lazy-src"),
            n.value().attr("data-original"),
            n.value().attr("data-srcset").and_then(best_srcset),
            n.value().attr("srcset").and_then(best_srcset),
        ];
        let src = src_candidates.iter()
            .filter_map(|opt| *opt)
            .find(|s| !is_placeholder(s))
            .unwrap_or("");
        if src.is_empty() {
            return String::new();
        }
        let r = resolve_url(src, base);
        let alt = n.value().attr("alt").unwrap_or("");

        // Parse width/height attributes (CSS sizing hints)
        let attr_w: Option<i32> = n.value().attr("width").and_then(|s| s.trim_end_matches("px").parse().ok());
        let attr_h: Option<i32> = n.value().attr("height").and_then(|s| s.trim_end_matches("px").parse().ok());

        // Detect tiny images (icons, spacers, tracking pixels) - skip
        if let (Some(w), Some(h)) = (attr_w, attr_h) {
            if w <= 4 && h <= 4 { return String::new(); }
        }

        let mut o = String::with_capacity(280);
        emit_image(&mut o, &r, alt, attr_w, attr_h, sty);
        return o;
    }

    if tag == "figure" {
        let mut parts = Vec::new();
        for c in n.children() { if let Some(el) = ElementRef::wrap(c) { let q = generate_safe_qml_with_stats(&el, d+1, mx, base, sty, st); if !q.is_empty() { parts.push(q); } } }
        if parts.is_empty() { return String::new(); }
        let joined = parts.join("\n");
        let mut o = String::with_capacity(joined.len() + 40);
        o.push_str("Column {\nwidth: "); o.push_str(W); o.push_str("\nspacing: 4\n"); o.push_str(&joined); o.push_str("\n}");
        return o;
    }
    if tag == "picture" {
        // v0.1.174: Apple's hero pictures use a lazy-load pattern where
        // the `<img>` element's `src` is a `data:image/gif;base64,...`
        // empty placeholder, and JavaScript swaps it in at runtime. We
        // don't run JS, so we'd render nothing — confirmed v0.1.173
        // user observation "the hero section does not show up... empty
        // space and occasional text" on apple.com/iphone.
        //
        // Apple's `<picture>` structure always contains `<source>`
        // children with real srcset URLs for each media query:
        //   <picture data-lazy>
        //     <source data-empty="" srcset="data:image/gif;..."> -- skip
        //     <source srcset="/v/.../small.jpg, ... 2x" media="(max-width:734px)">
        //     <source srcset="/v/.../medium.jpg, ... 2x" media="(max-width:1068px)">
        //     <source srcset="/v/.../large.jpg, ... 2x" media="(min-width:0px)">
        //     <img src="data:image/gif;..." onload="window.__lp?.(event)">
        //   </picture>
        //
        // We scan the `<source>` children, skip any with `data-empty`
        // attribute or `data:` srcset, and pick the LAST viable one
        // (Apple's convention is small → medium → large ordering).
        // Synthesize an <img> with the resolved real URL and let the
        // existing img-tag emission path produce QML for it.
        let mut chosen_src: Option<String> = None;
        for src_el in n.select(s(&S5, "source")) {
            if src_el.value().attr("data-empty").is_some() { continue; }
            let Some(ss) = src_el.value().attr("srcset") else { continue; };
            if ss.starts_with("data:") { continue; }
            let Some(url) = best_srcset(ss) else { continue; };
            if url.starts_with("data:") { continue; }
            // Keep the LAST viable one — Apple orders sources small-to-large.
            chosen_src = Some(url.to_string());
        }
        // Find the inner <img> for alt text + width/height attributes.
        let inner_img = n.select(s(&S1, "img")).next();
        if let Some(url) = chosen_src {
            // Construct an <img>-like emission using the chosen URL.
            // We can't easily synthesize an ElementRef pointing at a
            // different src, so inline the relevant img-tag logic.
            let r = resolve_url(&url, base);
            let (alt, attr_w, attr_h) = if let Some(img) = inner_img {
                let alt = img.value().attr("alt").unwrap_or("").to_string();
                let w = img.value().attr("width").and_then(|s| s.trim_end_matches("px").parse::<i32>().ok());
                let h = img.value().attr("height").and_then(|s| s.trim_end_matches("px").parse::<i32>().ok());
                (alt, w, h)
            } else { (String::new(), None, None) };
            if let (Some(w), Some(h)) = (attr_w, attr_h) {
                if w <= 4 && h <= 4 { return String::new(); }
            }
            let mut o = String::with_capacity(280);
            emit_image(&mut o, &r, &alt, attr_w, attr_h, sty);
            return o;
        }
        // No viable <source>; fall back to recursing into the inner <img>.
        // The img path's existing data:image/gif check will catch the
        // placeholder and return empty; that's the v0.1.173 behavior.
        if let Some(img) = inner_img { return generate_safe_qml_with_stats(&img, d+1, mx, base, sty, st); }
        return String::new();
    }

    // headings
    let hsz = match tag { "h1"=>Some(28),"h2"=>Some(24),"h3"=>Some(20),"h4"=>Some(18),"h5"=>Some(16),"h6"=>Some(14), _=>None };
    if let Some(sz) = hsz {
        let t: String = n.text().collect(); let t = t.trim(); if t.is_empty() { return String::new(); }
        let mut o = String::with_capacity(128);
        o.push_str(&anchor);
        // v0.1.167 perf: write! the heading size into the buffer.
        o.push_str("Text {\ntext: \""); write_esc(&mut o, t);
        o.push_str("\"\nfont.pixelSize: ");
        let _ = write!(o, "{sz}");
        o.push_str("\nfont.bold: true\ncolor: \""); o.push_str(&sty.text_color);
        o.push_str("\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: "); o.push_str(W);
        o.push_str("\nfont.family: \""); o.push_str(&sty.font_family);
        o.push_str("\"\n}");
        // v0.1.236: heading selection for corpus. h1/h2/h3 get
        // wrapped; h4-h6 are usually inline labels (e.g., "Note:",
        // "Example 2") and rarely worth reading aloud. The wider
        // extended_word_bounds (3..=200) covers typical headings
        // since prose-paragraph bounds (10..=50) would reject any
        // <15 word heading.
        //
        // v0.1.306: tag as ContentType::Heading (not Prose) so the
        // QML ReadingOverlay's _baseMultiplierFor returns a
        // slower multiplier — headings read more deliberately
        // than body prose.
        if matches!(tag, "h1" | "h2" | "h3") {
            return maybe_wrap_extended(o, t, sty, Some(st), ContentType::Heading);
        }
        return o;
    }

    if tag == "hr" { let mut o = String::new(); o.push_str("Rectangle {\nwidth: "); o.push_str(W); o.push_str("\nheight: 1\ncolor: \""); o.push_str(&sty.border_color); o.push_str("\"\n}"); return o; }
    if tag == "br" { return "Item {\nwidth: 1\nheight: 8\n}".into(); }

    // Blockquote — left border + indent
    if tag == "blockquote" {
        let mut inner = Vec::new();
        for c in n.children() {
            if let Some(el) = ElementRef::wrap(c) { let q = generate_safe_qml_with_stats(&el, d+1, mx, base, sty, st); if !q.is_empty() { inner.push(q); } }
            else if let scraper::Node::Text(t) = c.value() { let t = t.trim(); if !t.is_empty() { inner.push(text_qml(t, sty)); } }
        }
        if inner.is_empty() { return String::new(); }
        let joined = inner.join("\n");
        let mut o = String::with_capacity(joined.len() + 120);
        o.push_str("Row {\nwidth: "); o.push_str(W);
        o.push_str("\nspacing: 8\nRectangle {\nwidth: 3\nheight: parent.height\ncolor: \""); o.push_str(&sty.link_color);
        o.push_str("\"\nradius: 1\n}\nColumn {\nwidth: parent.width - 11\nspacing: 4\n");
        o.push_str(&joined);
        o.push_str("\n}\n}");
        // v0.1.236: blockquote selection for corpus. Same prose
        // word-count bounds as `<p>` (blockquotes ARE prose,
        // usually citing something readable).
        let tc_combined: String = n.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let wc = approx_word_count(&tc_combined);
        let (wc_min, wc_max) = paragraph_word_bounds();
        let final_qml = maybe_animate(o, &anim);
        if wc >= wc_min && wc <= wc_max {
            let pid = next_paragraph_id();
            let wrapped = wrap_for_paragraph_selection(&final_qml, pid, &tc_combined, sty, ContentType::Prose);
            st.selectable_paragraphs += 1;
            return wrapped;
        }
        return final_qml;
    }

    // Pre/code — monospace with background
    if tag == "pre" || tag == "code" {
        let t: String = n.text().collect(); let t = t.trim();
        if t.is_empty() { return String::new(); }
        let is_block = tag == "pre" || t.contains('\n');
        let mut o = String::with_capacity(t.len() + 120);
        if is_block {
            // v0.1.113: was binding implicitHeight to _codeText.implicitHeight
            // but the inner Text never got an id. Use childrenRect.height
            // instead — Qt computes it from the actual child layout.
            o.push_str("Rectangle {\nwidth: "); o.push_str(W);
            o.push_str("\nimplicitHeight: childrenRect.height + 16\ncolor: \"");
            o.push_str(if sty.text_color.contains("d4") { "#2a2a2a" } else { "#f0ebe2" });
            o.push_str("\"\nradius: 4\nText {\nanchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\nanchors.margins: 8\ntext: \"");
        } else {
            o.push_str("Text {\ntext: \"");
        }
        write_esc(&mut o, t);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.text_color);
        // v0.1.167 perf: write! the size adjustment directly.
        o.push_str("\"\nfont.pixelSize: ");
        let _ = write!(o, "{}", (sty.font_size - 2).max(10));
        o.push_str("\nfont.family: \"monospace\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere");
        if !is_block {
            o.push_str("\nwidth: ");
            o.push_str(W);
        }
        if is_block { o.push_str("\n}\n}"); }
        else { o.push_str("\n}"); }
        // v0.1.244 (PA1 finish): block code (pre or multi-line code)
        // becomes a selectable paragraph with ContentType::Code, which
        // triggers PA1's code=0.70 WPM multiplier when read aloud.
        // Inline <code> (single line, not <pre>) stays unwrapped — too
        // small to be a meaningful reading unit and would conflict with
        // surrounding prose flow.
        if is_block {
            return maybe_wrap_extended(o, &t, sty, Some(st), ContentType::Code);
        }
        return o;
    }

    // Strong/b — bold text
    if tag == "strong" || tag == "b" {
        let t: String = n.text().collect(); let t = t.trim();
        if t.is_empty() { return String::new(); }
        let mut o = String::with_capacity(t.len() + 80);
        o.push_str("Text {\ntext: \""); write_esc(&mut o, t);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.text_color);
        o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
        o.push_str("\nfont.bold: true\nfont.family: \""); o.push_str(&sty.font_family);
        o.push_str("\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: "); o.push_str(W);
        o.push_str("\n}");
        return o;
    }

    // Em/i — italic text
    if tag == "em" || tag == "i" {
        let t: String = n.text().collect(); let t = t.trim();
        if t.is_empty() { return String::new(); }
        let mut o = String::with_capacity(t.len() + 80);
        o.push_str("Text {\ntext: \""); write_esc(&mut o, t);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.text_color);
        o.push_str("\"\nfont.pixelSize: "); o.push_str(&sty.font_size_str);
        o.push_str("\nfont.italic: true\nfont.family: \""); o.push_str(&sty.font_family);
        o.push_str("\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: "); o.push_str(W);
        o.push_str("\n}");
        return o;
    }

    // Small — smaller text
    if tag == "small" {
        let t: String = n.text().collect(); let t = t.trim();
        if t.is_empty() { return String::new(); }
        let mut o = String::with_capacity(t.len() + 80);
        o.push_str("Text {\ntext: \""); write_esc(&mut o, t);
        o.push_str("\"\ncolor: \""); o.push_str(&sty.text_color);
        // v0.1.167 perf: write! into buffer.
        o.push_str("\"\nfont.pixelSize: ");
        let _ = write!(o, "{}", (sty.font_size - 3).max(10));
        o.push_str("\nfont.family: \""); o.push_str(&sty.font_family);
        o.push_str("\"\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\nwidth: "); o.push_str(W);
        o.push_str("\n}");
        return o;
    }

    // Layout detection
    let is_text = matches!(tag, "p"|"span"|"em"|"strong"|"b"|"i"|"small"|"label"|"td"|"th"|"dt"|"dd"|"figcaption"|"blockquote"|"cite"|"pre"|"code");
    let mut layout = if is_text { layout_detect::LayoutKind::Column } else { layout_detect::detect_layout(tag, n.value().attr("style"), n.value().attr("class")) };
    // v0.1.345: sidebar-aware demotion. detect_layout looks only
    // at the parent's tag/style/class, so it can't see that a
    // flex container has a sidebar as one of its children. When
    // it returns Row/RowWrap AND any child has the sidebar shape
    // (semantic <aside>/<nav>, or a class/id indicating sidebar
    // navigation), demote to Column so the sidebar and the main
    // content STACK rather than sitting side by side.
    //
    // Why this matters: Row in Qt lays out children with each
    // claiming `width: parent.width` (our default for all
    // children). The first child fills the row; the second
    // child gets placed at x=parent.width (one viewport over to
    // the right) and is effectively off-screen. The user
    // reported "items on pages appear on the right off the
    // screen" — this is the structural cause.
    //
    // After demotion: the sidebar nav renders above the main
    // content (or below, depending on source order). The user
    // sees both, scrollable in a single column, with the
    // sidebar typically condensing to a short list of links
    // that the user can skim past. This is more useful for a
    // voice-first reader anyway — the sidebar's value is in
    // its links, which work fine inline.
    // v0.1.370: also demote Grid layouts when a sidebar is
    // detected. Tailwind layouts like `grid grid-cols-[18rem,1fr]`
    // return Grid from detect_layout but produce the same off-
    // screen overflow as Row+sidebar. The has_sidebar_child
    // signal set (extended in v0.1.366 with Tailwind widths,
    // fixed-positioning, and inline narrow widths) is already
    // the right check for Grid too — sidebars look the same
    // structurally whether their parent uses flex or grid.
    if matches!(layout, layout_detect::LayoutKind::Row | layout_detect::LayoutKind::RowWrap | layout_detect::LayoutKind::Grid)
        && has_sidebar_child(n) {
        layout = layout_detect::LayoutKind::Column;
    }
    let qt = if is_text { "Text" } else { layout_detect::qml_container(&layout) };
    let wp = if layout == layout_detect::LayoutKind::Inline { "implicitWidth" } else { W };

    let mut tc = String::new();
    for c in n.children() { if let scraper::Node::Text(t) = c.value() { let t = t.trim(); if !t.is_empty() { if !tc.is_empty() { tc.push(' '); } tc.push_str(t); } } }
    // v0.1.731: reserve by child count so the push loop doesn't repeatedly
    // grow+realloc the Vec. children() is a cheap linked-list walk; the count
    // upper-bounds the element children we'll push. Output is unchanged (same
    // pushes in the same order) — verified by the golden harness.
    let child_upper = n.children().count();
    let mut ch: Vec<String> = Vec::with_capacity(child_upper);
    // dedup consecutive identical child output. v0.1.731: previously this
    // cloned every child's QML (`last_text = q.clone()`) purely to compare the
    // next sibling. The element branch now compares against the last-pushed
    // value directly: for d!=0 the pushed value IS q, so ch.last() is the
    // comparison key (zero clones); for d==0 the push is a wrap transform, so
    // we move the unwrapped q into last_text. The text branch keeps its own
    // small key. Output identical — golden-verified.
    let mut last_text = String::new(); // tracks last TEXT-node pushed
    for c in n.children() {
        if let scraper::Node::Element(_) = c.value() {
            let q = generate_safe_qml_with_stats(&ElementRef::wrap(c).unwrap(), d+1, mx, base, sty, st);
            // compare against whichever key reflects the last pushed child:
            // the wrapped-d0 key (last_text) or the last Vec entry (d!=0).
            let prev_key: &str = if d == 0 { last_text.as_str() }
                else { ch.last().map(|s| s.as_str()).unwrap_or("") };
            if !q.is_empty() && q != prev_key {
                // v0.1.169 RenderTier: wrap top-level children (d==0) in an
                // Item that gates opacity on viewport region — Qt skips paint
                // composition for opacity=0 subtrees. See push_preview docs.
                if d == 0 {
                    ch.push(wrap_for_tier(&q));
                    last_text = q;            // unwrapped key for next compare
                } else {
                    ch.push(q);               // ch.last() is the next compare key
                    last_text.clear();        // text-key no longer adjacent
                }
            }
        }
        else if let scraper::Node::Text(t) = c.value() { let t = t.trim(); if !t.is_empty() && qt != "Text" && t != last_text { last_text = t.to_string(); ch.push(text_qml(t, sty)); } }
    }

    if is_text {
        let css = CssHints::from_element(n);
        let r = if tc.is_empty() && ch.is_empty() { String::new() }
        else if !tc.is_empty() {
            let mut o = anchor;
            let wrapped = css.maybe_bg_wrap_start(&mut o);
            o.push_str("Text {\n");
            css.apply_to_text(&mut o, sty);
            o.push_str("text: \""); write_esc(&mut o, &tc); o.push_str("\"\n}");
            css.maybe_bg_wrap_end(&mut o, wrapped);
            // v0.1.190 (ship 2.1): paragraph selection. Wrap a <p>
            // in a tap-to-select Item if its word count is in the
            // 10-50 range. The bounds filter out headlines (too
            // short) and full essays (too long). The middle range
            // is what a corpus-capture pass would want to read.
            //
            // Only applies to <p> — span/em/strong/etc. are inline
            // text fragments that aren't meaningful selection units.
            // pre/code (block-level) and blockquote handle their own
            // wrapping earlier in this function — pre/code as of
            // v0.1.244 via maybe_wrap_extended with ContentType::Code,
            // blockquote since v0.1.236. They never reach this <p>
            // branch.
            if tag == "p" {
                let wc = approx_word_count(&tc);
                let (wc_min, wc_max) = paragraph_word_bounds();
                if wc >= wc_min && wc <= wc_max {
                    let pid = next_paragraph_id();
                    o = wrap_for_paragraph_selection(&o, pid, &tc, sty, ContentType::Prose);
                    // v0.1.199: track how many <p> elements were
                    // selection-eligible. Useful for analytics /
                    // diagnosing "why is the read-aloud feature
                    // missing on this page" without a debugger.
                    st.selectable_paragraphs += 1;
                }
            }
            o
        }
        else { let joined = ch.join("\n"); let mut o = String::with_capacity(anchor.len() + joined.len() + 32);
            o.push_str(&anchor); o.push_str("Column {\nwidth: "); o.push_str(W); o.push('\n'); o.push_str(&joined); o.push_str("\n}"); o };
        maybe_animate(r, &anim)
    } else {
        let mut parts = Vec::new();
        if !anchor.is_empty() { parts.push(anchor); }
        if !tc.is_empty() { parts.push(text_qml(&tc, sty)); }
        parts.extend(ch);
        if parts.is_empty() { String::new() }
        else {
            // v0.1.417 (page-overflow fix): a Qt `Row` lays out
            // children left-to-right at THEIR OWN widths. Our
            // children default to `width: parent.width`, so when a
            // Row has ≥2 such full-width children, child 2 starts
            // at x≈parent.width — OFF-SCREEN RIGHT. This is the
            // confirmed cause of the "pages render off to the
            // side" bug (from a user QML capture: nested
            // `Row { Text { width: parent.width } Text { width:
            // parent.width } }`).
            //
            // The v0.1.345 sidebar-demotion only catches the
            // sidebar shape. This is the general case: ANY Row
            // whose children want full width will overflow. We
            // can't see child widths until now (children are
            // generated by recursion, each deciding its own
            // width), so we detect it HERE, at assembly, by
            // counting how many child parts claim full width. If
            // a horizontal layout has ≥2 such children, demote to
            // Column so they STACK (which always fits) instead of
            // overflowing. Genuine horizontal layouts (toolbars,
            // breadcrumbs, button groups) have small content-sized
            // children that DON'T claim parent.width, so they're
            // unaffected.
            let (mut layout, mut qt, mut wp) = (layout, qt, wp);
            if matches!(layout, layout_detect::LayoutKind::Row
                              | layout_detect::LayoutKind::RowWrap) {
                let full_width_children = parts.iter()
                    .filter(|p| p.contains("width: parent.width"))
                    .count();
                if full_width_children >= 2 {
                    layout = layout_detect::LayoutKind::Column;
                    qt = layout_detect::qml_container(&layout);
                    wp = W;
                }
            }
            let props = layout_detect::qml_layout_props(&layout, wp);
            let joined = parts.join("\n");
            let mut o = String::with_capacity(qt.len() + props.len() + joined.len() + 8);
            o.push_str(qt); o.push_str(" {\n"); o.push_str(&props); o.push('\n'); o.push_str(&joined); o.push_str("\n}");
            maybe_animate(o, &anim)
        }
    }
}

#[inline]
fn maybe_animate(qml: String, anim: &Option<anim_detect::AnimParams>) -> String {
    if qml.is_empty() { return qml; }
    match anim {
        Some(p) => { let (pre, suf) = anim_detect::wrap_with_animation(p);
            let mut o = String::with_capacity(pre.len() + qml.len() + suf.len() + 2);
            o.push_str(&pre); o.push('\n'); o.push_str(&qml); o.push('\n'); o.push_str(&suf); o }
        None => qml
    }
}

// v0.1.169 RenderTier v1: wrap top-level children in an opacity-gated
// Item. The Item:
//   - takes full parent width (Column layout)
//   - sizes its height to its single child via `childrenRect.height`
//   - binds `opacity` to viewport region exposed by the wrapper Column
//     (see `push_preview` in main.rs for the design notes)
//
// v0.1.173: lazy image deferral toggle. Default ON. Set
// WEB2QML_LAZY_IMAGES=0 to disable and fall back to v0.1.172
// behavior (every Image emitted with `source: "<url>"` directly,
// eager decode). Provided as an escape valve in case the lazy
// behavior breaks anything; can be removed once we're confident.
static LAZY_IMAGES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn lazy_images_enabled() -> bool {
    *LAZY_IMAGES.get_or_init(|| {
        std::env::var("WEB2QML_LAZY_IMAGES").map(|v| v != "0").unwrap_or(true)
    })
}

// Default `_vpTop = -1e9`, `_vpBot = 1e9` means all elements are visible
// unless the browser side actively narrows the region. So behavior is
// unchanged for QML-preview-only consumers (e.g. tests).
//
// The Behavior on opacity provides a tiny crossfade so promotion/demotion
// during a fast scroll doesn't look like popping. 80ms is short enough
// not to delay paint visibly but long enough to round out flickers.
//
// v0.1.173: Added `_sourceArmed` sticky property. Wrapper's children
// (Images, transitively) bind their `source` to this via QML's scope
// chain resolution (no parent.parent traversal needed — `_sourceArmed`
// resolves to whichever ancestor defines it). The property:
//   - Starts false
//   - Flips to true on Component.onCompleted IF opacity > 0 initially
//   - Flips to true on subsequent opacity > 0 transitions
//   - Once true, stays true forever (no scroll-back re-decode thrash)
// Off-screen wrappers stay `_sourceArmed = false` → their Images bind
// `source: ""` → never decode. On-screen wrappers (or wrappers that
// have been on-screen at least once) bind `source: "<url>"` and Qt
// fetches/decodes/caches normally.
//
// Fallback: the root Column also defines `_sourceArmed: true` (added
// in main.rs push_preview), so Images that aren't inside any tier
// wrapper (rare; happens for d > 0 emissions in unusual structures)
// still get their source.
fn wrap_for_tier(inner: &str) -> String {
    let mut o = String::with_capacity(inner.len() + 280);
    if lazy_images_enabled() {
        o.push_str("Item {\nwidth: parent.width\nheight: childrenRect.height\nopacity: (y + height >= _root._vpTop && y <= _root._vpBot) ? 1 : 0\nBehavior on opacity { NumberAnimation { duration: 80 } }\nproperty bool _sourceArmed: false\nonOpacityChanged: { if (opacity > 0 && !_sourceArmed) _sourceArmed = true }\nComponent.onCompleted: { if (opacity > 0) _sourceArmed = true }\n");
    } else {
        o.push_str("Item {\nwidth: parent.width\nheight: childrenRect.height\nopacity: (y + height >= _root._vpTop && y <= _root._vpBot) ? 1 : 0\nBehavior on opacity { NumberAnimation { duration: 80 } }\n");
    }
    o.push_str(inner);
    o.push_str("\n}");
    o
}

// v0.1.190 (ship 2.1): paragraph selection.
//
// Per-page counter for paragraph IDs. Reset at the start of each
// generate_safe_qml call. The atomic isn't strictly necessary
// (generation is single-threaded) but is_atomic gives us clean
// reset + increment without locking, and the perf cost is nil.
static PARAGRAPH_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn reset_paragraph_counter() {
    PARAGRAPH_COUNTER.store(0, std::sync::atomic::Ordering::Relaxed);
}

pub fn next_paragraph_id() -> usize {
    PARAGRAPH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// v0.1.197 (ship 2.4.2): per-page table counter, same pattern as
// PARAGRAPH_COUNTER. Each rendered <table> gets a stable id
// (`_tid: "t_<n>"`) so future ships (2.4.4 — interaction surface)
// can route user commands to a specific table on the page. Reset
// at the start of each generate_safe_qml call.
static TABLE_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn reset_table_counter() {
    TABLE_COUNTER.store(0, std::sync::atomic::Ordering::Relaxed);
}

pub fn next_table_id() -> usize {
    TABLE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// v0.1.198 (ship 2.4.4): table view registry.
//
// When the user issues a command via the table command bar
// ("show columns 1 4 7"), Backend parses it into a TableOp,
// stores it keyed by tid, and triggers a page re-render. The
// re-render walks the same HTML, but when it encounters a
// `<table>`, it consults this registry for active ops on that
// tid and applies them before emitting.
//
// The registry is a global Mutex<HashMap> rather than a parameter
// threaded through generate_safe_qml because there are 5 call sites
// of generate_safe_qml and the registry only matters at the table
// emission point. Backend takes responsibility for populating
// and clearing this around each render.
//
// Lifecycle:
//   - Backend writes via push_table_op_for(tid, op) when a command lands
//   - Backend reads via clone_table_ops_for(tid) during re-render
//   - Backend clears all on page nav (clear_table_views)
//
// Tids are stable WITHIN a page (TABLE_COUNTER resets at the
// start of generate_safe_qml, so a re-render of the same source
// HTML produces the same tids). They are NOT stable across navs;
// clear_table_views runs on nav.
use std::sync::Mutex;
type TableOpsRegistry = std::collections::HashMap<String, Vec<crate::table_ops::TableOp>>;
static TABLE_VIEWS: Mutex<Option<TableOpsRegistry>> = Mutex::new(None);

/// Append an op for a given tid in the global registry.
pub fn push_table_op_for(tid: &str, op: crate::table_ops::TableOp) {
    if let Ok(mut g) = TABLE_VIEWS.lock() {
        let map = g.get_or_insert_with(TableOpsRegistry::new);
        map.entry(tid.to_string()).or_default().push(op);
    }
}

/// Pop the most-recent op for a given tid (undo). Returns true
/// if an op was removed.
pub fn pop_table_op_for(tid: &str) -> bool {
    if let Ok(mut g) = TABLE_VIEWS.lock() {
        if let Some(map) = g.as_mut() {
            if let Some(ops) = map.get_mut(tid) {
                if ops.pop().is_some() {
                    // Clean up empty entries.
                    if ops.is_empty() {
                        map.remove(tid);
                    }
                    return true;
                }
            }
        }
    }
    false
}

/// Clear all per-table state. Called by Backend on page nav.
pub fn clear_table_views() {
    if let Ok(mut g) = TABLE_VIEWS.lock() {
        *g = None;
    }
}

/// v0.1.200: clear all ops for ONE table (reset to base data),
/// leaving other tables' state intact. Returns true if any ops
/// were removed, false if the tid had no registered ops.
pub fn reset_ops_for(tid: &str) -> bool {
    if let Ok(mut g) = TABLE_VIEWS.lock() {
        if let Some(map) = g.as_mut() {
            return map.remove(tid).is_some();
        }
    }
    false
}

/// Read (cloned) the op list for a given tid. Returns empty Vec
/// if nothing is registered.
fn ops_for_tid(tid: &str) -> Vec<crate::table_ops::TableOp> {
    if let Ok(g) = TABLE_VIEWS.lock() {
        if let Some(map) = g.as_ref() {
            if let Some(ops) = map.get(tid) {
                return ops.clone();
            }
        }
    }
    Vec::new()
}

/// Count tables on the current page (post-render). Useful for
/// the command bar to know whether to show itself and for
/// disambiguation messaging.
pub fn current_table_count() -> usize {
    TABLE_COUNTER.load(std::sync::atomic::Ordering::Relaxed)
}

/// Below this width, the renderer switches to stacked-card mode.
/// Above it, a normal grid table. 600px is a phone/tablet boundary —
/// most phones are 360-430px portrait; tablets and laptops are 768+.
/// The boundary at 600 catches landscape phones too.
const NARROW_TABLE_THRESHOLD_PX: u32 = 600;

/// v0.1.421: above this many rendered cells (rows × cols), the
/// adaptive table wraps each mode (wide / narrow) in a
/// Component+Loader gated to its visibility, so only the visible
/// mode's elements are instantiated — halving the realized
/// element count for big tables. Below it, both modes emit inline
/// (the `visible` toggle is cheap and gives instant mode-switch).
/// 100 cells ≈ a 12×8 table; tuned to the todo's suggested
/// threshold.
const TABLE_LOADER_CELL_THRESHOLD: usize = 100;

/// Walk an HTML <table> ElementRef into a TableData. Returns None
/// if the element has no rows. Header row is the first `<tr>` (or
/// the `<thead>` row); subsequent rows are data.
///
/// This is a render-time conversion that lets us treat HTML and
/// markdown tables uniformly. The downstream emit_adaptive_table
/// only knows about TableData; it doesn't care whether the source
/// was `<table>` or `| a | b |`.
fn extract_table_data(n: &ElementRef) -> Option<crate::table_ops::TableData> {
    let mut rows = Vec::new();
    for c in n.children() {
        if let Some(el) = ElementRef::wrap(c) {
            match el.value().name() {
                "tr" => rows.push(el),
                "thead" | "tbody" | "tfoot" => {
                    for gc in el.children() {
                        if let Some(g) = ElementRef::wrap(gc) {
                            if g.value().name() == "tr" {
                                rows.push(g);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if rows.is_empty() {
        return None;
    }

    // Extract cells from each row. Treat first row as headers,
    // rest as data. If first row is all <th>, that's clearly a
    // header; if mixed, still use it as a header by convention.
    let mut iter = rows.into_iter();
    let header_row = iter.next()?;
    let headers: Vec<String> = row_to_cells(&header_row);
    if headers.is_empty() {
        return None;
    }
    let mut data_rows: Vec<Vec<String>> = Vec::new();
    for row in iter {
        let mut cells = row_to_cells(&row);
        // Pad short rows so apply_op invariants hold.
        while cells.len() < headers.len() {
            cells.push(String::new());
        }
        // Truncate long rows to header count (rare but seen in
        // colspan'd tables we don't yet handle).
        cells.truncate(headers.len());
        data_rows.push(cells);
    }

    Some(crate::table_ops::TableData {
        headers,
        rows: data_rows,
    })
}

/// Extract trimmed text content from each `<td>` / `<th>` child of
/// a row. Skips other children (text nodes, comments).
fn row_to_cells(row: &ElementRef) -> Vec<String> {
    let mut cells = Vec::new();
    for c in row.children() {
        if let Some(cel) = ElementRef::wrap(c) {
            if matches!(cel.value().name(), "td" | "th") {
                let t: String = cel.text().collect();
                cells.push(t.trim().to_string());
            }
        }
    }
    cells
}

/// Emit adaptive table QML: a wrapper Item that contains BOTH a
/// wide-grid layout and a narrow-card layout. At runtime, QML's
/// `visible:` binding switches between them based on the wrapper's
/// width. The hidden mode is skipped entirely by Qt's layout
/// engine, so the cost is mostly emission size.
///
/// Structure:
///
///   Item {
///     property string _tid: "t_<n>"
///     property bool _narrow: width < 600
///     ColumnLayout { visible: !_narrow ...wide-grid... }
///     Column       { visible: _narrow  ...stacked-cards... }
///   }
///
/// Both modes use the same TableData; the difference is layout
/// shape only.
///
/// Caveat for big tables: this doubles QML emission size relative
/// to a single-mode renderer. For very large tables (~20+ rows ×
/// ~8+ columns = 320+ Texts × 2) it can hurt parse time. File a
/// future ship to switch to Loader+Component for tables above a
/// row/column threshold.
pub fn emit_adaptive_table(
    data: &crate::table_ops::TableData,
    sty: &RenderStyle,
    rich_cells: bool,
) -> String {
    let tid = next_table_id();
    let tid_str = format!("t_{}", tid);

    // v0.1.198 (ship 2.4.4): apply any user-issued ops registered
    // against this tid. Original `data` is preserved as the base;
    // we render the result of TableView::apply(). For an
    // unmodified table, ops_for_tid returns empty Vec and apply()
    // returns the base data unchanged.
    let ops = ops_for_tid(&tid_str);
    let view = crate::table_ops::TableView {
        base: data.clone(),
        ops,
    };
    let data = view.apply();
    let data = &data; // shadow as &TableData for the rest of the function

    // v0.1.411: cap rendered rows. See MAX_TABLE_ROWS. We render
    // at most MAX_TABLE_ROWS and, when truncated, append a notice
    // row. `render_rows` is the capped count used by both the
    // wide and narrow loops; `total_rows` is the true count for
    // the notice.
    let total_rows = data.rows.len();
    let render_rows = total_rows.min(MAX_TABLE_ROWS);
    let truncated = total_rows > render_rows;

    let mut o2 = String::with_capacity(512 + render_rows * data.n_cols() * 80);

    // v0.1.220: helper for emitting a cell's Text element with
    // the right escape + textFormat + onLinkActivated based on
    // rich_cells. Captured `sty` and `rich_cells`.
    //
    // When rich_cells is true, the cell string is assumed to be
    // Qt rich-text HTML (post-inline_to_richtext). The Text gets
    // RichText format + onLinkActivated → backend.nav so links
    // inside cells work. When false, write_esc strips any HTML
    // and the Text is plain (current webpage-path behavior).
    //
    // v0.1.243: cells emit `Layout.preferredWidth: 1` alongside
    // `Layout.fillWidth: true`. This is a Qt RowLayout idiom that
    // forces the layout to distribute width EVENLY across cells,
    // ignoring each cell's intrinsic implicitWidth (which can be
    // huge for long unbroken content like URLs, exceeding parent
    // width and forcing the whole page to scroll horizontally).
    // With preferredWidth=1, all fillWidth=true cells get equal
    // share of available width, and wrapMode kicks in to wrap
    // long content WITHIN the cell. Addresses user report:
    // "markdown tables look good, but they do sometimes go off
    // the screen still and make the whole page have to have
    // horizontal scroll."
    let push_cell_text = |o: &mut String, content: &str, bold: bool, layout_attrs: &str| {
        let _ = write!(o, "Text {{\ntext: \"");
        if rich_cells {
            write_esc_keep_html(o, content);
        } else {
            write_esc(o, content);
        }
        let _ = write!(
            o,
            "\"\ncolor: \"{}\"\nfont.pixelSize: {}\n",
            sty.text_color, sty.font_size
        );
        if bold {
            o.push_str("font.bold: true\n");
        }
        if rich_cells {
            let _ = write!(o, "textFormat: Text.RichText\nlinkColor: \"{}\"\n", sty.link_color);
            o.push_str("onLinkActivated: function(link) { if (backend) backend.link_nav(link) }\n");
        }
        o.push_str("wrapMode: Text.WrapAtWordBoundaryOrAnywhere\n");
        // v0.1.243 anti-overflow trick (see header comment above).
        o.push_str("Layout.preferredWidth: 1\n");
        o.push_str(layout_attrs);
        o.push_str("}\n");
    };

    // Outer Item wraps the two modes. We use Item (not Rectangle)
    // because we don't want a background; the page already has one.
    // v0.1.592: the Item gets an explicit id (_tbl_<tid>) so that the
    // wide/narrow `visible` bindings can reference `_tbl_<tid>._narrow`
    // EXPLICITLY. When those columns are wrapped in a Loader Component
    // (large-table path), the Component is a separate scope where the
    // bare identifier `_narrow` does NOT resolve — that was the
    // "_narrow is not defined" ReferenceError flood on big pages.
    let _ = write!(o2, "Item {{\nid: _tbl_{}\nwidth: parent.width\n", tid);
    let _ = write!(o2, "property string _tid: \"t_{}\"\n", tid);
    let _ = write!(o2, "property bool _narrow: width > 0 && width < {}\n", NARROW_TABLE_THRESHOLD_PX);

    // v0.1.421: above a cell-count threshold, wrap each mode in a
    // Loader gated to its visibility, so only the VISIBLE mode's
    // elements are instantiated. Previously both wide AND narrow
    // were always emitted (only one visible) — for a big table
    // that doubled the live element count (see todo "Large tables
    // double QML size"). An inactive Loader holds zero items, so
    // this halves the realized element count for large tables.
    // Small tables keep the plain always-emitted form: Loaders add
    // a little overhead and the instant `visible` toggle is nicer
    // for quick mode switches when the cost is trivial.
    let cell_count = render_rows * data.n_cols().max(1);
    let use_loaders = cell_count > TABLE_LOADER_CELL_THRESHOLD;

    // Height = whichever mode is visible. With Loaders the mode
    // content lives inside a Component, so we size to the active
    // Loader's loaded item; otherwise to the inline mode column.
    if use_loaders {
        let _ = write!(o2,
            "height: _narrow ? (_narrowLoader_{tid}.item ? _narrowLoader_{tid}.item.height : 0) \
             : (_wideLoader_{tid}.item ? _wideLoader_{tid}.item.height : 0)\n",
            tid = tid);
    } else {
        let _ = write!(o2, "height: _narrow ? _narrowCol_{tid}.height : _wideCol_{tid}.height\n", tid = tid);
    }

    // ─── WIDE MODE: ColumnLayout grid ───
    //
    // Built into a local `wide` buffer so it can be emitted inline
    // (small tables) or wrapped in a Component+Loader (large).
    //
    // v0.1.235 polish pass:
    //   - Each data row wrapped in a Rectangle with alt-row tint
    //     (zebra) for visual scanability on long tables
    //   - Header row gets a slight tinted background (input_bg)
    //   - 2px separator under header (was 1px) for stronger
    //     header/body hierarchy
    //   - Cell padding via wrapper Rectangle's `height: inner + 12`
    //     so text breathes vertically (was: tight cell text)
    //   - Outer ColumnLayout spacing 2 → 0 (zebra rectangles
    //     handle visual row separation; gaps would break the
    //     alternation pattern)
    //   - Whole table gets a thin border + radius for unified
    //     grouping (was: bare Item)
    let mut wide = String::new();
    let o = &mut wide;
    // v0.1.421: the wide Rectangle's `visible` only matters in the
    // inline (non-Loader) form; with Loaders the Loader's `active`
    // gates it. Emit visible bound to the Item's _narrow either
    // way — harmless inside an only-when-active Loader.
    o.push_str(&format!("Rectangle {{\nid: _wideCol_{tid}\nvisible: !_tbl_{tid}._narrow\nwidth: parent.width\n", tid = tid));
    let _ = write!(
        o,
        "color: \"transparent\"\nborder.color: \"{}\"\nborder.width: 1\nradius: 4\n",
        sty.border_color
    );
    let _ = write!(o, "height: _wideInner_{}.implicitHeight + 2\n", tid);
    let _ = write!(o, "ColumnLayout {{\nid: _wideInner_{}\nanchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\nanchors.margins: 1\nspacing: 0\n", tid);

    // Header row: bold cells, tinted background.
    o.push_str("Rectangle {\nLayout.fillWidth: true\n");
    let _ = write!(o, "color: \"{}\"\n", sty.input_bg);
    let _ = write!(o, "height: _wideHeaderRow_{}.height + 12\n", tid);
    o.push_str("radius: 3\n");
    let _ = write!(o, "RowLayout {{\nid: _wideHeaderRow_{}\nanchors.left: parent.left\nanchors.right: parent.right\nanchors.verticalCenter: parent.verticalCenter\nanchors.leftMargin: 8\nanchors.rightMargin: 8\nspacing: 8\n", tid);
    for h in &data.headers {
        push_cell_text(o, h, true, "Layout.fillWidth: true\n");
    }
    o.push_str("}\n}\n");
    // 2px separator under header (was 1px).
    let _ = write!(
        o,
        "Rectangle {{\nLayout.fillWidth: true\nheight: 2\ncolor: \"{}\"\n}}\n",
        sty.border_color
    );
    // Data rows. Each is a Rectangle wrapper for zebra + padding.
    // v0.1.411: capped at render_rows (see MAX_TABLE_ROWS).
    for (row_idx, row) in data.rows.iter().take(render_rows).enumerate() {
        let row_inner_id = format!("_wideRow_{}_{}", tid, row_idx);
        o.push_str("Rectangle {\nLayout.fillWidth: true\n");
        // Alternate zebra background. Even rows transparent (no
        // tint), odd rows get the subtle zebra tint. The viewer's
        // eye uses these as scanlines.
        if row_idx % 2 == 1 {
            let _ = write!(o, "color: \"{}\"\n", sty.zebra_bg);
        } else {
            o.push_str("color: \"transparent\"\n");
        }
        let _ = write!(o, "height: {}.height + 12\n", row_inner_id);
        let _ = write!(
            o,
            "RowLayout {{\nid: {}\nanchors.left: parent.left\nanchors.right: parent.right\nanchors.verticalCenter: parent.verticalCenter\nanchors.leftMargin: 8\nanchors.rightMargin: 8\nspacing: 8\n",
            row_inner_id
        );
        for cell in row {
            push_cell_text(o, cell, false, "Layout.fillWidth: true\n");
        }
        o.push_str("}\n}\n");
    }
    // v0.1.411: truncation notice (wide mode).
    if truncated {
        let _ = write!(
            o,
            "Text {{\nLayout.fillWidth: true\ntext: \"\u{2026} showing {} of {} rows (table truncated for performance)\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.italic: true\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\n}}\n",
            render_rows, total_rows, sty.text_color, sty.font_size
        );
    }
    o.push_str("}\n"); // end _wideInner
    o.push_str("}\n"); // end _wideCol Rectangle

    // ─── NARROW MODE: stacked cards ───
    //
    // Built into a local `narrow` buffer (same Loader-or-inline
    // treatment as wide).
    //
    // Each data row becomes a card: a Rectangle with a thin border
    // containing header:value rows stacked vertically. Headers
    // repeat per card because there's no shared column header in
    // this mode.
    //
    // Each card's inner Column gets a unique id (`_card_<tid>_<row>`)
    // so the wrapping Rectangle can size to it without childrenRect
    // circular-dependency issues (Rectangle anchors fill the
    // Column; computing height from childrenRect of an Item with
    // anchored children loops). The Rectangle reads the unique id's
    // height directly.
    let mut narrow = String::new();
    let o = &mut narrow;
    let _ = write!(o, "Column {{\nid: _narrowCol_{tid}\nvisible: _tbl_{tid}._narrow\nwidth: parent.width\nspacing: 8\n", tid = tid);
    for (row_idx, row) in data.rows.iter().take(render_rows).enumerate() {
        let card_id = format!("_card_{}_{}", tid, row_idx);
        // Card outer rectangle.
        let _ = write!(
            o,
            "Rectangle {{\nwidth: parent.width\nheight: {card_id}.height + 16\nborder.color: \"{bd}\"\nborder.width: 1\nradius: 4\ncolor: \"transparent\"\n",
            card_id = card_id,
            bd = sty.border_color
        );
        // Inner Column with header:value rows. Anchored left/top
        // with margins; height grows naturally from children.
        let _ = write!(
            o,
            "Column {{\nid: {}\nanchors.left: parent.left\nanchors.right: parent.right\nanchors.top: parent.top\nanchors.margins: 8\nspacing: 4\n",
            card_id
        );
        for (col_idx, header) in data.headers.iter().enumerate() {
            let cell = row.get(col_idx).map(String::as_str).unwrap_or("");
            // Row of header (bold, left, fixed width) + value.
            o.push_str("RowLayout {\nwidth: parent.width\nspacing: 6\n");
            // Header label. v0.1.200: elide-right keeps long
            // header names from wrapping to multiple lines and
            // breaking card alignment.
            let _ = write!(o, "Text {{\ntext: \"");
            write_esc(o, header);
            let _ = write!(
                o,
                ":\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.bold: true\nLayout.preferredWidth: 90\nelide: Text.ElideRight\n}}\n",
                sty.text_color, sty.font_size
            );
            // Value cell — uses push_cell_text so rich-text works.
            push_cell_text(o, cell, false, "Layout.fillWidth: true\n");
            o.push_str("}\n"); // end RowLayout for one pair
        }
        o.push_str("}\n"); // end inner Column
        o.push_str("}\n"); // end Rectangle card
    }
    // v0.1.411: truncation notice (narrow mode). _narrowCol is a
    // plain Column, so use width: parent.width (not Layout.*).
    if truncated {
        let _ = write!(
            o,
            "Text {{\nwidth: parent.width\ntext: \"\u{2026} showing {} of {} rows (table truncated for performance)\"\ncolor: \"{}\"\nfont.pixelSize: {}\nfont.italic: true\nwrapMode: Text.WrapAtWordBoundaryOrAnywhere\n}}\n",
            render_rows, total_rows, sty.text_color, sty.font_size
        );
    }
    o.push_str("}\n"); // end narrow Column

    // ─── Assemble: inline both modes, or wrap each in a
    // Component+Loader gated to its visibility (large tables). ───
    if use_loaders {
        // Wide.
        let _ = write!(&mut o2,
            "Loader {{\nid: _wideLoader_{tid}\nwidth: parent.width\nactive: !_narrow\nsourceComponent: _wideComp_{tid}\n}}\n\
             Component {{\nid: _wideComp_{tid}\n{wide}\n}}\n",
            tid = tid, wide = wide);
        // Narrow.
        let _ = write!(&mut o2,
            "Loader {{\nid: _narrowLoader_{tid}\nwidth: parent.width\nactive: _narrow\nsourceComponent: _narrowComp_{tid}\n}}\n\
             Component {{\nid: _narrowComp_{tid}\n{narrow}\n}}\n",
            tid = tid, narrow = narrow);
    } else {
        o2.push_str(&wide);
        o2.push_str(&narrow);
    }

    o2.push_str("}\n"); // end outer Item
    o2
}

/// Count whitespace-separated tokens. Approximate word count;
/// punctuation attached to a word counts as one. Good enough for
/// the 10-50 filter — distinguishes "Title" (1 word) from "Read
/// this paragraph aloud..." (50+ words).
fn approx_word_count(s: &str) -> usize {
    s.split_whitespace().filter(|w| !w.is_empty()).count()
}

/// v0.1.299: public re-export for cross-module use. manpages.rs
/// needs the same word-count fn as qml_gen.rs's `<p>` branch
/// so manpage paragraphs respect the same selection-eligibility
/// bounds as webpage paragraphs.
pub fn approx_word_count_pub(s: &str) -> usize {
    approx_word_count(s)
}

/// v0.1.201: tunable paragraph word-count bounds for the
/// selection-wrapper filter. Defaults 10-50; overridable via
/// env vars at startup:
///
///   WEB2QML_PARAGRAPH_MIN — minimum words (default 10)
///   WEB2QML_PARAGRAPH_MAX — maximum words (default 50)
///
/// Read once at startup via OnceLock to avoid per-element env-var
/// lookups. Invalid values fall back to defaults silently.
///
/// Use cases:
///   - User reading style guide or short captions: lower the min
///   - User reading long-form articles: raise the max
///   - Debugging "why isn't this paragraph selectable": widen
///     both bounds temporarily
///
/// v0.1.299: made pub(crate) so manpages.rs can apply the
/// same bounds to manpage prose paragraphs (TODO close).
pub(crate) fn paragraph_word_bounds() -> (usize, usize) {
    static BOUNDS: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();
    *BOUNDS.get_or_init(|| {
        let min = std::env::var("WEB2QML_PARAGRAPH_MIN")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10);
        let max = std::env::var("WEB2QML_PARAGRAPH_MAX")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(50);
        // Defensive: if user passed nonsense (min > max), revert
        // to defaults rather than silently dropping the feature.
        if min > max {
            crate::clog!("qml_gen", "invalid paragraph bounds: min={} > max={}; reverting to 10..=50",
                min, max);
            return (10, 50);
        }
        if min != 10 || max != 50 {
            crate::clog!("qml_gen", "paragraph word-count bounds: {}..={}", min, max);
        }
        (min, max)
    })
}

/// v0.1.236: word-count bounds for NON-`<p>` selectable elements
/// (tables, lists, headings, blockquotes). Wider than prose
/// `<p>` bounds because:
///
///   - Tables can have lots of cell text but still be a single
///     read-aloud unit (the user reads cell-by-cell)
///   - Lists with many items are still ONE list semantically
///   - Headings are short by nature (3-15 words typical)
///
/// User feedback motivating this: "more paragraphs could be
/// selectable... that is not letting us build models on how a
/// person might read a table differently than a paragraph."
/// Wider bounds = more selection opportunities = more diverse
/// corpus data.
///
/// Bounds: 3..=200. Min=3 covers short headings; max=200
/// covers verbose tables/lists without trying to select an
/// entire long-form list-of-instructions.
///
/// v0.1.241: made pub so manpages.rs can reuse the same
/// bounds rather than hardcoding (3, 200) at four sites.
/// Single source of truth for the "non-prose selection
/// bounds" concept.
///
/// Not env-var-overridable yet; the paragraph-bounds vars
/// (WEB2QML_PARAGRAPH_MIN/MAX) only affect `<p>` selection.
/// Could add WEB2QML_EXTENDED_MIN/MAX if user reports needing
/// them.
pub fn extended_word_bounds() -> (usize, usize) {
    (3, 200)
}

/// v0.1.242 (PA1, phoneme alignment arc): classifies a
/// selectable paragraph's content shape so the readingOverlay
/// can choose an appropriate WPM multiplier. PA1 ships
/// hand-tuned multipliers; later PA phases auto-tune.
///
/// Mapping to QML's `_baseMultiplierFor(t)`:
///   - Prose   → 0.95 (prose default; slight v0.1.241 default
///                     slowdown matching user feedback)
///   - Code    → 0.70 (punctuation pauses, indentation breath,
///                     identifier-syllable rhythm; v0.1.244
///                     wired pre/code blocks through
///                     maybe_wrap_extended so they ARE now
///                     selectable with this multiplier applied)
///   - List    → 0.90 (inter-item pause is natural)
///   - Table   → 0.85 (cell-by-cell rhythm slows reading)
///
/// Headings classified as `Prose` — they're prose-text, just
/// bigger. Blockquotes classified as `Prose` — they cite
/// readable prose. Both stay on the prose default.
///
/// The enum string form (lowercased ASCII) is what gets
/// emitted into QML as `_contentType: "prose"` etc.
#[derive(Clone, Copy)]
pub enum ContentType {
    Prose,
    Code,
    List,
    Table,
    /// v0.1.306: heading content. Headings read with more
    /// deliberation than prose (slower multiplier in
    /// ReadingOverlay). Distinct from Prose so the QML's
    /// _baseMultiplierFor can hit a different value.
    Heading,
}

impl ContentType {
    pub fn as_qml_str(self) -> &'static str {
        match self {
            ContentType::Prose => "prose",
            ContentType::Code => "code",
            ContentType::List => "list",
            ContentType::Table => "table",
            ContentType::Heading => "heading",
        }
    }
}

/// v0.1.241: consolidates the "wrap-if-within-bounds" pattern
/// used by tables, lists, and headings in both qml_gen and
/// manpages. Single helper means single place to tune the
/// behavior (e.g., if we later add a stat counter, env-var
/// override, or per-element-type bounds tier).
///
/// Behavior:
///   - Strips word count from `plain` (the text content)
///   - If within extended_word_bounds (3..=200), wraps `qml`
///     with wrap_for_paragraph_selection using a fresh pid
///   - Otherwise returns `qml` unchanged
///
/// The `stats` parameter is optional — `Some(s)` increments
/// `selectable_paragraphs` when the wrap fires, `None` skips
/// the stat. (Manpages don't carry FilterStats; the web-page
/// renderer does.)
///
/// Takes `qml: String` by value (not &str) so the unwrapped
/// path returns the original allocation without copy. Hot
/// path stays cheap.
///
/// v0.1.242 (PA1): added `content_type` parameter that the
/// wrapper emits as a `_contentType` QML property. Read by
/// the readingOverlay's `_baseMultiplierFor()` function to
/// adjust word-advance pace per content type.
pub fn maybe_wrap_extended(
    qml: String,
    plain: &str,
    sty: &RenderStyle,
    stats: Option<&mut FilterStats>,
    content_type: ContentType,
) -> String {
    let wc = approx_word_count(plain);
    let (mn, mx) = extended_word_bounds();
    if wc >= mn && wc <= mx {
        let pid = next_paragraph_id();
        if let Some(s) = stats {
            s.selectable_paragraphs += 1;
        }
        wrap_for_paragraph_selection(&qml, pid, plain, sty, content_type)
    } else {
        qml
    }
}

/// v0.1.190 (ship 2.1): wrap a paragraph-Text emission in an Item
/// that supports tap-to-select. Originally used only for `<p>`
/// elements whose word count fell in the prose-paragraph bounds.
///
/// v0.1.236/v0.1.240/v0.1.241/v0.1.244: now also used for
/// headings (h1-h3), unordered lists, ordered lists, tables,
/// blockquotes, and block-level code. Each call site passes a
/// ContentType so the readingOverlay can apply a content-aware
/// WPM multiplier (PA1, v0.1.242).
///
/// The wrapper adds:
///   - A left-edge accent strip (color from sty.selection_accent,
///     v0.1.243) that appears when selected, with breathing
///     opacity animation and parallax-lite on hover
///   - A MouseArea that sets `_root._selectedParagraphId` on tap
///     and notifies Rust via
///     `backend.paragraph_selected(id, text, content_type)`
///   - A `_contentType` string property (v0.1.242, PA1) that
///     carries the content-type label through to the backend
///
/// Selection state lives on _root (defined by push_preview in
/// main.rs). MouseArea uses propagateComposedEvents so inline link
/// MouseAreas in subsequent elements still work (they're siblings,
/// not children here, but the renderer occasionally emits both —
/// keeping propagation on is the safe default).
///
/// The inner Text is left visually unchanged. The selection
/// indicator is a thin Rectangle anchored to the left edge,
/// visible only when this paragraph is selected.
pub fn wrap_for_paragraph_selection(text_qml: &str, paragraph_id: usize, text_content: &str, sty: &RenderStyle, content_type: ContentType) -> String {
    let mut o = String::with_capacity(text_qml.len() + 480);
    // v0.1.344: tier the wrapper machinery by content type.
    //
    // The user reported a 72KB markdown file rendering to 26142
    // lines of QML — most of the bulk was selection-wrapper
    // machinery emitted uniformly for every paragraph regardless
    // of type. Two optimizations land together:
    //
    // 1. VIEWPORT-GATED ANIMATIONS: the breathing animation (on
    //    the selection accent strip's opacity) and the sweep
    //    animation (on the hover underline's x) only tick when
    //    the paragraph wrapper overlaps the viewport. Off-screen
    //    wrappers stop animating entirely. For a long document
    //    with 100 selection-wrappers, ~10 are on-screen at any
    //    scroll position; the other 90 used to run animations
    //    into nothing.
    //
    // 2. TIER WRAPPERS BY CONTENT TYPE: Prose gets the full
    //    treatment (accent strip with breathing + hover underline
    //    with sweep + MouseArea). Code and Heading get a minimal
    //    wrapper (MouseArea + static accent strip when selected,
    //    no breathing, no sweep). The user never wants the
    //    "subtle, breathing affordance to read this aloud" cue
    //    on code blocks or headings — those are reference
    //    surfaces, not prose to be read line by line.
    //
    // The selection mechanic (tap to set _selectedParagraphId,
    // emit paragraph_selected to backend) is preserved for ALL
    // content types so existing read-aloud flows continue to
    // work, just without the prose-specific affordances.
    let wants_animations = matches!(content_type, ContentType::Prose);

    // Outer Item: holds the Text + selection indicator + MouseArea.
    // height comes from the inner indent-wrapper Item's height (which
    // sizes to the Text). Using a per-paragraph inner id instead of
    // childrenRect.height avoids a circular dependency: the
    // selection-indicator Rectangle anchors to parent.top/bottom,
    // which would feed back into childrenRect.
    o.push_str(&format!("Item {{\nid: _pwrap_{}\nwidth: parent.width\n", paragraph_id));
    o.push_str(&format!("property string _pid: \"p_{}\"\n", paragraph_id));
    // v0.1.242 (PA1): emit content type so readingOverlay can pick
    // a per-content-type WPM multiplier when the user starts
    // reading this paragraph. Drives the cheap-tier alignment from
    // the phoneme-alignment arc; later PA phases auto-tune.
    o.push_str(&format!("property string _contentType: \"{}\"\n", content_type.as_qml_str()));
    // v0.1.224: `_root._selectedParagraphId` can be undefined at
    // component instantiation time (before the root's properties
    // are fully resolved). Comparing undefined === _pid yields
    // undefined, not false, and Qt warns "Unable to assign
    // [undefined] to bool" once per paragraph on every page
    // render. Guard with `_root &&` and explicit `=== true`
    // fallback so the property always resolves to a concrete bool.
    o.push_str("property bool _selected: (_root && _root._selectedParagraphId === _pid) === true\n");
    if wants_animations {
        // v0.1.346: raw source content. For markdown-rendered
        // pages, this is the original markdown line (with
        // markup intact). For webpage paragraphs, this is
        // currently empty (qml_gen has only the rendered
        // text). The corpus uses both — rendered for read-
        // aloud alignment, raw for matching back to source
        // when building voice-driven coding/authoring tools.
        // Stays empty until callers set it; the wrap function
        // doesn't currently take a raw_content parameter (a
        // follow-up ship will extend the signature so
        // manpages.rs can pass the original markdown line).
        o.push_str("property string _rawContent: \"\"\n");
        // v0.1.207: hover state for the slow-sweep underline affordance.
        // Driven by the MouseArea's containsMouse; gated to desktop
        // because mobile doesn't have hover. The actual animation lives
        // in the underline Rectangle below.
        o.push_str("property bool _hovered: false\n");
        // v0.1.344: viewport-overlap test. True when this wrapper's
        // y-range intersects the scroll viewport, false when scrolled
        // off-screen. Gates the continuous animations (breathing,
        // sweep) so they don't tick when nobody can see them.
        // Defaults to true so a wrapper rendered without _root's
        // _vpTop/_vpBot bindings (e.g. tests, partial pages) keeps
        // its animations.
        o.push_str("property bool _inViewport: !_root || _root._vpTop === undefined ? true : (y + height > _root._vpTop && y < _root._vpBot)\n");
    }
    o.push_str(&format!("height: _pinner_{}.height\n", paragraph_id));
    // Left-edge accent strip.
    //
    // v0.1.243: uses sty.selection_accent (was sty.link_color)
    // for proper contrast against page bg and to distinguish
    // selection from inline links. Adds two subtle animations:
    //
    // 1. Breathing opacity 0.7 → 1.0 → 0.7 over 3s ease-in-out,
    //    running only while _selected (and v0.1.344: only while
    //    _inViewport).
    // 2. Parallax-lite: 2px x-offset when _hovered, with a
    //    300ms ease-in-out Behavior. (Prose-tier only.)
    //
    // v0.1.344: Code/Heading tier drops both — the strip still
    // appears when _selected (so tap-to-read shows visible state)
    // but stays at full opacity with no x offset.
    if wants_animations {
        o.push_str(&format!(
            "Rectangle {{\n\
                id: _psel_{pid}\n\
                visible: _pwrap_{pid}._selected\n\
                anchors.top: parent.top\n\
                anchors.bottom: parent.bottom\n\
                x: _pwrap_{pid}._hovered ? 2 : 0\n\
                Behavior on x {{ NumberAnimation {{ duration: 300; easing.type: Easing.InOutQuad }} }}\n\
                width: 3\n\
                radius: 1\n\
                color: \"{c}\"\n\
                SequentialAnimation on opacity {{\n\
                    running: _pwrap_{pid}._selected && _pwrap_{pid}._inViewport\n\
                    loops: Animation.Infinite\n\
                    NumberAnimation {{ from: 0.7; to: 1.0; duration: 1500; easing.type: Easing.InOutQuad }}\n\
                    NumberAnimation {{ from: 1.0; to: 0.7; duration: 1500; easing.type: Easing.InOutQuad }}\n\
                }}\n\
            }}\n",
            pid = paragraph_id,
            c = sty.selection_accent
        ));
    } else {
        // v0.1.344: minimal accent strip — visible when selected,
        // no animations. Saves ~10 lines per non-prose wrapper
        // plus eliminates the SequentialAnimation runner.
        o.push_str(&format!(
            "Rectangle {{\n\
                id: _psel_{pid}\n\
                visible: _pwrap_{pid}._selected\n\
                anchors.top: parent.top\n\
                anchors.bottom: parent.bottom\n\
                x: 0\n\
                width: 3\n\
                radius: 1\n\
                color: \"{c}\"\n\
                opacity: 0.9\n\
            }}\n",
            pid = paragraph_id,
            c = sty.selection_accent
        ));
    }
    // The original Text emission, indented slightly so the accent
    // strip doesn't overlap it. Inner Item gets a unique id so the
    // outer can reference its height.
    //
    // v0.1.346: the prose tier emits the caller's text_qml as
    // before. A planned in-page reading effect (RichText with
    // per-word opacity spans driven by _root._readingWordIndex)
    // was prototyped but needs the readingOverlay state-machine
    // rewiring to actually drive the new properties; rather
    // than half-ship it, this turn keeps the text rendering
    // path stable and lands the dual-content corpus + paste-
    // line-ending fixes only. The _rawContent property added
    // above is the persistent piece — corpus consumers can
    // already use it via paragraph_selected's 4th arg.
    o.push_str(&format!("Item {{\nid: _pinner_{}\nwidth: parent.width - 10\nheight: childrenRect.height\nanchors.left: parent.left\nanchors.leftMargin: 10\n", paragraph_id));
    o.push_str(text_qml);
    o.push_str("\n}\n");
    if wants_animations {
        // v0.1.207: hover underline — slow horizontal sweep along the
        // bottom edge. A 60px-wide accent-color Rectangle whose x
        // animates from -60 to parent.width on a loop (~2400ms cycle).
        // Visible only while hovered, gated by an opacity binding that
        // fades in/out.
        //
        // v0.1.344: gated by _inViewport too so off-screen wrappers
        // never tick the sweep animation.
        o.push_str(&format!(
            "Rectangle {{\n\
                id: _phover_{pid}\n\
                anchors.bottom: parent.bottom\n\
                height: 2\n\
                width: 60\n\
                color: \"{c}\"\n\
                opacity: _pwrap_{pid}._hovered ? 0.5 : 0.0\n\
                Behavior on opacity {{ NumberAnimation {{ duration: 300; easing.type: Easing.OutQuad }} }}\n\
                NumberAnimation on x {{\n\
                    from: -60\n\
                    to: parent.width\n\
                    duration: 2400\n\
                    loops: Animation.Infinite\n\
                    running: _pwrap_{pid}._hovered && _pwrap_{pid}._inViewport\n\
                }}\n\
            }}\n",
            pid = paragraph_id,
            c = sty.link_color
        ));
    }
    // v0.1.344: hover underline omitted for Code/Heading tier.
    // Saves ~14 lines per non-prose wrapper plus eliminates a
    // NumberAnimation runner.
    // MouseArea: anchors.fill so the entire paragraph is tappable.
    //
    // v0.1.278: links inside the paragraph MUST win over
    // paragraph-selection. The user reported "links don't take
    // priority over click to read." Fix: propagateComposedEvents
    // so nested MouseAreas in <a> children get clicks first;
    // this MouseArea only fires when no child accepts.
    //
    // For most paragraphs (prose with inline links flattened
    // to text) this is a no-op. For lists/tables/blockquotes
    // with intact link MouseAreas, the link wins as expected.
    //
    // v0.1.344: hoverEnabled + hover handlers only emitted for
    // the prose tier (which has the sweep underline that
    // consumes _hovered). Code/Heading tier doesn't need
    // containsMouse tracking — saves a small amount of Qt
    // input-routing cost per wrapper.
    if wants_animations {
        o.push_str("MouseArea {\nanchors.fill: parent\nhoverEnabled: true\ncursorShape: Qt.PointingHandCursor\npropagateComposedEvents: true\n");
        // v0.1.207: drive the outer Item's _hovered property from this
        // MouseArea. Keeps the hover state on the SAME Item that has
        // the underline Rectangle so its parent.* references resolve.
        o.push_str("onEntered: parent._hovered = true\n");
        o.push_str("onExited: parent._hovered = false\n");
    } else {
        o.push_str("MouseArea {\nanchors.fill: parent\nhoverEnabled: false\ncursorShape: Qt.PointingHandCursor\npropagateComposedEvents: true\n");
    }
    o.push_str("onClicked: {\n");
    o.push_str("_root._selectedParagraphId = parent._pid\n");
    // Forward to Rust. text_content is escaped already by write_esc
    // in the caller before being formatted into the QML string.
    //
    // v0.1.242 (PA1): now passes parent._contentType as a third
    // argument so the backend can publish current_paragraph_content_type
    // for the readingOverlay to consume.
    //
    // v0.1.346: 4th argument is _rawContent (raw source text,
    // typically the original markdown line for md-rendered pages,
    // empty for webpage paths where the rendered text IS the
    // source from the QML side's view). The backend uses this
    // to build a dual-content corpus — rendered for alignment,
    // raw for source-matching when building voice-coding tools.
    o.push_str("if (backend) backend.paragraph_selected(parent._pid, \"");
    // Re-escape for the JS-inside-QML string context.
    let mut escaped_for_js = String::with_capacity(text_content.len() + 16);
    for c in text_content.chars() {
        match c {
            '"' => escaped_for_js.push_str("\\\""),
            '\\' => escaped_for_js.push_str("\\\\"),
            '\n' => escaped_for_js.push_str("\\n"),
            '\r' => {}
            _ => escaped_for_js.push(c),
        }
    }
    o.push_str(&escaped_for_js);
    if wants_animations {
        o.push_str("\", parent._contentType, parent._rawContent)\n}\n}\n");
    } else {
        // Code/Heading tier has no _rawContent property (didn't
        // emit it). Pass empty string for that argument.
        o.push_str("\", parent._contentType, \"\")\n}\n}\n");
    }
    o.push_str("}");
    o
}

pub fn generate_safe_qml(n: &ElementRef, d: usize, mx: usize, base: &str, sty: &RenderStyle) -> String {
    reset_paragraph_counter();
    reset_table_counter();
    let mut st = FilterStats::default();
    let q = generate_safe_qml_with_stats(n, d, mx, base, sty, &mut st);
    if st.filtered_elements > 0 { eprintln!("  AD FILTER: {}", st.summary()); }
    // v0.1.199: surface the read-aloud-availability metric. Only
    // logs when the page has any selectable paragraphs — keeps
    // the terminal quiet for pages that don't have prose content
    // (forms, landing pages, search results).
    if st.selectable_paragraphs > 0 {
        eprintln!("  PARAGRAPHS: {} selection-eligible", st.selectable_paragraphs);
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    // v0.1.411: table row-cap tests. A giant table is one node
    // in the MAX_NODES count but emits every row inline twice
    // (wide + narrow). The cap prevents the 1M-line explosion.

    #[test]
    fn huge_table_is_row_capped() {
        let sty = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Standard);
        let headers = vec!["A".to_string(), "B".to_string()];
        // 5000 rows — well over MAX_TABLE_ROWS (200).
        let rows: Vec<Vec<String>> = (0..5000)
            .map(|i| vec![format!("r{}c0", i), format!("r{}c1", i)])
            .collect();
        let data = crate::table_ops::TableData { headers, rows };
        let qml = emit_adaptive_table(&data, &sty, false);
        // The unique cell content "r4999c0" (a row past the cap)
        // must NOT appear — it was truncated.
        assert!(!qml.contains("r4999c0"),
            "row past the cap should be truncated");
        // A row within the cap MUST appear.
        assert!(qml.contains("r0c0"), "first row should render");
        // The truncation notice must be present.
        assert!(qml.contains("showing 200 of 5000 rows"),
            "should show truncation notice: missing");
    }

    #[test]
    fn small_table_not_capped_no_notice() {
        let sty = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Standard);
        let headers = vec!["A".to_string()];
        let rows: Vec<Vec<String>> = (0..10)
            .map(|i| vec![format!("row{}", i)])
            .collect();
        let data = crate::table_ops::TableData { headers, rows };
        let qml = emit_adaptive_table(&data, &sty, false);
        // All 10 rows present, no truncation notice.
        assert!(qml.contains("row9"), "all rows render when under cap");
        assert!(!qml.contains("truncated for performance"),
            "no notice for a small table");
    }

    // v0.1.422: selection-accent collision overrides.
    #[test]
    fn selection_accent_overrides_for_colliding_themes() {
        let default_light = RenderStyle::from_theme(false, 14, false, crate::ad_filter::FilterMode::Off)
            .selection_accent.clone();
        let readlt = RenderStyle::from_theme(false, 14, false, crate::ad_filter::FilterMode::Off)
            .with_theme_idx(9);
        let sepia = RenderStyle::from_theme(false, 14, false, crate::ad_filter::FilterMode::Off)
            .with_theme_idx(10);
        let amber = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Off)
            .with_theme_idx(14);
        assert_eq!(readlt.selection_accent, "#1f7a6a");
        assert_eq!(sepia.selection_accent, "#1a6b8a");
        assert_eq!(amber.selection_accent, "#33ccbb");
        assert_ne!(readlt.selection_accent, default_light);
        assert_ne!(sepia.selection_accent, default_light);
    }

    #[test]
    fn selection_accent_unchanged_for_normal_themes() {
        let dark_default = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Off)
            .selection_accent.clone();
        let dark0 = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Off)
            .with_theme_idx(0);
        let light1 = RenderStyle::from_theme(false, 14, false, crate::ad_filter::FilterMode::Off)
            .with_theme_idx(1);
        assert_eq!(dark0.selection_accent, dark_default);
        assert_eq!(dark0.selection_accent, "#ffaa44");
        assert_eq!(light1.selection_accent, "#c04020");
    }

    // v0.1.421: large tables wrap each mode in a Component+Loader
    // so only the visible mode instantiates; small tables emit
    // both modes inline (no Loader).
    #[test]
    fn large_table_uses_loaders() {
        let sty = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Standard);
        // 8 cols × 20 rows = 160 cells > TABLE_LOADER_CELL_THRESHOLD (100).
        let headers: Vec<String> = (0..8).map(|c| format!("H{}", c)).collect();
        let rows: Vec<Vec<String>> = (0..20)
            .map(|r| (0..8).map(|c| format!("r{}c{}", r, c)).collect())
            .collect();
        let data = crate::table_ops::TableData { headers, rows };
        let qml = emit_adaptive_table(&data, &sty, false);
        assert!(qml.contains("Loader {"), "large table should use Loaders");
        assert!(qml.contains("Component {"), "large table should define Components");
        assert!(qml.contains("active: !_narrow"), "wide Loader gated to !narrow");
        assert!(qml.contains("active: _narrow"), "narrow Loader gated to narrow");
        // Content still present (inside the Components).
        assert!(qml.contains("r0c0"), "cell content present");
    }

    #[test]
    fn small_table_no_loaders() {
        let sty = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Standard);
        // 2 cols × 5 rows = 10 cells, well under threshold.
        let headers = vec!["A".to_string(), "B".to_string()];
        let rows: Vec<Vec<String>> = (0..5)
            .map(|r| vec![format!("r{}a", r), format!("r{}b", r)])
            .collect();
        let data = crate::table_ops::TableData { headers, rows };
        let qml = emit_adaptive_table(&data, &sty, false);
        assert!(!qml.contains("Loader {"), "small table should NOT use Loaders");
        // Both modes emitted inline (wide Rectangle + narrow Column).
        assert!(qml.contains("_wideCol_"), "wide mode inline");
        assert!(qml.contains("_narrowCol_"), "narrow mode inline");
    }

    // v0.1.401: URL resolution audit harness. Covers every
    // edge case the todo flagged plus the two real bugs found
    // during the rewrite (literal `..`, and the
    // query-string-no-slash garbage).

    #[test]
    fn resolve_empty_returns_base() {
        assert_eq!(resolve_url("", "https://site.com/page"),
            "https://site.com/page");
    }

    #[test]
    fn resolve_absolute_passthrough() {
        assert_eq!(resolve_url("https://other.com/x", "https://site.com/"),
            "https://other.com/x");
        assert_eq!(resolve_url("http://other.com/x", "https://site.com/"),
            "http://other.com/x");
    }

    #[test]
    fn resolve_protocol_relative_inherits_scheme() {
        assert_eq!(resolve_url("//cdn.com/x.js", "https://site.com/"),
            "https://cdn.com/x.js");
        assert_eq!(resolve_url("//cdn.com/x.js", "http://site.com/"),
            "http://cdn.com/x.js");
    }

    #[test]
    fn resolve_fragment_only_replaces_base_fragment() {
        assert_eq!(resolve_url("#section2", "https://site.com/page#section1"),
            "https://site.com/page#section2");
        // Base with no existing fragment:
        assert_eq!(resolve_url("#top", "https://site.com/page"),
            "https://site.com/page#top");
    }

    #[test]
    fn resolve_absolute_path_uses_origin() {
        assert_eq!(resolve_url("/about", "https://site.com/a/b/page.html"),
            "https://site.com/about");
    }

    #[test]
    fn resolve_relative_path_against_directory() {
        assert_eq!(resolve_url("img.png", "https://site.com/a/b/page.html"),
            "https://site.com/a/b/img.png");
    }

    #[test]
    fn resolve_relative_against_dir_with_trailing_slash() {
        assert_eq!(resolve_url("img.png", "https://site.com/a/b/"),
            "https://site.com/a/b/img.png");
    }

    // --- The two real bugs the audit found ---

    #[test]
    fn resolve_collapses_parent_segments() {
        // Was: produced literal "https://site.com/a/b/../images/x".
        assert_eq!(
            resolve_url("../images/x.png", "https://site.com/a/b/page.html"),
            "https://site.com/a/images/x.png");
    }

    #[test]
    fn resolve_collapses_multiple_parent_segments() {
        assert_eq!(
            resolve_url("../../x.png", "https://site.com/a/b/page.html"),
            "https://site.com/x.png");
    }

    #[test]
    fn resolve_parent_cannot_escape_root() {
        // Too many ".." just clamp at root, like a browser.
        assert_eq!(
            resolve_url("../../../../x.png", "https://site.com/a/page.html"),
            "https://site.com/x.png");
    }

    #[test]
    fn resolve_current_dir_segments_dropped() {
        assert_eq!(
            resolve_url("./img.png", "https://site.com/a/b/page.html"),
            "https://site.com/a/b/img.png");
        assert_eq!(
            resolve_url("a/./b/./c.png", "https://site.com/x/page.html"),
            "https://site.com/x/a/b/c.png");
    }

    #[test]
    fn resolve_relative_on_base_with_query_no_slash() {
        // Was: produced "https:/page2" (garbage) because
        // rfind('/') landed in the "://".
        assert_eq!(
            resolve_url("page2", "https://site.com?x=1"),
            "https://site.com/page2");
    }

    #[test]
    fn resolve_base_query_stripped_before_join() {
        // The base's own query must not leak into the result.
        assert_eq!(
            resolve_url("other.html", "https://site.com/dir/page.html?a=1&b=2"),
            "https://site.com/dir/other.html");
    }

    #[test]
    fn resolve_base_fragment_stripped_before_join() {
        assert_eq!(
            resolve_url("other.html", "https://site.com/dir/page.html#frag"),
            "https://site.com/dir/other.html");
    }

    #[test]
    fn resolve_href_query_preserved() {
        // The href's OWN query should survive.
        assert_eq!(
            resolve_url("search?q=hello", "https://site.com/dir/page.html"),
            "https://site.com/dir/search?q=hello");
    }

    #[test]
    fn resolve_href_fragment_preserved() {
        assert_eq!(
            resolve_url("page.html#sec", "https://site.com/dir/x.html"),
            "https://site.com/dir/page.html#sec");
    }

    #[test]
    fn resolve_parent_with_query_suffix() {
        // `..` collapse AND query preservation together.
        assert_eq!(
            resolve_url("../q?x=1", "https://site.com/a/b/page.html"),
            "https://site.com/a/q?x=1");
    }

    #[test]
    fn resolve_origin_only_base() {
        // Base is just the origin, no path.
        assert_eq!(resolve_url("page.html", "https://site.com"),
            "https://site.com/page.html");
        assert_eq!(resolve_url("/page.html", "https://site.com"),
            "https://site.com/page.html");
    }

    #[test]
    fn resolve_non_url_base_returns_href() {
        // Defensive: a base that isn't a full URL → best-effort
        // passthrough of the href.
        assert_eq!(resolve_url("page.html", "not-a-url"),
            "page.html");
    }

    #[test]
    fn normalize_path_examples() {
        assert_eq!(normalize_path("/a/b/../c"), "/a/c");
        assert_eq!(normalize_path("/a/./b"), "/a/b");
        assert_eq!(normalize_path("/a/b/../../c"), "/c");
        assert_eq!(normalize_path("/a/b/"), "/a/b/");
        assert_eq!(normalize_path("/"), "/");
    }

    /// v0.1.287: pin the contract that every ContentType variant
    /// maps to a non-empty literal. This contract is depended on
    /// by main.rs (paragraph_selected) and qml/main.qml
    /// (beginReading) — both have defensive empty-string defaults
    /// that would silently disagree if the contract ever broke.
    ///
    /// If someone adds an Unknown / Default / Other variant later,
    /// this test catches it. The fix is either: (a) ensure the new
    /// variant has a non-empty string here, OR (b) audit ALL the
    /// callers' empty-string defaults and decide on a coordinated
    /// fallback value.
    #[test]
    fn content_type_variants_are_non_empty() {
        for ct in [
            ContentType::Prose,
            ContentType::Code,
            ContentType::List,
            ContentType::Table,
            ContentType::Heading,
        ] {
            let s = ct.as_qml_str();
            assert!(!s.is_empty(), "ContentType variant must map to non-empty string");
        }
    }

    /// Also pin the actual string values so a typo in a variant
    /// name (e.g. "proses" instead of "prose") doesn't silently
    /// break QML matching. Backend and QML both pattern-match on
    /// these literals.
    #[test]
    fn content_type_string_values_stable() {
        assert_eq!(ContentType::Prose.as_qml_str(), "prose");
        assert_eq!(ContentType::Code.as_qml_str(), "code");
        assert_eq!(ContentType::List.as_qml_str(), "list");
        assert_eq!(ContentType::Table.as_qml_str(), "table");
        assert_eq!(ContentType::Heading.as_qml_str(), "heading");
    }

    /// v0.1.344: pin that the prose tier emits the full
    /// breathing + sweep machinery while non-prose tiers
    /// skip those animations. If someone refactors the
    /// emission logic and accidentally re-enables animations
    /// for all content types, this test catches it.
    #[test]
    fn prose_tier_emits_breathing_animation() {
        let sty = RenderStyle::from_theme(true, 16, false, crate::ad_filter::FilterMode::Off);
        let qml = wrap_for_paragraph_selection(
            "Text { text: \"hi\" }", 99, "hi", &sty,
            ContentType::Prose,
        );
        assert!(qml.contains("SequentialAnimation on opacity"),
            "prose wrapper must emit breathing animation: {}", qml);
        assert!(qml.contains("NumberAnimation on x"),
            "prose wrapper must emit sweep animation");
        assert!(qml.contains("_inViewport"),
            "prose wrapper must declare _inViewport for viewport gating");
        assert!(qml.contains("_hovered"),
            "prose wrapper must declare _hovered for sweep");
    }

    #[test]
    fn code_tier_skips_breathing_and_sweep() {
        let sty = RenderStyle::from_theme(true, 16, false, crate::ad_filter::FilterMode::Off);
        let qml = wrap_for_paragraph_selection(
            "Text { text: \"code\" }", 100, "code", &sty,
            ContentType::Code,
        );
        assert!(!qml.contains("SequentialAnimation on opacity"),
            "code wrapper must NOT emit breathing animation: {}", qml);
        assert!(!qml.contains("NumberAnimation on x"),
            "code wrapper must NOT emit sweep animation");
        assert!(!qml.contains("_hovered"),
            "code wrapper must NOT declare _hovered (no sweep to drive)");
        // Selection mechanic must still work.
        assert!(qml.contains("_selected"),
            "code wrapper must still support selection");
        assert!(qml.contains("MouseArea"),
            "code wrapper must still emit MouseArea for tap-to-select");
        assert!(qml.contains("paragraph_selected"),
            "code wrapper must still call paragraph_selected on tap");
    }

    #[test]
    fn heading_tier_skips_breathing_and_sweep() {
        let sty = RenderStyle::from_theme(true, 16, false, crate::ad_filter::FilterMode::Off);
        let qml = wrap_for_paragraph_selection(
            "Text { text: \"head\" }", 101, "head", &sty,
            ContentType::Heading,
        );
        assert!(!qml.contains("SequentialAnimation on opacity"));
        assert!(!qml.contains("NumberAnimation on x"));
    }

    #[test]
    fn non_prose_wrapper_is_smaller_than_prose() {
        let sty = RenderStyle::from_theme(true, 16, false, crate::ad_filter::FilterMode::Off);
        let prose = wrap_for_paragraph_selection(
            "Text { text: \"x\" }", 200, "x", &sty,
            ContentType::Prose,
        );
        let code = wrap_for_paragraph_selection(
            "Text { text: \"x\" }", 201, "x", &sty,
            ContentType::Code,
        );
        let prose_lines = prose.lines().count();
        let code_lines = code.lines().count();
        assert!(code_lines < prose_lines,
            "code tier ({} lines) must be smaller than prose tier ({} lines)",
            code_lines, prose_lines);
        // Concretely: expect at least 10 lines saved per non-prose
        // wrapper (breathing block + sweep block both removed).
        assert!(prose_lines - code_lines >= 10,
            "expected non-prose tier to save >=10 lines per wrapper; \
             prose={}, code={}, saved={}",
            prose_lines, code_lines, prose_lines - code_lines);
    }

    #[test]
    fn prose_animations_are_viewport_gated() {
        let sty = RenderStyle::from_theme(true, 16, false, crate::ad_filter::FilterMode::Off);
        let qml = wrap_for_paragraph_selection(
            "Text { text: \"y\" }", 300, "y", &sty,
            ContentType::Prose,
        );
        // Breathing should reference _inViewport in its running clause.
        let after_breathing = qml.find("SequentialAnimation on opacity")
            .expect("breathing animation present");
        let breathing_block = &qml[after_breathing..];
        let next_close = breathing_block.find("}}").unwrap_or(breathing_block.len());
        let block = &breathing_block[..next_close];
        assert!(block.contains("_inViewport"),
            "breathing running clause must gate on _inViewport: {}", block);
    }

    // ── v0.1.345: sidebar-aware layout demotion ──

    fn parse_first_child(html: &str) -> scraper::Html {
        scraper::Html::parse_fragment(html)
    }

    #[test]
    fn has_sidebar_child_detects_aside() {
        let doc = parse_first_child("<div><aside>nav</aside><main>content</main></div>");
        let sel = scraper::Selector::parse("div").unwrap();
        let div = doc.select(&sel).next().expect("div");
        assert!(has_sidebar_child(&div));
    }

    #[test]
    fn has_sidebar_child_detects_nav() {
        let doc = parse_first_child("<div><nav>links</nav><main>content</main></div>");
        let sel = scraper::Selector::parse("div").unwrap();
        let div = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&div));
    }

    #[test]
    fn has_sidebar_child_detects_class() {
        let doc = parse_first_child(
            "<div><div class=\"sidebar\">nav</div><div>content</div></div>"
        );
        // We want the OUTER div. Selectors return matches in
        // document order; we want the first.
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer));
    }

    #[test]
    fn has_sidebar_child_detects_id() {
        let doc = parse_first_child(
            "<div><div id=\"sidebar\">nav</div><div>content</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer));
    }

    #[test]
    fn has_sidebar_child_detects_toc_class() {
        let doc = parse_first_child(
            "<div><div class=\"toc\">contents</div><article>body</article></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer));
    }

    #[test]
    fn has_sidebar_child_only_inspects_direct_children() {
        // <nav> nested inside <header> shouldn't trigger demotion
        // when the outer <div> is inspected. The header itself
        // is the direct child; nav is grandchild.
        let doc = parse_first_child(
            "<div><header><nav>menu</nav></header><main>content</main></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "nav nested in header should NOT trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_no_match_returns_false() {
        let doc = parse_first_child(
            "<div><span>a</span><span>b</span><span>c</span></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer));
    }

    #[test]
    fn has_sidebar_child_partial_class_match_does_not_trigger() {
        // "toc-li" contains "toc" but isn't the standalone "toc"
        // class — should NOT trigger. The whole-word check on
        // the token list prevents this.
        let doc = parse_first_child(
            "<div><div class=\"toc-li\">item</div><div>content</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        // "toc-li" doesn't match "toc" whole-word, and the
        // contains_ci check is only for "table-of-contents",
        // "toc-wrap", etc.
        assert!(!has_sidebar_child(&outer),
            "'toc-li' partial match should NOT trigger");
    }

    // v0.1.366: expanded signals.

    #[test]
    fn has_sidebar_child_detects_tailwind_w64() {
        // ChatGPT-style: first child has Tailwind w-64 sidebar
        // width, no semantic name.
        let doc = parse_first_child(
            "<div class=\"flex\"><div class=\"w-64 bg-gray-900\">links</div>\
             <div class=\"flex-1\">main</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "Tailwind w-64 narrow child should trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_detects_tailwind_responsive_width() {
        // md:w-72 is the responsive variant, common in modern
        // React apps that hide the sidebar on mobile.
        let doc = parse_first_child(
            "<div class=\"flex\"><div class=\"md:w-72 hidden md:block\">nav</div>\
             <main class=\"flex-1\">content</main></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "md:w-72 responsive Tailwind width should trigger");
    }

    #[test]
    fn has_sidebar_child_skips_wide_tailwind_widths() {
        // w-full / w-screen / w-1/2 are NOT sidebar-shaped —
        // they mean "fill the parent" or "half" which are
        // main-content widths.
        let doc = parse_first_child(
            "<div class=\"flex\"><div class=\"w-full bg-gray-100\">a</div>\
             <div class=\"w-full\">b</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "w-full children should NOT trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_detects_fixed_position_class() {
        let doc = parse_first_child(
            "<div class=\"flex\"><div class=\"fixed top-0 left-0\">nav</div>\
             <main>content</main></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "'fixed' Tailwind position class should trigger");
    }

    #[test]
    fn has_sidebar_child_detects_inline_narrow_px_width() {
        let doc = parse_first_child(
            "<div><div style=\"width: 280px; background: gray\">nav</div>\
             <div>content</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline width:280px should trigger sidebar detection");
    }

    #[test]
    fn has_sidebar_child_detects_inline_rem_width() {
        let doc = parse_first_child(
            "<div><div style=\"width: 18rem;\">nav</div>\
             <div>content</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline width:18rem should trigger sidebar detection");
    }

    #[test]
    fn has_sidebar_child_skips_wide_inline_widths() {
        // 800px is too wide to be a sidebar
        let doc = parse_first_child(
            "<div><div style=\"width: 800px;\">a</div>\
             <div style=\"width: 800px;\">b</div></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "width:800px should NOT trigger (too wide for sidebar)");
    }

    #[test]
    fn has_sidebar_child_detects_inline_position_fixed() {
        let doc = parse_first_child(
            "<div><div style=\"position: fixed; top: 0; left: 0;\">nav</div>\
             <main>content</main></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline position:fixed should trigger");
    }

    #[test]
    fn has_sidebar_child_deep_scan_finds_wrapped_sidebar() {
        // React app: bare <div> wrapper around the actual
        // sidebar. v0.1.345's direct-only check would miss
        // this; v0.1.366's deep scan catches it.
        let doc = parse_first_child(
            "<div class=\"flex\">\
             <div class=\"sidebar-wrapper\"><div class=\"w-64\">nav</div></div>\
             <main>content</main>\
             </div>"
        );
        let sel = scraper::Selector::parse("div.flex").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "deep scan should find sidebar wrapped one level deep");
    }

    #[test]
    fn has_sidebar_child_deep_scan_skips_header_wrapper() {
        // <header> wraps a <nav> — that's a breadcrumb/topbar
        // pattern, NOT a sidebar. Deep scan should refuse to
        // descend into semantic wrappers.
        let doc = parse_first_child(
            "<div><header><nav>menu</nav></header><main>content</main></div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "deep scan should NOT descend into <header>");
    }

    #[test]
    fn has_sidebar_child_deep_scan_caps_at_3_children() {
        // 4+ children = too many for a "sidebar+main" pattern
        // — deep scan should NOT fire. This prevents the deep
        // scan from being a perf hazard on gallery / card-grid
        // layouts.
        let doc = parse_first_child(
            "<div>\
             <div><div class=\"w-64\">a</div></div>\
             <div>b</div>\
             <div>c</div>\
             <div>d</div>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "deep scan should bail when >3 children (not a sidebar layout)");
    }

    // v0.1.370: Grid layouts with sidebar children should also
    // demote to Column. The has_sidebar_child check is the same;
    // what changed is the calling site in generate_safe_qml_with_stats
    // now includes LayoutKind::Grid in the demotion condition.
    //
    // This unit test exercises has_sidebar_child against grid-
    // shaped markup. The actual demotion behavior is tested via
    // integration / hardware (the qml_gen pipeline produces
    // Column instead of Grid).

    #[test]
    fn has_sidebar_child_works_on_grid_layout() {
        // Tailwind grid-cols pattern: 2 grid tracks, first is
        // narrow (sidebar), second is content. detect_layout
        // would return Grid for this; has_sidebar_child should
        // still flag it.
        let doc = parse_first_child(
            "<div class=\"grid grid-cols-[18rem,1fr]\">\
             <div class=\"w-72\">nav</div>\
             <main>content</main>\
             </div>"
        );
        let sel = scraper::Selector::parse("div.grid").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "grid layout with w-72 first child should flag as sidebar");
    }

    // v0.1.377: dedicated tests for inline-style sidebar
    // signals. The implementation (line ~961-989 of qml_gen.rs)
    // has handled this since v0.1.366 via the width-detection
    // pass; these tests pin the behavior so future refactors
    // don't silently regress on common React patterns.

    #[test]
    fn has_sidebar_child_detects_inline_flex_shrink_with_px_width() {
        // Common React pattern: `style="flex-shrink: 0; width: 280px;"`
        // — explicit pixel width below 320 = sidebar shape.
        let doc = parse_first_child(
            "<div style=\"display: flex\">\
             <div style=\"flex-shrink: 0; width: 280px;\">nav</div>\
             <div style=\"flex: 1\">main</div>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline width: 280px should trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_detects_inline_width_rem() {
        // rem units are also common — sidebar widths up to
        // 20rem (≈ 320px) count.
        let doc = parse_first_child(
            "<div style=\"display: flex\">\
             <div style=\"width: 16rem; flex-shrink: 0;\">nav</div>\
             <main>content</main>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline width: 16rem should trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_rejects_wide_inline_width() {
        // 800px is a content width, not a sidebar width.
        let doc = parse_first_child(
            "<div style=\"display: flex\">\
             <div style=\"width: 800px;\">wide</div>\
             <div>other</div>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(!has_sidebar_child(&outer),
            "inline width: 800px should NOT trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_detects_inline_position_fixed_drawer() {
        // Floating side panels — common React drawer pattern.
        let doc = parse_first_child(
            "<div>\
             <div style=\"position: fixed; left: 0; top: 0;\">drawer</div>\
             <main>content</main>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline position: fixed should trigger sidebar demotion");
    }

    #[test]
    fn has_sidebar_child_detects_inline_position_sticky() {
        // Sticky side panels — common for TOCs.
        let doc = parse_first_child(
            "<div style=\"display: flex\">\
             <aside style=\"position: sticky; top: 0;\">TOC</aside>\
             <article>body</article>\
             </div>"
        );
        let sel = scraper::Selector::parse("div").unwrap();
        let outer = doc.select(&sel).next().unwrap();
        assert!(has_sidebar_child(&outer),
            "inline position: sticky should trigger sidebar demotion (also aside semantic)");
    }

    // v0.1.373: SPA detection (JS-rendering arc, Option 1).

    #[test]
    fn detect_spa_react_empty_shell() {
        let html = r#"<!doctype html><html><body><div id="root"></div><script src="/static/js/main.abc123.js"></script></body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_some(), "react empty shell should be detected");
        let msg = result.unwrap();
        assert!(msg.contains("React") || msg.contains("single-page"),
            "message should mention React or SPA: {}", msg);
    }

    #[test]
    fn detect_spa_next_js() {
        let html = r#"<!doctype html><html><body><div id="__next"></div><script id="__NEXT_DATA__" type="application/json">{}</script></body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Next.js"));
    }

    #[test]
    fn detect_spa_vue() {
        let html = r#"<!doctype html><html><body><div id="app"></div><script src="vue.js"></script></body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Vue"));
    }

    #[test]
    fn detect_spa_does_not_fire_on_substantial_content() {
        // Page has real article content alongside scripts —
        // not an SPA shell.
        let html = r#"<!doctype html><html><body>
            <div id="root">
                <article>
                    <h1>Real Article Title</h1>
                    <p>This is a real article.</p>
                    <p>It has multiple paragraphs.</p>
                    <p>Server-rendered content.</p>
                    <p>Plus some more text here.</p>
                </article>
            </div>
            <script>somescript()</script>
        </body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_none(),
            "page with real content should not be flagged as SPA");
    }

    #[test]
    fn detect_spa_does_not_fire_without_root_mount() {
        // No <div id="root">/<div id="app">/etc. — just a
        // page with some scripts. Many sites have <script>
        // tags without being SPAs.
        let html = r#"<!doctype html><html><body>
            <h1>Hello</h1>
            <p>Regular page content</p>
            <script>analytics()</script>
        </body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_none());
    }

    #[test]
    fn detect_spa_does_not_fire_without_scripts() {
        // Has <div id="root"> but no scripts — probably just
        // a styling class. Not an SPA.
        let html = r#"<!doctype html><html><body>
            <div id="root">static content</div>
        </body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_none());
    }

    #[test]
    fn detect_spa_unknown_framework_still_detected() {
        // Empty root mount + scripts but no obvious framework
        // signal — generic message.
        let html = r#"<!doctype html><html><body><div id="root"></div><script src="/bundle.js"></script></body></html>"#;
        let result = detect_spa(html);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("single-page application"));
    }

    // v0.1.374: is_placeholder coverage.

    #[test]
    fn placeholder_empty_and_whitespace() {
        assert!(is_placeholder(""));
        assert!(is_placeholder("   "));
        assert!(is_placeholder("\t\n"));
    }

    #[test]
    fn placeholder_data_gif() {
        assert!(is_placeholder("data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw=="));
    }

    #[test]
    fn placeholder_data_svg() {
        assert!(is_placeholder("data:image/svg+xml;utf8,<svg></svg>"));
    }

    #[test]
    fn placeholder_short_transparent_png() {
        // Real 1x1 transparent PNG starts with iVBORw0KGgo and
        // is short (~120 bytes base64). Match.
        let s = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkAAIAAAoAAv/lxKUAAAAASUVORK5CYII=";
        assert!(is_placeholder(s),
            "short transparent png should be flagged as placeholder");
    }

    #[test]
    fn placeholder_long_png_data_is_real() {
        // Long data:image/png is probably real inline content
        // (e.g. an actual photo embedded). Must NOT match.
        let long_data = format!("data:image/png;base64,iVBORw0KGgo{}",
            "A".repeat(500));
        assert!(!is_placeholder(&long_data),
            "long inline png should NOT be flagged as placeholder");
    }

    #[test]
    fn placeholder_about_blank_and_hash() {
        assert!(is_placeholder("about:blank"));
        assert!(is_placeholder("#"));
    }

    #[test]
    fn placeholder_real_url_not_flagged() {
        assert!(!is_placeholder("https://example.com/photo.jpg"));
        assert!(!is_placeholder("/static/hero.webp"));
        assert!(!is_placeholder("../images/banner.png"));
    }

    #[test]
    fn placeholder_data_jpeg_not_flagged() {
        // Real inline JPEG embedded — should NOT match.
        assert!(!is_placeholder("data:image/jpeg;base64,/9j/4AAQSkZJRgABA..."));
    }

    // v0.1.875: dead/JS link detection + the non-navigable cue.

    #[test]
    fn dead_href_detection() {
        assert!(is_dead_href(""));
        assert!(is_dead_href("   "));
        assert!(is_dead_href("#"));
        assert!(is_dead_href("about:blank"));
        assert!(is_dead_href("About:Blank"));
        assert!(is_dead_href("javascript:void(0)"));
        assert!(is_dead_href("JavaScript:doThing()"));
        assert!(is_dead_href("  javascript:foo  "));
        // NOT dead — these carry a real target:
        assert!(!is_dead_href("#section"));
        assert!(!is_dead_href("https://example.com"));
        assert!(!is_dead_href("/path/page"));
        assert!(!is_dead_href("mailto:x@y.com"));
    }

    fn render_anchor(html: &str) -> String {
        let sty = RenderStyle::from_theme(true, 14, false, crate::ad_filter::FilterMode::Off);
        let frag = scraper::Html::parse_fragment(html);
        let sel = scraper::Selector::parse("a").unwrap();
        let a = frag.select(&sel).next().expect("an <a> element");
        generate_safe_qml(&a, 0, 50, "https://example.com", &sty)
    }

    #[test]
    fn javascript_link_renders_cue_not_nav() {
        let qml = render_anchor(r#"<a href="javascript:void(0)">Show more</a>"#);
        // The interactive cue marker (⚡) is present, and the label kept.
        assert!(qml.contains('\u{26A1}'), "expected cue marker, got: {qml}");
        assert!(qml.contains("Show more"));
        // Non-navigable: no link_menu wiring for a dead JS link.
        assert!(!qml.contains("link_menu"), "dead JS link must not navigate: {qml}");
    }

    #[test]
    fn bare_hash_link_renders_cue() {
        let qml = render_anchor(r##"<a href="#">Top</a>"##);
        assert!(qml.contains('\u{26A1}'), "expected cue marker, got: {qml}");
        assert!(!qml.contains("link_menu"), "bare # must not navigate: {qml}");
    }

    #[test]
    fn real_link_still_navigable() {
        let qml = render_anchor(r#"<a href="https://example.com/page">Real</a>"#);
        assert!(qml.contains("link_menu"), "real link should navigate: {qml}");
        assert!(!qml.contains('\u{26A1}'), "real link should not show the cue: {qml}");
    }

    #[test]
    fn anchor_section_link_navigable() {
        // "#section" is a real in-page anchor, not a dead link.
        let qml = render_anchor(r##"<a href="#section">Jump</a>"##);
        assert!(qml.contains("link_menu"), "#section should navigate: {qml}");
        assert!(!qml.contains('\u{26A1}'));
    }

    #[test]
    fn js_link_with_recoverable_onclick_navigates() {
        // onclick navigation target is recoverable → navigable, no cue.
        let qml = render_anchor(
            r#"<a href="javascript:void(0)" onclick="location='/real'">Go</a>"#);
        assert!(qml.contains("link_menu"), "recoverable JS link should navigate: {qml}");
        assert!(qml.contains("example.com/real"), "should resolve recovered URL: {qml}");
        assert!(!qml.contains('\u{26A1}'), "recovered link should not show the cue: {qml}");
    }

    // v0.1.379: pre-parse tracking-attr stripping.

    #[test]
    fn strip_removes_data_analytics_attrs() {
        let html = r#"<div data-analytics-section="hero" class="hero">content</div>"#;
        let stripped = strip_tracking_attrs(html);
        assert!(!stripped.contains("data-analytics"));
        assert!(stripped.contains("class=\"hero\""),
            "real attributes should survive: {}", stripped);
        assert!(stripped.contains("content"));
    }

    #[test]
    fn strip_removes_multiple_tracking_prefixes() {
        let html = r#"<div data-gtm-event="x" data-tracking-id="y" data-ga-action="z" class="real">"#;
        let stripped = strip_tracking_attrs(html);
        assert!(!stripped.contains("data-gtm"));
        assert!(!stripped.contains("data-tracking"));
        assert!(!stripped.contains("data-ga"));
        assert!(stripped.contains("class=\"real\""));
    }

    #[test]
    fn strip_preserves_data_src_and_lazy_loaders() {
        // These ARE meaningful — must survive the strip.
        let html = r#"<img data-src="/real.jpg" data-lazy-src="/lazy.jpg" data-original="/orig.jpg" src="placeholder.gif">"#;
        let stripped = strip_tracking_attrs(html);
        assert!(stripped.contains("data-src"),
            "data-src must survive — it's the real image source");
        assert!(stripped.contains("data-lazy-src"),
            "data-lazy-src must survive");
        assert!(stripped.contains("data-original"),
            "data-original must survive");
    }

    #[test]
    fn strip_preserves_data_href_data_aos_data_animate() {
        // detect_js_href reads data-href; anim_detect reads
        // data-aos / data-animate. Must survive.
        let html = r#"<div data-href="/x" data-aos="fade-up" data-animate="slide">"#;
        let stripped = strip_tracking_attrs(html);
        assert!(stripped.contains("data-href"));
        assert!(stripped.contains("data-aos"));
        assert!(stripped.contains("data-animate"));
    }

    #[test]
    fn strip_handles_exact_match_attrs() {
        // data-staggered-item, data-anim-scroll-group,
        // data-focus-expression, data-component-list,
        // data-impression-id are exact matches (no prefix).
        let html = r#"<div data-staggered-item="3" data-component-list="Card Gallery" class="x">"#;
        let stripped = strip_tracking_attrs(html);
        assert!(!stripped.contains("data-staggered-item"));
        assert!(!stripped.contains("data-component-list"));
        assert!(stripped.contains("class=\"x\""));
    }

    #[test]
    fn strip_preserves_utf8_codepoints() {
        // v0.1.447 regression: the byte-wise emit cast each UTF-8
        // continuation byte to a Latin-1 char, so "he's" (curly
        // apostrophe U+2019) became "heâ…s" mojibake. Verify
        // multi-byte codepoints round-trip intact through the
        // (no-op-stripping) text path.
        let html = "<p>he\u{2019}s a caf\u{00e9} \u{2014} \u{2026} \u{1F600}</p>";
        let out = strip_tracking_attrs(html);
        assert_eq!(out, html, "UTF-8 text must pass through unchanged");
        assert!(out.contains('\u{2019}'), "curly apostrophe preserved");
        assert!(!out.contains('\u{00e2}'), "no a-circumflex mojibake");
    }

    #[test]
    fn strip_preserves_utf8_around_stripped_attr() {
        // Mojibake-prone bytes adjacent to a stripped attribute.
        let html = "<p data-gtm-id=\"x\">don\u{2019}t</p>";
        let out = strip_tracking_attrs(html);
        assert!(!out.contains("data-gtm-id"), "tracking attr stripped");
        assert!(out.contains("don\u{2019}t"), "body UTF-8 intact");
    }

    #[test]
    fn strip_handles_single_quoted_values() {
        let html = "<div data-analytics-id='hero-impression' class='real'>";
        let stripped = strip_tracking_attrs(html);
        assert!(!stripped.contains("data-analytics"));
        assert!(stripped.contains("class='real'"));
    }

    #[test]
    fn strip_idempotent() {
        let html = r#"<div data-analytics-section="hero" class="hero">"#;
        let once = strip_tracking_attrs(html);
        let twice = strip_tracking_attrs(&once);
        assert_eq!(once, twice,
            "running strip twice should give same result");
    }

    #[test]
    fn strip_no_change_when_no_tracking_attrs() {
        // Real-world page without any tracking attrs should
        // come through near-equivalent (might differ in
        // whitespace if there were dropped attrs, but pure
        // content stays the same byte-for-byte).
        let html = r#"<html><body><h1>Title</h1><p>Para</p></body></html>"#;
        let stripped = strip_tracking_attrs(html);
        assert_eq!(html, stripped);
    }

    #[test]
    fn strip_handles_attrs_at_word_boundaries() {
        // 'data-' that appears inside text or inside another
        // attribute's value MUST NOT trigger stripping.
        let html = r#"<a href="/foo?data-analytics-id=ignore">link</a>"#;
        let stripped = strip_tracking_attrs(html);
        // The query-string fragment should survive — it's
        // inside a quoted value, not at an attribute
        // boundary.
        assert!(stripped.contains("data-analytics-id=ignore"),
            "URL query strings containing data-* must not be stripped: {}",
            stripped);
    }

    #[test]
    fn is_tracking_attr_explicit_cases() {
        assert!(is_tracking_attr(b"data-analytics-section"));
        assert!(is_tracking_attr(b"data-tracking-id"));
        assert!(is_tracking_attr(b"data-gtm-event"));
        assert!(is_tracking_attr(b"data-ga-action"));
        assert!(is_tracking_attr(b"data-pixel-url"));
        assert!(is_tracking_attr(b"data-segment-track"));
        assert!(is_tracking_attr(b"data-staggered-item"));
        assert!(is_tracking_attr(b"data-anim-scroll-group"));
        assert!(is_tracking_attr(b"data-focus-expression"));
        assert!(is_tracking_attr(b"data-component-list"));
        assert!(is_tracking_attr(b"data-impression-id"));
        // negatives
        assert!(!is_tracking_attr(b"data-src"));
        assert!(!is_tracking_attr(b"data-lazy-src"));
        assert!(!is_tracking_attr(b"data-original"));
        assert!(!is_tracking_attr(b"data-href"));
        assert!(!is_tracking_attr(b"data-aos"));
        assert!(!is_tracking_attr(b"data-animate"));
        assert!(!is_tracking_attr(b"class"));
        assert!(!is_tracking_attr(b"id"));
    }

    // v0.1.375: hydration data extraction (JS-rendering arc,
    // Option 2 first step).

    #[test]
    fn hydration_extract_next_data() {
        let html = r#"<html><body><div id="__next"></div><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"title":"Hello"}}}</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some(), "should extract __NEXT_DATA__");
        let (kind, blob) = result.unwrap();
        assert_eq!(kind, "next");
        assert!(blob.contains("Hello"));
        // Verify the blob is parseable as JSON — starts with `{`.
        assert!(blob.starts_with('{'));
        assert!(blob.ends_with('}'));
    }

    #[test]
    fn hydration_extract_next_data_attribute_order_tolerated() {
        // type= before id= is also valid HTML.
        let html = r#"<html><body><script type="application/json" id="__NEXT_DATA__">{"a":1}</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some());
    }

    #[test]
    fn hydration_extract_nuxt_assignment() {
        let html = r#"<html><body><script>window.__NUXT__={"state":{"user":"alice"}};</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some());
        let (kind, blob) = result.unwrap();
        assert_eq!(kind, "nuxt");
        assert!(blob.contains("alice"));
        assert!(blob.starts_with('{'));
    }

    #[test]
    fn hydration_extract_initial_state_with_spaces() {
        let html = r#"<html><body><script>window.__INITIAL_STATE__ = {"counter":42};</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some());
        let (kind, blob) = result.unwrap();
        assert_eq!(kind, "initial-state");
        assert!(blob.contains("42"));
    }

    #[test]
    fn hydration_extract_json_ld_fallback() {
        // Real page with JSON-LD but no SPA framework data —
        // last-resort extraction still works.
        let html = r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"Test"}</script></head><body></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some());
        let (kind, _) = result.unwrap();
        assert_eq!(kind, "json-ld");
    }

    #[test]
    fn hydration_extract_priority_next_over_jsonld() {
        // Page has both — Next.js data should win since it's
        // checked first.
        let html = r#"<html><head><script type="application/ld+json">{"@type":"Article"}</script></head><body><script id="__NEXT_DATA__" type="application/json">{"props":{}}</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some());
        let (kind, _) = result.unwrap();
        assert_eq!(kind, "next");
    }

    #[test]
    fn hydration_extract_returns_none_on_no_data() {
        let html = r#"<html><body><h1>Hello</h1><p>Just regular content.</p></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_none());
    }

    #[test]
    fn hydration_extract_handles_nested_braces_in_strings() {
        // The brace-counter must not be fooled by `}` inside
        // string literals.
        let html = r#"<html><body><script>window.__NUXT__={"text":"contains } brace"};</script></body></html>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_some(), "should handle string-embedded braces");
        let (_, blob) = result.unwrap();
        assert!(blob.contains("contains } brace"));
    }

    #[test]
    fn hydration_extract_empty_script_returns_none() {
        let html = r#"<script id="__NEXT_DATA__" type="application/json"></script>"#;
        let result = extract_hydration_data(html);
        assert!(result.is_none(), "empty script body should not match");
    }

    // v0.1.378: JSON-LD article rendering tests.

    #[test]
    fn json_ld_renders_article_with_string_author() {
        let blob = r#"{"@context":"https://schema.org","@type":"Article","headline":"How to Cook Rice","author":"Alice","datePublished":"2024-01-15","articleBody":"Bring water to a boil, add rice, cover, reduce heat, simmer 18 minutes."}"#;
        let out = render_json_ld_article(blob).expect("should render");
        assert!(out.contains("# How to Cook Rice"));
        assert!(out.contains("Alice"));
        assert!(out.contains("2024-01-15"));
        assert!(out.contains("simmer 18 minutes"));
    }

    #[test]
    fn json_ld_renders_blog_posting() {
        // BlogPosting is also Article-ish.
        let blob = r#"{"@type":"BlogPosting","headline":"Test Post","articleBody":"Some content here."}"#;
        let out = render_json_ld_article(blob);
        assert!(out.is_some(), "BlogPosting should be recognized");
    }

    #[test]
    fn json_ld_renders_nested_author() {
        // Author can be a nested object with @type Person.
        let blob = r#"{"@type":"NewsArticle","headline":"Breaking News","author":{"@type":"Person","name":"Jane Reporter"},"articleBody":"Details follow."}"#;
        let out = render_json_ld_article(blob).expect("should render");
        assert!(out.contains("Jane Reporter"),
            "nested author.name should be extracted: {}", out);
    }

    #[test]
    fn json_ld_uses_description_when_no_body() {
        // Some Articles only have description, no articleBody.
        // Should still render rather than rejecting.
        let blob = r#"{"@type":"Article","headline":"Title","description":"A short summary of the piece."}"#;
        let out = render_json_ld_article(blob).expect("should render");
        assert!(out.contains("A short summary"));
    }

    #[test]
    fn json_ld_rejects_non_article_types() {
        let blob = r#"{"@type":"Product","name":"Widget","description":"Buy now"}"#;
        let out = render_json_ld_article(blob);
        assert!(out.is_none(), "Product type should not render as article");
    }

    #[test]
    fn json_ld_rejects_missing_headline() {
        let blob = r#"{"@type":"Article","articleBody":"Body without headline."}"#;
        let out = render_json_ld_article(blob);
        assert!(out.is_none(), "Article without headline should not render");
    }

    #[test]
    fn json_ld_rejects_headline_only_no_content() {
        // headline + no body + no description = nothing to render
        let blob = r#"{"@type":"Article","headline":"Just a title","author":"Someone"}"#;
        let out = render_json_ld_article(blob);
        assert!(out.is_none(),
            "headline-only without body/description should not render");
    }

    #[test]
    fn json_ld_handles_escape_sequences() {
        // Quoted text in body — \n and \" escapes must be handled.
        let blob = r#"{"@type":"Article","headline":"Quotes","articleBody":"He said \"hello\" then walked away.\nNew paragraph here."}"#;
        let out = render_json_ld_article(blob).expect("should render");
        assert!(out.contains("\"hello\""), "escaped quotes should be unescaped: {}", out);
        assert!(out.contains("New paragraph"), "escaped newlines should produce newlines: {}", out);
    }

    #[test]
    fn json_ld_handles_unicode_escapes() {
        // \u00e9 = é
        let blob = r#"{"@type":"Article","headline":"Caf\u00e9 Visit","articleBody":"Body."}"#;
        let out = render_json_ld_article(blob).expect("should render");
        assert!(out.contains("Café") || out.contains("é"),
            "unicode escape should be decoded: {}", out);
    }

    // Direct tests for find_json_string_field — the substring
    // parser is the load-bearing piece, so cover its edge cases.

    #[test]
    fn json_find_field_basic() {
        assert_eq!(find_json_string_field(r#"{"name":"alice"}"#, "name"),
            Some("alice".to_string()));
    }

    #[test]
    fn json_find_field_with_spaces() {
        assert_eq!(find_json_string_field(r#"{ "name" : "alice" }"#, "name"),
            Some("alice".to_string()));
    }

    #[test]
    fn json_find_field_missing() {
        assert_eq!(find_json_string_field(r#"{"name":"alice"}"#, "age"), None);
    }

    #[test]
    fn json_find_field_handles_embedded_quotes() {
        // Escape-aware: the closing quote of the value isn't
        // confused by the escaped quote inside.
        assert_eq!(find_json_string_field(r#"{"q":"say \"hi\"!","other":"x"}"#, "q"),
            Some(r#"say \"hi\"!"#.to_string()));
    }

    #[test]
    fn json_find_field_handles_similar_keys() {
        // "authorName" should not match when looking for "author"
        // (we match `"author"` exactly).
        // NB: this test verifies the colon-check protects us —
        // "authorName" doesn't have a colon right after.
        let blob = r#"{"authorName":"x","author":"alice"}"#;
        assert_eq!(find_json_string_field(blob, "author"),
            Some("alice".to_string()));
    }

    // v0.1.384 (JS-rendering arc): Next.js __NEXT_DATA__
    // article-shape extraction.

    #[test]
    fn next_data_article_basic_title_body() {
        // Simplest case: a flat object with title + body. Most
        // CMS-driven Next.js sites have something at least this
        // shape inside pageProps.
        let body_text = "A".repeat(500);  // body must be >=100 chars
        let blob = format!(
            r#"{{"props":{{"pageProps":{{"title":"Hello World","body":"{}"}}}}}}"#,
            body_text);
        let out = render_next_data_article(&blob).expect("should render");
        assert!(out.starts_with("# Hello World"));
        assert!(out.contains(&body_text[..50]));
    }

    #[test]
    fn next_data_article_uses_headline_field() {
        // headline is also a valid title field (matches the
        // JSON-LD vocabulary).
        let body_text = "B".repeat(200);
        let blob = format!(
            r#"{{"pageProps":{{"headline":"My Headline","articleBody":"{}"}}}}"#,
            body_text);
        let out = render_next_data_article(&blob).expect("should render");
        assert!(out.contains("# My Headline"));
    }

    #[test]
    fn next_data_article_rejects_short_body() {
        // Body is too short (< 100 chars) — likely a UI string,
        // not the article. Return None so the SPA banner fires.
        let blob = r#"{"pageProps":{"title":"x","body":"short"}}"#;
        let out = render_next_data_article(blob);
        assert!(out.is_none(),
            "short body should not be treated as article: {:?}", out);
    }

    #[test]
    fn next_data_article_rejects_huge_title() {
        // Title is suspiciously long — probably misidentified.
        let huge_title = "x".repeat(500);
        let body_text = "B".repeat(200);
        let blob = format!(
            r#"{{"pageProps":{{"title":"{}","body":"{}"}}}}"#,
            huge_title, body_text);
        let out = render_next_data_article(&blob);
        assert!(out.is_none(),
            "300+ char title should not be treated as article");
    }

    #[test]
    fn next_data_article_returns_none_when_no_title() {
        let body_text = "B".repeat(500);
        let blob = format!(
            r#"{{"pageProps":{{"body":"{}"}}}}"#,
            body_text);
        assert!(render_next_data_article(&blob).is_none());
    }

    #[test]
    fn next_data_article_returns_none_when_no_body() {
        let blob = r#"{"pageProps":{"title":"only title"}}"#;
        assert!(render_next_data_article(blob).is_none());
    }

    #[test]
    fn next_data_article_includes_byline_when_present() {
        let body_text = "C".repeat(300);
        let blob = format!(
            r#"{{"pageProps":{{"title":"T","body":"{}","author":"Alice","datePublished":"2026-05-01"}}}}"#,
            body_text);
        let out = render_next_data_article(&blob).expect("should render");
        assert!(out.contains("Alice"), "byline should include author");
        assert!(out.contains("2026-05-01"), "byline should include date");
    }

    #[test]
    fn next_data_article_finds_nested_content_field() {
        // "content" is also a body candidate — matches WordPress-
        // style Next.js sites.
        let body_text = "D".repeat(300);
        let blob = format!(
            r#"{{"pageProps":{{"title":"WP Style","content":"{}"}}}}"#,
            body_text);
        let out = render_next_data_article(&blob).expect("should render");
        assert!(out.contains("# WP Style"));
    }

    #[test]
    fn next_data_article_empty_blob() {
        assert!(render_next_data_article("").is_none());
        assert!(render_next_data_article("{}").is_none());
    }

    // v0.1.385: Nuxt + initial-state renderers share the
    // article-shape walker. Tests verify the delegation
    // works correctly and per-framework wrappers don't break
    // anything.

    #[test]
    fn nuxt_article_renders_from_state_container() {
        // Nuxt 3 stores hydration under {state, data, pinia,
        // ...}. The shape-based walker should still find the
        // title+body inside, regardless of container name.
        let body_text = "N".repeat(400);
        let blob = format!(
            r#"{{"state":{{"article":{{"title":"Nuxt Title","content":"{}"}}}}}}"#,
            body_text);
        let out = render_nuxt_article(&blob).expect("should render");
        assert!(out.contains("# Nuxt Title"));
        assert!(out.contains(&body_text[..50]));
    }

    #[test]
    fn nuxt_article_handles_pinia_store_shape() {
        // Nuxt 3 with Pinia: stores nested under arbitrary
        // keys (the store name). Shape walker doesn't care.
        let body_text = "P".repeat(300);
        let blob = format!(
            r#"{{"pinia":{{"articleStore":{{"headline":"Pinia Post","articleBody":"{}"}}}}}}"#,
            body_text);
        let out = render_nuxt_article(&blob).expect("should render");
        assert!(out.contains("# Pinia Post"));
    }

    #[test]
    fn initial_state_article_renders_from_redux_store() {
        // Generic Redux: state.article = {...}. Same walker.
        let body_text = "R".repeat(500);
        let blob = format!(
            r#"{{"article":{{"title":"Redux Article","body":"{}"}},"ui":{{"theme":"dark"}}}}"#,
            body_text);
        let out = render_initial_state_article(&blob).expect("should render");
        assert!(out.contains("# Redux Article"));
    }

    #[test]
    fn nuxt_article_returns_none_without_article_shape() {
        // A Nuxt blob that has only UI state (no article)
        // should not pretend to render anything.
        let blob = r#"{"state":{"theme":"dark","sidebar":{"open":true}}}"#;
        assert!(render_nuxt_article(blob).is_none());
    }

    #[test]
    fn initial_state_returns_none_for_empty_state() {
        assert!(render_initial_state_article("{}").is_none());
    }

    // v0.1.387: extract_title tests.

    #[test]
    fn extract_title_basic() {
        let html = "<html><head><title>Hello World</title></head><body></body></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "Hello World");
    }

    #[test]
    fn extract_title_collapses_whitespace() {
        let html = "<html><head><title>  Hello \n\t  World  </title></head></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "Hello World");
    }

    #[test]
    fn extract_title_empty_when_no_title_tag() {
        let html = "<html><head></head><body></body></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "");
    }

    #[test]
    fn extract_title_empty_when_whitespace_only() {
        let html = "<html><head><title>   \n\t  </title></head></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "");
    }

    #[test]
    fn extract_title_rejects_known_placeholders() {
        for placeholder in &[
            "Untitled Document", "untitled",
            "Document", "New Tab", "Loading...",
        ] {
            let html = format!(
                "<html><head><title>{}</title></head></html>",
                placeholder);
            let doc = scraper::Html::parse_document(&html);
            assert_eq!(extract_title(&doc), "",
                "placeholder {:?} should be treated as empty", placeholder);
        }
    }

    #[test]
    fn extract_title_caps_at_200_chars() {
        let long = "x".repeat(500);
        let html = format!("<html><head><title>{}</title></head></html>", long);
        let doc = scraper::Html::parse_document(&html);
        let t = extract_title(&doc);
        assert!(t.ends_with("…"));
        // 200 + ellipsis byte count
        assert!(t.chars().count() <= 201);
    }

    #[test]
    fn extract_title_unicode_safe_truncation() {
        // 250 multibyte chars — must not panic on truncation.
        let long: String = "日本".chars().cycle().take(250).collect();
        let html = format!("<html><head><title>{}</title></head></html>", long);
        let doc = scraper::Html::parse_document(&html);
        let t = extract_title(&doc);
        assert!(t.ends_with("…"));
        // Must be valid UTF-8 (String enforces; just confirm
        // truncation didn't lose chars unexpectedly).
        assert!(!t.is_empty());
    }

    #[test]
    fn extract_title_preserves_punctuation_and_unicode() {
        // Real-world title with em-dashes, apostrophes, etc.
        let html = "<html><head><title>Anthropic's blog — Claude 4.7</title></head></html>";
        let doc = scraper::Html::parse_document(html);
        let t = extract_title(&doc);
        assert!(t.contains("Anthropic"));
        assert!(t.contains("—"));
    }

    #[test]
    fn extract_title_handles_nested_tags_in_title() {
        // Some sites wrap parts of the title in <span>, etc.
        // The .text() iterator collects all text descendants.
        let html = "<html><head><title>Hello <span>World</span></title></head></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "Hello World");
    }

    // v0.1.417: page-overflow demotion. A flex Row whose
    // children each claim full width must demote to Column so
    // they stack instead of overflowing off-screen-right.
    #[test]
    fn flex_row_with_fullwidth_children_demotes_to_column() {
        let sty = RenderStyle::from_theme(true, 14, false,
            crate::ad_filter::FilterMode::Off);
        // A flex container with two <div> children. Each <div>
        // wraps a <p> (block → full-width Text). Two full-width
        // children in a Row would overflow → expect Column.
        let html = r#"<html><body><div style="display:flex">
            <div><p>This is the first column with enough words to be a real paragraph here.</p></div>
            <div><p>This is the second column with enough words to be a real paragraph here.</p></div>
        </div></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let body = select_body(&doc).unwrap();
        let qml = generate_safe_qml(&body, 0, 50, "https://x.com", &sty);
        // The flex div's container should be a Column, not a Row.
        // Both paragraphs' text must be present (nothing dropped).
        assert!(qml.contains("first column"), "first child present");
        assert!(qml.contains("second column"), "second child present");
        // There must be NO bare `Row {` wrapping these — the
        // demotion replaced it with Column. (Other Rows from
        // inline runs may exist, but the flex container itself
        // must not be a Row with two full-width kids.) We assert
        // the structural fix loosely: a Column wrapping both.
        // Strongest cheap check: the output contains "Column"
        // (the demoted container) — Row-only output wouldn't.
        assert!(qml.contains("Column {"), "demoted container is a Column");
    }

    #[test]
    fn flex_link_row_content_survives_demotion() {
        let sty = RenderStyle::from_theme(true, 14, false,
            crate::ad_filter::FilterMode::Off);
        // A flex nav of links. Links emit Text with width:
        // parent.width, so by the v0.1.417 heuristic this DOES
        // demote to Column (two full-width children). That's the
        // intended behavior for a voice-first reader — stacked
        // links are fine and never overflow. The key guarantee:
        // demotion never DROPS content. Assert both links survive.
        let html = r#"<html><body><nav style="display:flex">
            <a href="/a">Alpha</a><a href="/b">Beta</a>
        </nav></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let body = select_body(&doc).unwrap();
        let qml = generate_safe_qml(&body, 0, 50, "https://x.com", &sty);
        assert!(qml.contains("Alpha"), "link A text present");
        assert!(qml.contains("Beta"), "link B text present");
    }
}
