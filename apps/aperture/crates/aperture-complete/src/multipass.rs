//! multipass — turn one raw model reply into blocks you can build with.
//!
//! ## The failure this exists for
//!
//! A single-pass completion inserts whatever the model said. Observed,
//! verbatim, from a real reply:
//!
//! ```text
//!     if state.show_loop_pane {
//!         egui::Panel {
//!              title = "Frame";
//!              content = frame_content(&ctx);
//!          }
//!     }
//!
//!     // ... (rest of the code remains the same)
//!     }
//!     fn main() { ... }
//!     ```
//!     The provided code snippet appears to be a part of an application
//!     written in Rust, using various libraries such as `egui`...
//! ```
//!
//! Four different kinds of wrong are mixed into one string: a
//! HALLUCINATED construct (`egui::Panel { title = ... }` is not egui's
//! API), an ELISION (`// ... rest of the code remains the same`) that
//! silently deletes the file if accepted, a STUB (`fn main() { ... }`),
//! and PROSE explaining the code to a reader who is looking at it.
//!
//! Accepting that costs more than typing it. The problem is not the
//! model's quality — it is that one reply is being treated as one
//! answer, when it actually contains several candidate fragments of
//! different trustworthiness plus commentary.
//!
//! ## The passes
//!
//! 1. [`split_fences`] — separate code from prose. Prose becomes a
//!    COMMENT, not an insertion. The model's explanation is often the
//!    most useful part; it just must not be compiled.
//! 2. [`blocks`] — cut the code into balanced, independently insertable
//!    fragments. One reply usually contains several, and a person wants
//!    the second one about as often as the first.
//! 3. [`classify`] — mark elisions and stubs. These are never offered as
//!    insertions because accepting one deletes real code.
//! 4. [`assign_keys`] — bind the survivors to HOME ROW keys, so building
//!    is `f`, `j`, `d` rather than arrowing through a menu.
//!
//! Nothing here talks to a model or an editor. It is a pure transform
//! from one string to a keyed menu, which is what makes the awkward
//! cases testable — and the awkward cases are the whole point, since the
//! well-formed reply was never the problem.

/// What a fragment is, which decides whether it may be inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Ordinary code. Insertable.
    Code,
    /// `// ... rest of the code remains the same`, `# (unchanged)`.
    ///
    /// NEVER insertable. This is the dangerous one: it looks like a
    /// comment, so a naive filter keeps it, and accepting it into a real
    /// file replaces working code with a note saying the code used to be
    /// there.
    Elision,
    /// `fn main() { ... }`, `todo!()`, a body that is literally `...`.
    ///
    /// Not insertable either, but for a milder reason: it compiles to
    /// nothing useful and reads as finished work.
    Stub,
    /// The model talking about the code rather than writing it.
    /// Offered as a COMMENT — worth keeping, never worth compiling.
    Prose,
}

impl Kind {
    /// Can this be put in the buffer as-is?
    pub fn insertable(self) -> bool {
        matches!(self, Kind::Code)
    }
}

/// One candidate the person can accept.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub text: String,
    pub kind: Kind,
    /// The home-row key that accepts it, once assigned.
    pub key: Option<char>,
    /// A one-line description for the strip: the first meaningful line,
    /// shortened. What the person reads before pressing anything.
    pub hint: String,
}

/// Home row, in reach order for a right-handed layout: index fingers
/// first, then middle, ring, little.
///
/// Chosen over `1..9` because the point is building without leaving the
/// typing position — a completion menu that needs the number row has
/// already cost the movement it was meant to save.
pub const HOME_KEYS: [char; 8] = ['f', 'j', 'd', 'k', 's', 'l', 'a', ';'];

