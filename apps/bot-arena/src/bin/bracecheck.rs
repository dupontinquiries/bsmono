//! bracecheck — verify (), [], {} are balanced in source files, skipping braces
//! inside string/char literals and // /* */ comments. Exit non-zero if unbalanced.
//!
//!   bracecheck <file>...
//!
//! A fast structural sanity check an agent can run before the (expensive) build.

use std::path::Path;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: bracecheck <file>...");
        std::process::exit(2);
    }
    let mut bad = false;
    for f in &files {
        match std::fs::read_to_string(Path::new(f)) {
            Ok(src) => match check_balance(&src) {
                Ok(()) => println!("OK    {f}"),
                Err(msg) => { println!("FAIL  {f}: {msg}"); bad = true; }
            },
            Err(e) => { println!("ERR   {f}: {e}"); bad = true; }
        }
    }
    if bad { std::process::exit(1); }
}

/// Returns Ok(()) if balanced, else a human message describing the first problem.
fn check_balance(src: &str) -> Result<(), String> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let bytes: Vec<char> = src.chars().collect();
    let mut line = 1usize;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '\n' => line += 1,
            '/' if i + 1 < bytes.len() && bytes[i + 1] == '/' => {
                while i < bytes.len() && bytes[i] != '\n' { i += 1; }
                continue;
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == '*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                    if bytes[i] == '\n' { line += 1; }
                    i += 1;
                }
                i += 2;
                continue;
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == '\\' { i += 1; } // skip escaped char
                    else if bytes[i] == '\n' { line += 1; }
                    i += 1;
                }
            }
            '(' | '[' | '{' => stack.push((c, line)),
            ')' | ']' | '}' => {
                let want = match c { ')' => '(', ']' => '[', _ => '{' };
                match stack.pop() {
                    Some((open, _)) if open == want => {}
                    Some((open, oline)) => return Err(format!("line {line}: '{c}' closes '{open}' opened at line {oline}")),
                    None => return Err(format!("line {line}: stray '{c}'")),
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some((open, oline)) = stack.last() {
        return Err(format!("unclosed '{open}' opened at line {oline}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_balance;
    #[test]
    fn balanced_and_unbalanced() {
        assert!(check_balance("fn f() { let a = [1,2,(3)]; }").is_ok());
        assert!(check_balance("fn f() { ").is_err()); // unclosed {
        assert!(check_balance("a)").is_err());          // stray )
        assert!(check_balance("{ ]").is_err());          // mismatched
        // braces inside strings/comments must be ignored
        assert!(check_balance(r#"let s = "}{)("; // ){ "#).is_ok());
        assert!(check_balance("/* } { */ fn g(){}").is_ok());
    }
}
