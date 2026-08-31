//! jsonmask — byte-level JSON grammar constraint for decoding.
//!
//! Structured-output consumers (drivemind's suggestion tools, aperture
//! completions that must be machine-readable, toolpipe's FSM parsing
//! model text into pipelines) currently rely on "prompt hard, parse
//! defensively, reject failures". This module makes the failure class
//! unrepresentable instead: a pushdown automaton over BYTES tracks the
//! exact set of continuations that keep the output a valid JSON prefix,
//! and [`mask_tokens`] projects that through any byte-level vocabulary
//! into a per-token allow mask the sampler applies before argmax/top-k.
//!
//! Byte-level on purpose: it needs nothing from the tokenizer except
//! each token's bytes, so it works identically for BPE/SentencePiece/
//! byte-fallback vocabularies and never desynchronizes from merges.
//! Cost per decode step is |vocab| × mean-token-len byte transitions on
//! a ~30-byte state — microseconds on CPU next to a matmul; uploading
//! the mask for the GPU argmax (`kernels/argmax.wgsl` takes it as a
//! buffer of stride-1 u32s) is the documented integration point, and
//! [`force_complete`] closes the document validly when the token budget
//! runs out mid-structure — a truncated reply parses instead of
//! becoming one more rejection.
//!
//! Deliberately permissive exactly where JSON is: numbers, nesting
//! depth (bounded by [`MAX_DEPTH`] to keep the state small and the
//! clone cheap), unicode escapes. Deliberately strict everywhere else:
//! no trailing commas, no comments, no bare keys, one top-level value.

/// Nesting bound. 64 levels of containers is beyond any sane tool
/// schema; hitting it disallows further openers rather than growing.
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Expecting the start of a value.
    Value,
    /// Inside a string (key or value); `escape` = previous byte was '\',
    /// `uni` = hex digits still owed from a \u escape.
    Str { escape: bool, uni: u8, is_key: bool },
    /// Inside a number; phase machine (strict JSON: no leading zeros,
    /// no trailing '.', exponents need digits).
    Num(NumPhase),
    /// Just entered '[': first element value or immediate ']'.
    ArrFirst,
    /// Inside true/false/null; index into the expected literal.
    Lit { which: u8, at: u8 },
    /// A value just finished: expecting ',' or a closer (or end at top).
    AfterValue,
    /// Inside an object, expecting a key string or '}'.
    ObjKeyOrClose,
    /// Inside an object, expecting a key string (after ',').
    ObjKey,
    /// Key finished, expecting ':'.
    AfterKey,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NumPhase {
    /// '-' seen, digit required.
    Minus,
    /// Leading zero: only '.', 'e', or end may follow.
    ZeroInt,
    /// 1-9 then digits.
    Int,
    /// '.' seen, fraction digit required.
    Dot,
    /// In fraction digits.
    Frac,
    /// 'e'/'E' seen: sign or digit required.
    Exp,
    /// exponent sign seen: digit required.
    ExpSigned,
    /// exponent digits.
    ExpDigits,
}

const LITS: [&[u8]; 3] = [b"true", b"false", b"null"];

/// Incremental JSON validity state. Small and `Clone`-cheap by design —
/// token feasibility is "clone, replay the token's bytes".
#[derive(Clone, Debug)]
pub struct JsonState {
    mode: Mode,
    /// Container stack: b'{' or b'['.
    stack: Vec<u8>,
    /// True when the bytes so far form a COMPLETE valid document (a
    /// number like `12` is complete yet still extensible; both facts
    /// are represented).
    complete: bool,
}

