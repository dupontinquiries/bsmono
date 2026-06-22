//! note_cards: carve captured web content into atomic note-cards.
//!
//! This is the pure, dependency-free intermediate layer for the
//! web-capture → Helix-editable cards → dual-save feature (see
//! docs/design/web_capture_cards.md). It performs the "split cards
//! out of it" operation by STRUCTURE — the same selection/span model
//! the Helix verb grammar (editor_voice) operates on, so a card is a
//! span and card-level verbs (split/merge) layer onto it later.
//!
//! No IO, no QML, no on-disk format here — those are staged. This
//! module only decides card boundaries and builds Card values.

/// What kind of content a card holds — drives rendering + later the
/// auto-organization (code cards filed differently from prose, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// A heading line (becomes a section title / card header).
    Heading,
    /// A prose paragraph.
    Prose,
    /// A run of list items grouped into one card.
    List,
    /// A fenced code block (kept whole — never split mid-block).
    Code,
}

/// Where a card came from — the source ref for dual-save + provenance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardSource {
    /// Page URL the content was captured from (may be empty).
    pub url: String,
    /// The paragraph id from paragraph_selected, if known.
    pub paragraph_id: String,
}

/// One atomic note-card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Short title: first heading text, else first ~N chars of body.
    pub title: String,
    /// The card's content.
    pub body: String,
    pub kind: CardKind,
    pub source: CardSource,
}

const TITLE_MAX: usize = 60;

/// Derive a card title from a body: trimmed, first line, capped at
/// TITLE_MAX chars on a char boundary (… if truncated). Empty body →
/// empty title.
fn derive_title(body: &str) -> String {
    let first_line = body.trim().lines().next().unwrap_or("").trim();
    // Strip leading markdown heading markers / list bullets for the title.
    let cleaned = first_line
        .trim_start_matches('#')
        .trim_start_matches(|c| c == '-' || c == '*' || c == '+')
        .trim();
    if cleaned.chars().count() <= TITLE_MAX {
        cleaned.to_string()
    } else {
        let mut t: String = cleaned.chars().take(TITLE_MAX).collect();
        t.push('…');
        t
    }
}

fn is_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#')
}

fn is_list_item(line: &str) -> bool {
    let t = line.trim_start();
    // unordered
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return true;
    }
    // ordered: "<digits>. " or "<digits>) "
    let mut chars = t.char_indices();
    let mut saw_digit = false;
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_digit() {
            saw_digit = true;
            continue;
        }
        if saw_digit && (c == '.' || c == ')') {
            // must be followed by a space
            return t[i + 1..].starts_with(' ');
        }
        break;
    }
    false
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Split captured content into atomic cards by structure.
///
/// Boundary rules (each produces a card):
///   - a heading line → its own Heading card;
///   - a fenced code block (``` … ```), kept WHOLE → one Code card
///     (never split, even on blank lines inside it);
///   - a run of consecutive list items → one List card;
///   - a run of non-blank prose lines (a paragraph) → one Prose card;
///   - blank lines separate paragraphs but never appear in a card.
///
/// Each card's source is cloned from `source`. Order is preserved.
pub fn split_into_cards(captured: &str, source: &CardSource) -> Vec<Card> {
    let mut cards = Vec::new();
    let lines: Vec<&str> = captured.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Skip blank lines between blocks.
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block: consume until the closing fence (or EOF).
        if is_fence(line) {
            let mut block = String::new();
            block.push_str(line);
            block.push('\n');
            i += 1;
            while i < lines.len() {
                block.push_str(lines[i]);
                block.push('\n');
                let closed = is_fence(lines[i]);
                i += 1;
                if closed { break; }
            }
            let body = block.trim_end().to_string();
            cards.push(Card {
                title: derive_title(&body),
                body,
                kind: CardKind::Code,
                source: source.clone(),
            });
            continue;
        }

        // Heading: a single line → its own card.
        if is_heading(line) {
            let body = line.trim().to_string();
            cards.push(Card {
                title: derive_title(&body),
                body,
                kind: CardKind::Heading,
                source: source.clone(),
            });
            i += 1;
            continue;
        }

        // List run: consecutive list items grouped.
        if is_list_item(line) {
            let mut block = String::new();
            while i < lines.len() && is_list_item(lines[i]) {
                block.push_str(lines[i].trim_end());
                block.push('\n');
                i += 1;
            }
            let body = block.trim_end().to_string();
            cards.push(Card {
                title: derive_title(&body),
                body,
                kind: CardKind::List,
                source: source.clone(),
            });
            continue;
        }

        // Prose paragraph: consecutive non-blank, non-special lines.
        let mut block = String::new();
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() || is_heading(l) || is_list_item(l) || is_fence(l) {
                break;
            }
            block.push_str(l.trim_end());
            block.push('\n');
            i += 1;
        }
        let body = block.trim_end().to_string();
        if !body.is_empty() {
            cards.push(Card {
                title: derive_title(&body),
                body,
                kind: CardKind::Prose,
                source: source.clone(),
            });
        }
    }

    cards
}

