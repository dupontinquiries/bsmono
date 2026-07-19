//! decompose.rs — structured decomposition: split a code SKELETON (function signatures with `todo!()`
//! bodies) into short, KV-shareable sub-tasks, and splice the filled bodies back together. This is the
//! "chainload sub-functions through shorter sidechannels" idea made concrete.
//!
//! The trick that makes it cheap is the KV prefix tree (docs/08). Every sub-task prompt shares the SAME
//! leading text — the whole skeleton — and differs only in a short trailing line ("implement THIS body").
//! So the engine prefills the skeleton's KV exactly once and each sub-task is a cheap divergent suffix
//! (the fan-out: 1 prefill + N completions). That's the resolution of the "shorter sidechannel" intuition:
//! the per-sub-task *compute* is short even though each sub-task still sees the *full* context — because
//! the shared bulk is cached, not recomputed. And because each sub-task asks for exactly one body with all
//! the signatures/types in view, a SMALL model gets a tightly-scoped task it can often complete even when
//! it couldn't write the whole file (the large model supplies the structure; the small model fills it).
//!
//! Pure + tested. The egui layer runs the fan (enqueue sub-tasks) and the splice (assemble).

/// A hole to fill: the `todo!()`/`unimplemented!()` call and the signature of its enclosing `fn`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    pub sig: String,   // the enclosing `fn …(…) -> …` signature, whitespace-normalized
    pub start: usize,  // byte range of the macro call to replace on assembly
    pub end: usize,
}

const MACROS: [&str; 2] = ["todo!()", "unimplemented!()"];

/// Find the holes: every `todo!()`/`unimplemented!()` plus the `fn` signature it sits inside. Heuristic
/// (scans for the nearest preceding `fn …{`), which covers ordinary scaffolds; it won't perfectly parse
/// pathological signatures with a `{` before the body brace, and that's fine for a fill scaffold.
pub fn find_holes(src: &str) -> Vec<Hole> {
    let mut holes = Vec::new();
    for m in MACROS {
        let mut from = 0;
        while let Some(rel) = src[from..].find(m) {
            let start = from + rel;
            let end = start + m.len();
            if let Some(sig) = enclosing_fn_sig(src, start) {
                holes.push(Hole { sig, start, end });
            }
            from = end;
        }
    }
    holes.sort_by_key(|h| h.start);
    holes
}

/// Signature of the `fn` whose body contains byte `pos`: scan back to the nearest `fn ` and take up to
/// the `{` that opens its body.
fn enclosing_fn_sig(src: &str, pos: usize) -> Option<String> {
    let fn_at = src[..pos].rfind("fn ")?;
    let after = &src[fn_at..pos];
    let brace = after.find('{')?;
    Some(src[fn_at..fn_at + brace].split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The prompt for ONE sub-task: the whole skeleton (the shared, cached prefix) + "implement only this
/// body". The skeleton is kept in full on purpose — it's the KV-reused prefix, so it's effectively free
/// after the first sub-task, and it gives the model every signature and type it needs.
pub fn subtask_prompt(skeleton: &str, hole: &Hole) -> String {
    format!(
        "Here is a Rust skeleton:\n\n```rust\n{skeleton}\n```\n\nImplement ONLY the body of `{}`. \
         Reply with just the Rust code that replaces its `todo!()` (an expression or a `{{ … }}` block) — \
         no signature, no explanation, no code fence.",
        hole.sig
    )
}

/// Strip a single leading/trailing ``` code fence (and an optional language tag) from a model reply, so a
/// body that came back fenced still splices cleanly. Leaves unfenced text untouched.
pub fn strip_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.splitn(2, '\n').nth(1).unwrap_or("");           // drop the ```lang line
        return rest.trim_end().strip_suffix("```").unwrap_or(rest).trim().to_string();
    }
    t.to_string()
}

/// Pull the contents of the first ```fenced``` code block out of model prose (dropping the optional
/// ```lang marker line), so a skeleton embedded in a reply can be used directly as the skeleton. Returns
/// None if there's no fence — the caller then treats the text as raw code.
pub fn extract_code_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len()); // skip the ```lang line
    let body = &after[body_start..];
    let end = body.find("```")?;
    Some(body[..end].trim_end().to_string())
}

/// Reduce a model reply to just the code to splice as a hole body. Handles reasoning models and chatty
/// replies, in three steps: (1) drop a `<think>…</think>` reasoning channel if present; (2) take the LAST
/// fenced code block (the answer usually follows the reasoning) — falling back to the de-fenced text;
/// (3) if the result is a whole `fn …{ body }`, return just the inner body (since the hole already has a
/// signature). This is what makes `:fill` splice clean code instead of an essay when the backend is a
/// reasoning model like R1-Distill.
pub fn extract_body(reply: &str) -> String {
    let s = strip_think(reply);
    let code = last_code_block(&s).unwrap_or_else(|| s.trim().to_string());
    inner_fn_body(&code).unwrap_or(code).trim().to_string()
}

/// Drop a `<think>…</think>` reasoning channel: text after the final `</think>` if it closed, else the
/// text after an unclosed `<think>` (truncated reasoning — best effort), else the input unchanged.
fn strip_think(s: &str) -> &str {
    if let Some(i) = s.rfind("</think>") { s[i + "</think>".len()..].trim_start() }
    else if let Some(i) = s.find("<think>") { s[i + "<think>".len()..].trim_start() }
    else { s }
}