impl Default for JsonState {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonState {
    pub fn new() -> Self {
        Self { mode: Mode::Value, stack: Vec::new(), complete: false }
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Advance by one byte. Returns false (state unchanged semantically —
    /// callers treat it as poisoned) when the byte is not a legal
    /// continuation.
    pub fn push(&mut self, b: u8) -> bool {
        use Mode::*;
        let ws = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
        // A complete top-level document accepts trailing whitespace only —
        // but ONLY once the value can no longer extend. A top-level bare
        // number is complete after every digit yet still extensible
        // (`-12.7` then `5`); its own Num arm handles both futures, so
        // the gate binds only in AfterValue.
        if self.complete && self.stack.is_empty() && matches!(self.mode, Mode::AfterValue) {
            return ws(b);
        }
        match self.mode {
            Value => {
                if ws(b) {
                    return true;
                }
                match b {
                    b'{' => {
                        if self.stack.len() >= MAX_DEPTH {
                            return false;
                        }
                        self.stack.push(b'{');
                        self.mode = ObjKeyOrClose;
                    }
                    b'[' => {
                        if self.stack.len() >= MAX_DEPTH {
                            return false;
                        }
                        self.stack.push(b'[');
                        self.mode = ArrFirst;
                    }
                    b'"' => self.mode = Str { escape: false, uni: 0, is_key: false },
                    b'-' => self.mode = Num(NumPhase::Minus),
                    b'0' => {
                        self.mode = Num(NumPhase::ZeroInt);
                        self.mark_if_top_number();
                    }
                    b'1'..=b'9' => {
                        self.mode = Num(NumPhase::Int);
                        self.mark_if_top_number();
                    }
                    b't' => self.mode = Lit { which: 0, at: 1 },
                    b'f' => self.mode = Lit { which: 1, at: 1 },
                    b'n' => self.mode = Lit { which: 2, at: 1 },
                    _ => return false,
                }
                true
            }
            Str { escape, uni, is_key } => {
                if uni > 0 {
                    if b.is_ascii_hexdigit() {
                        self.mode = Str { escape: false, uni: uni - 1, is_key };
                        return true;
                    }
                    return false;
                }
                if escape {
                    match b {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.mode = Str { escape: false, uni: 0, is_key }
                        }
                        b'u' => self.mode = Str { escape: false, uni: 4, is_key },
                        _ => return false,
                    }
                    return true;
                }
                match b {
                    b'"' => {
                        if is_key {
                            self.mode = AfterKey;
                        } else {
                            self.finish_value();
                        }
                    }
                    b'\\' => self.mode = Str { escape: true, uni: 0, is_key },
                    0x00..=0x1F => return false, // control chars must be escaped
                    _ => {} // any other byte, including UTF-8 continuation bytes
                }
                true
            }
            Num(phase) => {
                use NumPhase::*;
                let digit_ok = |p: NumPhase| matches!(p, Int | ZeroInt | Frac | ExpDigits);
                match b {
                    b'0'..=b'9' => {
                        let next = match phase {
                            Minus => {
                                if b == b'0' { ZeroInt } else { Int }
                            }
                            ZeroInt => return false, // no leading zeros
                            Int => Int,
                            Dot | Frac => Frac,
                            Exp | ExpSigned | ExpDigits => ExpDigits,
                        };
                        self.mode = Num(next);
                        if digit_ok(next) {
                            self.mark_if_top_number();
                        }
                        true
                    }
                    b'.' if matches!(phase, Int | ZeroInt) => {
                        self.mode = Num(Dot);
                        self.complete = false;
                        true
                    }
                    b'e' | b'E' if matches!(phase, Int | ZeroInt | Frac) => {
                        self.mode = Num(Exp);
                        self.complete = false;
                        true
                    }
                    b'+' | b'-' if phase == Exp => {
                        self.mode = Num(ExpSigned);
                        true
                    }
                    _ => {
                        // End of number: only legal from a phase where the
                        // number is well-formed as it stands.
                        if !matches!(phase, Int | ZeroInt | Frac | ExpDigits) {
                            return false;
                        }
                        self.finish_value();
                        self.push(b)
                    }
                }
            }
            Lit { which, at } => {
                let lit = LITS[which as usize];
                if (at as usize) < lit.len() && b == lit[at as usize] {
                    if at as usize + 1 == lit.len() {
                        self.finish_value();
                    } else {
                        self.mode = Lit { which, at: at + 1 };
                    }
                    return true;
                }
                false
            }
            AfterValue => {
                if ws(b) {
                    return true;
                }
                match (b, self.stack.last().copied()) {
                    (b',', Some(b'[')) => {
                        self.mode = Value;
                        self.complete = false;
                        true
                    }
                    (b',', Some(b'{')) => {
                        self.mode = ObjKey;
                        self.complete = false;
                        true
                    }
                    (b']', Some(b'[')) | (b'}', Some(b'{')) => {
                        self.stack.pop();
                        self.finish_value();
                        true
                    }
                    _ => false,
                }
            }
            ArrFirst => {
                if ws(b) {
                    return true;
                }
                if b == b']' {
                    self.stack.pop();
                    self.finish_value();
                    return true;
                }
                self.mode = Mode::Value;
                self.push(b)
            }
            ObjKeyOrClose => {
                if ws(b) {
                    return true;
                }
                match b {
                    b'"' => {
                        self.mode = Str { escape: false, uni: 0, is_key: true };
                        true
                    }
                    b'}' => {
                        self.stack.pop();
                        self.finish_value();
                        true
                    }
                    _ => false,
                }
            }
            ObjKey => {
                if ws(b) {
                    return true;
                }
                if b == b'"' {
                    self.mode = Str { escape: false, uni: 0, is_key: true };
                    return true;
                }
                false
            }
            AfterKey => {
                if ws(b) {
                    return true;
                }
                if b == b':' {
                    self.mode = Mode::Value;
                    return true;
                }
                false
            }
        }
    }

    fn finish_value(&mut self) {
        self.mode = Mode::AfterValue;
        if self.stack.is_empty() {
            self.complete = true;
        }
    }

    /// Top-level bare numbers are complete after every digit.
    fn mark_if_top_number(&mut self) {
        if self.stack.is_empty() {
            self.complete = true;
        }
    }

    /// Is `b` a legal next byte? Non-mutating.
    pub fn allowed(&self, b: u8) -> bool {
        self.clone().push(b)
    }

}

/// Fill `allow[i] = state can emit vocab[i] in full`. `allow.len()` must
/// equal `vocab.len()`.
pub fn mask_tokens(state: &JsonState, vocab: &[&[u8]], allow: &mut [bool]) {
    for (i, tok) in vocab.iter().enumerate() {
        allow[i] = token_feasible(state, tok);
    }
}

fn token_feasible(state: &JsonState, tok: &[u8]) -> bool {
    if tok.is_empty() {
        return false;
    }
    let mut s = state.clone();
    for &b in tok.iter() {
        if !s.push(b) {
            return false;
        }
    }
    true
}

/// Bytes that, appended now, produce a COMPLETE valid document — used
/// when the token budget runs out mid-structure. Returns None only from
/// states that cannot be completed by any suffix this function knows
/// (mid \u escape is completed with zeros; mid-literal with its
/// remainder; mid-number by ensuring a digit; then close string, then
/// close every open container).
pub fn force_complete(state: &JsonState) -> Option<Vec<u8>> {
    let mut s = state.clone();
    let mut out = Vec::new();
    let mut guard = 0;
    while !(s.is_complete() && s.stack.is_empty()) {
        guard += 1;
        if guard > 16 * MAX_DEPTH + 64 {
            return None;
        }
        // Candidate order is a termination argument, learned twice:
        // '"' FIRST because inside a string every byte is legal and only
        // the close-quote makes progress; CLOSERS BEFORE DIGITS because
        // a well-formed number self-extends (from ExpDigits the digit
        // candidate succeeds forever — the string bug in different
        // clothes) and must be ENDED by '}'/']' instead. Digits and the
        // literal letters exist only to make ill-formed atoms
        // (dangling '-', 'e', '\u', 'tru') well-formed so a closer can
        // then fire. Every state consumes a bounded number of filler
        // bytes before a closer applies, and each closer shrinks the
        // stack — hence the loop terminates within the guard.
        let candidates: &[&[u8]] = &[
            b"\"", b"]", b"}", b":", b"null", b"0", b"e", b"u", b"l", b"a", b"s", b"r",
        ];
        let mut advanced = false;
        for cand in candidates {
            let mut t = s.clone();
            if cand.iter().all(|&b| t.push(b)) {
                out.extend_from_slice(cand);
                s = t;
                advanced = true;
                break;
            }
        }
        if !advanced {
            return None;
        }
    }
    Some(out)
}

/// Constrained greedy decode simulation for tests and for CPU-side use:
/// pick, at each step, the highest-scoring FEASIBLE token; append the
/// forced completion when the budget ends. This is exactly the loop the
/// sampler integration performs, minus the model.
pub fn constrained_greedy(
    state: &mut JsonState,
    vocab: &[&[u8]],
    mut score: impl FnMut(usize) -> f32,
    budget: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..budget {
        if state.is_complete() && state.stack.is_empty() {
            break;
        }
        let mut best: Option<(usize, f32)> = None;
        for (i, tok) in vocab.iter().enumerate() {
            if token_feasible(state, tok) {
                let sc = score(i);
                if best.map(|(_, b)| sc > b).unwrap_or(true) {
                    best = Some((i, sc));
                }
            }
        }
        let Some((i, _)) = best else { break };
        for &b in vocab[i] {
            state.push(b);
        }
        out.extend_from_slice(vocab[i]);
    }
    if let Some(tail) = force_complete(state) {
        for &b in &tail {
            state.push(b);
        }
        out.extend(tail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(doc: &[u8]) -> (JsonState, bool) {
        let mut s = JsonState::new();
        for &b in doc {
            if !s.push(b) {
                return (s, false);
            }
        }
        (s, true)
    }

    #[test]
    fn accepts_valid_documents_byte_by_byte() {
        for doc in [
            br#"{"a":1,"b":[true,null,-0.5e+10],"c":{"d":"x\ny \u00e9"}}"#.as_ref(),
            br#"[]"#.as_ref(),
            br#"{}"#.as_ref(),
            br#""just a string""#.as_ref(),
            br#"-12.75"#.as_ref(),
            br#"[ 1 , 2 , [ 3 ] ]"#.as_ref(),
        ] {
            let (s, ok) = feed(doc);
            assert!(ok, "rejected valid doc: {}", String::from_utf8_lossy(doc));
            assert!(s.is_complete(), "not complete: {}", String::from_utf8_lossy(doc));
            // Oracle agreement:
            assert!(serde_json::from_slice::<serde_json::Value>(doc).is_ok());
        }
    }

    #[test]
    fn rejects_illegal_continuations() {
        for (prefix, bad) in [
            (br#"{"#.as_ref(), b','),   // ',' right after '{'
            (br#"["#.as_ref(), b','),   // ',' right after '['
            (br#"{"a""#.as_ref(), b','),// ',' where ':' is owed
            (br#"01"#.as_ref(), b'2'),  // leading zero
            (br#"1."#.as_ref(), b','),  // trailing dot then delimiter
            (br#"1e"#.as_ref(), b','),  // exponent without digits
            (br#"tru"#.as_ref(), b'x'), // broken literal
            (br#"{}"#.as_ref(), b'{'),  // second top-level value
        ] {
            let mut s = JsonState::new();
            let mut ok = true;
            for &b in prefix {
                if !s.push(b) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                // "01" case: the prefix itself is already illegal at '1'.
                continue;
            }
            assert!(
                !s.allowed(bad),
                "accepted illegal {:?} after {:?}",
                bad as char,
                String::from_utf8_lossy(prefix)
            );
        }
    }

    #[test]
    fn constrained_greedy_over_toy_vocab_always_parses() {
        // A hostile little vocab: fragments that can combine into JSON or
        // garbage. Score prefers the garbage-prone long tokens; the mask
        // must keep the output valid anyway.
        let vocab: Vec<&[u8]> = vec![
            b"{\"", b"id", b"\":", b"\"s1\"", b",", b"}", b"[", b"]", b"1",
            b"23", b"true", b"null", b"\"x", b"y\"", b"{", b"\"", b":", b"e",
        ];
        for seed in 0..8u32 {
            let mut st = JsonState::new();
            let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
            let out = constrained_greedy(
                &mut st,
                &vocab,
                |i| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    ((x >> 8) & 0xFFFF) as f32 + i as f32 * 0.01
                },
                12,
            );
            let txt = String::from_utf8_lossy(&out);
            assert!(
                serde_json::from_slice::<serde_json::Value>(&out).is_ok(),
                "seed {seed} produced unparseable: {txt}"
            );
        }
    }

    #[test]
    fn force_complete_closes_mid_structure_states() {
        for prefix in [
            br#"{"a":[1,{"b":"unterminated"#.as_ref(),
            br#"{"key"#.as_ref(),
            br#"[1,2,"#.as_ref(),
            br#"{"a":-"#.as_ref(),
            br#"{"a":1.5e"#.as_ref(),
            br#"{"a":"\u00"#.as_ref(),
        ] {
            let (s, ok) = feed(prefix);
            assert!(ok, "prefix should be a valid prefix: {}", String::from_utf8_lossy(prefix));
            let tail = force_complete(&s)
                .unwrap_or_else(|| panic!("not completable: {}", String::from_utf8_lossy(prefix)));
            let mut doc = prefix.to_vec();
            doc.extend_from_slice(&tail);
            assert!(
                serde_json::from_slice::<serde_json::Value>(&doc).is_ok(),
                "completion failed: {}",
                String::from_utf8_lossy(&doc)
            );
        }
    }

    #[test]
    fn mask_tokens_matches_feasibility() {
        let (s, ok) = feed(br#"{"a""#);
        assert!(ok);
        let vocab: Vec<&[u8]> = vec![b":", b",", b"}", b":1}", b" : \"v\"}"];
        let mut allow = vec![false; vocab.len()];
        mask_tokens(&s, &vocab, &mut allow);
        assert_eq!(allow, vec![true, false, false, true, true]);
    }
}
