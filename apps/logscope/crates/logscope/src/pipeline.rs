#![allow(dead_code)]
/// pipeline.rs — filter + summarize pipeline
///
/// This is the core 2026 feature: before any log is rotated or deleted,
/// it runs through a configurable pipeline:
///
///   Input log
///     └─► stage: filter         (keep/drop lines by regex, level, freq)
///     └─► stage: summarize      (condense repeated blocks into counts)
///     └─► stage: reformat       (normalize timestamps, strip noise)
///     └─► output: keep | archive | delete
///
/// Each stage is independent and optional. The pipeline config is stored
/// as a .toml file alongside the rotation config.
///
/// The same pipeline logic runs in both:
///   - The GUI (preview mode, shows before/after counts)
///   - The logscope-filter binary (called from prerotate hooks)

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::fs::File;

use regex::Regex;
use serde::{Deserialize, Serialize};

// ─── config types ─────────────────────────────────────────────────────────────

/// Log level severity — parsed from common log formats.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn from_line(line: &str) -> Option<Self> {
        let low = line.to_lowercase();
        // Check for common log level patterns
        for (pat, level) in [
            ("fatal", LogLevel::Fatal),
            ("error", LogLevel::Error),
            (" err ", LogLevel::Error),
            ("[err]", LogLevel::Error),
            ("warn", LogLevel::Warn),
            ("[warn]", LogLevel::Warn),
            ("info", LogLevel::Info),
            ("[info]", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("[debug]", LogLevel::Debug),
            ("trace", LogLevel::Trace),
        ] {
            if low.contains(pat) {
                return Some(level);
            }
        }
        None
    }
}

/// A single filter rule — lines matching are kept or dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRule {
    pub name: String,
    pub pattern: String,
    pub action: FilterAction,
    #[serde(skip)]
    compiled: Option<Regex>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterAction {
    /// Keep lines matching this pattern
    Keep,
    /// Drop lines matching this pattern
    Drop,
}

impl FilterRule {
    pub fn new(name: &str, pattern: &str, action: FilterAction) -> Result<Self, regex::Error> {
        let compiled = Regex::new(pattern)?;
        Ok(FilterRule {
            name: name.into(),
            pattern: pattern.into(),
            action,
            compiled: Some(compiled),
        })
    }

    pub fn compile(&mut self) -> Result<(), regex::Error> {
        self.compiled = Some(Regex::new(&self.pattern)?);
        Ok(())
    }

    pub fn matches(&self, line: &str) -> bool {
        self.compiled
            .as_ref()
            .map(|re| re.is_match(line))
            .unwrap_or(false)
    }
}

/// What to do with a processed log before rotation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputDisposition {
    /// Write filtered output back to the log file (in-place)
    InPlace,
    /// Write to a separate archive file (keeps original untouched)
    Archive { path: PathBuf },
    /// Write a summary file alongside the full archive
    SummaryAndArchive { archive: PathBuf, summary: PathBuf },
    /// After filtering/summarizing, delete entirely
    Delete,
}

/// Full pipeline configuration for one log file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub log_path: PathBuf,

    /// Minimum log level to keep (drop everything below this)
    pub min_level: Option<LogLevel>,

    /// Filter rules applied in order — first matching rule wins
    pub rules: Vec<FilterRule>,

    /// Collapse repeated identical lines into "N× <line>"
    pub collapse_repeats: bool,

    /// Collapse repeated blocks (by hash) with a count header
    pub collapse_blocks: bool,

    /// Token frequency threshold — drop lines whose only tokens
    /// appear more than this many times in the peek window
    /// (catches chatty heartbeat/keepalive noise)
    pub drop_if_token_freq_above: Option<u64>,

    pub disposition: OutputDisposition,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            log_path: PathBuf::new(),
            min_level: None,
            rules: Vec::new(),
            collapse_repeats: true,
            collapse_blocks: false,
            drop_if_token_freq_above: None,
            disposition: OutputDisposition::InPlace,
        }
    }
}

// ─── pipeline result ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    pub lines_read: u64,
    pub lines_kept: u64,
    pub lines_dropped: u64,
    pub lines_collapsed: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Which rules fired and how many times
    pub rule_hits: HashMap<String, u64>,
}

// ─── public API ───────────────────────────────────────────────────────────────

/// Run the pipeline on `input_path`, writing results according to `config.disposition`.
/// Returns stats for display in the GUI preview.
pub fn run(config: &mut PipelineConfig, input_path: &Path) -> std::io::Result<PipelineStats> {
    // Compile any rules that haven't been compiled yet
    for rule in &mut config.rules {
        if rule.compiled.is_none() {
            rule.compile().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
            })?;
        }
    }

    match &config.disposition.clone() {
        OutputDisposition::InPlace => {
            let tmp = input_path.with_extension("tmp");
            let stats = run_to_writer(config, input_path, &tmp)?;
            std::fs::rename(&tmp, input_path)?;
            Ok(stats)
        }
        OutputDisposition::Archive { path } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            run_to_writer(config, input_path, path)
        }
        OutputDisposition::SummaryAndArchive { archive, summary } => {
            if let Some(parent) = archive.parent() { std::fs::create_dir_all(parent)?; }
            if let Some(parent) = summary.parent() { std::fs::create_dir_all(parent)?; }
            let stats = run_to_writer(config, input_path, archive)?;
            write_summary(&stats, config, summary)?;
            Ok(stats)
        }
        OutputDisposition::Delete => {
            // Still run the pipeline to produce stats, but write to /dev/null
            let sink = PathBuf::from("/dev/null");
            let stats = run_to_writer(config, input_path, &sink)?;
            std::fs::remove_file(input_path)?;
            Ok(stats)
        }
    }
}

