//! wire — CrdtOps and carets as bytes, without a serialization crate.
//!
//! studio-collab uses serde. This crate does not, because the CRDT core
//! is dependency-free on purpose and a wire format is a dozen lines of
//! parsing. If the two ever share a transport, the carrier can wrap
//! these strings as an opaque payload — a format that is one line per
//! op, ASCII, and diffable is easier to debug over a flaky link than a
//! binary blob, and this is a link that WILL be flaky.
//!
//! ## Format
//!
//! One op per line, `|`-separated. Characters travel as decimal
//! codepoints rather than literal text, which sidesteps every escaping
//! question at once — a newline, a `|`, and a lone surrogate all encode
//! the same way.
//!
//! ```text
//! I|lamport|site|origin_lamport|origin_site|codepoint
//! I|lamport|site|-|-|codepoint            (inserted at the start)
//! D|lamport|site                          (tombstone that character)
//! C|site|anchor_lam|anchor_site|head_lam|head_site
//! C|site|-|-|-|-                          (caret at the start)
//! ```
//!
//! ## Unknown lines are skipped, not fatal
//!
//! A peer running a newer build may send a line kind this one does not
//! know. Dropping the whole batch would take a working session down over
//! a field nobody needed; skipping the line degrades to "we do not see
//! their new feature", which is recoverable. Malformed lines of a kind
//! we DO know are a different matter and fail the parse, because that
//! means the stream is corrupt rather than merely newer.

use crate::presence::Caret;
use crate::rga::{CharId, CrdtOp, SiteId};

/// Encode ops, one per line.
pub fn encode_ops(ops: &[CrdtOp]) -> String {
    let mut s = String::new();
    for op in ops {
        match op {
            CrdtOp::Insert { id, ch, after } => {
                let (al, as_) = match after {
                    Some(a) => (a.lamport.to_string(), a.site.0.to_string()),
                    None => ("-".into(), "-".into()),
                };
                s.push_str(&format!(
                    "I|{}|{}|{}|{}|{}\n",
                    id.lamport, id.site.0, al, as_, *ch as u32
                ));
            }
            CrdtOp::Delete { target } => {
                s.push_str(&format!("D|{}|{}\n", target.lamport, target.site.0));
            }
        }
    }
    s
}

/// Decode ops. Returns None if a line we recognize is malformed;
/// unrecognized line kinds are skipped.
pub fn decode_ops(text: &str) -> Option<Vec<CrdtOp>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('|').collect();
        match f[0] {
            "I" => {
                if f.len() != 6 {
                    return None;
                }
                let id = CharId { lamport: f[1].parse().ok()?, site: SiteId(f[2].parse().ok()?) };
                let after = match (f[3], f[4]) {
                    ("-", "-") => None,
                    (l, s) => {
                        Some(CharId { lamport: l.parse().ok()?, site: SiteId(s.parse().ok()?) })
                    }
                };
                let ch = char::from_u32(f[5].parse().ok()?)?;
                out.push(CrdtOp::Insert { id, ch, after });
            }
            "D" => {
                if f.len() != 3 {
                    return None;
                }
                out.push(CrdtOp::Delete {
                    target: CharId {
                        lamport: f[1].parse().ok()?,
                        site: SiteId(f[2].parse().ok()?),
                    },
                });
            }
            // a newer peer's line kind: skip rather than drop the batch
            _ => continue,
        }
    }
    Some(out)
}

pub fn encode_caret(site: SiteId, c: &Caret) -> String {
    let f = |id: Option<CharId>| match id {
        Some(i) => format!("{}|{}", i.lamport, i.site.0),
        None => "-|-".to_string(),
    };
    format!("C|{}|{}|{}\n", site.0, f(c.anchor), f(c.head))
}

