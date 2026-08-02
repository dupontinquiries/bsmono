//! `shelfctl` — a small command-line companion to tomescope for scripting
//! and headless use: scanning a library folder, listing books, and
//! exporting highlights/annotations without opening the GUI.
//!
//! Usage:
//!   shelfctl scan <root-dir>
//!   shelfctl list [--sort recent|title|author|progress]
//!   shelfctl export-highlights <book-id-hex> [--format md|json] [--out FILE]
//!   shelfctl stats
//!   shelfctl info <book-id-hex>

use std::env;
use std::path::PathBuf;

// The binary shares the library crate's modules by depending on the
// `tomescope` lib target (see Cargo.toml `[lib]` + `[[bin]]` sections).
use tomescope::library::{Library, SortKey};
use tomescope::persist;
use tomescope::study::export_highlights_markdown;
use tomescope::util::humanize_bytes;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        std::process::exit(1);
    };

    let result = match cmd.as_str() {
        "scan" => cmd_scan(&args[1..]),
        "list" => cmd_list(&args[1..]),
        "export-highlights" => cmd_export_highlights(&args[1..]),
        "stats" => cmd_stats(),
        "info" => cmd_info(&args[1..]),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    println!(
        "shelfctl — command-line companion to tomescope\n\n\
         Commands:\n  \
         scan <root-dir>                       Scan a folder into the library\n  \
         list [--sort recent|title|author|progress]\n                                        List books in the library\n  \
         export-highlights <book-id-hex> [--format md|json] [--out FILE]\n  \
         stats                                 Show reading stats summary\n  \
         info <book-id-hex>                    Show metadata for one book"
    );
}

fn cmd_scan(args: &[String]) -> anyhow::Result<()> {
    let root = args
        .first()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("scan requires a root directory argument"))?;

    let mut library = persist::load_library();
    if !library.scan_roots.iter().any(|r| r == &root) {
        library.scan_roots.push(root.clone());
    }
    let report = library.scan(&persist::covers_dir());
    persist::save_library(&library)?;

    println!(
        "Scanned {}: {} added, {} updated, {} missing, {} errors",
        root.display(),
        report.added,
        report.updated,
        report.missing,
        report.errors.len()
    );
    // ScanReport::errors is (path, message) — printing the tuple does not
    // compile, and printing only the message loses which file failed, which is
    // the one thing the operator needs from a scan of a thousand books.
    for (path, msg) in &report.errors {
        eprintln!("  ! {}: {msg}", path.display());
    }
    Ok(())
}

fn cmd_list(args: &[String]) -> anyhow::Result<()> {
    let library = persist::load_library();
    let sort = parse_sort_flag(args).unwrap_or(SortKey::RecentlyAdded);
    let ids = library.view("", tomescope::library::ShelfSel::All, sort);

    println!("{:<18} {:<8} {:>6}  {}", "ID", "FORMAT", "PROG", "TITLE — AUTHOR");
    for id in ids {
        if let Some(book) = library.get(id) {
            println!(
                "{:016x}  {:<8} {:>5}%  {} — {}",
                book.id,
                book.format.label(),
                (book.percent * 100.0).round() as i32,
                book.title,
                book.author_line(),
            );
        }
    }
    Ok(())
}

fn parse_sort_flag(args: &[String]) -> Option<SortKey> {
    let idx = args.iter().position(|a| a == "--sort")?;
    match args.get(idx + 1).map(|s| s.as_str()) {
        Some("recent") => Some(SortKey::RecentlyRead),
        Some("title") => Some(SortKey::Title),
        Some("author") => Some(SortKey::Author),
        Some("progress") => Some(SortKey::Progress),
        _ => None,
    }
}

fn parse_book_id(hex: &str) -> anyhow::Result<u64> {
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|_| anyhow::anyhow!("invalid book id, expected hex string like 0a1b2c3d4e5f6789"))
}

fn cmd_export_highlights(args: &[String]) -> anyhow::Result<()> {
    let id_hex = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("export-highlights requires a book id"))?;
    let book_id = parse_book_id(id_hex)?;

    let format = args
        .iter()
        .position(|a| a == "--format")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("md");

    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let library = persist::load_library();
    let title = library
        .get(book_id)
        .map(|b| b.title.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let state = persist::load_book_state(book_id);
    let output = match format {
        "json" => tomescope::study::export_highlights_json(&state.highlights),
        _ => export_highlights_markdown(&title, &state.highlights),
    };

    match out_path {
        Some(path) => {
            std::fs::write(&path, output)?;
            println!("Wrote {}", path.display());
        }
        None => println!("{output}"),
    }
    Ok(())
}

fn cmd_stats() -> anyhow::Result<()> {
    let stats = persist::load_stats();
    println!("Total reading time: {}", tomescope::util::humanize_duration_secs(stats.total_secs()));
    println!("Current streak: {} day(s)", stats.current_streak(60));
    println!("Longest streak: {} day(s)", stats.longest_streak(60));
    println!("Pages/hour: {:.1}", stats.pages_per_hour());
    println!("\nTime per book:");
    let library = persist::load_library();
    for (book_id, secs) in stats.time_per_book().into_iter().take(10) {
        let title = library
            .get(book_id)
            .map(|b| b.title.clone())
            .unwrap_or_else(|| format!("{book_id:016x}"));
        println!("  {:<40} {}", title, tomescope::util::humanize_duration_secs(secs));
    }
    Ok(())
}

fn cmd_info(args: &[String]) -> anyhow::Result<()> {
    let id_hex = args.first().ok_or_else(|| anyhow::anyhow!("info requires a book id"))?;
    let book_id = parse_book_id(id_hex)?;
    let library = persist::load_library();
    let Some(book) = library.get(book_id) else {
        anyhow::bail!("no book with id {id_hex} in library");
    };
    println!("Title:     {}", book.title);
    println!("Author:    {}", book.author_line());
    if let Some(series) = book.series_line() {
        println!("Series:    {series}");
    }
    println!("Format:    {}", book.format.label());
    println!("Size:      {}", humanize_bytes(book.file_size));
    println!("Progress:  {:.0}%", book.percent * 100.0);
    println!("Path:      {}", book.path.display());
    Ok(())
}
