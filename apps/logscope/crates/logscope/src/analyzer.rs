/// analyzer.rs — streaming log analysis engine
use std::collections::HashMap;
use std::fs::File;
use std::hash::Hasher;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use twox_hash::XxHash64;

pub const CHUNK_SIZE:    usize = 8 * 1024 * 1024;
pub const PEEK_LINES:    usize = 5_000;   // reduced: 10k was slow for wide HTML
pub const MIN_TOKEN_LEN: usize = 3;
pub const TOP_TOKENS:    usize = 30;

/// Max bytes we'll tokenize per line. HTML/minified files have lines
/// tens of thousands of chars wide — tokenizing them is O(n) with huge n.
const MAX_TOKENIZE_LINE_BYTES: usize = 2_048;

#[derive(Debug, Clone)]
pub enum AnalyzerMsg {
    PeekReady { lines: Vec<String>, token_freqs: Vec<(String, u64)> },
    Progress   { bytes_done: u64, total_bytes: u64, unique_blocks: usize, repeated_blocks: u64 },
    BlockHistogram(Vec<(u64, u64)>),
    Done { elapsed_secs: f64, total_bytes: u64, total_blocks: u64, unique_blocks: usize, top_tokens: Vec<(String, u64)> },
    Error(String),
}

pub fn analyze_async(path: PathBuf, tx: Sender<AnalyzerMsg>, cancel: Arc<AtomicBool>) {
    // Use rayon instead of std::thread so we share the pool and avoid
    // spawning unbounded OS threads on mobile.
    rayon::spawn(move || {
        if let Err(e) = run_analysis(&path, &tx, &cancel) {
            let _ = tx.send(AnalyzerMsg::Error(e.to_string()));
        }
    });
}

fn run_analysis(path: &Path, tx: &Sender<AnalyzerMsg>, cancel: &Arc<AtomicBool>) -> std::io::Result<()> {
    let start = Instant::now();
    let file = File::open(path)?;
    let total_bytes = file.metadata()?.len();
    let reader = BufReader::with_capacity(64 * 1024, &file); // small buffer — faster first-byte

    let (peeked_lines, token_freqs, bytes_after_peek) = peek_phase(reader, cancel)?;

    if cancel.load(Ordering::Relaxed) { return Ok(()); }

    let _ = tx.send(AnalyzerMsg::PeekReady {
        lines: peeked_lines,
        token_freqs: token_freqs.clone(),
    });

    // Phase 2: block-hash pass with larger buffer
    let mut file2 = File::open(path)?;
    file2.seek(SeekFrom::Start(bytes_after_peek))?;
    let mut reader2 = BufReader::with_capacity(CHUNK_SIZE, file2);

    let mut block_counts: HashMap<u64, u64> = HashMap::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut bytes_done = bytes_after_peek;
    let mut total_blocks: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) { break; }
        let n = reader2.read(&mut buf)?;
        if n == 0 { break; }
        let hash = xxhash64(&buf[..n]);
        *block_counts.entry(hash).or_default() += 1;
        bytes_done += n as u64;
        total_blocks += 1;
        if total_blocks % 64 == 0 {
            let unique = block_counts.len();
            let repeated: u64 = block_counts.values().filter(|&&c| c>1).map(|&c| c-1).sum();
            let _ = tx.send(AnalyzerMsg::Progress { bytes_done, total_bytes, unique_blocks: unique, repeated_blocks: repeated });
        }
    }

    let mut histogram: Vec<(u64,u64)> = block_counts.into_iter().collect();
    histogram.sort_by(|a,b| b.1.cmp(&a.1));
    let _ = tx.send(AnalyzerMsg::BlockHistogram(histogram.clone()));

    let unique_blocks = histogram.len();
    let _repeated_blocks: u64 = histogram.iter().filter(|(_,c)| *c>1).map(|(_,c)| c-1).sum();
    let _ = tx.send(AnalyzerMsg::Done {
        elapsed_secs: start.elapsed().as_secs_f64(),
        total_bytes, total_blocks, unique_blocks,
        top_tokens: token_freqs,
    });
    Ok(())
}

fn peek_phase<R: BufRead>(mut reader: R, cancel: &Arc<AtomicBool>)
    -> std::io::Result<(Vec<String>, Vec<(String,u64)>, u64)>
{
    let mut lines:      Vec<String>           = Vec::with_capacity(PEEK_LINES);
    let mut token_map:  HashMap<String, u64>  = HashMap::new();
    let mut byte_offset: u64 = 0;

    for _ in 0..PEEK_LINES {
        if cancel.load(Ordering::Relaxed) { break; }
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 { break; }
        byte_offset += n as u64;

        // Skip tokenizing very long lines (minified HTML/JS/CSS).
        // They have no log structure and take O(len) time for zero value.
        if line.len() <= MAX_TOKENIZE_LINE_BYTES {
            for token in tokenize(&line) {
                if token.len() >= MIN_TOKEN_LEN {
                    *token_map.entry(token).or_default() += 1;
                }
            }
        }
        lines.push(line);
    }

    let mut freqs: Vec<(String,u64)> = token_map.into_iter().collect();
    freqs.sort_by(|a,b| b.1.cmp(&a.1));
    freqs.truncate(TOP_TOKENS);
    Ok((lines, freqs, byte_offset))
}

fn tokenize(line: &str) -> impl Iterator<Item=String> + '_ {
    line.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
}

fn xxhash64(data: &[u8]) -> u64 {
    let mut h = XxHash64::default();
    h.write(data);
    h.finish()
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}
impl FileInfo {
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(FileInfo { path: path.to_owned(), size_bytes: meta.len(), modified: meta.modified().ok() })
    }
}

pub fn default_pins() -> Vec<PathBuf> {
    let mut pins = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let helix = PathBuf::from(&home).join(".cache/helix/helix.log");
        if helix.exists() { pins.push(helix); }
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let c = PathBuf::from(xdg).join("helix/helix.log");
        if c.exists() && !pins.contains(&c) { pins.push(c); }
    }
    pins
}