/// Decode caret lines, ignoring op lines in the same stream so a caller
/// can mix both on one channel.
pub fn decode_carets(text: &str) -> Option<Vec<(SiteId, Caret)>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split('|').collect();
        if f.first() != Some(&"C") {
            continue;
        }
        if f.len() != 6 {
            return None;
        }
        let site = SiteId(f[1].parse().ok()?);
        let pick = |l: &str, s: &str| -> Option<Option<CharId>> {
            if l == "-" && s == "-" {
                return Some(None);
            }
            Some(Some(CharId { lamport: l.parse().ok()?, site: SiteId(s.parse().ok()?) }))
        };
        out.push((site, Caret { anchor: pick(f[2], f[3])?, head: pick(f[4], f[5])? }));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rga::Rga;

    const A: SiteId = SiteId(1);

    #[test]
    fn ops_round_trip() {
        let mut r = Rga::new();
        let mut ops = r.local_insert(0, "hello", A);
        ops.extend(r.local_delete(1, 2));
        let text = encode_ops(&ops);
        assert_eq!(decode_ops(&text).unwrap(), ops);
    }

    #[test]
    fn a_replica_rebuilt_from_the_wire_matches() {
        let mut a = Rga::from_text("fn main() {}\n", A);
        a.local_insert(0, "// note\n", A);
        a.local_delete(3, 4);
        let text = encode_ops(&a.history());
        let mut b = Rga::new();
        b.apply_all(&decode_ops(&text).unwrap());
        assert_eq!(b.text(), a.text());
    }

    #[test]
    fn characters_needing_escapes_survive() {
        // codepoints, so the separator and the line break are not
        // special cases
        let mut r = Rga::new();
        let ops = r.local_insert(0, "a|b\nc\td é 日 🙂", A);
        let back = decode_ops(&encode_ops(&ops)).unwrap();
        assert_eq!(back, ops);
        let mut b = Rga::new();
        b.apply_all(&back);
        assert_eq!(b.text(), "a|b\nc\td é 日 🙂");
    }

    #[test]
    fn an_unknown_line_kind_is_skipped_not_fatal() {
        // a newer peer's field should cost us their new feature, not the
        // session
        let mut r = Rga::new();
        let ops = r.local_insert(0, "ab", A);
        let mut text = encode_ops(&ops);
        text.push_str("Z|something|from|the|future\n");
        let back = decode_ops(&text).unwrap();
        assert_eq!(back, ops);
    }

    #[test]
    fn a_malformed_known_line_fails_the_parse() {
        // corrupt is different from newer
        assert!(decode_ops("I|1|2|-|-\n").is_none(), "wrong field count");
        assert!(decode_ops("I|x|2|-|-|97\n").is_none(), "unparseable lamport");
        assert!(decode_ops("D|1\n").is_none());
        assert!(decode_ops("I|1|2|-|-|1114112\n").is_none(), "not a codepoint");
    }

    #[test]
    fn blank_lines_are_ignored() {
        assert_eq!(decode_ops("\n\n  \n").unwrap(), Vec::new());
    }

    #[test]
    fn carets_round_trip_including_the_document_start() {
        let doc = Rga::from_text("abc", A);
        let mid = crate::presence::caret_at_index(&doc, 2);
        let start = crate::presence::caret_at_index(&doc, 0);
        let sel = crate::presence::selection(&doc, 1, 3);
        let mut text = String::new();
        for (i, c) in [mid, start, sel].iter().enumerate() {
            text.push_str(&encode_caret(SiteId(i as u64), c));
        }
        let back = decode_carets(&text).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].1, mid);
        assert_eq!(back[1].1, start);
        assert_eq!(back[1].1.anchor, None);
        assert_eq!(back[2].1, sel);
    }

    #[test]
    fn ops_and_carets_can_share_one_channel() {
        let mut r = Rga::new();
        let ops = r.local_insert(0, "hi", A);
        let caret = crate::presence::caret_at_index(&r, 1);
        let mut chan = encode_ops(&ops);
        chan.push_str(&encode_caret(A, &caret));
        assert_eq!(decode_ops(&chan).unwrap(), ops, "caret lines skipped by the op decoder");
        assert_eq!(decode_carets(&chan).unwrap().len(), 1, "op lines skipped by the caret decoder");
    }

    #[test]
    fn the_wire_stays_ascii_and_line_oriented() {
        // debuggability over a flaky link is the reason for the format
        let mut r = Rga::new();
        let ops = r.local_insert(0, "日本", A);
        let text = encode_ops(&ops);
        assert!(text.is_ascii(), "{text}");
        assert_eq!(text.lines().count(), 2, "one line per op");
    }
}