/// Merge two adjacent cards into one (the card-level "merge up" verb's
/// pure operation). The merged card takes the FIRST card's kind and
/// title; bodies are joined with a blank line. Source taken from the
/// first.
pub fn merge_cards(a: &Card, b: &Card) -> Card {
    let mut body = a.body.clone();
    if !body.is_empty() && !b.body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(&b.body);
    Card {
        title: a.title.clone(),
        kind: a.kind,
        source: a.source.clone(),
        body,
    }
}

/// What changed about a card between two splits, by position index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardDelta {
    /// Card at this index is unchanged (body identical).
    Unchanged(usize),
    /// Card at this index changed body — re-render this section.
    Changed(usize),
    /// A card was added at this index (didn't exist before).
    Added(usize),
    /// The card previously at this index is gone.
    Removed(usize),
}

/// Diff two card lists positionally to find which SECTIONS need
/// re-rendering after a Helix edit. This is the pipeline's scoped-
/// re-render core: instead of re-rendering the whole page when the
/// edited tmp file changes, the backend re-splits the edited content,
/// diffs against the prior cards, and re-renders only the Changed /
/// Added sections (and drops Removed ones).
///
/// Positional (index-aligned) diff: simple and predictable, matching
/// how block_ids are assigned by position. Body equality is the
/// change signal (title/kind are derived from body, so body covers
/// them). A length change yields Added/Removed for the tail.
pub fn diff_cards(old: &[Card], new: &[Card]) -> Vec<CardDelta> {
    let mut deltas = Vec::new();
    let common = old.len().min(new.len());
    for i in 0..common {
        if old[i].body == new[i].body {
            deltas.push(CardDelta::Unchanged(i));
        } else {
            deltas.push(CardDelta::Changed(i));
        }
    }
    // Tail: new longer → Added; old longer → Removed.
    for i in common..new.len() {
        deltas.push(CardDelta::Added(i));
    }
    for i in common..old.len() {
        deltas.push(CardDelta::Removed(i));
    }
    deltas
}