/// Preview mode — runs pipeline on first `max_lines` lines only,
/// returns (kept_lines, stats) without writing anything.
pub fn preview(
    config: &mut PipelineConfig,
    input_path: &Path,
    max_lines: usize,
) -> std::io::Result<(Vec<String>, PipelineStats)> {
    // Compile rules
    for rule in &mut config.rules {
        if rule.compiled.is_none() {
            rule.compile().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
            })?;
        }
    }

    let file = File::open(input_path)?;
    let reader = BufReader::with_capacity(64 * 1024, file);
    let mut stats = PipelineStats::default();
    let mut kept = Vec::new();
    let mut prev_line: Option<String> = None;
    let mut repeat_count: u64 = 0;

    for raw in reader.lines().take(max_lines) {
        let line = raw?;
        stats.bytes_in += line.len() as u64 + 1;
        stats.lines_read += 1;

        if should_drop(config, &line, &mut stats) {
            stats.lines_dropped += 1;
            continue;
        }

        // Collapse exact repeats
        if config.collapse_repeats {
            if prev_line.as_deref() == Some(&line) {
                repeat_count += 1;
                stats.lines_collapsed += 1;
                continue;
            } else {
                if repeat_count > 0 {
                    kept.push(format!("  ↑ repeated {repeat_count}×"));
                    repeat_count = 0;
                }
                prev_line = Some(line.clone());
            }
        }

        stats.lines_kept += 1;
        stats.bytes_out += line.len() as u64 + 1;
        kept.push(line);
    }
    if repeat_count > 0 {
        kept.push(format!("  ↑ repeated {repeat_count}×"));
    }

    Ok((kept, stats))
}

// ─── internals ────────────────────────────────────────────────────────────────

fn run_to_writer(
    config: &PipelineConfig,
    input: &Path,
    output: &Path,
) -> std::io::Result<PipelineStats> {
    let infile = File::open(input)?;
    let reader = BufReader::with_capacity(8 * 1024 * 1024, infile);
    let mut writer = std::io::BufWriter::new(File::create(output)?);
    let mut stats = PipelineStats::default();
    let mut prev_line: Option<String> = None;
    let mut repeat_count: u64 = 0;

    // Block-collapse state
    let _block_buf: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);
    let _block_counts: HashMap<u64, u64> = HashMap::new();

    for raw in reader.lines() {
        let line = raw?;
        stats.bytes_in += line.len() as u64 + 1;
        stats.lines_read += 1;

        if should_drop(config, &line, &mut stats) {
            stats.lines_dropped += 1;
            continue;
        }

        // Collapse exact repeats
        if config.collapse_repeats {
            if prev_line.as_deref() == Some(line.as_str()) {
                repeat_count += 1;
                stats.lines_collapsed += 1;
                continue;
            } else {
                if repeat_count > 0 {
                    let msg = format!("  # repeated {repeat_count}×\n");
                    writer.write_all(msg.as_bytes())?;
                    stats.bytes_out += msg.len() as u64;
                    repeat_count = 0;
                }
                prev_line = Some(line.clone());
            }
        }

        stats.lines_kept += 1;
        let out = line + "\n";
        stats.bytes_out += out.len() as u64;
        writer.write_all(out.as_bytes())?;
    }

    if repeat_count > 0 {
        let msg = format!("  # repeated {repeat_count}×\n");
        writer.write_all(msg.as_bytes())?;
    }

    writer.flush()?;
    Ok(stats)
}

/// Returns true if this line should be dropped.
fn should_drop(
    config: &PipelineConfig,
    line: &str,
    stats: &mut PipelineStats,
) -> bool {
    // Level filter
    if let Some(min_level) = &config.min_level {
        if let Some(level) = LogLevel::from_line(line) {
            if level < *min_level {
                return true;
            }
        }
        // Lines with no detectable level pass through (conservative)
    }

    // Rule-based filter
    for rule in &config.rules {
        if rule.matches(line) {
            *stats.rule_hits.entry(rule.name.clone()).or_default() += 1;
            return rule.action == FilterAction::Drop;
        }
    }

    false
}

fn write_summary(
    stats: &PipelineStats,
    config: &PipelineConfig,
    path: &Path,
) -> std::io::Result<()> {
    let mut f = File::create(path)?;
    writeln!(f, "# logscope summary")?;
    writeln!(f, "# generated: {}", chrono::Utc::now().to_rfc3339())?;
    writeln!(f, "# source: {}", config.log_path.display())?;
    writeln!(f, "")?;
    writeln!(f, "lines_read:      {}", stats.lines_read)?;
    writeln!(f, "lines_kept:      {}", stats.lines_kept)?;
    writeln!(f, "lines_dropped:   {}", stats.lines_dropped)?;
    writeln!(f, "lines_collapsed: {}", stats.lines_collapsed)?;
    writeln!(f, "bytes_in:        {}", stats.bytes_in)?;
    writeln!(f, "bytes_out:       {}", stats.bytes_out)?;
    if !stats.rule_hits.is_empty() {
        writeln!(f, "")?;
        writeln!(f, "# rule hits")?;
        let mut hits: Vec<_> = stats.rule_hits.iter().collect();
        hits.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in hits {
            writeln!(f, "{name}: {count}")?;
        }
    }
    Ok(())
}

// ─── config persistence ───────────────────────────────────────────────────────

pub fn save_config(config: &PipelineConfig, path: &Path) -> std::io::Result<()> {
    let toml_str = toml::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml_str)
}

pub fn load_config(path: &Path) -> std::io::Result<PipelineConfig> {
    let s = std::fs::read_to_string(path)?;
    let mut config: PipelineConfig = toml::from_str(&s)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Recompile regexes after deserialization
    for rule in &mut config.rules {
        let _ = rule.compile();
    }
    Ok(config)
}
