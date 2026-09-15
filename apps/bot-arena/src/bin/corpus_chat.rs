//! corpus_chat — convert Claude conversation exports into one training corpus.
//!
//! The chat-log sibling of `corpus_md` (which does HTML), and it takes the
//! SAME argv shape on purpose — `<input_dir> <output_file> [-j N]` — so it
//! drops into Corpus Studio's stage 0 without the studio needing a second
//! calling convention.
//!
//!   corpus_chat <input_dir> <output_file> [options]
//!
//!     --repo <dir>        correlate tool paths against this checkout
//!     --no-thinking       drop assistant reasoning blocks
//!     --no-tools          drop tool_use and tool_result blocks
//!     --no-tool-results   keep the calls, drop their output
//!     --result-cap N      max chars per tool result (default 2000, 0 = all)
//!     --input-cap N       max chars per tool_use value (default 4000)
//!     -j N                worker threads (default 4)
//!
//! ## Why the repo correlation is the point
//!
//! These logs are a record of work done ON a specific tree. A `str_replace`
//! at `/home/claude/unendlich/src/main.rs` is only meaningful if you can say
//! which file that actually was — the sandbox root is per-session scratch and
//! is gone. `--repo` turns those into `repo_path:` lines, which is what makes
//! the corpus correlatable with the code instead of being 220 orphaned
//! transcripts. See `repo_index` for why that is a suffix search.
//!
//! ## Encoding
//!
//! Files are read as BYTES and lossily decoded, not `read_to_string`. One file
//! in the 220-file sample has an invalid UTF-8 byte partway through
//! (`0xed` at offset ~7.5 MB) and `read_to_string` refuses the whole 7 MB
//! export over it. One replacement character is the right price.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bot_arena::chatlog::{self, Correlate, NoCorrelation, Policy};
use bot_arena::repo_index::{Match, RepoIndex};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut positional: Vec<String> = Vec::new();
    let mut jobs: usize = 4;
    let mut repo: Option<PathBuf> = None;
    let mut policy = Policy::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-j" | "--jobs" => {
                i += 1;
                if i < args.len() {
                    jobs = args[i].parse().unwrap_or(4);
                }
            }
            "--repo" => {
                i += 1;
                if i < args.len() {
                    repo = Some(PathBuf::from(&args[i]));
                }
            }
            "--result-cap" => {
                i += 1;
                if i < args.len() {
                    policy.tool_result_cap = args[i].parse().unwrap_or(2000);
                }
            }
            "--input-cap" => {
                i += 1;
                if i < args.len() {
                    policy.tool_input_cap = args[i].parse().unwrap_or(4000);
                }
            }
            "--no-thinking" => policy.include_thinking = false,
            "--no-tools" => {
                policy.include_tool_use = false;
                policy.include_tool_results = false;
            }
            "--no-tool-results" => policy.include_tool_results = false,
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                std::process::exit(0);
            }
            s => positional.push(s.to_string()),
        }
        i += 1;
    }
    if positional.len() < 2 {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let input = PathBuf::from(&positional[0]);
    let output = PathBuf::from(&positional[1]);
    jobs = jobs.clamp(1, 64);

    // Build the correlation index first: it is the slow setup step and a bad
    // --repo should fail before spending minutes parsing exports.
    let index = match &repo {
        Some(dir) => {
            if !dir.is_dir() {
                eprintln!("--repo {} is not a directory", dir.display());
                std::process::exit(2);
            }
            let ix = RepoIndex::from_dir(dir);
            println!("Indexed {} repo files from {}", ix.file_count(), dir.display());
            if ix.is_empty() {
                eprintln!("warning: repo index is empty; no paths will correlate");
            }
            Some(ix)
        }
        None => {
            println!("No --repo given: tool paths will not be correlated");
            None
        }
    };
    policy.correlate = index.is_some();

    let mut files = Vec::new();
    find_json(&input, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("no .json conversation files under {}", input.display());
        std::process::exit(1);
    }
    println!("Found {} conversation files", files.len());
    println!("Using {jobs} workers");

    let nthreads = jobs.min(files.len().max(1));
    let chunk = (files.len() / nthreads).max(1);
    let slots: Mutex<Vec<(usize, Vec<String>, Stats)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for (idx, part) in files.chunks(chunk).enumerate() {
            let slots = &slots;
            let policy = &policy;
            let index = &index;
            s.spawn(move || {
                let mut out = Vec::new();
                let mut stats = Stats::default();
                for p in part {
                    match convert_file(p, policy, index.as_ref(), &mut stats) {
                        Ok(text) => out.push(text),
                        Err(e) => {
                            // A damaged export is reported and skipped. One bad
                            // file must not cost the other 219.
                            eprintln!("skipping {}: {e}", p.display());
                            stats.failed += 1;
                        }
                    }
                }
                slots.lock().unwrap().push((idx, out, stats));
            });
        }
    });

    let mut ordered = slots.into_inner().unwrap();
    ordered.sort_by_key(|(idx, _, _)| *idx);
    let mut combined = String::new();
    let mut total = Stats::default();
    for (_, chunk, stats) in ordered {
        for text in chunk {
            combined.push_str(&text);
            combined.push('\n');
        }
        total.merge(&stats);
    }

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Err(e) = std::fs::write(&output, &combined) {
        eprintln!("cannot write {}: {e}", output.display());
        std::process::exit(1);
    }

    println!("\nWrote {} ({} bytes)", output.display(), combined.len());
    println!(
        "Episodes: {} ok, {} failed. Turns: {}.",
        total.episodes, total.failed, total.turns
    );
    println!(
        "Blocks: {} text, {} thinking, {} tool_use, {} tool_result, {} other.",
        total.text, total.thinking, total.tool_use, total.tool_result, total.other
    );
    // The correlation counts are the quality signal for this corpus, so they
    // are printed rather than hidden: a sudden collapse in `unique` means the
    // --repo checkout no longer matches the era the logs came from.
    println!(
        "Correlation: {} unique, {} ambiguous, {} unmatched ({} paths seen).",
        total.corr_unique,
        total.corr_ambiguous,
        total.corr_none,
        total.corr_unique + total.corr_ambiguous + total.corr_none
    );
}

