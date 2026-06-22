// src/sequence_diagram.rs
//
// v0.1.398 (sequence-diagram arc, step 1): minimal parser
// for mermaid-style sequence diagrams. Recognizes:
//
//   sequenceDiagram                # header (case-insensitive)
//   participant A as Alice         # named participant
//   participant B                  # bare id, label = id
//   A->>B: Message text            # solid arrow with filled head
//   A-->>B: Reply                  # dashed arrow with filled head
//   A->B: Message                  # solid arrow with open head
//   A-->B: Async reply             # dashed arrow with open head
//   Note left of A: comment        # parsed but not yet rendered
//   Note right of B: comment
//   Note over A,B: spanning
//
// What this does NOT support (filed for follow-up ships):
//   - activate/deactivate
//   - loop / alt / opt / par blocks
//   - autonumber
//   - actor (vs participant) distinction
//   - destruction (-x A)
//   - critical/break/box
//
// Visual rendering is NOT in this module. This module is the
// parser only; v0.1.398 ships with a textual fallback in
// main.rs, and v0.1.399+ adds a real visual renderer.

#![allow(dead_code)]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    /// Stable identifier as it appears in messages
    /// (left-hand side of `A as Alice`).
    pub id: String,
    /// Display label (right-hand side of `A as Alice`, or
    /// `id` if no `as` form was used).
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowStyle {
    /// `->>`  solid line, filled arrowhead
    SolidFilled,
    /// `-->>` dashed line, filled arrowhead
    DashedFilled,
    /// `->`   solid line, open arrowhead
    SolidOpen,
    /// `-->`  dashed line, open arrowhead
    DashedOpen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub from: String,
    pub to: String,
    pub text: String,
    pub arrow: ArrowStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePosition {
    /// `Note left of A`
    LeftOf(usize),
    /// `Note right of A`
    RightOf(usize),
    /// `Note over A,B` — indices into participants
    Over(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub position: NotePosition,
    pub text: String,
    /// 0-based position in the message stream so notes can
    /// be rendered between messages in document order.
    pub after_message: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SequenceDiagram {
    pub participants: Vec<Participant>,
    pub messages: Vec<Message>,
    pub notes: Vec<Note>,
    /// Lines we didn't understand. Mostly for diagnostics —
    /// the textual fallback renderer shows them as a "raw"
    /// section.
    pub unrecognized: Vec<String>,
}

impl SequenceDiagram {
    /// Find a participant by id, returning its index. Used
    /// during parsing to validate message endpoints.
    pub fn participant_idx(&self, id: &str) -> Option<usize> {
        self.participants.iter().position(|p| p.id == id)
    }

    /// Ensure a participant exists. When a message references
    /// an id with no prior `participant` declaration, we
    /// auto-create one with id == label. Matches mermaid's
    /// permissive behavior.
    fn ensure_participant(&mut self, id: &str) -> usize {
        if let Some(idx) = self.participant_idx(id) {
            return idx;
        }
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
        });
        self.participants.len() - 1
    }
}

/// Quick check: is this source a sequence diagram? Looks at
/// the first non-empty, non-comment line for the
/// `sequenceDiagram` header (case-insensitive — mermaid
/// allows `sequencediagram` too).
pub fn is_sequence_diagram(source: &str) -> bool {
    for raw_line in source.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() { continue; }
        return line.eq_ignore_ascii_case("sequenceDiagram");
    }
    false
}

