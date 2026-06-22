//! peek2 — indexed line/bucket/symbol viewer (thin CLI over the `peek`
//! library's view engine). Groups ranges by path so each file is indexed
//! once, then queried many times.
use anyhow::Result;
use clap::Parser;
use globwalk::GlobWalkerBuilder;
use peek::view::{parse_sel, Indexed, Sel};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "peek2", about = "Fast line/bucket/symbol viewer (v2, indexed)", version = "0.2.0")]
struct Args {
    #[arg(required = true)]
    files: Vec<String>,
    #[arg(short = 'h', long = "headers")]
    headers: bool,
    #[arg(short = 'H', long = "fullheaders")]
    full_headers: bool,
    #[arg(short = 'n', long = "numbered")]
    numbered: bool,
    #[arg(short = 'C', long = "context", default_value_t = 0)]
    context: usize,
}

fn expand(patterns: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in patterns {
        let (path, suf) = match p.rfind(':') {
            Some(i) => (&p[..i], &p[i..]),
            None => (p.as_str(), ""),
        };
        if path.contains('*') || path.contains('?') || path.contains('[') {
            let mut hits: Vec<String> = GlobWalkerBuilder::from_patterns(".", &[path])
                .build()?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| {
                    let s = e.path().to_string_lossy();
                    format!("{}{}", s.strip_prefix("./").unwrap_or(&s), suf)
                })
                .collect();
            hits.sort();
            out.extend(hits);
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let files = expand(&args.files)?;
    if files.is_empty() {
        eprintln!("peek2: no matching files");
        std::process::exit(1);
    }
    let mut by_path: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let mut idx: HashMap<String, usize> = HashMap::new();
    for spec in &files {
        let (path, range) = match spec.rfind(':') {
            Some(i) => (spec[..i].to_string(), spec[i + 1..].to_string()),
            None => (spec.clone(), String::new()),
        };
        let e = *idx.entry(path.clone()).or_insert_with(|| {
            by_path.push((PathBuf::from(&path), Vec::new()));
            by_path.len() - 1
        });
        by_path[e].1.push(range);
    }
    for (path, ranges) in &by_path {
        let indexed = match Indexed::open(path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("peek2: {}: {e}", path.display());
                continue;
            }
        };
        for r in ranges {
            let (mut s, mut e) = match if r.is_empty() { Sel::Range(1, usize::MAX) } else { parse_sel(r)? } {
                Sel::Range(a, b) => (a, b),
                Sel::Symbol(sym) => match indexed.find_symbol(&sym) {
                    Some(ln) => (ln, ln + 20),
                    None => {
                        eprintln!("peek2: {}: symbol '{sym}' not found", path.display());
                        continue;
                    }
                },
            };
            if args.context > 0 {
                s = s.saturating_sub(args.context).max(1);
                e = e.saturating_add(args.context);
            }
            let lines = indexed.range(s, e);
            let shown_end = (s + lines.len()).saturating_sub(1);
            if args.full_headers {
                println!("== {}:{}-{} ==", std::fs::canonicalize(path)?.display(), s, shown_end);
            } else if args.headers {
                println!("== {}:{}-{} ==", path.display(), s, shown_end);
            }
            for (i, ln) in lines.iter().enumerate() {
                if args.numbered {
                    println!("{}: {}", s + i, ln);
                } else {
                    println!("{ln}");
                }
            }
            if args.headers || args.full_headers {
                println!();
            }
        }
    }
    Ok(())
}
