//! lexscan — a tiny lexer driven by an explicit finite-state machine. It walks the
//! source one char at a time through states (Normal, InString, InChar, InLineComment,
//! InBlockComment, InNumber, InIdent), counts token classes, and flags unterminated
//! strings / block comments (a structural bug `bracecheck` won't catch).
//!
//!   lexscan <file>

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Debug)]
enum S { Normal, Str, Chr, Line, Block, Num, Ident }

#[derive(Default, Debug, PartialEq)]
struct Counts {
    idents: usize, numbers: usize, strings: usize, chars: usize,
    line_comments: usize, block_comments: usize, operators: usize,
    unterminated_string: bool, unterminated_block: bool,
}

fn main() {
    let Some(file) = std::env::args().nth(1) else { eprintln!("usage: lexscan <file>"); std::process::exit(2); };
    let src = match std::fs::read_to_string(Path::new(&file)) {
        Ok(s) => s, Err(e) => { eprintln!("cannot read {file}: {e}"); std::process::exit(1); }
    };
    let c = lex(&src);
    println!("# {file}");
    println!("  identifiers     {}", c.idents);
    println!("  numbers         {}", c.numbers);
    println!("  strings         {}", c.strings);
    println!("  chars           {}", c.chars);
    println!("  line comments   {}", c.line_comments);
    println!("  block comments  {}", c.block_comments);
    println!("  operators       {}", c.operators);
    if c.unterminated_string { println!("  ⚠ unterminated string literal"); }
    if c.unterminated_block { println!("  ⚠ unterminated block comment"); }
    if c.unterminated_string || c.unterminated_block { std::process::exit(1); }
}

/// The FSM. Transitions are decided purely by (current state, current char).
fn lex(src: &str) -> Counts {
    let cs: Vec<char> = src.chars().collect();
    let mut st = S::Normal;
    let mut c = Counts::default();
    let mut i = 0;
    let mut esc = false; // inside string/char: previous char was a backslash
    while i < cs.len() {
        let ch = cs[i];
        let next = cs.get(i + 1).copied().unwrap_or('\0');
        match st {
            S::Normal => {
                if ch == '/' && next == '/' { st = S::Line; c.line_comments += 1; i += 2; continue; }
                if ch == '/' && next == '*' { st = S::Block; c.block_comments += 1; i += 2; continue; }
                if ch == '"' { st = S::Str; c.strings += 1; esc = false; }
                else if ch == '\'' { st = S::Chr; c.chars += 1; esc = false; }
                else if ch.is_ascii_digit() { st = S::Num; c.numbers += 1; }
                else if ch.is_alphabetic() || ch == '_' { st = S::Ident; c.idents += 1; }
                else if "+-*/%=<>!&|^~?:".contains(ch) { c.operators += 1; }
            }
            S::Str => {
                if esc { esc = false; }
                else if ch == '\\' { esc = true; }
                else if ch == '"' { st = S::Normal; }
            }
            S::Chr => {
                if esc { esc = false; }
                else if ch == '\\' { esc = true; }
                else if ch == '\'' { st = S::Normal; }
            }
            S::Line => { if ch == '\n' { st = S::Normal; } }
            S::Block => { if ch == '*' && next == '/' { st = S::Normal; i += 2; continue; } }
            S::Num => { if !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_') { st = S::Normal; continue; } }
            S::Ident => { if !(ch.is_ascii_alphanumeric() || ch == '_') { st = S::Normal; continue; } }
        }
        i += 1;
    }
    c.unterminated_string = st == S::Str || st == S::Chr;
    c.unterminated_block = st == S::Block;
    c
}

#[cfg(test)]
mod tests {
    use super::lex;
    #[test]
    fn counts_and_flags() {
        let ok = lex(r#"let x = 42; // note
let s = "hi"; /* blk */ fn foo() {}"#);
        assert_eq!(ok.numbers, 1);
        assert_eq!(ok.strings, 1);
        assert_eq!(ok.line_comments, 1);
        assert_eq!(ok.block_comments, 1);
        assert!(ok.idents >= 4); // let x let s fn foo ...
        assert!(!ok.unterminated_string && !ok.unterminated_block);

        assert!(lex("let s = \"oops").unterminated_string, "open string flagged");
        assert!(lex("code /* open").unterminated_block, "open block comment flagged");
        // a string containing comment/quote markers must not confuse the FSM
        let tricky = lex(r#"let s = "// not a comment \" still string";"#);
        assert_eq!(tricky.line_comments, 0);
        assert_eq!(tricky.strings, 1);
        assert!(!tricky.unterminated_string);
    }
}
