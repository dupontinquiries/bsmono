//! v0.1.737: graph node-id hygiene. Mermaid/niftyplot node ids come straight
//! from user/source text (`tmp5`, `RegDst`, `5a`, even `{"4\nconst"}`). Those
//! ids flow into the card wire (\x1F-delimited) and become QML `modelData.id`,
//! a key passed back to backend methods. Two hazards we want abstracted away
//! from the QML layer entirely:
//!
//!   1. A raw id containing the wire separator (\x1F) or a newline corrupts the
//!      serialized layout (QML splits on \x1F / \n). The label was already
//!      scrubbed; the id was emitted raw.
//!   2. (The class behind the v736 lvalue bug) an id that happens to match a
//!      QML internal name can shadow/confuse resolution. Namespacing graph ids
//!      under a reserved prefix guarantees they can never alias a QML id or
//!      property.
//!
//! This module is the single source of truth for turning a raw source id into a
//! safe wire/QML id, and back. Pure + tested.

/// Reserved prefix for emitted graph node ids. No QML id or property in the
/// app starts with this, so a namespaced id can never collide. Kept short to
/// not bloat the wire.
pub const NS: &str = "gn_";

/// Characters that must never appear in an emitted id because they are wire or
/// QML-structural separators.
fn is_unsafe(c: char) -> bool {
    c == '\u{1F}'      // ASCII unit separator — the card-wire field delimiter
        || c == '\n' || c == '\r'
        || c == '\t'
        || c.is_control()
}

/// Sanitize a raw source id into a wire/QML-safe slug: unsafe chars → '_',
/// runs of '_' collapsed, trimmed. Preserves the recognizable shape of normal
/// ids (`tmp5` stays `tmp5`, `5a` stays `5a`) while neutralizing hazards. An
/// id that sanitizes to empty becomes `x`.
pub fn slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_us = false;
    for c in raw.chars() {
        if is_unsafe(c) {
            if !last_us { out.push('_'); last_us = true; }
        } else {
            out.push(c);
            last_us = false;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() { "x".to_string() } else { trimmed.to_string() }
}

/// The wire/QML id for a node: namespaced + sanitized. This is what gets
/// emitted to cardData and round-tripped through QML/backend. Deterministic,
/// so the same source id always maps to the same wire id within a run.
pub fn wire_id(raw: &str) -> String {
    format!("{}{}", NS, slug(raw))
}

/// Recover the source-facing slug from a wire id (strip the namespace). Used
/// when a backend method needs to relate a tapped card back to its node. If the
/// id isn't namespaced (legacy / non-graph), returns it unchanged.
pub fn strip_ns(wire: &str) -> &str {
    wire.strip_prefix(NS).unwrap_or(wire)
}

/// True if `id` is one of our namespaced graph ids.
pub fn is_graph_id(id: &str) -> bool {
    id.starts_with(NS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_ids_survive() {
        assert_eq!(slug("tmp5"), "tmp5");
        assert_eq!(slug("5a"), "5a");
        assert_eq!(slug("RegDst"), "RegDst");
        assert_eq!(slug("Read register 1"), "Read register 1"); // spaces ok
    }

    #[test]
    fn wire_separator_neutralized() {
        // a \x1F in the id would corrupt the card wire — must be scrubbed.
        let raw = "a\u{1F}b";
        let s = slug(raw);
        assert!(!s.contains('\u{1F}'));
        assert_eq!(s, "a_b");
    }

    #[test]
    fn newlines_and_control_scrubbed() {
        // the literal `{"4\nconst"}` node from the dogfood mermaid source.
        let s = slug("{\"4\nconst\"}");
        assert!(!s.contains('\n'));
        assert!(!s.contains('\r'));
        // collapses the newline to a single underscore between the parts
        assert_eq!(s, "{\"4_const\"}");
    }

    #[test]
    fn collapses_and_trims_underscores() {
        assert_eq!(slug("\n\nfoo\n\n"), "foo");
        assert_eq!(slug("a\u{1F}\u{1F}\nb"), "a_b");
    }

    #[test]
    fn empty_becomes_x() {
        assert_eq!(slug(""), "x");
        assert_eq!(slug("\n\t\r"), "x");
    }

    #[test]
    fn wire_id_namespaced_and_safe() {
        let w = wire_id("tmp5");
        assert_eq!(w, "gn_tmp5");
        assert!(is_graph_id(&w));
        assert_eq!(strip_ns(&w), "tmp5");
        // round-trips a hazardous id safely
        let w2 = wire_id("x\u{1F}y");
        assert!(!w2.contains('\u{1F}'));
        assert_eq!(strip_ns(&w2), "x_y");
    }

    #[test]
    fn namespace_cannot_collide_with_qml_internal_names() {
        // app QML ids/props look like _previewText, root, cardsCanvas — none
        // start with the reserved prefix.
        for qml_name in ["_previewText", "root", "cardsCanvas", "_statusText",
                         "termScreen", "backend"] {
            assert!(!qml_name.starts_with(NS),
                "QML name {} must not collide with graph ns", qml_name);
        }
    }

    #[test]
    fn strip_ns_passes_through_non_namespaced() {
        // legacy / non-graph card ids (note cards use raw note keys) unchanged.
        assert_eq!(strip_ns("note:project alpha"), "note:project alpha");
        assert!(!is_graph_id("note:project alpha"));
    }
}