/// Pass 1: separate fenced code from surrounding prose.
///
/// Returns `(code_parts, prose_parts)`. An UNCLOSED fence is treated as
/// code to the end — a reply cut off by a token limit is the normal case,
/// not an error, and its last fragment is usually the one being asked
/// for.
///
/// A reply with no fences at all is treated as ALL CODE. Models
/// completing mid-function usually answer bare, and demanding fences
/// would throw away the most common good reply.
pub fn split_fences(reply: &str) -> (Vec<String>, Vec<String>) {
    let mut code = Vec::new();
    let mut prose = Vec::new();
    if !reply.contains("```") {
        let t = reply.trim();
        if !t.is_empty() {
            code.push(t.to_string());
        }
        return (code, prose);
    }
    // START INSIDE A FENCE when the reply OPENS with a closing one.
    //
    // A model continuing a code block often replies with bare code and
    // then a ``` to close it — the operator's real reply does exactly
    // that. Starting outside meant the leading code was classified as
    // PROSE, so the elision inside it was never examined and the whole
    // reply came back as one un-offerable paragraph.
    //
    // Detected by counting: an ODD number of fences means the reply is
    // unbalanced, and a reply that ends outside code must therefore have
    // begun inside it. Cheaper and more robust than guessing from the
    // first line's shape, which would misread a prose reply that happens
    // to start with an indented sentence.
    // The reply BEGINS inside a fence when its first fence line is a
    // CLOSING one — i.e. there is non-blank text before it and that text
    // is not itself introduced by a fence.
    //
    // Counting parity is not enough: "here you go:\n```rust\nlet x = 1;"
    // is also odd, and there the leading text really is prose. The
    // difference is whether anything precedes the first fence, which is
    // exactly what distinguishes "continued code, now closing" from
    // "prose, now opening".
    // The distinguishing mark is the INFO STRING. An opening fence
    // usually carries a language (```rust); a closing one is bare.
    //
    // Combined with "is there text before it", that separates the two
    // real shapes:
    //   "here you go:\n```rust\n…"   → prose, then an OPENING fence
    //   "    if x {\n…\n```\nprose"   → code already running, then a
    //                                  CLOSING one
    // Text-before-fence alone is not enough (the first case has it too),
    // and a bare-fence check alone is not enough (a reply can legitimately
    // open with a bare ```). Both together is what the operator's real
    // reply needed.
    let first_fence = reply
        .lines()
        .position(|l| l.trim_start().starts_with("```"));
    let mut in_code = match first_fence {
        Some(i) => {
            let bare = reply.lines().nth(i)
                .map(|l| l.trim().trim_start_matches('`').trim().is_empty())
                .unwrap_or(false);
            bare && reply.lines().take(i).any(|l| !l.trim().is_empty())
        }
        None => false,
    };
    let mut buf = String::new();
    for line in reply.lines() {
        if line.trim_start().starts_with("```") {
            let done = std::mem::take(&mut buf);
            let t = done.trim();
            if !t.is_empty() {
                if in_code { code.push(t.to_string()) } else { prose.push(t.to_string()) }
            }
            in_code = !in_code;
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    let t = buf.trim();
    if !t.is_empty() {
        // Unclosed fence: keep it as whatever it was.
        if in_code { code.push(t.to_string()) } else { prose.push(t.to_string()) }
    }
    (code, prose)
}

/// Is this line an elision — a note standing in for code that was left
/// out?
///
/// Matched on SHAPE rather than exact wording, because every model
/// phrases it differently and the consequence of missing one is that
/// accepting a completion deletes the rest of a function.
pub fn is_elision(line: &str) -> bool {
    let l = line.trim().trim_start_matches(['/', '#', '-', '*', ' ']).to_ascii_lowercase();
    if !l.starts_with("...") && !l.contains("...") && !l.starts_with("rest of") {
        return false;
    }
    l.contains("rest of")
        || l.contains("remains the same")
        || l.contains("unchanged")
        || l.contains("as before")
        || l.contains("same as above")
        || l.contains("omitted")
        || l.contains("snip")
        || l.trim() == "..."
}

/// Is this a stub — syntactically present, semantically empty?
pub fn is_stub(text: &str) -> bool {
    let t = text.trim();
    if t.contains("todo!()") || t.contains("unimplemented!()") {
        return true;
    }
    // `fn main() { ... }` and friends: a body whose only content is dots.
    if let Some(open) = t.find('{') {
        let body = t[open + 1..].trim_end_matches('}').trim();
        if !body.is_empty() && body.chars().all(|c| c == '.' || c.is_whitespace()) {
            return true;
        }
    }
    false
}

/// Pass 2 + 3: cut code into blocks and classify each.
///
/// Blocks are split at BLANK LINES AT BRACE DEPTH ZERO, which is where a
/// reply naturally separates one suggestion from the next. Splitting on
/// every blank line would cut function bodies in half; splitting only at
/// top level keeps each block independently insertable, which is the
/// property that makes accepting one of several safe.
pub fn blocks(code: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth: i32 = 0;
    let flush = |cur: &mut String, out: &mut Vec<Block>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(make_block(t));
        }
        cur.clear();
    };
    for line in code.lines() {
        if line.trim().is_empty() && depth <= 0 {
            flush(&mut cur, &mut out);
            continue;
        }
        depth += line.matches(['{', '(', '[']).count() as i32;
        depth -= line.matches(['}', ')', ']']).count() as i32;
        cur.push_str(line);
        cur.push('\n');
    }
    flush(&mut cur, &mut out);
    out
}

