//! html -- a script as one self-contained page for printing: no scripts, no external fonts or files.
//!
//! The text column is centred on the page (`margin: 0 auto`). Spoken lines (and their speakers, directions
//! and sections) are `text-align: center` inside it, TRULY centred, so a wrapped line stays centred about the
//! same axis; scene headings, action and transitions sit at the left of the column as on a screenplay page
//! (`Options::center_all` centres those too). No rule anywhere uses an indent, a left margin or left padding
//! (a test holds the CSS to that).

use crate::script::{Item, Meta, Script};
use crate::style::Options;

/// Escape for HTML text.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Escape, then apply `***bold italic***`, `**bold**`, `*italic*` and `_underline_`. A marker with no
/// partner stays literal, and a `\*` or `\_` is the plain character. `\n` becomes `<br>`.
pub fn inline(s: &str) -> String {
    // tokenise on the escaped text, so a `<` typed by a person can never become a tag
    let chars: Vec<char> = esc(s).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '*' || chars[i + 1] == '_') {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '\n' { out.push_str("<br>"); i += 1; continue; }
        // `_` only marks underline at a word boundary: snake_case_name stays as typed
        let word_char = |ch: char| ch.is_alphanumeric();
        if c == '_' && (i > 0 && word_char(chars[i - 1])) {
            out.push(c);
            i += 1;
            continue;
        }
        if c == '*' || c == '_' {
            let run = chars[i..].iter().take_while(|x| **x == c).count();
            let n = if c == '_' { 1 } else { run.min(3) };
            // find a closing run of the same marker
            let mark: String = std::iter::repeat(c).take(n).collect();
            let rest: String = chars[i + n..].iter().collect();
            if let Some(end) = rest.find(&mark) {
                let inner = &rest[..end];
                let after = rest[end + mark.len()..].chars().next();
                let closes_at_boundary = c != '_' || after.is_none_or(|a| !a.is_alphanumeric());
                if closes_at_boundary && !inner.is_empty() && !inner.starts_with(' ') && !inner.ends_with(' ') && !inner.contains('\n') {
                    let (open, close) = match (c, n) {
                        ('_', _) => ("<u>", "</u>"),
                        (_, 3) => ("<strong><em>", "</em></strong>"),
                        (_, 2) => ("<strong>", "</strong>"),
                        _ => ("<em>", "</em>"),
                    };
                    out.push_str(open);
                    out.push_str(&inline_unescaped(inner));
                    out.push_str(close);
                    i += n + inner.chars().count() + n;
                    continue;
                }
            }
            for _ in 0..n { out.push(c); }
            i += n;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `inline` for text that is ALREADY escaped (the inside of a marked span): no second escape.
fn inline_unescaped(escaped: &str) -> String {
    // the nested markers inside a span are rare; keep them literal rather than double-escaping
    escaped.to_string()
}

/// The text with its emphasis markers applied away (what a plain-text preview shows): `**a**` is `a`.
pub fn plain(s: &str) -> String {
    let marked = inline(s);
    let mut out = String::with_capacity(marked.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for c in marked.chars() {
        match c {
            '<' => { in_tag = true; tag.clear(); }
            '>' if in_tag => { in_tag = false; if tag == "br" { out.push('\n'); } }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

fn css(o: &Options) -> String {
    let s = &o.style;
    let size = o.size();
    let dark = s.paper != "#ffffff" && s.paper.starts_with("#1");
    let scene_caps = if s.scene.uppercase { "text-transform:uppercase;" } else { "" };
    let scene_sc = if s.scene.small_caps { "font-variant:small-caps;" } else { "" };
    let scene_b = if s.scene.bold { "font-weight:700;" } else { "font-weight:400;" };
    let rule = if s.scene.rule { format!("padding-bottom:.45em;border-bottom:1px solid {};", s.accent) } else { String::new() };
    let sp_caps = if s.speaker.uppercase { "text-transform:uppercase;" } else { "" };
    let sp_sc = if s.speaker.small_caps { "font-variant:small-caps;" } else { "" };
    let sp_b = if s.speaker.bold { "font-weight:700;" } else { "font-weight:400;" };
    let page_numbers = if o.page_numbers { "@bottom-center{content:counter(page);font:10pt var(--font);color:#555}" } else { "" };
    let print_ink = if dark { "html,body{background:#fff!important;color:#000!important}.scene{border-color:#000!important}" } else { "" };
    format!(
"@page{{size:{paper};margin:1in;{page_numbers}}}
:root{{--font:{font};--ink:{ink};--paper:{paper_bg};--accent:{accent}}}
*{{box-sizing:border-box}}
html{{background:var(--paper);color:var(--ink)}}
body{{margin:0;padding:2.5rem 1rem;font-family:var(--font);font-size:{size}pt;line-height:{leading};text-align:center;-webkit-print-color-adjust:exact;print-color-adjust:exact}}
main{{width:min(100%,{column}in);margin:0 auto;text-align:center}}
.line,.direction,.section,.titlepage *{{text-align:center}}
.scene,.action,.transition{{text-align:{block_align}}}
.line{{margin:0 0 .85em}}
.line.cont{{margin-top:-.45em}}
.speaker{{display:block;margin:1.3em 0 .15em;letter-spacing:{sp_ls}em;{sp_caps}{sp_sc}{sp_b}}}
.said{{display:block}}
.direction{{margin:0 0 .85em;font-style:italic}}
.action{{margin:0 0 .85em}}
.transition{{margin:0 0 .85em;text-transform:uppercase}}
.scene{{margin:2.4em 0 1.1em;letter-spacing:{sc_ls}em;{scene_caps}{scene_sc}{scene_b}{rule}break-after:avoid;font-size:1em}}
.scene .num{{margin-right:.7em;color:var(--accent)}}
.section{{margin:2em 0 .6em;font-size:.85em;letter-spacing:.2em;text-transform:uppercase;color:var(--accent);break-after:avoid}}
.pagebreak{{height:0;break-after:page;page-break-after:always}}
.titlepage{{min-height:7in;display:flex;flex-direction:column;justify-content:center;align-items:center;break-after:page;page-break-after:always}}
.titlepage h1{{font-size:2.1em;letter-spacing:{sc_ls}em;margin:0 0 .5em;{scene_caps}}}
.titlepage p{{margin:.25em 0}}
.titlepage .credit{{margin-top:1.5em;font-style:italic}}
.titlepage .small{{margin-top:3em;font-size:.8em;opacity:.75;white-space:pre-line}}
@media print{{html{{background:#fff;color:#000}}body{{padding:0}}{print_ink}}}
",
        block_align = if o.center_all { "center" } else { "left" },
        paper = o.paper.css(), page_numbers = page_numbers, font = o.font_stack(), ink = s.ink, paper_bg = s.paper, accent = s.accent,
        size = size, leading = s.leading, column = s.column_in,
        sp_ls = s.speaker.spacing_em, sc_ls = s.scene.spacing_em,
        sp_caps = sp_caps, sp_sc = sp_sc, sp_b = sp_b, scene_caps = scene_caps, scene_sc = scene_sc, scene_b = scene_b, rule = rule, print_ink = print_ink,
    )
}

fn title_page(m: &Meta) -> String {
    let mut h = String::from("<section class=\"titlepage\">");
    if let Some(t) = &m.title { h.push_str(&format!("<h1>{}</h1>", inline(t))); }
    if let Some(c) = &m.credit { h.push_str(&format!("<p class=\"credit\">{}</p>", inline(c))); }
    if let Some(a) = &m.author { h.push_str(&format!("<p class=\"author\">{}</p>", inline(a))); }
    if let Some(s) = &m.source { h.push_str(&format!("<p>{}</p>", inline(s))); }
    let small: Vec<String> = [m.draft_date.as_ref(), m.contact.as_ref(), m.copyright.as_ref(), m.notes.as_ref()].into_iter().flatten().map(|s| esc(s)).collect();
    if !small.is_empty() { h.push_str(&format!("<p class=\"small\">{}</p>", small.join("\n"))); }
    h.push_str("</section>");
    h
}

/// The whole page.
pub fn render(script: &Script, o: &Options) -> String {
    let title = script.meta.title.clone().unwrap_or_else(|| "script".to_string());
    let mut body = String::new();
    if o.title_page && !script.meta.is_empty() { body.push_str(&title_page(&script.meta)); }
    let mut scene_no = 0;
    let mut prev_speaker_line = false;
    for item in &script.items {
        match item {
            Item::Scene(t) => {
                scene_no += 1;
                let num = if o.scene_numbers { format!("<span class=\"num\">{scene_no}.</span>") } else { String::new() };
                body.push_str(&format!("<h2 class=\"scene\">{num}{}</h2>\n", inline(t)));
                prev_speaker_line = false;
            }
            Item::Line { speaker, text } => {
                match speaker {
                    Some(n) => {
                        body.push_str(&format!("<p class=\"line\"><span class=\"speaker\">{}</span><span class=\"said\">{}</span></p>\n", inline(n), inline(text)));
                        prev_speaker_line = true;
                    }
                    None => {
                        let cont = if prev_speaker_line { " cont" } else { "" };
                        body.push_str(&format!("<p class=\"line{cont}\">{}</p>\n", inline(text)));
                    }
                }
            }
            Item::Direction(t) => { body.push_str(&format!("<p class=\"direction\">{}</p>\n", inline(t))); prev_speaker_line = false; }
            Item::Action(t) => { body.push_str(&format!("<p class=\"action\">{}</p>\n", inline(t))); prev_speaker_line = false; }
            Item::Transition(t) => { body.push_str(&format!("<p class=\"transition\">{}</p>\n", inline(t))); prev_speaker_line = false; }
            Item::Section { text, level } => { body.push_str(&format!("<h3 class=\"section l{level}\">{}</h3>\n", inline(text))); prev_speaker_line = false; }
            Item::PageBreak => { body.push_str("<div class=\"pagebreak\"></div>\n"); prev_speaker_line = false; }
        }
        if !matches!(item, Item::Line { speaker: None, .. }) && !matches!(item, Item::Line { speaker: Some(_), .. }) { prev_speaker_line = false; }
    }
    format!("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{}</title>\n<style>\n{}</style>\n</head>\n<body>\n<main>\n{}</main>\n</body>\n</html>\n",
        esc(&title), css(o), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{parse, Dialect};
    use crate::style::{Options, Paper, STYLES};

    fn page(text: &str, o: &Options) -> String { render(&parse(text, Dialect::Quick), o) }

    #[test]
    fn dialogue_is_truly_centred_scenes_and_action_sit_left_and_nothing_is_indented() {
        for style in STYLES {
            let h = page("# a\nmara: hi\nplain line that is long enough to wrap onto a second line when it needs to\n(beat)", &Options { style, ..Options::default() });
            let css = &h[h.find("<style>").unwrap()..h.find("</style>").unwrap()];
            assert!(css.contains(".line,.direction,.section,.titlepage *{text-align:center}"), "{}: spoken lines are centred", style.name);
            assert!(css.contains(".scene,.action,.transition{text-align:left}"), "{}: scenes and action are left by default", style.name);
            assert!(css.contains("main{width:min(100%,") && css.contains("margin:0 auto;text-align:center}"), "{}: the column is centred on the page", style.name);
            for banned in ["text-indent", "padding-left", "margin-left", "text-align:right", "text-align:justify", "float:"] {
                assert!(!css.contains(banned), "{}: the CSS must not use {banned}", style.name);
            }
            assert!(h.contains("<p class=\"line\">") && h.contains("<h2 class=\"scene\">"), "{}", style.name);
            // the owner's switch: centre everything
            let all = page("# a", &Options { style, center_all: true, ..Options::default() });
            assert!(all.contains(".scene,.action,.transition{text-align:center}"), "{}: centre-all centres them too", style.name);
        }
    }

    #[test]
    fn fountain_action_and_transitions_get_their_own_classes() {
        let h = render(&parse("EXT. ALLEY - NIGHT\n\nA lamp buzzes.\nFlies circle it.\n\nJASMINE\nHello.\n\nCUT TO:\n", Dialect::Fountain), &Options::default());
        assert!(h.contains("<h2 class=\"scene\">EXT. ALLEY - NIGHT</h2>"));
        assert!(h.contains("<p class=\"action\">A lamp buzzes.<br>Flies circle it.</p>"), "{h}");
        assert!(h.contains("<span class=\"speaker\">JASMINE</span><span class=\"said\">Hello.</span>"));
        assert!(h.contains("<p class=\"transition\">CUT TO:</p>"));
    }

    #[test]
    fn text_is_escaped_and_emphasis_works() {
        let h = page("# <b>x</b> & y\nsays \"hi\" <script>alert(1)</script>", &Options::default());
        assert!(!h.contains("<script>") && !h.contains("<b>x"), "no markup gets through");
        assert!(h.contains("&lt;b&gt;x&lt;/b&gt; &amp; y") && h.contains("&quot;hi&quot;"));
        assert_eq!(inline("a *b* **c** ***d*** _e_ f"), "a <em>b</em> <strong>c</strong> <strong><em>d</em></strong> <u>e</u> f");
        assert_eq!(inline("2 * 3 = 6 and snake_case_name"), "2 * 3 = 6 and snake_case_name", "a lone star stays; underscores inside words stay");
        assert_eq!(inline("an _underlined_ word"), "an <u>underlined</u> word");
        assert_eq!(inline("a * b"), "a * b", "a star with no partner is literal");
        assert_eq!(inline("\\*not italic\\*"), "*not italic*");
        assert_eq!(inline("line one\nline two"), "line one<br>line two");
        assert_eq!(inline("unclosed *star"), "unclosed *star");
    }

    #[test]
    fn the_title_page_comes_from_the_meta_and_is_optional() {
        let s = crate::script::parse("Title: FEMME FATALE\nAuthor: N. M.\nCredit: Written by\nDraft date: 2026-10-03\n\n# a\nx", Dialect::Fountain);
        let h = render(&s, &Options::default());
        assert!(h.contains("<section class=\"titlepage\"><h1>FEMME FATALE</h1>"), "{h}");
        assert!(h.contains("<title>FEMME FATALE</title>"));
        assert!(h.find("titlepage").unwrap() < h.find("class=\"section").unwrap(), "title page first");
        let h = render(&s, &Options { title_page: false, ..Options::default() });
        assert!(!h.contains("<section class=\"titlepage\""));
        let plain = page("# a\nx", &Options::default());
        assert!(!plain.contains("<section class=\"titlepage\""), "no title keys, no title page");
    }

    #[test]
    fn options_reach_the_page() {
        let o = Options { style: STYLES[3], font: Some("Georgia".into()), size_pt: Some(15.0), paper: Paper::A4, scene_numbers: true, page_numbers: false, ..Options::default() };
        let h = page("# one\nx\n===\n# two\ny", &o);
        assert!(h.contains("@page{size:A4;margin:1in;}"), "paper, and no page-number box when off");
        assert!(h.contains("--font:Georgia, "), "font override leads");
        assert!(h.contains("font-size:15pt"));
        assert!(h.contains("<span class=\"num\">1.</span>one") && h.contains("<span class=\"num\">2.</span>two"), "scene numbers");
        assert!(h.contains("<div class=\"pagebreak\"></div>"));
        let n = page("# a", &Options::default());
        assert!(n.contains("@bottom-center{content:counter(page)"), "page numbers by default");
        assert!(!n.contains("class=\"num\""), "no scene numbers by default");
    }

    #[test]
    fn the_dark_style_prints_black_on_white_and_the_others_are_left_alone() {
        let dark = page("# a", &Options { style: STYLES[4], ..Options::default() });
        assert!(dark.contains("html,body{background:#fff!important;color:#000!important}"));
        let light = page("# a", &Options::default());
        assert!(!light.contains("!important}"), "a light style needs no print override");
    }

    #[test]
    fn dialogue_sets_the_speaker_above_and_continuing_lines_tuck_under() {
        let h = render(&parse("JASMINE\nHello there.\n(softly)\nFor a price.\n\nCARL\nNo.", Dialect::Fountain), &Options::default());
        assert!(h.contains("<span class=\"speaker\">JASMINE</span><span class=\"said\">Hello there.</span>"));
        assert!(h.contains("<p class=\"direction\">(softly)</p>"));
        // the continuing line follows a parenthetical, not a speaker line: it is its own line
        assert!(h.contains("For a price.") && h.contains("<span class=\"speaker\">CARL</span>"));
    }

    #[test]
    fn plain_text_drops_markers_and_keeps_the_words() {
        assert_eq!(plain("a **bold** and *soft* _line_ & <b>"), "a bold and soft line & <b>");
        assert_eq!(plain("one\ntwo"), "one\ntwo");
        assert_eq!(plain("snake_case 2 * 3"), "snake_case 2 * 3");
    }
}
