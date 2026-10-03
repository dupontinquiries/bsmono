//! script -- plain text in, a [`Script`] out. Two dialects, one model.
//!
//! **Quick** (the fast way to type): a line starting with `#` is a scene; every other line is a line;
//! `NAME: words` puts a speaker over the words; `(words)` is a direction; `===` is a page break; blank
//! lines are ignored. `INT.` / `EXT.` lines are scenes without a `#`.
//!
//! **Fountain** (the screenwriting plain-text standard, so an existing `.fountain` opens as it is):
//! a title page of `Key: value` lines, scene headings (`INT.`/`EXT.`/`EST.`/`I/E`, or a leading `.`),
//! character cues (an UPPERCASE line right before dialogue, or `@Name`), parentheticals, transitions
//! (`CUT TO:`, `> TRANSITION`), centred text (`> text <`), sections (`#`), notes `[[ ]]`, boneyard
//! `/* */`, page breaks (`===`), lyrics (`~`), forced action (`!`).
//!
//! Both dialects produce the same items, and the HTML renderer centres them all: the model carries no
//! indent or alignment at all.

/// The title page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub title: Option<String>,
    pub credit: Option<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    pub draft_date: Option<String>,
    pub contact: Option<String>,
    pub copyright: Option<String>,
    pub notes: Option<String>,
    /// Any other `Key: value`.
    pub extra: Vec<(String, String)>,
}