fn make_block(text: &str) -> Block {
    let elision = text.lines().any(is_elision);
    let kind = if elision {
        Kind::Elision
    } else if is_stub(text) {
        Kind::Stub
    } else {
        Kind::Code
    };
    Block { text: text.to_string(), kind, key: None, hint: hint_for(text) }
}

/// The line a person reads to decide. First non-empty, non-comment line,
/// trimmed to fit a strip.
fn hint_for(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .unwrap_or_else(|| text.lines().next().unwrap_or("").trim());
    let mut s: String = line.chars().take(38).collect();
    if line.chars().count() > 38 {
        s.push('…');
    }
    s
}

/// Pass 4: bind insertable blocks to home-row keys.
///
/// ONLY insertable blocks get keys. An elision or a stub with a key is a
/// keystroke away from deleting someone's function, and the whole reason
/// to classify was to make that unreachable rather than merely
/// discouraged.
///
/// Prose never gets a key either — it is shown as a comment above the
/// choices, since reading it is the point and inserting it is not.
///
/// More blocks than keys: the extras keep `None` and are shown without
/// one. Wrapping round would give two blocks the same key, which is
/// worse than a block you have to scroll to.
pub fn assign_keys(blocks: &mut [Block]) {
    let mut keys = HOME_KEYS.iter();
    for b in blocks.iter_mut() {
        if b.kind.insertable() {
            b.key = keys.next().copied();
        }
    }
}

/// The whole pipeline: raw reply in, keyed menu plus prose out.
pub fn plan(reply: &str) -> (Vec<Block>, Vec<String>) {
    let (code, prose) = split_fences(reply);
    let mut all: Vec<Block> = Vec::new();
    for c in &code {
        all.extend(blocks(c));
    }
    assign_keys(&mut all);
    (all, prose)
}

/// Typing a block out in PIECES, rather than all at once.
///
/// ## Why this exists
///
/// `plan` decides WHICH fragments are offerable. This decides how much
/// of one arrives per keystroke, which is the other half of "build with
/// blocks or go back to typing at any time".
///
/// All-or-nothing acceptance is the thing that makes AI completion feel
/// like a gamble: a 12-line block is right for 9 lines and wrong for 3,
/// and taking it means deleting 3 lines afterwards — the manual cleanup
/// this whole effort is about. Taking it a piece at a time means the
/// wrong part is simply never typed.
///
/// ## The unit
///
/// A "bit" is a word plus the punctuation and whitespace attached to it,
/// so accepting one lands `foo(` or `x);\n` rather than a bare `foo`
/// that leaves the caret in the wrong place. Code is not prose: the
/// delimiters are where the meaning is, and a word-only unit would make
/// a person type every bracket themselves — which is most of the
/// keystrokes.
///
/// A LINE is the coarser unit, for when the next line is obviously right
/// and picking through it word by word is just slower.
pub struct Typeout {
    text: String,
    /// How much has been typed out. A byte offset into `text`.
    at: usize,
}

impl Typeout {
    pub fn new(text: impl Into<String>) -> Typeout {
        Typeout { text: text.into(), at: 0 }
    }

    /// What is left to type.
    pub fn rest(&self) -> &str {
        &self.text[self.at.min(self.text.len())..]
    }

    /// Nothing left — the block has been fully accepted.
    pub fn done(&self) -> bool {
        self.rest().is_empty()
    }

