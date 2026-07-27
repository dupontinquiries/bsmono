//! components.rs — FUNCTIONAL CODE COMPONENTS extracted from the conversation.
//!
//! Every fenced code block in the chat becomes (or joins) a COMPONENT: content-hashed after
//! whitespace normalization, deduplicated across the whole history, and NAMED — first by a
//! heuristic (the block's own top-level declarations), then, when the descriptor model gets an
//! idle moment, by a short model-written description that replaces the heuristic.
//!
//! Two jobs:
//!   1. a REGISTRY the UI lists ("#3 · rust · resolve hud slab overlaps · ×4 uses");
//!   2. CONTEXT CLEANUP — when the prompt is rebuilt, any block whose identical text already
//!      appeared EARLIER in the context is replaced by a one-line reference to its component,
//!      so a long session stops paying for the same code five times. The user-visible
//!      transcript is never touched; only what the model re-reads changes.
//!
//! Pure and unit-tested; the chat tab owns one instance and feeds it messages incrementally.

/// One deduplicated code block.
#[derive(Clone, Debug)]
pub struct CodeComponent {
    pub id: usize,
    pub lang: String,
    pub hash: u64,
    pub text: String,          // normalized (per-line trailing whitespace trimmed)
    pub name: String,          // heuristic until the descriptor model speaks
    pub named_by_model: bool,
    pub uses: usize,           // total occurrences seen across the transcript
    pub first_msg: usize,      // message index of the first occurrence
    pub lines: usize,
}

#[derive(Default)]
pub struct ComponentRegistry {
    pub items: Vec<CodeComponent>,
    by_hash: std::collections::HashMap<u64, usize>,
    ingested_msgs: usize,      // how many messages have been scanned (incremental sync)
}

/// A fenced block found in a message: (lang, normalized body, byte span of the WHOLE fence).
fn fenced_blocks(text: &str) -> Vec<(String, String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(open) = text[i..].find("```") {
        let start = i + open;
        let line_end = text[start..].find('\n').map(|p| start + p).unwrap_or(text.len());
        let lang = text[start + 3..line_end].trim().to_string();
        let body_start = (line_end + 1).min(text.len());
        let Some(close_rel) = text[body_start..].find("```") else { break };
        let body_end = body_start + close_rel;
        let fence_end = (body_end + 3).min(text.len());
        let body = text[body_start..body_end]
            .lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
        out.push((lang, body, start..fence_end));
        i = fence_end;
    }
    out
}

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}

/// Heuristic name from the block's own declarations: collect top-ish level fn/def/class/struct
/// names across languages; fall back to the first meaningful line. Short, lowercase-ish, honest.
pub fn heuristic_name(lang: &str, body: &str) -> String {
    let mut decls: Vec<String> = Vec::new();
    for l in body.lines().map(|l| l.trim_start()) {
        for (kw, strip) in [("fn ", "("), ("def ", "("), ("class ", ":"), ("struct ", " "),
                            ("pub fn ", "("), ("function ", "("), ("impl ", " ")] {
            if let Some(rest) = l.strip_prefix(kw) {
                let name = rest.split(strip).next().unwrap_or("").split_whitespace().next()
                    .unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                if !name.is_empty() && !decls.contains(&name.to_string()) { decls.push(name.to_string()); }
                break;
            }
        }
        if decls.len() >= 3 { break; }
    }
    if !decls.is_empty() {
        let head = decls[0].clone();
        return if decls.len() > 1 { format!("{head} (+{} more)", decls.len() - 1) } else { head };
    }
    let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let words: Vec<&str> = first.split_whitespace().take(6).collect();
    if words.is_empty() { format!("{} block", if lang.is_empty() { "code" } else { lang }) }
    else { words.join(" ") }
}

