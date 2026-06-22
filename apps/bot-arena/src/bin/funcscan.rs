//! funcscan — list the functions in a source file and flag the ones that have no
//! callers (good candidates for new tests). Most codebases have far more testable
//! surface than tests; this points at it.
//!
//!   funcscan <file>...

use std::path::Path;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: funcscan <file>...");
        std::process::exit(2);
    }
    for f in &files {
        let src = match std::fs::read_to_string(Path::new(f)) {
            Ok(s) => s,
            Err(e) => { println!("ERR {f}: {e}"); continue; }
        };
        let funcs = analyze(&src);
        println!("# {f}");
        let mut untested = 0;
        for fi in &funcs {
            let flag = if fi.refs == 0 { untested += 1; "  [no callers — test candidate]" } else { "" };
            println!("  {:<28} line {:<5} callers {}{}", fi.name, fi.line, fi.refs, flag);
        }
        println!("  → {} functions, {} with no callers\n", funcs.len(), untested);
    }
}

struct FuncInfo { name: String, line: usize, refs: usize }

/// Find `fn`/`def`/`function NAME(` definitions and count how many times each name
/// is called (`NAME(`) elsewhere — a crude "is anything exercising this?" signal.
fn analyze(src: &str) -> Vec<FuncInfo> {
    let mut funcs: Vec<(String, usize)> = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let t = raw.trim_start();
        for kw in ["fn ", "def ", "function "] {
            if let Some(rest) = t.strip_prefix(kw) {
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !name.is_empty() && rest[name.len()..].trim_start().starts_with('(') {
                    funcs.push((name, n + 1));
                }
                break;
            }
        }
    }
    funcs.into_iter().map(|(name, line)| {
        let calls = src.matches(&format!("{name}(")).count();
        // subtract the definition occurrence itself
        let refs = calls.saturating_sub(1);
        FuncInfo { name, line, refs }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::analyze;
    #[test]
    fn flags_uncalled_functions() {
        let src = "fn used(){}\nfn caller(){ used(); used(); }\nfn orphan(){}\ndef py_fn():\n";
        let f = analyze(src);
        let used = f.iter().find(|x| x.name == "used").unwrap();
        let orphan = f.iter().find(|x| x.name == "orphan").unwrap();
        assert_eq!(used.refs, 2, "used() called twice");
        assert_eq!(orphan.refs, 0, "orphan() has no callers");
        assert!(f.iter().any(|x| x.name == "py_fn"), "python def detected");
    }
}