impl Meta {
    pub fn is_empty(&self) -> bool { self == &Meta::default() }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A scene heading.
    Scene(String),
    /// A line, optionally spoken. A continuing line of the same speech has `speaker: None`.
    /// `text` may hold `\n` (a Fountain paragraph's own line breaks).
    Line { speaker: Option<String>, text: String },
    /// A parenthetical, a transition, a stage direction.
    Direction(String),
    /// A section heading (`#` in Fountain): an outline level and its title.
    Section { level: usize, text: String },
    PageBreak,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Script {
    pub meta: Meta,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Quick,
    Fountain,
}

impl Script {
    pub fn scenes(&self) -> usize { self.items.iter().filter(|i| matches!(i, Item::Scene(_))).count() }
    pub fn lines(&self) -> usize { self.items.iter().filter(|i| matches!(i, Item::Line { .. })).count() }
    pub fn words(&self) -> usize {
        self.items.iter().map(|i| match i {
            Item::Line { text, .. } => text.split_whitespace().count(),
            Item::Scene(t) | Item::Direction(t) => t.split_whitespace().count(),
            _ => 0,
        }).sum()
    }
}

/// Which dialect a text is: Fountain when named `.fountain`, or when it has a Fountain-only mark (a title
/// page key or an `INT.`/`EXT.` heading and no `#` scene lines); otherwise Quick.
pub fn detect(name: Option<&str>, text: &str) -> Dialect {
    if name.is_some_and(|n| n.to_ascii_lowercase().ends_with(".fountain")) { return Dialect::Fountain; }
    let has_hash_scene = text.lines().any(|l| l.trim_start().starts_with("# "));
    let has_heading = text.lines().any(|l| is_scene_heading(l.trim()));
    let has_title_key = text.lines().take(8).any(|l| meta_key(l).is_some());
    if !has_hash_scene && (has_heading || has_title_key) { Dialect::Fountain } else { Dialect::Quick }
}

/// Parse, detecting the dialect.
pub fn parse_any(name: Option<&str>, text: &str) -> Script { parse(text, detect(name, text)) }

pub fn is_scene_heading(l: &str) -> bool {
    let up = l.to_ascii_uppercase();
    for p in ["INT.", "EXT.", "EST.", "INT/EXT.", "INT./EXT.", "I/E.", "I/E "] {
        if up.starts_with(p) { return true; }
    }
    for p in ["INT ", "EXT ", "EST "] {
        if up.starts_with(p) { return true; }
    }
    false
}

/// `Key: value` for the title page, with the known keys.
fn meta_key(l: &str) -> Option<(String, String)> {
    let (k, v) = l.split_once(':')?;
    let k = k.trim();
    if k.is_empty() || k.len() > 24 || k.chars().any(|c| !(c.is_alphanumeric() || c == ' ' || c == '-')) { return None; }
    let known = ["title", "credit", "author", "authors", "source", "draft date", "date", "contact", "copyright", "notes", "revision", "draft"];
    known.contains(&k.to_ascii_lowercase().as_str()).then(|| (k.to_string(), v.trim().to_string()))
}

fn set_meta(m: &mut Meta, k: &str, v: String) {
    match k.to_ascii_lowercase().as_str() {
        "title" => m.title = Some(v),
        "credit" => m.credit = Some(v),
        "author" | "authors" => m.author = Some(v),
        "source" => m.source = Some(v),
        "draft date" | "date" => m.draft_date = Some(v),
        "contact" => m.contact = Some(v),
        "copyright" => m.copyright = Some(v),
        "notes" => m.notes = Some(v),
        _ => m.extra.push((k.to_string(), v)),
    }
}

/// Remove `/* boneyard */` and `[[ notes ]]` (which may span lines).
fn strip_hidden(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if text[i..].starts_with("/*") {
            match text[i + 2..].find("*/") { Some(e) => { i += 2 + e + 2; continue; } None => break }
        }
        if text[i..].starts_with("[[") {
            match text[i + 2..].find("]]") { Some(e) => { i += 2 + e + 2; continue; } None => break }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// An UPPERCASE cue: letters and digits and the punctuation a name takes, at least one letter, no lowercase
/// outside a trailing `(extension)`.
fn is_cue(l: &str) -> bool {
    let l = l.trim();
    if l.is_empty() || l.len() > 60 { return false; }
    let name = match l.find('(') { Some(p) => l[..p].trim_end(), None => l };
    if name.is_empty() || !name.chars().any(|c| c.is_alphabetic()) { return false; }
    if name.chars().any(|c| c.is_lowercase()) { return false; }
    if l.ends_with("TO:") { return false; } // a transition
    // the extension (V.O.), (O.S.), (CONT'D) is uppercase too; only (cont'd) is allowed in lowercase.
    // So "DETECTIVE CAROL DAVIS (30s)" in a character list is not a cue.
    if let Some(p) = l.find('(') {
        let ext = &l[p..];
        if ext.chars().any(|c| c.is_lowercase()) && !ext.eq_ignore_ascii_case("(cont'd)") { return false; }
    }
    name.chars().all(|c| c.is_alphanumeric() || " .'-^@#&/".contains(c))
}

fn is_transition(l: &str) -> bool {
    let t = l.trim();
    (t.ends_with("TO:") && t.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase())) || (t.starts_with('>') && !t.ends_with('<'))
}

/// `---` or `===` (three or more of one): a page break.
fn is_break(l: &str) -> bool {
    let t = l.trim();
    t.len() >= 3 && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-'))
}

pub fn parse(text: &str, dialect: Dialect) -> Script {
    let text = strip_hidden(&text.replace("\r\n", "\n"));
    let lines: Vec<&str> = text.lines().collect();
    let mut s = Script::default();
    let mut i = 0;
    // TITLE PAGE: Key: value lines at the very top, up to the first blank line
    while i < lines.len() && lines[i].trim().is_empty() { i += 1; }
    let mut j = i;
    let mut meta = Meta::default();
    let mut any = false;
    while j < lines.len() && !lines[j].trim().is_empty() {
        if let Some((k, v)) = meta_key(lines[j]) {
            let mut v = v;
            // indented continuation lines belong to the value
            while j + 1 < lines.len() && (lines[j + 1].starts_with("   ") || lines[j + 1].starts_with('\t')) && !lines[j + 1].trim().is_empty() {
                j += 1;
                if !v.is_empty() { v.push('\n'); }
                v.push_str(lines[j].trim());
            }
            set_meta(&mut meta, &k, v);
            any = true;
            j += 1;
        } else { break; }
    }
    if any { s.meta = meta; i = j; }

    let n = lines.len();
    let blank = |k: usize| k >= n || lines[k].trim().is_empty();
    let mut last_blank = true;
    while i < n {
        let raw = lines[i];
        let t = raw.trim();
        if t.is_empty() { last_blank = true; i += 1; continue; }
        let prev_blank = last_blank;
        last_blank = false;

        if is_break(t) { s.items.push(Item::PageBreak); i += 1; continue; }

        // sections and (quick) scenes
        if t.starts_with('#') {
            let level = t.chars().take_while(|c| *c == '#').count();
            let body = t[level..].trim().to_string();
            if dialect == Dialect::Quick {
                s.items.push(Item::Scene(body));
            } else {
                s.items.push(Item::Section { level, text: body });
            }
            i += 1;
            continue;
        }
        if dialect == Dialect::Fountain && t.starts_with('=') && !t.starts_with("==") { i += 1; continue; } // synopsis
        if let Some(rest) = t.strip_prefix('.') {
            if !rest.starts_with('.') && !rest.is_empty() { s.items.push(Item::Scene(rest.trim().to_string())); i += 1; continue; }
        }
        if is_scene_heading(t) { s.items.push(Item::Scene(t.to_string())); i += 1; continue; }
        // centred text `> text <`
        if t.starts_with('>') && t.ends_with('<') && t.len() > 2 {
            s.items.push(Item::Line { speaker: None, text: t[1..t.len() - 1].trim().to_string() });
            i += 1;
            continue;
        }
        if is_transition(t) {
            s.items.push(Item::Direction(t.trim_start_matches('>').trim().to_string()));
            i += 1;
            continue;
        }
        // a whole-line parenthetical
        if t.starts_with('(') && t.ends_with(')') {
            s.items.push(Item::Direction(t.to_string()));
            i += 1;
            continue;
        }

        match dialect {
            Dialect::Quick => {
                // `NAME: words`, any case, a short name; `\` escapes
                if let Some(esc) = t.strip_prefix('\\') {
                    s.items.push(Item::Line { speaker: None, text: esc.to_string() });
                } else if let Some((name, words)) = quick_speaker(t) {
                    s.items.push(Item::Line { speaker: Some(name), text: words });
                } else {
                    s.items.push(Item::Line { speaker: None, text: t.to_string() });
                }
                i += 1;
            }
            Dialect::Fountain => {
                // forced action / lyrics
                if let Some(a) = t.strip_prefix('!') {
                    s.items.push(Item::Line { speaker: None, text: a.trim().to_string() });
                    i += 1;
                    continue;
                }
                if let Some(a) = t.strip_prefix('~') {
                    s.items.push(Item::Line { speaker: None, text: a.trim().to_string() });
                    i += 1;
                    continue;
                }
                // a character cue: forced with @, or UPPERCASE after a blank and before dialogue
                let forced = t.strip_prefix('@');
                if forced.is_some() || (prev_blank && is_cue(t) && !blank(i + 1) && !is_scene_heading(lines[i + 1].trim())) {
                    let name = forced.unwrap_or(t).trim().trim_end_matches('^').trim().to_string();
                    i += 1;
                    let mut first = true;
                    while i < n && !lines[i].trim().is_empty() && !is_scene_heading(lines[i].trim()) {
                        let d = lines[i].trim();
                        if d.starts_with('(') && d.ends_with(')') {
                            s.items.push(Item::Direction(d.to_string()));
                        } else {
                            s.items.push(Item::Line { speaker: if first { Some(name.clone()) } else { None }, text: d.trim_start_matches('~').trim().to_string() });
                            first = false;
                        }
                        i += 1;
                    }
                    last_blank = true;
                    continue;
                }
                // action: a paragraph, its own line breaks kept
                let mut para = vec![t.to_string()];
                i += 1;
                // a scene heading (or a page break) ends the paragraph even with no blank line before it: loose
                // text pasted from a word processor often runs "SCENE 1" straight into its heading
                while i < n && !lines[i].trim().is_empty() && !is_scene_heading(lines[i].trim()) && !is_break(lines[i]) {
                    para.push(lines[i].trim().to_string());
                    i += 1;
                }
                last_blank = !(i < n && !lines[i].trim().is_empty());
                s.items.push(Item::Line { speaker: None, text: para.join("\n") });
            }
        }
    }
    // a page break first thing is the title page's own break
    while matches!(s.items.first(), Some(Item::PageBreak)) { s.items.remove(0); }
    s
}

/// Quick `NAME: words`: one or two words (letters, digits, `.'-`), then a colon and a space.
fn quick_speaker(t: &str) -> Option<(String, String)> {
    let (name, words) = t.split_once(": ")?;
    let name = name.trim();
    if name.is_empty() || name.len() > 24 || name.split_whitespace().count() > 2 { return None; }
    if name.chars().any(|c| !(c.is_alphanumeric() || " .'-".contains(c))) { return None; }
    Some((name.to_string(), words.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_typing_is_scenes_and_lines() {
        let s = parse("# kitchen - night\nthe kettle starts to scream\nmara: I told you not to.\n(beat)\n===\n# roof\nwind", Dialect::Quick);
        assert_eq!(s.items, vec![
            Item::Scene("kitchen - night".into()),
            Item::Line { speaker: None, text: "the kettle starts to scream".into() },
            Item::Line { speaker: Some("mara".into()), text: "I told you not to.".into() },
            Item::Direction("(beat)".into()),
            Item::PageBreak,
            Item::Scene("roof".into()),
            Item::Line { speaker: None, text: "wind".into() },
        ]);
        assert_eq!((s.scenes(), s.lines()), (2, 3));
        // INT./EXT. need no hash, a long "name" is not a speaker, a backslash escapes
        let s = parse("INT. HALL - DAY\nNote that this is long: not a speaker\n\\Mara: literally this", Dialect::Quick);
        assert_eq!(s.items[0], Item::Scene("INT. HALL - DAY".into()));
        assert_eq!(s.items[1], Item::Line { speaker: None, text: "Note that this is long: not a speaker".into() });
        assert_eq!(s.items[2], Item::Line { speaker: None, text: "Mara: literally this".into() });
    }

    const FOUNTAIN: &str = "Title: FEMME FATALE\nCredit: Written by\nAuthor: Nelson Martin Jr\nDraft date: 2026-10-03\nNotes: Running time approx. 20 minutes\n\n====\n\n**CHARACTERS**\n\nDETECTIVE CAROL DAVIS (30s)\nJASMINE (20s), a prostitute. Victim #1\n\n===\n\nEXT. ALLEYWAY - NIGHT\n\nA single streetlamp buzzes. Flies circle it.\nA second line of the same paragraph.\n\nJASMINE\nHello sweetie, this is a first.\n(softly)\nFor the right price.\n\nCUT TO:\n\n.FLASHBACK\n\n> THE END <\n\n[[a note to self]]\n/* boneyard\nmore */\nCAROL (V.O.)\nIt was cold.\n";

    #[test]
    fn a_fountain_file_opens_with_its_title_page_scenes_cues_and_transitions() {
        let s = parse(FOUNTAIN, Dialect::Fountain);
        assert_eq!(s.meta.title.as_deref(), Some("FEMME FATALE"));
        assert_eq!(s.meta.author.as_deref(), Some("Nelson Martin Jr"));
        assert_eq!(s.meta.draft_date.as_deref(), Some("2026-10-03"));
        assert_eq!(s.meta.notes.as_deref(), Some("Running time approx. 20 minutes"));
        // the title page's own break is dropped; the character list lines are action, not cues (lowercase in them)
        assert!(matches!(&s.items[0], Item::Line { text, .. } if text == "**CHARACTERS**"), "{:?}", s.items[0]);
        let has = |it: &Item| s.items.contains(it);
        assert!(has(&Item::Line { speaker: None, text: "DETECTIVE CAROL DAVIS (30s)\nJASMINE (20s), a prostitute. Victim #1".into() }), "{:#?}", s.items);
        assert!(has(&Item::PageBreak));
        assert!(has(&Item::Scene("EXT. ALLEYWAY - NIGHT".into())));
        assert!(has(&Item::Line { speaker: None, text: "A single streetlamp buzzes. Flies circle it.\nA second line of the same paragraph.".into() }), "an action paragraph keeps its line breaks");
        assert!(has(&Item::Line { speaker: Some("JASMINE".into()), text: "Hello sweetie, this is a first.".into() }));
        assert!(has(&Item::Direction("(softly)".into())));
        assert!(has(&Item::Line { speaker: None, text: "For the right price.".into() }), "a continuing line has no speaker");
        assert!(has(&Item::Direction("CUT TO:".into())));
        assert!(has(&Item::Scene("FLASHBACK".into())), "a leading dot forces a scene");
        assert!(has(&Item::Line { speaker: None, text: "THE END".into() }), "centred text");
        assert!(has(&Item::Line { speaker: Some("CAROL (V.O.)".into()), text: "It was cold.".into() }), "notes and boneyard are gone, the cue after them is found");
        assert!(!s.items.iter().any(|i| matches!(i, Item::Line { text, .. } if text.contains("note to self") || text.contains("boneyard"))));
    }

    #[test]
    fn the_dialect_is_detected() {
        assert_eq!(detect(Some("a.fountain"), "plain"), Dialect::Fountain);
        assert_eq!(detect(Some("a.txt"), "# kitchen\nline"), Dialect::Quick);
        assert_eq!(detect(None, "Title: X\n\nINT. ROOM - DAY\n\nTEXT"), Dialect::Fountain);
        assert_eq!(detect(None, "INT. ROOM - DAY\nsomeone enters"), Dialect::Fountain);
        assert_eq!(detect(None, "just typing\nsome lines"), Dialect::Quick);
        // a Fountain file's `# ACT ONE` is a section, not a scene
        let s = parse_any(Some("x.fountain"), "# ACT ONE\n\nINT. ROOM - DAY\n\nAction.");
        assert_eq!(s.items[0], Item::Section { level: 1, text: "ACT ONE".into() });
        assert_eq!(s.items[1], Item::Scene("INT. ROOM - DAY".into()));
    }

    #[test]
    fn a_heading_with_no_blank_line_before_it_still_starts_a_scene() {
        let s = parse("SCENE 1\n    EXT. ALLEYWAY- NIGHT\nA streetlamp buzzes.\n\nJASMINE\nHello.", Dialect::Fountain);
        assert_eq!(s.items[0], Item::Line { speaker: None, text: "SCENE 1".into() });
        assert_eq!(s.items[1], Item::Scene("EXT. ALLEYWAY- NIGHT".into()));
        assert_eq!(s.items[2], Item::Line { speaker: None, text: "A streetlamp buzzes.".into() });
        assert_eq!(s.items[3], Item::Line { speaker: Some("JASMINE".into()), text: "Hello.".into() });
    }

    #[test]
    fn odd_input_does_not_panic_and_crlf_is_fine() {
        for t in ["", "\n\n\n", "#", "# ", "===", "[[ unclosed", "/* unclosed", "(", ")", "A:", ": b", "Title:", "   \n  "] {
            let _ = parse(t, Dialect::Quick);
            let _ = parse(t, Dialect::Fountain);
        }
        let s = parse("# a\r\nb\r\n", Dialect::Quick);
        assert_eq!(s.items, vec![Item::Scene("a".into()), Item::Line { speaker: None, text: "b".into() }]);
    }
}