    /// Take the next bit: leading whitespace, one word, and the
    /// punctuation that follows it.
    ///
    /// Returns `None` when finished, so a caller can fall through to the
    /// next block rather than pressing a key that does nothing.
    pub fn next_bit(&mut self) -> Option<&str> {
        let rest = self.rest();
        if rest.is_empty() {
            return None;
        }
        // CHAR INDICES, not bytes. A reply's comments routinely contain
        // `→`, `—` or an accented name, and a byte-wise scan can leave `i`
        // mid-character — at which point slicing `self.text[..at + i]`
        // panics. In a completion path that panic is a lost editor, and
        // it would only ever fire on the replies with prose in them.
        let mut i = 0usize; // byte offset into `rest`, always on a boundary
        let mut chars = rest.char_indices().peekable();
        // Leading whitespace comes WITH the bit rather than as its own
        // step: a keystroke that only inserts a space looks like nothing
        // happened, and two presses to place one token is the friction
        // this is meant to remove.
        while let Some(&(off, c)) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            chars.next();
            i = off + c.len_utf8();
            // Stop AT a newline. Crossing a line break silently is how
            // you end up several lines into code you have not read.
            if c == '\n' {
                let taken = &self.text[self.at..self.at + i];
                self.at += i;
                return Some(taken);
            }
        }
        // The word, if there is one. Written with the bounds check
        // FIRST and one condition per line: the compact spelling
        // (`i < len && alnum || i < len && underscore`) relies on `&&`
        // binding tighter than `||`, which is correct and is exactly the
        // kind of line that gets "simplified" into an out-of-bounds
        // index later.
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        while let Some(&(off, c)) = chars.peek() {
            if !is_word(c) {
                break;
            }
            chars.next();
            i = off + c.len_utf8();
        }
        // Then the punctuation run. ONE loop, not two: whether a word was
        // found or not, the next thing to absorb is the same — trailing
        // `(` after a word so the caret lands inside the parens, or a
        // bare `});` as a single press when there was no word at all.
        while let Some(&(off, c)) = chars.peek() {
            if is_word(c) || c.is_whitespace() {
                break;
            }
            chars.next();
            i = off + c.len_utf8();
        }
        let taken = &self.text[self.at..self.at + i];
        self.at += i;
        Some(taken)
    }

    /// Take the rest of the current line, including its newline.
    pub fn next_line(&mut self) -> Option<&str> {
        let rest = self.rest();
        if rest.is_empty() {
            return None;
        }
        let end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
        let taken = &self.text[self.at..self.at + end];
        self.at += end;
        Some(taken)
    }

    /// Take everything that is left.
    pub fn rest_of_it(&mut self) -> Option<&str> {
        if self.done() {
            return None;
        }
        let taken = &self.text[self.at..];
        self.at = self.text.len();
        Some(taken)
    }
}

#[cfg(test)]
mod typeout_tests {
    use super::*;