/// Parse a sequence diagram source. Caller should have
/// confirmed `is_sequence_diagram(source)` first; we tolerate
/// missing header here too (defensive) but the standard call
/// gates on the detector.
pub fn parse(source: &str) -> SequenceDiagram {
    let mut sd = SequenceDiagram::default();
    let mut saw_header = false;

    for raw_line in source.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() { continue; }

        if !saw_header {
            if line.eq_ignore_ascii_case("sequenceDiagram") {
                saw_header = true;
                continue;
            }
            // Tolerate missing header — fall through.
            saw_header = true;
        }

        // 1. participant declaration
        if let Some(rest) = strip_prefix_ci(line, "participant ") {
            parse_participant(rest, &mut sd);
            continue;
        }
        // `actor` is mermaid's alternative — treat identically
        // for v1 (visual distinction filed for follow-up).
        if let Some(rest) = strip_prefix_ci(line, "actor ") {
            parse_participant(rest, &mut sd);
            continue;
        }

        // 2. note line
        if let Some(rest) = strip_prefix_ci(line, "note ") {
            if parse_note(rest, &mut sd) {
                continue;
            }
        }

        // 3. message arrow
        if parse_message(line, &mut sd) {
            continue;
        }

        // Unrecognized — record but don't fail.
        sd.unrecognized.push(line.to_string());
    }

    sd
}

fn strip_comment(line: &str) -> &str {
    // Sequence-diagram comments use `%%` (same as flowcharts).
    if let Some(idx) = line.find("%%") {
        &line[..idx]
    } else {
        line
    }
}

/// Case-insensitive prefix-strip. Helper rather than a
/// dep-pulling utility crate. Returns the suffix when the
/// prefix matches (case-insensitive), else None.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() { return None; }
    let head = &s[..prefix.len()];
    if head.eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn parse_participant(rest: &str, sd: &mut SequenceDiagram) {
    let rest = rest.trim();
    // Forms:
    //   A
    //   A as Alice
    //   "A B" as Whatever  (quoted ids — not in v1)
    let (id, label) = match find_as_split(rest) {
        Some((id, label)) => (id.trim().to_string(), label.trim().to_string()),
        None => (rest.to_string(), rest.to_string()),
    };
    if id.is_empty() { return; }
    // If a participant with this id already exists (e.g. a
    // message auto-created it earlier), update the label.
    if let Some(idx) = sd.participant_idx(&id) {
        sd.participants[idx].label = label;
        return;
    }
    sd.participants.push(Participant { id, label });
}

/// Find ` as ` token at a word boundary. Avoids splitting
/// the id `Pasta` which contains `as`. Returns (before, after).
fn find_as_split(s: &str) -> Option<(&str, &str)> {
    // Look for whitespace + "as" + whitespace.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i].is_ascii_whitespace()
            && bytes[i + 1] == b'a' && bytes[i + 2] == b's'
            && bytes[i + 3].is_ascii_whitespace()
        {
            return Some((&s[..i], &s[i + 4..]));
        }
        // Also lowercase A/S — bytes are ASCII so direct compare ok
        if bytes[i].is_ascii_whitespace()
            && (bytes[i + 1] == b'A' || bytes[i + 1] == b'a')
            && (bytes[i + 2] == b'S' || bytes[i + 2] == b's')
            && bytes[i + 3].is_ascii_whitespace()
        {
            return Some((&s[..i], &s[i + 4..]));
        }
        i += 1;
    }
    None
}

/// Parse a message line. Returns true if matched.
/// Patterns (tried in order — longer arrows first to avoid
/// prefix collisions):
///   A-->>B: text     DashedFilled
///   A->>B: text      SolidFilled
///   A-->B: text      DashedOpen
///   A->B: text       SolidOpen
fn parse_message(line: &str, sd: &mut SequenceDiagram) -> bool {
    // Try arrows in descending length order. The arrow
    // tokens never overlap when matched longest-first.
    let arrows: &[(&str, ArrowStyle)] = &[
        ("-->>", ArrowStyle::DashedFilled),
        ("->>", ArrowStyle::SolidFilled),
        ("-->", ArrowStyle::DashedOpen),
        ("->", ArrowStyle::SolidOpen),
    ];
    for (tok, style) in arrows {
        if let Some(pos) = line.find(tok) {
            let from = line[..pos].trim().to_string();
            let after = &line[pos + tok.len()..];
            // Split on `:` to separate `to` from `text`.
            let (to, text) = match after.find(':') {
                Some(c_pos) => (
                    after[..c_pos].trim().to_string(),
                    after[c_pos + 1..].trim().to_string(),
                ),
                // No colon → empty message text (still valid mermaid)
                None => (after.trim().to_string(), String::new()),
            };
            if from.is_empty() || to.is_empty() {
                // Malformed; skip but don't bail
                return false;
            }
            // Auto-create participants if not declared.
            sd.ensure_participant(&from);
            sd.ensure_participant(&to);
            sd.messages.push(Message {
                from, to, text,
                arrow: *style,
            });
            return true;
        }
    }
    false
}