/// Convenience: the indices that need a re-render (Changed + Added).
/// Removed sections are handled separately (the QML drops them).
pub fn sections_to_rerender(deltas: &[CardDelta]) -> Vec<usize> {
    deltas.iter().filter_map(|d| match d {
        CardDelta::Changed(i) | CardDelta::Added(i) => Some(*i),
        _ => None,
    }).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> CardSource {
        CardSource { url: "https://example.com/x".into(), paragraph_id: "p1".into() }
    }

    #[test]
    fn empty_input_no_cards() {
        assert!(split_into_cards("", &src()).is_empty());
        assert!(split_into_cards("\n\n   \n", &src()).is_empty());
    }

    #[test]
    fn single_paragraph_one_prose_card() {
        let cards = split_into_cards("Hello world.\nSecond line.", &src());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::Prose);
        assert_eq!(cards[0].body, "Hello world.\nSecond line.");
        assert_eq!(cards[0].title, "Hello world.");
        assert_eq!(cards[0].source, src());
    }

    #[test]
    fn blank_line_splits_paragraphs() {
        let cards = split_into_cards("Para one.\n\nPara two.", &src());
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].body, "Para one.");
        assert_eq!(cards[1].body, "Para two.");
    }

    #[test]
    fn heading_is_its_own_card() {
        let cards = split_into_cards("# Title\n\nBody text.", &src());
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].kind, CardKind::Heading);
        assert_eq!(cards[0].title, "Title"); // # stripped
        assert_eq!(cards[1].kind, CardKind::Prose);
    }

    #[test]
    fn list_run_grouped_into_one_card() {
        let cards = split_into_cards("- a\n- b\n- c", &src());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::List);
        assert_eq!(cards[0].body, "- a\n- b\n- c");
    }

    #[test]
    fn ordered_list_recognized() {
        let cards = split_into_cards("1. first\n2. second", &src());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::List);
    }

    #[test]
    fn fenced_code_kept_whole_even_with_blank_lines() {
        // Blank lines inside a fence must NOT split it.
        let input = "```rust\nlet x = 1;\n\nlet y = 2;\n```";
        let cards = split_into_cards(input, &src());
        assert_eq!(cards.len(), 1, "code block must be ONE card");
        assert_eq!(cards[0].kind, CardKind::Code);
        assert!(cards[0].body.contains("let x = 1;"));
        assert!(cards[0].body.contains("let y = 2;"));
    }

    #[test]
    fn mixed_content_splits_correctly() {
        let input = "# Heading\n\nA prose para\nwith two lines.\n\n- item 1\n- item 2\n\n```\ncode\n```\n\nFinal para.";
        let cards = split_into_cards(input, &src());
        let kinds: Vec<CardKind> = cards.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![
            CardKind::Heading, CardKind::Prose, CardKind::List,
            CardKind::Code, CardKind::Prose,
        ]);
    }

    #[test]
    fn unclosed_fence_consumes_to_eof_no_panic() {
        let cards = split_into_cards("```\nunterminated\nmore", &src());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::Code);
    }

    #[test]
    fn title_truncates_on_char_boundary() {
        let long = "x".repeat(100);
        let cards = split_into_cards(&long, &src());
        assert_eq!(cards.len(), 1);
        // capped at TITLE_MAX + the ellipsis
        assert!(cards[0].title.chars().count() <= TITLE_MAX + 1);
        assert!(cards[0].title.ends_with('…'));
    }

    #[test]
    fn list_then_prose_are_separate_cards() {
        let cards = split_into_cards("- item\nprose right after", &src());
        // a non-list line ends the list run and starts prose
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].kind, CardKind::List);
        assert_eq!(cards[1].kind, CardKind::Prose);
    }

    #[test]
    fn merge_cards_joins_bodies() {
        let a = Card { title: "A".into(), body: "first".into(), kind: CardKind::Prose, source: src() };
        let b = Card { title: "B".into(), body: "second".into(), kind: CardKind::Prose, source: src() };
        let m = merge_cards(&a, &b);
        assert_eq!(m.body, "first\n\nsecond");
        assert_eq!(m.title, "A");      // takes first card's title
        assert_eq!(m.kind, CardKind::Prose);
    }

    #[test]
    fn not_a_list_when_digit_without_punct() {
        // "3 blind mice" is prose, not an ordered list
        let cards = split_into_cards("3 blind mice", &src());
        assert_eq!(cards[0].kind, CardKind::Prose);
    }

    // v0.1.504: scoped-re-render diff (the helix-edit → which-sections
    // changed pipeline core).

    #[test]
    fn diff_identical_all_unchanged() {
        let cards = split_into_cards("# A\n\nbody\n\n- x\n- y", &src());
        let d = diff_cards(&cards, &cards);
        assert!(d.iter().all(|x| matches!(x, CardDelta::Unchanged(_))));
        assert!(sections_to_rerender(&d).is_empty(), "no edit → no re-render");
    }

    #[test]
    fn diff_one_body_change_flags_one_section() {
        let old = split_into_cards("# A\n\nfirst para\n\nsecond para", &src());
        let new = split_into_cards("# A\n\nfirst para EDITED\n\nsecond para", &src());
        let d = diff_cards(&old, &new);
        // index 0 heading unchanged, index 1 changed, index 2 unchanged
        assert_eq!(d[0], CardDelta::Unchanged(0));
        assert_eq!(d[1], CardDelta::Changed(1));
        assert_eq!(d[2], CardDelta::Unchanged(2));
        assert_eq!(sections_to_rerender(&d), vec![1], "only the edited section re-renders");
    }

    #[test]
    fn diff_added_card_at_tail() {
        let old = split_into_cards("para one", &src());
        let new = split_into_cards("para one\n\npara two", &src());
        let d = diff_cards(&old, &new);
        assert_eq!(d[0], CardDelta::Unchanged(0));
        assert_eq!(d[1], CardDelta::Added(1));
        assert_eq!(sections_to_rerender(&d), vec![1]);
    }

    #[test]
    fn diff_removed_card_at_tail() {
        let old = split_into_cards("para one\n\npara two", &src());
        let new = split_into_cards("para one", &src());
        let d = diff_cards(&old, &new);
        assert_eq!(d[0], CardDelta::Unchanged(0));
        assert_eq!(d[1], CardDelta::Removed(1));
        // removed sections aren't in the re-render set (QML drops them)
        assert!(sections_to_rerender(&d).is_empty());
    }

    #[test]
    fn diff_empty_to_content_all_added() {
        let new = split_into_cards("# A\n\nbody", &src());
        let d = diff_cards(&[], &new);
        assert_eq!(sections_to_rerender(&d), vec![0, 1]);
    }
}
