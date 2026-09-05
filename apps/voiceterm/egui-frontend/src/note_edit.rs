//! note_edit — what the palace's Fork and Tag actually DO to the notes.
//!
//! `palace::Palace` resolves a keypress into an `Action` and parks Fork and
//! Tag in `pending` for a host to apply. Nothing applied them, so both keys
//! were inert. These are the applications, kept pure over `Vec<Card>` so
//! "forking twice gives two differently-named children" is a test and not
//! something you discover by pressing F twice in a palace.
//!
//! Both edits are made IN THE BODY TEXT rather than in new `Card` fields:
//!
//! * a fork writes a real `[[wikilink]]` into the parent, so the new note is
//!   joined to it in the same graph `link_graph` already builds — the one
//!   the palace is drawing. A fork that did not create an edge would put a
//!   node in the room with nothing leading to it.
//! * a tag is written as `#tag`, which is what the rest of this app's text
//!   pipeline (TF-IDF, search) already reads.
//!
//! So both survive a save/reload with no schema change, and both are
//! visible in the note itself, which is the point of a plain-text
//! zettelkasten.

use crate::cards_view::note_cards::{Card, CardKind, CardSource};

/// A tag as it is written into a body. Lower-cased, spaces to hyphens, and
/// stripped of anything that would not survive a round trip through the
/// text (`#` itself, brackets, whitespace runs).
pub fn normalize_tag(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .trim_start_matches('#')
        .chars()
        .map(|c| if c.is_whitespace() || c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

/// Does this body already carry `#tag`, as a whole tag rather than as the
/// prefix of a longer one?
pub fn has_tag(body: &str, tag: &str) -> bool {
    body.split_whitespace().any(|w| {
        w.strip_prefix('#')
            .map(|rest| rest.trim_end_matches(|c: char| c == '.' || c == ',').eq_ignore_ascii_case(tag))
            .unwrap_or(false)
    })
}

/// Add `#tag` to the note titled `title`. Returns what happened, so the
/// caller can say it rather than guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagOutcome {
    Added(String),
    /// Already there — pressing `t` twice with the same word is not an error.
    AlreadyPresent(String),
    /// The typed text had no usable characters.
    Unusable,
    NoSuchNote,
}

pub fn tag_note(cards: &mut [Card], title: &str, raw: &str) -> TagOutcome {
    let Some(tag) = normalize_tag(raw) else { return TagOutcome::Unusable };
    let Some(card) = cards.iter_mut().find(|c| c.title == title) else {
        return TagOutcome::NoSuchNote;
    };
    if has_tag(&card.body, &tag) {
        return TagOutcome::AlreadyPresent(tag);
    }
    // On its own line at the end: a tag appended to the last sentence reads
    // as part of it, and a body that ends mid-word is a body someone was
    // still writing.
    if !card.body.is_empty() && !card.body.ends_with('\n') {
        card.body.push('\n');
    }
    card.body.push_str(&format!("#{tag}"));
    TagOutcome::Added(tag)
}

/// The title a fork of `parent` gets: the first of "parent/1", "parent/2"…
/// that nobody is using. Slash rather than a space so the child reads as
/// subordinate in any sorted list of titles.
pub fn fork_title(cards: &[Card], parent: &str) -> String {
    for n in 1..1000 {
        let candidate = format!("{parent}/{n}");
        if !cards.iter().any(|c| c.title == candidate) {
            return candidate;
        }
    }
    format!("{parent}/{}", cards.len() + 1)
}

/// Create a child note of `parent` and link the parent to it. Returns the
/// new note's title, or `None` if there is no such parent.
///
/// The child inherits the parent's `CardSource`: a forked thought came from
/// wherever the thought it forked from came from, and losing that is losing
/// the provenance the deck shows.
pub fn fork_note(cards: &mut Vec<Card>, parent: &str) -> Option<String> {
    let (idx, source) = cards
        .iter()
        .position(|c| c.title == parent)
        .map(|i| (i, cards[i].source.clone()))?;
    let title = fork_title(cards, parent);
    let parent_body = &mut cards[idx].body;
    if !parent_body.is_empty() && !parent_body.ends_with('\n') {
        parent_body.push('\n');
    }
    parent_body.push_str(&format!("[[{title}]]"));
    cards.push(Card {
        title: title.clone(),
        body: String::new(),
        kind: CardKind::Prose,
        source,
    });
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(title: &str, body: &str) -> Card {
        Card {
            title: title.into(),
            body: body.into(),
            kind: CardKind::Prose,
            source: CardSource { url: "http://example/x".into(), paragraph_id: "p1".into() },
        }
    }

    #[test]
    fn a_tag_is_normalised_and_junk_is_refused() {
        assert_eq!(normalize_tag("Todo"), Some("todo".into()));
        assert_eq!(normalize_tag("  #Read Later "), Some("read-later".into()));
        assert_eq!(normalize_tag("snake_case"), Some("snake-case".into()));
        assert_eq!(normalize_tag("###"), None);
        assert_eq!(normalize_tag("   "), None);
        assert_eq!(normalize_tag("-- --"), None);
    }

    #[test]
    fn tagging_appends_once_and_says_which_happened() {
        let mut cards = vec![card("Alpha", "some prose")];
        assert_eq!(tag_note(&mut cards, "Alpha", "Todo"), TagOutcome::Added("todo".into()));
        assert!(cards[0].body.ends_with("\n#todo"), "{:?}", cards[0].body);
        // Twice is not an error and does not duplicate.
        assert_eq!(tag_note(&mut cards, "Alpha", "#todo"), TagOutcome::AlreadyPresent("todo".into()));
        assert_eq!(cards[0].body.matches("#todo").count(), 1);
        assert_eq!(tag_note(&mut cards, "Nobody", "todo"), TagOutcome::NoSuchNote);
        assert_eq!(tag_note(&mut cards, "Alpha", " ## "), TagOutcome::Unusable);
    }

    #[test]
    fn a_tag_prefix_is_not_the_same_tag() {
        // "#todo" must not count as already carrying "#todos".
        let mut cards = vec![card("A", "#todo")];
        assert!(has_tag(&cards[0].body, "todo"));
        assert!(!has_tag(&cards[0].body, "todos"));
        assert_eq!(tag_note(&mut cards, "A", "todos"), TagOutcome::Added("todos".into()));
        // Trailing punctuation still reads as the tag.
        assert!(has_tag("ends the line #done.", "done"));
    }

    #[test]
    fn forking_links_the_parent_and_names_children_uniquely() {
        let mut cards = vec![card("Alpha", "the parent")];
        let first = fork_note(&mut cards, "Alpha").unwrap();
        assert_eq!(first, "Alpha/1");
        assert_eq!(cards.len(), 2);
        assert!(cards[0].body.contains("[[Alpha/1]]"), "{:?}", cards[0].body);
        assert_eq!(cards[1].title, "Alpha/1");
        assert!(cards[1].body.is_empty());
        // Provenance is inherited, not dropped.
        assert_eq!(cards[1].source, cards[0].source);
        // A second fork does not collide with the first.
        let second = fork_note(&mut cards, "Alpha").unwrap();
        assert_eq!(second, "Alpha/2");
        assert_eq!(cards.len(), 3);
        assert!(fork_note(&mut cards, "Nobody").is_none());
    }

    #[test]
    fn a_fork_is_a_real_edge_in_the_graph_the_palace_draws() {
        // The whole point: the child must be reachable from the parent in
        // the same topology `topology_from_notes` builds.
        let mut cards = vec![card("Alpha", "the parent")];
        let child = fork_note(&mut cards, "Alpha").unwrap();
        let notes: Vec<(String, String)> =
            cards.iter().map(|c| (c.title.clone(), c.body.clone())).collect();
        let topo = crate::graph_view::topology_from_notes(&notes);
        assert!(topo.neighbors("Alpha").iter().any(|n| *n == child),
                "{:?}", topo.neighbors("Alpha"));
    }
}