/// Parse `left of A: ...`, `right of A: ...`, `over A,B: ...`.
/// Returns true if matched. Caller has already stripped the
/// `Note ` prefix.
fn parse_note(rest: &str, sd: &mut SequenceDiagram) -> bool {
    let rest = rest.trim();
    let colon_pos = match rest.find(':') {
        Some(p) => p,
        None => return false,
    };
    let header = rest[..colon_pos].trim();
    let text = rest[colon_pos + 1..].trim().to_string();

    // Match the directive keyword case-insensitively, but
    // preserve the ORIGINAL case of the participant id —
    // participant ids are case-sensitive (`A` != `a`), so
    // lowercasing the target would fail to match an
    // already-declared participant and spuriously create a
    // new lowercase one. (Bug caught by unit test before
    // ship: `Note left of A` was creating participant "a".)
    let lc = header.to_lowercase();
    let position = if lc.starts_with("left of ") {
        let target = header["left of ".len()..].trim();
        let idx = sd.ensure_participant(target);
        NotePosition::LeftOf(idx)
    } else if lc.starts_with("right of ") {
        let target = header["right of ".len()..].trim();
        let idx = sd.ensure_participant(target);
        NotePosition::RightOf(idx)
    } else if lc.starts_with("over ") {
        let targets = header["over ".len()..].trim();
        let parts: Vec<&str> = targets.split(',').map(|s| s.trim()).collect();
        if parts.is_empty() || parts[0].is_empty() { return false; }
        let a = sd.ensure_participant(parts[0]);
        let b = if parts.len() >= 2 && !parts[1].is_empty() {
            sd.ensure_participant(parts[1])
        } else {
            a
        };
        NotePosition::Over(a, b)
    } else {
        return false;
    };
    let after_message = sd.messages.len();
    sd.notes.push(Note { position, text, after_message });
    true
}

