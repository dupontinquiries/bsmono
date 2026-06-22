//! Indexed file viewer — the engine behind peek2 and the gitmind GUI's
//! file-viewer pane. An mmap'd file gets a one-time newline byte-offset
//! index so any line range is O(1) to slice; selection supports ranges,
//! buckets (`5xx`, `5k`), bucket extension (`5xx+20`), and `@symbol`
//! definition jumps.

use anyhow::{bail, Result};
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

/// An mmap'd file with a newline byte-offset index. `line_start[i]` is the
/// byte offset of line `i+1`.
pub struct Indexed {
    mmap: Mmap,
    line_start: Vec<usize>,
}

impl Indexed {
    pub fn open(p: &Path) -> Result<Indexed> {
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

    pub fn line_count(&self) -> usize {
        self.line_start.len()
    }

    /// 1-indexed inclusive range → borrowed line slices.
    pub fn range(&self, start: usize, end: usize) -> Vec<&str> {
        let n = self.line_count();
        let s = start.max(1);
        let e = end.min(n);
        let mut out = Vec::new();
        if s > n {
            return out;
        }
        for ln in s..=e {
            let b0 = self.line_start[ln - 1];
            let b1 = if ln < n { self.line_start[ln] - 1 } else { self.mmap.len() };
            let b1 = b1.min(self.mmap.len());
            out.push(std::str::from_utf8(&self.mmap[b0..b1]).unwrap_or("").trim_end_matches('\r'));
        }
        out
    }

    /// First line that looks like a definition of `sym`, else first mention.
    pub fn find_symbol(&self, sym: &str) -> Option<usize> {
        let kws = ["fn ", "struct ", "enum ", "trait ", "type ", "mod ", "const ", "static ", "class ", "def "];
        let mut first_any = None;
        for ln in 1..=self.line_count() {
            let t = self.range(ln, ln).first().copied().unwrap_or("");
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

pub enum Sel {
    Range(usize, usize),
    Symbol(String),
}

/// Parse a selection: `@sym`, `5xx`, `5xx+20`, `5k`, `A-B`, or `N`.
pub fn parse_sel(range: &str) -> Result<Sel> {
    if let Some(s) = range.strip_prefix('@') {
        return Ok(Sel::Symbol(s.to_string()));
    }
    let (core, ext): (&str, i64) = if let Some(i) = range.find('+') {
        (&range[..i], range[i + 1..].parse().unwrap_or(0))
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

pub fn digits(s: &str) -> Option<usize> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

/// High-level read for callers (the GUI): returns the start line and the
/// owned lines for `path` at `spec`, expanded by `context` on each side.
/// Empty `spec` means the whole file.
pub fn read_view(path: &Path, spec: &str, context: usize) -> Result<(usize, Vec<String>)> {
    let idx = Indexed::open(path)?;
    let sel = if spec.is_empty() { Sel::Range(1, usize::MAX) } else { parse_sel(spec)? };
    let (mut s, mut e) = match sel {
        Sel::Range(a, b) => (a, b),
        Sel::Symbol(sym) => match idx.find_symbol(&sym) {
            Some(ln) => (ln, ln + 20),
            None => bail!("symbol '{sym}' not found"),
        },
    };
    if context > 0 {
        s = s.saturating_sub(context).max(1);
        e = e.saturating_add(context);
    }
    let lines = idx.range(s, e).into_iter().map(|l| l.to_string()).collect();
    Ok((s, lines))
}

/// Convenience for the GUI: a single string, optionally line-numbered.
pub fn read_to_string(path: &Path, spec: &str, context: usize, numbered: bool) -> Result<String> {
    let (start, lines) = read_view(path, spec, context)?;
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if numbered {
            out.push_str(&format!("{:>6}  {}\n", start + i, l));
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn range_and_bucket() {
        let f = tmp("l1\nl2\nl3\nl4\nl5\n");
        let (start, lines) = read_view(f.path(), "2-4", 0).unwrap();
        assert_eq!(start, 2);
        assert_eq!(lines, vec!["l2", "l3", "l4"]);
    }

    #[test]
    fn symbol_jump() {
        let f = tmp("a\nb\nfn target() {}\nbody\n");
        let (start, lines) = read_view(f.path(), "@target", 0).unwrap();
        assert_eq!(start, 3);
        assert!(lines[0].contains("fn target"));
    }

    #[test]
    fn context_expands() {
        let f = tmp("1\n2\n3\n4\n5\n");
        let (start, lines) = read_view(f.path(), "3", 1).unwrap();
        assert_eq!(start, 2);
        assert_eq!(lines, vec!["2", "3", "4"]);
    }
}
