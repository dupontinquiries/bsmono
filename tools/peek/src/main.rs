//! Peek — fast line-range file viewer with bucket notation.
mod cli;
mod formatter;
mod glob;
mod parser;
mod query;
mod reader;

use anyhow::Result;
use clap::Parser;
use cli::Args;
use formatter::{FormatOptions, HeaderMode};
use parser::parse_query;
use query::Query;
use reader::Reader;

fn main() -> Result<()> {
    let args = Args::parse();
    let files = glob::expand_globs(&args.files)?;

    if files.is_empty() {
        eprintln!("peek: no matching files");
        std::process::exit(1);
    }

    let format_opts = FormatOptions {
        numbered: args.numbered,
        header_mode: if args.full_headers {
            HeaderMode::Absolute
        } else if args.headers {
            HeaderMode::Relative
        } else {
            HeaderMode::None
        },
    };

    let mut had_error = false;
    for file_spec in &files {
        let query = match parse_query(file_spec) {
            Ok(q) => q,
            Err(e) => {
                eprintln!("peek: {file_spec}: {e}");
                had_error = true;
                continue;
            }
        };
        let (path, start, end) = match query {
            Query::Range { path, start, end } => (path, start, end),
            Query::Bucket { path, bucket, width } => {
                let start = bucket * width;
                let end = start + width - 1; // inclusive bucket (5xx → 500..=599)
                (path, start.max(1), end)
            }
            Query::Symbol { .. } => {
                eprintln!("peek: {file_spec}: symbol queries not yet implemented");
                had_error = true;
                continue;
            }
        };

        let reader = match Reader::open(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("peek: {}: {e}", path.display());
                had_error = true;
                continue;
            }
        };
        let content = reader.read_range(start, end)?;
        let real = path.to_string_lossy();
        let output = formatter::format_output(&content, file_spec, &real, start, end, &format_opts);
        print!("{output}");
    }
    if had_error {
        std::process::exit(1);
    }
    Ok(())
}