/// Contents of the LAST ```fenced``` block (dropping the optional ```lang line).
fn last_code_block(s: &str) -> Option<String> {
    let open = s.rfind("```")?;          // closing fence of the last block
    let before = &s[..open];
    let prev = before.rfind("```")?;     // its opening fence
    let after = &s[prev + 3..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
    let body = &after[body_start..];
    let end = body.find("```").unwrap_or(body.len());
    Some(body[..end].trim_end().to_string())
}

/// If `s` contains a `fn …{ … }`, return the brace-balanced inner body; else None.
fn inner_fn_body(s: &str) -> Option<String> {
    let f = s.find("fn ")?;
    let open = s[f..].find('{')? + f;
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    for i in open..s.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => { depth -= 1; if depth == 0 { return Some(s[open + 1..i].trim().to_string()); } }
            _ => {}
        }
    }
    None
}

/// Splice filled bodies back into the skeleton: replace each hole's macro span with its body. Applied
/// right-to-left so earlier byte offsets stay valid. Bodies are trimmed + de-fenced.
pub fn assemble(skeleton: &str, fills: &[(Hole, String)]) -> String {
    let mut out = skeleton.to_string();
    let mut ordered: Vec<&(Hole, String)> = fills.iter().collect();
    ordered.sort_by_key(|(h, _)| std::cmp::Reverse(h.start));
    for (h, body) in ordered {
        if h.end <= out.len() {
            out.replace_range(h.start..h.end, strip_fence(body).as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_holes_with_signatures() {
        let src = "use std::collections::HashMap;\n\nimpl C {\n  fn new() -> Self { todo!() }\n  fn get(&self, k: u64) -> Option<u32> { unimplemented!() }\n}";
        let h = find_holes(src);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].sig, "fn new() -> Self");
        assert_eq!(h[1].sig, "fn get(&self, k: u64) -> Option<u32>");
    }

    #[test]
    fn no_holes_when_complete() {
        assert!(find_holes("fn a() -> u32 { 1 + 1 }").is_empty());
    }

    #[test]
    fn subtask_prompt_carries_skeleton_and_one_sig() {
        let src = "fn a() -> u32 { todo!() }\nfn b() -> u32 { todo!() }";
        let holes = find_holes(src);
        let p = subtask_prompt(src, &holes[1]);
        assert!(p.contains("skeleton"), "includes the shared skeleton");
        assert!(p.contains("fn b() -> u32"), "targets the right body");
        assert!(p.contains("fn a() -> u32 { todo!() }"), "full skeleton is the shared prefix");
    }

    #[test]
    fn strip_fence_unwraps_code() {
        assert_eq!(strip_fence("```rust\n1 + 1\n```"), "1 + 1");
        assert_eq!(strip_fence("  2 * 2  "), "2 * 2");
        assert_eq!(strip_fence("```\n{ let x = 1; x }\n```"), "{ let x = 1; x }");
    }

    #[test]
    fn extracts_fenced_code_from_prose() {
        let reply = "Here's a skeleton:\n```rust\nfn a() { todo!() }\n```\nHope that helps!";
        assert_eq!(extract_code_block(reply).as_deref(), Some("fn a() { todo!() }"));
        assert_eq!(extract_code_block("no fences here"), None);
    }

    #[test]
    fn extract_body_pulls_code_from_reasoning() {
        // a reasoning reply with no fence, code buried at the end (the real R1 `fn new` shape)
        let r1 = "Okay, I need to implement new. The map starts empty.\n\nSo in code terms:\n\nfn new( cap: usize ) -> Self {\n    Lru { cap, map: HashMap::new(), order: Vec::new() }\n}\n\nYes, that should do it.";
        assert_eq!(extract_body(r1), "Lru { cap, map: HashMap::new(), order: Vec::new() }");
    }

    #[test]
    fn extract_body_handles_think_and_fence() {
        let r = "<think>\nI'll use a match.\n</think>\nHere:\n```rust\nmatch e { Expr::Num(n) => *n, _ => 0.0 }\n```";
        assert_eq!(extract_body(r), "match e { Expr::Num(n) => *n, _ => 0.0 }");
    }

    #[test]
    fn extract_body_takes_fn_inner_from_fenced() {
        let r = "```rust\nfn put(&mut self, k: u64, v: u64) {\n    self.map.insert(k, v);\n}\n```";
        assert_eq!(extract_body(r), "self.map.insert(k, v);");
    }

    #[test]
    fn extract_body_plain_expression_passthrough() {
        assert_eq!(extract_body("  a + b  "), "a + b");
    }

    #[test]
    fn assemble_splices_each_body() {
        let src = "fn a() -> u32 { todo!() }\nfn b() -> u32 { todo!() }";
        let holes = find_holes(src);
        let fills = vec![
            (holes[0].clone(), "1 + 1".to_string()),
            (holes[1].clone(), "```rust\n2 * 2\n```".to_string()), // fenced reply still splices
        ];
        let out = assemble(src, &fills);
        assert!(out.contains("fn a() -> u32 { 1 + 1 }"), "first body spliced");
        assert!(out.contains("fn b() -> u32 { 2 * 2 }"), "second body spliced + de-fenced");
        assert!(!out.contains("todo!()"), "no holes left");
    }

    #[test]
    fn assemble_is_order_independent_of_fill_vec() {
        // passing fills out of source order must still splice correctly (right-to-left by offset)
        let src = "fn a() { todo!() }\nfn b() { todo!() }\nfn c() { todo!() }";
        let h = find_holes(src);
        let fills = vec![
            (h[2].clone(), "/*c*/".into()),
            (h[0].clone(), "/*a*/".into()),
            (h[1].clone(), "/*b*/".into()),
        ];
        let out = assemble(src, &fills);
        assert_eq!(out, "fn a() { /*a*/ }\nfn b() { /*b*/ }\nfn c() { /*c*/ }");
    }
}
