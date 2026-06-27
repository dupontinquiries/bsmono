//! Formatter
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum HeaderMode { None, Relative, Absolute }

pub struct FormatOptions {
    pub numbered: bool,
    pub header_mode: HeaderMode,
}

pub fn format_output(
    lines: &[String],
    display: &str,
    real: &str,
    start: usize,
    end: usize,
    opts: &FormatOptions,
) -> String {
    let mut out = String::new();
    match opts.header_mode {
        HeaderMode::Relative => out.push_str(&format!("== {}:{}-{} ==\n", display, start, end)),
        HeaderMode::Absolute => out.push_str(&format!("== {}:{}-{} ==\n", Path::new(real).display(), start, end)),
        _ => {}
    }
    for (i, ln) in lines.iter().enumerate() {
        let n = start + i;
        if opts.numbered { out.push_str(&format!("{}: {}\n", n, ln)); }
        else { out.push_str(&format!("{}\n", ln)); }
    }
    if !lines.is_empty() { out.push('\n'); }
    out
}
