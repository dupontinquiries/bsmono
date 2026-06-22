//! peek v2 — daemonless, but with a per-invocation newline index (mmap → byte
//! offsets) so multiple ranges of the same file seek in O(1) instead of v1's
//! line-by-line scan, plus richer queries: `@symbol` jumps to a definition,
//! `-C N` adds context, `5xx+20` extends a bucket. Still a single binary, no
//! daemon, no persistent cache. The index is built once per file per call and
//! reused across that call's ranges (the only "cache" a daemonless tool can have).
use anyhow::{bail, Result};
use clap::Parser;
use globwalk::GlobWalkerBuilder;
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
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

/// An mmap'd file with a lazily-built newline byte-offset index. `line_start[i]`
/// is the byte offset of line `i+1`. Built once, reused for every range.
struct Indexed {
    mmap: Mmap,
    line_start: Vec<usize>,
}
impl Indexed {
    fn open(p: &PathBuf) -> Result<Indexed> {
        let f = File::open(p)?;
        let mmap = unsafe { Mmap::map(&f)? };
        let mut line_start = Vec::with_capacity(1 + mmap.len() / 32);
        line_start.push(0);
        for (i, b) in mmap.iter().enumerate() {
            if *b == b'\n' {
                line_start.push(i + 1);
            }
        }
        Ok(Indexed { mmap, line_start })
    }
    fn line_count(&self) -> usize {
        self.line_start.len()
    }
    /// 1-indexed inclusive range → the lines (no allocation per line beyond the slice).
    fn range(&self, start: usize, end: usize) -> Vec<&str> {
        let n = self.line_count();
        let s = start.max(1);
        let e = end.min(n);
        let mut out = Vec::new();
        for ln in s..=e {
            let b0 = self.line_start[ln - 1];
            let b1 = if ln < n { self.line_start[ln] - 1 } else { self.mmap.len() };
            let b1 = b1.min(self.mmap.len());
            out.push(std::str::from_utf8(&self.mmap[b0..b1]).unwrap_or("").trim_end_matches('\r'));
        }
        out
    }
    /// Find the first line whose text contains a likely definition of `sym`
    /// (`fn sym`, `struct sym`, `enum sym`, `trait sym`, `mod sym`, `sym =`),
    /// else the first line that merely contains `sym`.
    fn find_symbol(&self, sym: &str) -> Option<usize> {
        let kws = ["fn ", "struct ", "enum ", "trait ", "type ", "mod ", "const ", "static "];
        let mut first_any = None;
        for ln in 1..=self.line_count() {
            let txt = self.range(ln, ln);
            let t = txt.first().copied().unwrap_or("");
            if t.contains(sym) {
                if first_any.is_none() {
                    first_any = Some(ln);
                }
                if kws.iter().any(|k| t.contains(&format!("{k}{sym}"))) {
                    return Some(ln);
                }
            }
        }
        first_any
    }
}

enum Sel {
    Range(usize, usize),
    Symbol(String),
}

fn parse_sel(range: &str) -> Result<Sel> {
    if let Some(s) = range.strip_prefix('@') {
        return Ok(Sel::Symbol(s.to_string()));
    }
    // bucket + extend: 5xx+20  /  5xx-10
    let (core, ext): (&str, i64) = if let Some(i) = range.find(['+']) {
        (&range[..i], range[i + 1..].parse().unwrap_or(0))
    } else if range.ends_with("xx") {
        (range, 0)
    } else {
        (range, 0)
    };
    if let Some(b) = core.strip_suffix("xx") {
        let base: usize = digits(b).unwrap_or(0) * 100;
        return Ok(Sel::Range(base.max(1), base + 99 + ext.max(0) as usize));
    }
    if let Some(b) = core.strip_suffix('k') {
        let base: usize = digits(b).unwrap_or(0) * 1000;
        return Ok(Sel::Range(base.max(1), base + 999 + ext.max(0) as usize));
    }
    if let Some(d) = core.find('-') {
        let a = digits(&core[..d]).unwrap_or(1);
        let b = digits(&core[d + 1..]).unwrap_or(usize::MAX);
        return Ok(Sel::Range(a.max(1), b));
    }
    if let Some(n) = digits(core) {
        return Ok(Sel::Range(n.max(1), n));
    }
    bail!("bad range '{range}'")
}
fn digits(s: &str) -> Option<usize> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
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
    // group ranges by path so each file is indexed once, then queried many times.
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
                    Some(ln) => (ln, ln + 20), // a symbol jump shows the def + 20 lines
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