const USAGE: &str = "usage: corpus_chat <input_dir> <output_file> [--repo DIR] \
[--no-thinking] [--no-tools] [--no-tool-results] [--result-cap N] [--input-cap N] [-j N]";

#[derive(Default, Clone)]
struct Stats {
    episodes: usize,
    failed: usize,
    turns: usize,
    text: usize,
    thinking: usize,
    tool_use: usize,
    tool_result: usize,
    other: usize,
    corr_unique: usize,
    corr_ambiguous: usize,
    corr_none: usize,
}

impl Stats {
    fn merge(&mut self, o: &Stats) {
        self.episodes += o.episodes;
        self.failed += o.failed;
        self.turns += o.turns;
        self.text += o.text;
        self.thinking += o.thinking;
        self.tool_use += o.tool_use;
        self.tool_result += o.tool_result;
        self.other += o.other;
        self.corr_unique += o.corr_unique;
        self.corr_ambiguous += o.corr_ambiguous;
        self.corr_none += o.corr_none;
    }
}

fn convert_file(
    path: &Path,
    policy: &Policy,
    index: Option<&RepoIndex>,
    stats: &mut Stats,
) -> Result<String, String> {
    // Bytes + lossy, not read_to_string: see the module header. One export in
    // the sample dies on read_to_string over a single invalid byte.
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let src = String::from_utf8_lossy(&bytes);
    let ep = chatlog::parse_episode(&src)?;

    let (t, th, tu, tr, o) = ep.block_counts();
    stats.episodes += 1;
    stats.turns += ep.turns.len();
    stats.text += t;
    stats.thinking += th;
    stats.tool_use += tu;
    stats.tool_result += tr;
    stats.other += o;

    if let Some(ix) = index {
        for p in ep.touched_paths() {
            match ix.resolve_detailed(p) {
                Match::Unique(_) => stats.corr_unique += 1,
                Match::Ambiguous(_) => stats.corr_ambiguous += 1,
                Match::None => stats.corr_none += 1,
            }
        }
    }

    let corr: &dyn Correlate = match index {
        Some(ix) => ix,
        None => &NoCorrelation,
    };
    Ok(chatlog::render(&ep, policy, corr))
}

/// Collect `.json` files. Export trees are `<conversation>/<conversation>.json`
/// alongside an `artifacts/` dir; only the conversation JSON is a transcript,
/// and the artifacts are already source files the repo has.
fn find_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            find_json(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
}
