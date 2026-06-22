//! corpus_md — convert a directory tree of HTML into one cleaned Markdown corpus.
//!
//! A dependency-free, multithreaded Rust rewrite of tools/create-md-from-web.py.
//! Unlike the python version it does NOT shell out to `html2text`, so the pipeline
//! has no python/html2text dependency.
//!
//!   corpus_md <input_dir> <output_file> [-j N]
//!
//! Walks <input_dir> for .html/.htm, converts each to readable Markdown-ish text,
//! prefixes it with `# FILE: <path>`, and concatenates everything into
//! <output_file> (parent dirs created). `-j` sets the worker thread count.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut positional: Vec<String> = Vec::new();
    let mut jobs: usize = 4;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-j" | "--jobs" => { i += 1; if i < args.len() { jobs = args[i].parse().unwrap_or(4); } }
            "-h" | "--help" => { eprintln!("usage: corpus_md <input_dir> <output_file> [-j N]"); std::process::exit(0); }
            s => positional.push(s.to_string()),
        }
        i += 1;
    }
    if positional.len() < 2 {
        eprintln!("usage: corpus_md <input_dir> <output_file> [-j N]");
        std::process::exit(2);
    }
    let input = PathBuf::from(&positional[0]);
    let output = PathBuf::from(&positional[1]);
    jobs = jobs.clamp(1, 64);

    let mut files = Vec::new();
    find_html(&input, &mut files);
    files.sort();
    println!("Found {} HTML files", files.len());
    println!("Using {jobs} workers");

    // Split into `jobs` contiguous chunks; each thread converts its chunk in order.
    let nthreads = jobs.min(files.len().max(1));
    let chunk = (files.len() / nthreads).max(1);
    let slots: Mutex<Vec<(usize, Vec<String>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for (idx, part) in files.chunks(chunk).enumerate() {
            let slots = &slots;
            s.spawn(move || {
                let out: Vec<String> = part.iter().map(|p| convert_file(p)).collect();
                slots.lock().unwrap().push((idx, out));
            });
        }
    });

    let mut ordered = slots.into_inner().unwrap();
    ordered.sort_by_key(|(idx, _)| *idx);
    let mut combined = String::new();
    for (_, chunk) in ordered {
        for block in chunk {
            combined.push_str(&block);
        }
    }

    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&output, combined) {
        eprintln!("error writing {}: {e}", output.display());
        std::process::exit(1);
    }
    println!("Done: {}", output.display());
}

fn find_html(dir: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            find_html(&p, out);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if ext == "html" || ext == "htm" { out.push(p); }
        }
    }
}

fn convert_file(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(html) => format!("# FILE: {}\n\n{}\n\n", path.display(), clean(&html_to_md(&html))),
        Err(e) => format!("# FILE: {}\n\nERROR: {e}\n\n", path.display()),
    }
}

/// Minimal HTML → Markdown-ish text. Not a full parser — good enough for building
/// a text corpus (the downstream BPE/word2vec care about words, not perfect MD).
fn html_to_md(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '>' { j += 1; }
            let tag: String = chars[i + 1..j.min(chars.len())].iter().collect();
            let t = tag.trim().to_ascii_lowercase();
            let closing = t.starts_with('/');
            let name: String = t.trim_start_matches('/')
                .chars().take_while(|c| c.is_ascii_alphanumeric()).collect();

            // Drop <script>/<style> blocks wholesale.
            if (name == "script" || name == "style") && !closing {
                let close = format!("</{name}>");
                let rest: String = chars[j + 1..].iter().collect();
                if let Some(pos) = rest.to_ascii_lowercase().find(&close) {
                    let consumed = rest[..pos + close.len()].chars().count();
                    i = j + 1 + consumed;
                } else {
                    i = chars.len();
                }
                continue;
            }

            match name.as_str() {
                "br" => out.push('\n'),
                "p" | "div" | "section" | "article" | "tr" | "ul" | "ol" | "table"
                | "blockquote" | "pre" | "header" | "footer" | "nav" => out.push('\n'),
                "li" if !closing => out.push_str("\n- "),
                "li" => out.push('\n'),
                "h1" if !closing => out.push_str("\n# "),
                "h2" if !closing => out.push_str("\n## "),
                "h3" if !closing => out.push_str("\n### "),
                "h4" | "h5" | "h6" if !closing => out.push_str("\n#### "),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push('\n'),
                _ => {}
            }
            i = j + 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    decode_entities(&out)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' {
            if let Some(semi) = chars[i + 1..].iter().position(|&c| c == ';') {
                let ent: String = chars[i + 1..i + 1 + semi].iter().collect();
                let replacement = match ent.as_str() {
                    "amp" => Some("&".to_string()),
                    "lt" => Some("<".to_string()),
                    "gt" => Some(">".to_string()),
                    "quot" => Some("\"".to_string()),
                    "apos" | "#39" => Some("'".to_string()),
                    "nbsp" => Some(" ".to_string()),
                    "mdash" => Some("—".to_string()),
                    "ndash" => Some("–".to_string()),
                    "hellip" => Some("…".to_string()),
                    other => other.strip_prefix('#')
                        .and_then(|n| n.parse::<u32>().ok())
                        .and_then(char::from_u32)
                        .map(|c| c.to_string()),
                };
                if let Some(r) = replacement {
                    out.push_str(&r);
                    i += semi + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Mirror the python cleaner: strip markdown images, flatten links, collapse blank
/// lines, and tidy whitespace.
fn clean(md: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut blank_run = 0;
    for raw in md.lines() {
        // collapse internal runs of spaces/tabs
        let mut line = String::with_capacity(raw.len());
        let mut prev_space = false;
        for ch in raw.chars() {
            if ch == ' ' || ch == '\t' {
                if !prev_space { line.push(' '); }
                prev_space = true;
            } else {
                line.push(ch);
                prev_space = false;
            }
        }
        let trimmed = line.trim_end().to_string();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 { lines.push(String::new()); } // collapse 2+ blanks to 1
        } else {
            blank_run = 0;
            lines.push(trimmed);
        }
    }
    lines.join("\n").trim().to_string()
}