/// v0.1.398: textual fallback renderer. Until step 2 lands
/// a real visual sequence-diagram canvas, this produces a
/// markdown-formatted summary of the parsed sequence so the
/// user gets something useful instead of literal nodes
/// scattered on the cards canvas.
pub fn render_textual_fallback(sd: &SequenceDiagram) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("# Sequence Diagram\n\n");
    out.push_str("*Sequence diagram visual rendering is coming in a follow-up ship; for now, here's the parsed structure.*\n\n");

    if !sd.participants.is_empty() {
        out.push_str("## Participants\n\n");
        for p in &sd.participants {
            if p.id == p.label {
                out.push_str(&format!("- `{}`\n", p.id));
            } else {
                out.push_str(&format!("- `{}` — {}\n", p.id, p.label));
            }
        }
        out.push('\n');
    }

    if !sd.messages.is_empty() {
        out.push_str("## Messages\n\n");
        for (i, m) in sd.messages.iter().enumerate() {
            let arrow_str = match m.arrow {
                ArrowStyle::SolidFilled => "→",
                ArrowStyle::DashedFilled => "⇢",
                ArrowStyle::SolidOpen => "→",
                ArrowStyle::DashedOpen => "⇢",
            };
            out.push_str(&format!("{}. **{}** {} **{}**",
                i + 1, m.from, arrow_str, m.to));
            if !m.text.is_empty() {
                out.push_str(&format!(": *{}*", m.text));
            }
            out.push('\n');
            // Inline any notes that came AFTER this message
            // and BEFORE the next.
            for n in &sd.notes {
                if n.after_message == i + 1 {
                    out.push_str(&format!("   - 📝 {}\n", n.text));
                }
            }
        }
        out.push('\n');
    }

    // Notes that came before any message.
    let leading_notes: Vec<&Note> = sd.notes.iter()
        .filter(|n| n.after_message == 0)
        .collect();
    if !leading_notes.is_empty() && sd.messages.is_empty() {
        out.push_str("## Notes\n\n");
        for n in &leading_notes {
            out.push_str(&format!("- {}\n", n.text));
        }
        out.push('\n');
    }

    if !sd.unrecognized.is_empty() {
        out.push_str("## Unrecognized lines\n\n");
        out.push_str("*Sequence-diagram features the parser doesn't yet understand. These will be silently ignored until proper support lands.*\n\n");
        for line in &sd.unrecognized {
            out.push_str(&format!("- `{}`\n", line));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_header() {
        assert!(is_sequence_diagram("sequenceDiagram\n  A->>B: x"));
        assert!(is_sequence_diagram("sequencediagram\n  A->>B: x"));
        assert!(is_sequence_diagram("SequenceDiagram\n  A->>B: x"));
        assert!(!is_sequence_diagram("graph TD\n  A --> B"));
        assert!(!is_sequence_diagram(""));
    }

    #[test]
    fn detects_with_leading_comment() {
        let src = "%% this is a sequence diagram\nsequenceDiagram\nA->>B: x";
        assert!(is_sequence_diagram(src));
    }

    #[test]
    fn parses_named_participant() {
        let sd = parse("sequenceDiagram\nparticipant A as Alice\nparticipant B as Bob");
        assert_eq!(sd.participants.len(), 2);
        assert_eq!(sd.participants[0].id, "A");
        assert_eq!(sd.participants[0].label, "Alice");
        assert_eq!(sd.participants[1].id, "B");
        assert_eq!(sd.participants[1].label, "Bob");
    }

    #[test]
    fn parses_bare_participant() {
        let sd = parse("sequenceDiagram\nparticipant Alice");
        assert_eq!(sd.participants.len(), 1);
        assert_eq!(sd.participants[0].id, "Alice");
        assert_eq!(sd.participants[0].label, "Alice");
    }

    #[test]
    fn participant_id_with_as_substring_not_split() {
        // `Pasta` contains "as" but not at word boundary.
        let sd = parse("sequenceDiagram\nparticipant Pasta");
        assert_eq!(sd.participants[0].id, "Pasta");
    }

    #[test]
    fn parses_solid_filled_message() {
        let sd = parse("sequenceDiagram\nA->>B: Hello");
        assert_eq!(sd.messages.len(), 1);
        assert_eq!(sd.messages[0].from, "A");
        assert_eq!(sd.messages[0].to, "B");
        assert_eq!(sd.messages[0].text, "Hello");
        assert_eq!(sd.messages[0].arrow, ArrowStyle::SolidFilled);
    }

    #[test]
    fn parses_dashed_filled_message() {
        let sd = parse("sequenceDiagram\nB-->>A: Reply");
        assert_eq!(sd.messages.len(), 1);
        assert_eq!(sd.messages[0].arrow, ArrowStyle::DashedFilled);
    }

    #[test]
    fn parses_open_arrows() {
        let sd = parse("sequenceDiagram\nA->B: x\nA-->B: y");
        assert_eq!(sd.messages[0].arrow, ArrowStyle::SolidOpen);
        assert_eq!(sd.messages[1].arrow, ArrowStyle::DashedOpen);
    }

    #[test]
    fn auto_creates_participants_from_messages() {
        // No participant decls — should auto-create from
        // the first message endpoint references.
        let sd = parse("sequenceDiagram\nA->>B: Hello\nB-->>A: Hi");
        assert_eq!(sd.participants.len(), 2);
        let ids: Vec<&str> = sd.participants.iter()
            .map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"A"));
        assert!(ids.contains(&"B"));
    }

    #[test]
    fn message_without_text_doesnt_panic() {
        let sd = parse("sequenceDiagram\nA->>B");
        assert_eq!(sd.messages.len(), 1);
        assert_eq!(sd.messages[0].text, "");
    }

    #[test]
    fn longer_arrows_match_first() {
        // `-->>` must match before `->>` or `-->`.
        let sd = parse("sequenceDiagram\nA-->>B: dashed-filled");
        assert_eq!(sd.messages[0].arrow, ArrowStyle::DashedFilled);
    }

    #[test]
    fn parses_note_left_of() {
        let sd = parse(
            "sequenceDiagram\n\
             participant A\n\
             A->>A: self-call\n\
             Note left of A: a note");
        assert_eq!(sd.notes.len(), 1);
        match sd.notes[0].position {
            NotePosition::LeftOf(idx) => assert_eq!(idx, 0),
            _ => panic!("expected LeftOf"),
        }
        assert_eq!(sd.notes[0].text, "a note");
    }

    #[test]
    fn parses_note_over_two_participants() {
        let sd = parse(
            "sequenceDiagram\n\
             participant A\n\
             participant B\n\
             Note over A,B: spans both");
        match sd.notes[0].position {
            NotePosition::Over(a, b) => {
                assert_eq!(a, 0);
                assert_eq!(b, 1);
            }
            _ => panic!("expected Over"),
        }
    }

    #[test]
    fn comments_stripped() {
        let sd = parse(
            "sequenceDiagram\n\
             %% top-level comment\n\
             A->>B: msg %% trailing comment");
        assert_eq!(sd.messages[0].text, "msg");
        // No unrecognized lines — comment was stripped clean.
        assert_eq!(sd.unrecognized.len(), 0);
    }

    #[test]
    fn unrecognized_lines_collected() {
        let sd = parse(
            "sequenceDiagram\n\
             loop every minute\n\
             A->>B: ping\n\
             end");
        // `loop ... end` not yet supported — both lines
        // should land in unrecognized.
        assert!(sd.unrecognized.iter().any(|l| l.starts_with("loop")));
        assert!(sd.unrecognized.iter().any(|l| l == "end"));
    }

    #[test]
    fn no_header_still_parses_messages() {
        // Tolerant — even without explicit sequenceDiagram
        // header, messages parse if the caller invoked us
        // directly.
        let sd = parse("A->>B: x");
        assert_eq!(sd.messages.len(), 1);
    }

    #[test]
    fn render_textual_fallback_includes_participants_and_messages() {
        let sd = parse(
            "sequenceDiagram\n\
             participant A as Alice\n\
             participant B as Bob\n\
             A->>B: Hello\n\
             B-->>A: Hi back");
        let md = render_textual_fallback(&sd);
        assert!(md.contains("Alice"));
        assert!(md.contains("Bob"));
        assert!(md.contains("Hello"));
        assert!(md.contains("Hi back"));
        // The fallback should make clear this is a placeholder.
        assert!(md.contains("follow-up"));
    }

    #[test]
    fn parses_user_reported_pattern() {
        // The exact pattern the user described in the
        // hardware feedback: participant declarations + B->>A
        // arrows. Before v0.1.398, this produced literal
        // nodes named "participant", "B", "as", "A" on the
        // cards canvas. Now it parses cleanly.
        let sd = parse(
            "sequenceDiagram\n\
             participant A as Alice\n\
             participant B as Bob\n\
             B->>A: hi");
        assert_eq!(sd.participants.len(), 2);
        assert_eq!(sd.messages.len(), 1);
        assert_eq!(sd.messages[0].from, "B");
        assert_eq!(sd.messages[0].to, "A");
    }
}