impl ComponentRegistry {
    /// Incrementally scan messages `[ingested..len)`. Returns the ids of NEWLY created
    /// components (the naming queue feeds from this).
    pub fn sync<'a>(&mut self, msgs: impl Iterator<Item = &'a str>) -> Vec<usize> {
        let mut new_ids = Vec::new();
        for (mi, text) in msgs.enumerate() {
            if mi < self.ingested_msgs { continue; }
            for (lang, body, _) in fenced_blocks(text) {
                if body.lines().count() < 3 { continue; }   // one-liners aren't components
                let h = fnv1a(&body);
                match self.by_hash.get(&h) {
                    Some(&id) => self.items[id].uses += 1,
                    None => {
                        let id = self.items.len();
                        self.items.push(CodeComponent {
                            id, hash: h, name: heuristic_name(&lang, &body),
                            named_by_model: false, uses: 1, first_msg: mi,
                            lines: body.lines().count(), lang, text: body,
                        });
                        self.by_hash.insert(h, id);
                        new_ids.push(id);
                    }
                }
            }
            self.ingested_msgs = mi + 1;
        }
        new_ids
    }

    pub fn set_name(&mut self, id: usize, name: String) {
        if let Some(c) = self.items.get_mut(id) {
            // first line FIRST, then strip quoting — the closing quote sits before the
            // newline, so trimming quotes on the multi-line whole misses it.
            let n = name.lines().next().unwrap_or("").trim().trim_matches('"').trim();
            if !n.is_empty() { c.name = n.chars().take(60).collect(); c.named_by_model = true; }
        }
    }

    /// Rewrite ONE message for the context window: any fenced block identical to a component
    /// that already appeared EARLIER (an earlier message, or earlier in this one) collapses to
    /// a one-line reference. `seen` carries hashes across the whole prompt build, so the FIRST
    /// occurrence in the assembled context always survives verbatim.
    pub fn dedup_for_context(&self, text: &str, seen: &mut std::collections::HashSet<u64>) -> String {
        let blocks = fenced_blocks(text);
        if blocks.is_empty() { return text.to_string(); }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;
        for (_, body, span) in blocks {
            let h = fnv1a(&body);
            out.push_str(&text[cursor..span.start]);
            if let (Some(&id), false) = (self.by_hash.get(&h), seen.insert(h)) {
                let c = &self.items[id];
                out.push_str(&format!("⟦component #{} \"{}\" — {} lines, unchanged from above⟧",
                                      c.id + 1, c.name, c.lines));
            } else {
                out.push_str(&text[span.clone()]);
            }
            cursor = span.end;
        }
        out.push_str(&text[cursor..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "```rust\nfn hud_resolve(a: u32) {\n    pack(a);\n    done(a);\n}\n```";

    #[test]
    fn identical_blocks_become_one_component_with_counted_uses() {
        let mut r = ComponentRegistry::default();
        let m1 = format!("here:\n{BLOCK}\nok");
        let m2 = format!("again:\n{BLOCK}\nbye");
        let new1 = r.sync([m1.as_str()].into_iter());
        let new2 = r.sync([m1.as_str(), m2.as_str()].into_iter());
        assert_eq!(new1.len(), 1);
        assert_eq!(new2.len(), 0, "second identical block joins, not duplicates");
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].uses, 2);
        assert_eq!(r.items[0].name, "hud_resolve");
    }

    #[test]
    fn context_dedup_keeps_the_first_and_references_the_rest() {
        let mut r = ComponentRegistry::default();
        let m1 = format!("first:\n{BLOCK}");
        let m2 = format!("second:\n{BLOCK}");
        r.sync([m1.as_str(), m2.as_str()].into_iter());
        let mut seen = std::collections::HashSet::new();
        let c1 = r.dedup_for_context(&m1, &mut seen);
        let c2 = r.dedup_for_context(&m2, &mut seen);
        assert!(c1.contains("fn hud_resolve"), "first occurrence stays verbatim");
        assert!(!c2.contains("fn hud_resolve") && c2.contains("component #1"), "{c2}");
        assert!(c2.contains("hud_resolve"), "the reference carries the name");
    }

    #[test]
    fn different_blocks_stay_distinct_and_tiny_ones_are_ignored() {
        let mut r = ComponentRegistry::default();
        let other = "```py\ndef load(path):\n    x = read(path)\n    return x\n```";
        let tiny = "```sh\nls\n```";
        let m = format!("{BLOCK}\n{other}\n{tiny}");
        r.sync([m.as_str()].into_iter());
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.items[1].name, "load");
    }

    #[test]
    fn heuristic_falls_back_to_the_first_line() {
        assert_eq!(heuristic_name("sql", "SELECT id, name\nFROM users\nWHERE age > 21"),
                   "SELECT id, name");
    }

    #[test]
    fn model_name_overrides_heuristic_once() {
        let mut r = ComponentRegistry::default();
        let m1 = format!("x\n{BLOCK}");
        r.sync([m1.as_str()].into_iter());
        r.set_name(0, "\"resolve hud slab overlaps\"\nextra".into());
        assert_eq!(r.items[0].name, "resolve hud slab overlaps");
        assert!(r.items[0].named_by_model);
    }
}