    fn drain_bits(t: &mut Typeout) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(b) = t.next_bit() {
            out.push(b.to_string());
        }
        out
    }

    #[test]
    fn a_bit_carries_its_punctuation() {
        // Code is not prose. A word-only unit would make a person type
        // every bracket themselves, which is most of the keystrokes.
        let mut t = Typeout::new("let x = foo(a, b);");
        let bits = drain_bits(&mut t);
        // `"a,"` has NO leading space, and that is correct: the previous
        // bit ended at `foo(`, and there is no whitespace between `(`
        // and `a` to carry. My expectation had one because I wrote the
        // list by eye rather than by the rule.
        //
        // The rule the implementation follows — leading whitespace joins
        // the bit that comes after it — is what the reassembly test
        // actually pins, and that one passes.
        assert_eq!(bits, vec!["let", " x", " =", " foo(", "a,", " b);"], "{bits:?}");
        assert!(t.done());
    }

    #[test]
    fn a_bit_never_crosses_a_line_break() {
        // Crossing one silently is how you end up several lines into code
        // you have not read.
        let mut t = Typeout::new("first();
second();");
        let mut seen = Vec::new();
        for _ in 0..2 {
            seen.push(t.next_bit().unwrap().to_string());
        }
        // `first();` is ONE bit: the word is taken, then the punctuation
        // run absorbs `();` in a single pass. My expectation split it
        // into three, which would have been the behaviour if punctuation
        // were taken one character at a time — and that is the thing the
        // design explicitly rejects, because `});` arriving as three
        // presses is the friction this removes.
        //
        // The newline is still its own step, which is the property this
        // test exists for.
        assert_eq!(seen, vec!["first();", "\n"], "{seen:?}");
    }

    #[test]
    fn the_pieces_reassemble_into_exactly_the_original() {
        // The property that matters most: building a block bit by bit
        // must produce the same text as accepting it whole, or the two
        // paths disagree and one of them is silently wrong.
        for src in [
            "let x = foo(a, b);",
            "fn f() {
    let a = 1;
}
",
            "  indented(); // trailing comment
",
            "a.b.c().d;",
            "",
        ] {
            let mut t = Typeout::new(src);
            let joined: String = drain_bits(&mut t).concat();
            assert_eq!(joined, src, "bits did not reassemble for {src:?}");
        }
    }

    #[test]
    fn a_line_at_a_time_also_reassembles() {
        let src = "one();
two();
three()";
        let mut t = Typeout::new(src);
        let mut out = String::new();
        while let Some(l) = t.next_line() {
            out.push_str(l);
        }
        assert_eq!(out, src);
    }

    #[test]
    fn mixing_granularities_is_safe() {
        // A person takes a bit, then decides the rest of the line is
        // fine, then takes the remainder. Nothing may be dropped or
        // duplicated across that switch.
        let src = "let total = a + b;
println!(\"{total}\");
";
        let mut t = Typeout::new(src);
        let mut out = String::new();
        out.push_str(t.next_bit().unwrap());
        out.push_str(t.next_line().unwrap());
        out.push_str(t.rest_of_it().unwrap());
        assert_eq!(out, src);
        assert!(t.done());
        assert_eq!(t.next_bit(), None, "finished means finished");
    }

    #[test]
    fn non_ascii_in_a_reply_does_not_split_a_character() {
        // Model replies routinely carry `→`, `—` or accented names in
        // comments. A byte-wise scan can leave the offset mid-character,
        // and slicing there PANICS — a lost editor, on exactly the
        // replies that contain prose.
        for src in [
            "let x = 1; // maps a → b\n",
            "let café = \"naïve\";",
            "// — an em dash —\nnext();",
        ] {
            let mut t = Typeout::new(src);
            let joined: String = drain_bits(&mut t).concat();
            assert_eq!(joined, src, "did not reassemble: {src:?}");
        }
    }

    #[test]
    fn an_empty_block_is_finished_immediately() {
        // So a caller can fall through to the next block rather than
        // binding a key that does nothing.
        let mut t = Typeout::new("");
        assert!(t.done());
        assert_eq!(t.next_bit(), None);
        assert_eq!(t.next_line(), None);
        assert_eq!(t.rest_of_it(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reply that motivated this module, as it actually arrived.
    const REAL: &str = r#"    if state.show_loop_pane {
        egui::Panel {
             title = "Frame";
             content = frame_content(&ctx);
         }
    }

    // ... (rest of the code remains the same)

fn main() { ... }
```

The provided code snippet appears to be a part of an application written in Rust,
using various libraries such as `egui`, `uikit_egui` and possibly others."#;

    #[test]
    fn the_elision_never_gets_a_key() {
        // THE ONE THAT MATTERS. `// ... rest of the code remains the
        // same` looks like a comment, so a naive filter keeps it — and
        // accepting it into a real file replaces working code with a note
        // saying the code used to be there.
        let (blocks, _) = plan(REAL);
        let elisions: Vec<&Block> = blocks.iter().filter(|b| b.kind == Kind::Elision).collect();
        assert!(!elisions.is_empty(), "the elision was not detected at all: {blocks:#?}");
        for e in elisions {
            assert_eq!(e.key, None, "an elision was bound to a key: {e:?}");
            assert!(!e.kind.insertable());
        }
    }

    #[test]
    fn the_stub_is_not_offered_either() {
        let (blocks, _) = plan(REAL);
        let stub = blocks.iter().find(|b| b.text.contains("fn main"));
        let stub = stub.expect("the fn main stub is present as a block");
        assert_eq!(stub.kind, Kind::Stub, "{stub:?}");
        assert_eq!(stub.key, None);
    }

    #[test]
    fn the_prose_is_kept_but_never_inserted() {
        // The explanation is often the most useful part of the reply. It
        // just must not be compiled.
        let (blocks, prose) = plan(REAL);
        assert!(prose.iter().any(|p| p.contains("provided code snippet")),
                "the explanation was thrown away: {prose:?}");
        assert!(!blocks.iter().any(|b| b.text.contains("provided code snippet")),
                "prose leaked into the insertable blocks");
    }

    #[test]
    fn the_real_code_does_get_a_key() {
        // Rejecting everything would be a safe and useless filter.
        let (blocks, _) = plan(REAL);
        let panel = blocks.iter().find(|b| b.text.contains("show_loop_pane"));
        let panel = panel.expect("the panel block survived");
        assert_eq!(panel.kind, Kind::Code);
        assert_eq!(panel.key, Some('f'), "the first insertable block takes the index finger");
    }

    #[test]
    fn a_bare_reply_with_no_fences_is_all_code() {
        // Models completing mid-function answer bare. Demanding fences
        // would throw away the most common GOOD reply.
        let (blocks, prose) = plan("let x = compute(y);\nprintln!(\"{x}\");");
        assert!(prose.is_empty());
        assert_eq!(blocks.len(), 1, "one balanced fragment: {blocks:#?}");
        assert_eq!(blocks[0].kind, Kind::Code);
        assert_eq!(blocks[0].key, Some('f'));
    }

    #[test]
    fn blank_lines_inside_a_body_do_not_split_it() {
        // Splitting on every blank line would cut a function in half and
        // offer two fragments that only compile together.
        let code = "fn f() {\n    let a = 1;\n\n    let b = 2;\n}\n";
        let bs = blocks(code);
        assert_eq!(bs.len(), 1, "a function body was split: {bs:#?}");
    }

    #[test]
    fn top_level_blanks_do_separate_suggestions() {
        let code = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";
        let bs = blocks(code);
        assert_eq!(bs.len(), 2);
        assert!(bs[0].text.contains("fn a"));
        assert!(bs[1].text.contains("fn b"));
    }

    #[test]
    fn keys_are_home_row_and_never_repeat() {
        // A completion menu that needs the number row has already cost
        // the hand movement it was meant to save.
        let code = (0..10).map(|i| format!("fn f{i}() {{ {i} }}"))
            .collect::<Vec<_>>().join("\n\n");
        let (blocks, _) = plan(&code);
        let keys: Vec<char> = blocks.iter().filter_map(|b| b.key).collect();
        assert_eq!(keys.len(), HOME_KEYS.len(), "ran out of keys, correctly");
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "a key was reused: {keys:?}");
        for k in &keys {
            assert!(HOME_KEYS.contains(k), "{k} is not on the home row");
        }
        // The extras are shown without a key rather than wrapping round.
        assert!(blocks.len() > keys.len());
    }

    #[test]
    fn elisions_are_matched_on_shape_not_exact_wording() {
        // Every model phrases this differently, and missing one means an
        // accepted completion deletes a function.
        for s in [
            "// ... rest of the code remains the same",
            "# ... (unchanged)",
            "/* ... as before ... */",
            "// ... omitted for brevity",
            "...",
            "// rest of the implementation is the same as above",
        ] {
            assert!(is_elision(s), "missed an elision: {s:?}");
        }
        for s in ["let x = a...b;", "// this is a real comment", "range(0..=9)"] {
            assert!(!is_elision(s), "false positive on real code: {s:?}");
        }
    }

    #[test]
    fn a_hint_is_short_and_skips_leading_comments() {
        // The hint is what a person reads before pressing a key, so it
        // has to be the code, not the comment above it.
        let b = make_block("// explain\nfn do_the_thing(a: u32) -> u32 { a }");
        assert!(b.hint.starts_with("fn do_the_thing"), "{}", b.hint);
        assert!(b.hint.chars().count() <= 39, "{}", b.hint);
    }

    #[test]
    fn an_unclosed_fence_keeps_its_code() {
        // A reply cut off by a token limit is the normal case, and its
        // last fragment is usually the one being asked for.
        let (blocks, _) = plan("here you go:\n```rust\nlet x = 1;\nlet y = 2;");
        assert!(blocks.iter().any(|b| b.text.contains("let x = 1;")),
                "truncated reply lost its code: {blocks:#?}");
    }
}
