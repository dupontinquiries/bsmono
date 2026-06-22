//! Voice-to-terminal command classifier backed by tldr pages.
//!
//! Indexes the user's tldr cache (~/.cache/tldr/pages/) — a curated, plain-English
//! description of how each command is used. When voice text doesn't match any of
//! our structured intents, we score it against this index and surface the top
//! match as a 🎯 candidate.
//!
//! Why tldr instead of man pages: tldr entries are short (~5-10 hand-picked
//! usage examples per command), written in natural language ("Display the
//! largest directories at the top"), and already ranked by usefulness. Man pages
//! are verbose, jargon-heavy, and structured for reference rather than search.
//! Indexing tldr is ~10MB of text vs ~100MB for man pages.
//!
//! Index format on disk: bincode-serialized Vec<CommandDoc>, cached at
//! ~/.config/web2qml/command_index.bin. Built once at first call (~5s for 5000
//! tldr pages), reloaded instantly on subsequent runs. User can force a rebuild
//! by deleting the cache file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CommandDoc {
    pub name: String,
    /// One-line summary from the tldr "> ..." description block (pre-examples)
    pub summary: String,
    /// Each example: (description, command_template)
    pub examples: Vec<(String, String)>,
    /// True if `name` exists on the user's current $PATH
    pub on_path: bool,
}

#[derive(Debug, Clone)]
pub struct CommandMatch {
    pub name: String,
    /// The command line to execute (from the best-matching example, or just `name`)
    pub command: String,
    /// Score 0.0 - 1.0. Anything > 0.7 is a strong match.
    pub score: f32,
    /// Human-readable reason ("matched 'disk usage' in dust summary")
    pub reason: String,
    /// True if installed; false if tldr knows about it but it's not on PATH
    pub installed: bool,
}

// ─── Synonym expansion ────────────────────────────────────────────────────

/// Common verb synonyms for natural-language shell intent. Adding a token also
/// adds its synonyms to the search vocabulary, so "show files" matches commands
/// described with "list", "display", or "print".
const VERB_SYNONYMS: &[(&str, &[&str])] = &[
    ("show",     &["display", "list", "print", "view", "see"]),
    ("display",  &["show", "list", "print", "view"]),
    ("list",     &["show", "display", "print", "ls"]),
    ("find",     &["search", "locate", "grep", "look"]),
    ("search",   &["find", "grep", "locate", "look"]),
    ("delete",   &["remove", "rm", "destroy", "drop"]),
    ("remove",   &["delete", "rm", "drop"]),
    ("kill",     &["stop", "terminate", "end"]),
    ("stop",     &["kill", "terminate", "end", "halt"]),
    ("start",    &["run", "launch", "begin", "open"]),
    ("run",      &["start", "execute", "launch"]),
    ("create",   &["make", "new", "add"]),
    ("make",     &["create", "new", "build"]),
    ("install",  &["add", "setup"]),
    ("update",   &["upgrade", "refresh"]),
    ("upgrade",  &["update", "refresh"]),
    ("download", &["fetch", "get", "pull"]),
    ("upload",   &["push", "send"]),
    ("connect",  &["link", "join", "attach"]),
    ("monitor",  &["watch", "track", "observe"]),
    ("watch",    &["monitor", "follow"]),
    ("size",     &["disk", "space", "usage"]),
    ("disk",     &["size", "space", "storage"]),
    ("usage",    &["size", "space", "stat"]),
    ("process",  &["proc", "task"]),
    ("network",  &["net", "connection"]),
    ("file",     &["files"]),
    ("folder",   &["directory", "dir"]),
    ("directory",&["folder", "dir"]),
    // "list" is a high-leverage natural verb. "list X" → ls/ps/du/find/etc.
    // We don't pick the command here; we just expand so ps's "list all
    // running processes" example matches when the user voices "list ...".
    ("list",     &["show", "display", "print"]),
    ("running",  &["active", "live"]),
    ("processes",&["procs", "tasks", "jobs"]),
];

/// Expand a token into [token, synonyms...]
fn expand_token(tok: &str) -> Vec<&'static str> {
    for (key, syns) in VERB_SYNONYMS {
        if *key == tok {
            let mut out = vec![*key];
            out.extend_from_slice(syns);
            return out;
        }
    }
    Vec::new()
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "of", "in", "on", "at",
    "to", "for", "with", "and", "or", "but", "this", "that", "these", "those",
    "i", "me", "my", "you", "your", "it", "its", "be", "been", "being",
    "do", "does", "did", "have", "has", "had", "will", "would", "should",
    "can", "could", "may", "might", "please",
];

fn is_stopword(s: &str) -> bool { STOPWORDS.iter().any(|w| *w == s) }

// ─── Tokenization ─────────────────────────────────────────────────────────

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && t.len() >= 2 && !is_stopword(t))
        .map(|s| s.to_string())
        .collect()
}

// ─── Index loading ────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    dirs_config().map(|p| p.join("command_index.bin"))
}

fn dirs_config() -> Option<PathBuf> {
    let p = std::env::var("HOME").ok().map(PathBuf::from)?
        .join(".config").join("web2qml");
    let _ = std::fs::create_dir_all(&p);
    Some(p)
}

/// Locate the user's tldr cache. Returns Some(path_to_pages_dir) if found.
fn tldr_cache_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from)?;
    // tealdeer (Rust): ~/.cache/tldr/tldr-main/pages/
    // tldr-pages official: ~/.cache/tldr/pages/
    // Some installs: ~/.local/share/tldr/pages/
    let candidates = [
        home.join(".cache/tldr/tldr-main/pages"),
        home.join(".cache/tldr/pages"),
        home.join(".local/share/tldr/pages"),
        home.join(".tldr/cache/pages"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

/// Build a hash set of executables present on $PATH.
fn path_set() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        out.insert(name);
                    }
                }
            }
        }
    }
    out
}

/// Parse one tldr markdown page into a CommandDoc.
fn parse_tldr_page(content: &str, name: &str, on_path: bool) -> CommandDoc {
    let mut summary = String::new();
    let mut examples: Vec<(String, String)> = Vec::new();
    let mut current_desc: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }

        // # Title (skipped)
        if line.starts_with('#') { continue; }

        // > Summary line
        if let Some(rest) = line.strip_prefix('>') {
            let s = rest.trim();
            // First > is the summary; subsequent > lines are "see also" / "more info"
            if summary.is_empty() && !s.starts_with("See also")
                && !s.starts_with("More info") && !s.starts_with("Note") {
                summary = s.trim_end_matches('.').to_string();
            }
            continue;
        }

        // - Description (an example heading)
        if let Some(rest) = line.strip_prefix('-') {
            let desc = rest.trim()
                .trim_end_matches(':').trim()
                .to_string();
            current_desc = Some(desc);
            continue;
        }

        // `command template` (example body)
        if line.starts_with('`') && line.ends_with('`') && line.len() >= 3 {
            let cmd = line[1..line.len() - 1].to_string();
            if let Some(d) = current_desc.take() {
                examples.push((d, cmd));
            }
        }
    }

    CommandDoc {
        name: name.to_string(),
        summary,
        examples,
        on_path,
    }
}

/// Walk the tldr cache and parse every page into a CommandDoc. Returns
/// the parsed docs and a count of pages skipped due to errors.
fn build_index_from_disk() -> Vec<CommandDoc> {
    let Some(root) = tldr_cache_root() else {
        crate::clog!("cmd_index", "no tldr cache found — install tealdeer (cargo install tealdeer) and run `tldr --update`");
        return Vec::new();
    };
    crate::clog!("cmd_index", "indexing tldr from {:?}", root);
    let t0 = Instant::now();
    let path_bins = path_set();
    let mut docs: Vec<CommandDoc> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Walk the platform subdirectories. Order matters: prefer linux/osx specific
    // pages over `common`, so we take the first occurrence of each name.
    let platform = if cfg!(target_os = "linux") { "linux" }
                   else if cfg!(target_os = "macos") { "osx" }
                   else if cfg!(target_os = "windows") { "windows" }
                   else { "linux" };

    let preferred_dirs = [platform, "common"];
    for dir in preferred_dirs {
        let dir_path = root.join(dir);
        if !dir_path.is_dir() { continue; }
        let Ok(entries) = fs::read_dir(&dir_path) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") { continue; }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if !seen.insert(stem.to_string()) { continue; }  // first wins (platform-specific)
            let Ok(content) = fs::read_to_string(&path) else { continue };
            let doc = parse_tldr_page(&content, stem, path_bins.contains(stem));
            // Skip empties
            if doc.summary.is_empty() && doc.examples.is_empty() { continue; }
            docs.push(doc);
        }
    }

    crate::clog!("cmd_index", "indexed {} commands in {:.1}s",
        docs.len(), t0.elapsed().as_secs_f32());
    docs
}

/// Manual binary serialization to avoid pulling in serde for a single use.
/// Format: u32 count, then per-doc [u8 on_path, u32 name_len, name, u32 summary_len,
/// summary, u32 example_count, per-example (u32 desc_len, desc, u32 cmd_len, cmd)].
fn serialize_index(docs: &[CommandDoc]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(docs.len() * 256);
    out.extend_from_slice(&(docs.len() as u32).to_le_bytes());
    for d in docs {
        out.push(if d.on_path { 1 } else { 0 });
        out.extend_from_slice(&(d.name.len() as u32).to_le_bytes());
        out.extend_from_slice(d.name.as_bytes());
        out.extend_from_slice(&(d.summary.len() as u32).to_le_bytes());
        out.extend_from_slice(d.summary.as_bytes());
        out.extend_from_slice(&(d.examples.len() as u32).to_le_bytes());
        for (desc, cmd) in &d.examples {
            out.extend_from_slice(&(desc.len() as u32).to_le_bytes());
            out.extend_from_slice(desc.as_bytes());
            out.extend_from_slice(&(cmd.len() as u32).to_le_bytes());
            out.extend_from_slice(cmd.as_bytes());
        }
    }
    out
}

fn deserialize_index(data: &[u8]) -> Option<Vec<CommandDoc>> {
    let mut p = 0;
    let read_u32 = |p: &mut usize, data: &[u8]| -> Option<u32> {
        if *p + 4 > data.len() { return None; }
        let v = u32::from_le_bytes(data[*p..*p+4].try_into().ok()?);
        *p += 4;
        Some(v)
    };
    let read_str = |p: &mut usize, data: &[u8]| -> Option<String> {
        let len = read_u32(p, data)? as usize;
        if *p + len > data.len() { return None; }
        let s = std::str::from_utf8(&data[*p..*p+len]).ok()?.to_string();
        *p += len;
        Some(s)
    };

    let count = read_u32(&mut p, data)? as usize;
    let mut docs = Vec::with_capacity(count);
    for _ in 0..count {
        if p + 1 > data.len() { return None; }
        let on_path = data[p] != 0;
        p += 1;
        let name = read_str(&mut p, data)?;
        let summary = read_str(&mut p, data)?;
        let example_count = read_u32(&mut p, data)? as usize;
        let mut examples = Vec::with_capacity(example_count);
        for _ in 0..example_count {
            let desc = read_str(&mut p, data)?;
            let cmd = read_str(&mut p, data)?;
            examples.push((desc, cmd));
        }
        docs.push(CommandDoc { name, summary, examples, on_path });
    }
    Some(docs)
}

// ─── Lazy-init shared index ───────────────────────────────────────────────

static INDEX: std::sync::OnceLock<Vec<CommandDoc>> = std::sync::OnceLock::new();

/// Get the command index, building it on first call. Returns empty slice if
/// no tldr cache is available — the caller should fall back gracefully.
pub fn index() -> &'static [CommandDoc] {
    INDEX.get_or_init(|| {
        // Try cached first. Cache is invalidated by deletion (manual) — no
        // automatic staleness check, since users update tldr manually anyway.
        if let Some(p) = cache_path() {
            if let Ok(data) = fs::read(&p) {
                if let Some(docs) = deserialize_index(&data) {
                    crate::clog!("cmd_index", "loaded {} commands from cache {:?}", docs.len(), p);
                    // Rebuild PATH membership since binaries may have changed
                    let path_bins = path_set();
                    let mut docs = docs;
                    for d in &mut docs {
                        d.on_path = path_bins.contains(&d.name);
                    }
                    return docs;
                }
            }
        }
        let mut docs = build_index_from_disk();
        let used_fallback = docs.is_empty();
        if used_fallback {
            // No tldr cache available — fall back to a small built-in index of
            // the most common commands so the classifier still does something useful.
            // User can install tealdeer + run `tldr --update` to get a much richer index.
            crate::clog!("cmd_index", "using built-in fallback index ({} commands). For richer matching: install tealdeer (cargo install tealdeer && tldr --update)", BUILTIN_INDEX.len());
            let path_bins = path_set();
            docs = BUILTIN_INDEX.iter().map(|(name, summary, examples)| {
                CommandDoc {
                    name: (*name).to_string(),
                    summary: (*summary).to_string(),
                    examples: examples.iter().map(|(d, c)| (d.to_string(), c.to_string())).collect(),
                    on_path: path_bins.contains(*name),
                }
            }).collect();
        }
        // Only persist real (tldr-derived) indexes to disk. The built-in fallback
        // is cheap to rebuild from constants and we don't want it to "stick" — if
        // the user installs tealdeer later, the next run should pick up the real
        // tldr cache instead of our fallback.
        if !used_fallback {
            if let Some(p) = cache_path() {
                let bytes = serialize_index(&docs);
                let _ = fs::write(&p, bytes);
            }
        }
        docs
    })
}

// ─── Built-in fallback index ──────────────────────────────────────────────
//
// A minimal hand-curated set of the most common shell commands, in the same
// shape as parsed tldr pages. Used when no tldr cache is available.
// Format: (name, summary, &[(description, command_template)])
type FallbackEntry = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const BUILTIN_INDEX: &[FallbackEntry] = &[
    ("ls",    "List directory contents",
        &[("List files in long format with sizes", "ls -lh"),
          ("List all files including hidden", "ls -la"),
          ("List files sorted by modification time", "ls -lt")]),
    ("cd",    "Change the current working directory", &[]),
    ("pwd",   "Print the current working directory", &[("Print current directory", "pwd")]),
    ("cat",   "Print and concatenate files",
        &[("Display contents of a file", "cat {{file}}")]),
    ("less",  "View text files with paging",
        &[("View a file with paging", "less {{file}}")]),
    ("grep",  "Search text patterns in files",
        &[("Search recursively for a pattern", "grep -r {{pattern}} ."),
          ("Search ignoring case", "grep -i {{pattern}} {{file}}")]),
    ("rg",    "Recursively search for a pattern with ripgrep",
        &[("Search for a pattern recursively in current directory", "rg {{pattern}}"),
          ("Search and show context lines", "rg -C 3 {{pattern}}")]),
    ("find",  "Find files by name, type, or other attributes",
        &[("Find files by name pattern", "find . -name '{{pattern}}'"),
          ("Find files modified in last 7 days", "find . -mtime -7"),
          ("Find files containing text (use grep instead for content)", "find . -type f -exec grep -l '{{pattern}}' {} +")]),
    ("fd",    "Find files and directories — fast and user friendly find alternative",
        &[("Find files matching a pattern", "fd {{pattern}}")]),
    ("du",    "Display disk usage of files and directories",
        &[("Show total size of current directory", "du -sh ."),
          ("List sizes of immediate children", "du -h --max-depth=1")]),
    ("dust",  "Display disk usage as an interactive tree (better du)",
        &[("Show disk usage of current directory", "dust"),
          ("Show top 30 directories", "dust -n 30")]),
    ("ncdu",  "Interactive disk usage analyzer with ncurses",
        &[("Analyze disk usage interactively", "ncdu")]),
    ("df",    "Display disk free space",
        &[("Show free space in human-readable format", "df -h")]),
    ("ps",    "Display running processes",
        &[("List all running processes", "ps aux"),
          ("Find a specific process", "ps aux | grep {{name}}")]),
    ("pgrep", "Look up running processes by name",
        &[("Find PIDs of a process by name", "pgrep {{name}}")]),
    ("pkill", "Kill processes by name",
        &[("Kill all processes matching name", "pkill {{name}}"),
          ("Kill with SIGKILL", "pkill -9 {{name}}")]),
    ("kill",  "Send a signal to a process",
        &[("Terminate a process by PID", "kill {{pid}}"),
          ("Force kill a process", "kill -9 {{pid}}")]),
    ("htop",  "Interactive process viewer (better top)",
        &[("Launch htop", "htop")]),
    ("top",   "Display sorted information about running processes",
        &[("Show top processes interactively", "top")]),
    ("ss",    "Show network sockets and connections",
        &[("List all TCP connections", "ss -tunap"),
          ("Show listening ports", "ss -tlnp")]),
    ("netstat", "Network statistics and connections",
        &[("Show listening ports", "netstat -tlnp")]),
    ("ping",  "Send ICMP echo requests to a host",
        &[("Ping a host", "ping {{host}}")]),
    ("curl",  "Transfer data from or to a server",
        &[("Download a URL", "curl -O {{url}}"),
          ("Make a GET request", "curl {{url}}")]),
    ("wget",  "Download files from the web",
        &[("Download a file", "wget {{url}}")]),
    ("git",   "Distributed version control system",
        &[("Show repository status", "git status"),
          ("View commit history", "git log --oneline"),
          ("Stage all changes and commit", "git add -A && git commit -m '{{message}}'")]),
    ("tar",   "Archive files",
        &[("Extract a tar.gz archive", "tar -xzf {{file}}.tar.gz"),
          ("Create a tar.gz archive", "tar -czf {{out}}.tar.gz {{path}}")]),
    ("ssh",   "Secure shell remote login",
        &[("Connect to a host", "ssh {{user}}@{{host}}")]),
    ("scp",   "Secure copy files over ssh",
        &[("Copy a file to a remote host", "scp {{file}} {{user}}@{{host}}:{{path}}")]),
    ("rsync", "Sync files locally or to a remote host",
        &[("Sync a directory tree", "rsync -av {{src}}/ {{dest}}/")]),
    ("vim",   "Vi-improved text editor",
        &[("Edit a file", "vim {{file}}")]),
    ("nano",  "Simple text editor",
        &[("Edit a file", "nano {{file}}")]),
    ("hx",    "Helix modal text editor",
        &[("Edit a file", "hx {{file}}")]),
    ("man",   "Display the manual page of a command",
        &[("Show manual for a command", "man {{command}}")]),
    ("tldr",  "Display simple practical examples for commands",
        &[("Show examples for a command", "tldr {{command}}")]),
    ("which", "Locate a command on the PATH",
        &[("Find where a command lives", "which {{command}}")]),
    ("chmod", "Change file permissions",
        &[("Make a file executable", "chmod +x {{file}}")]),
    ("chown", "Change file owner and group",
        &[("Change owner", "chown {{user}}:{{group}} {{file}}")]),
    ("ln",    "Create file links",
        &[("Create a symbolic link", "ln -s {{target}} {{linkname}}")]),
    ("mv",    "Move or rename files",
        &[("Move or rename a file", "mv {{src}} {{dest}}")]),
    ("cp",    "Copy files and directories",
        &[("Copy a file", "cp {{src}} {{dest}}"),
          ("Copy a directory recursively", "cp -r {{src}} {{dest}}")]),
    ("rm",    "Remove files and directories",
        &[("Remove a file", "rm {{file}}"),
          ("Remove a directory recursively (DESTRUCTIVE)", "rm -rf {{dir}}")]),
    ("mkdir", "Make directories",
        &[("Create a directory", "mkdir {{name}}"),
          ("Create nested directories", "mkdir -p {{path}}")]),
    ("touch", "Create empty files or update timestamps",
        &[("Create a file", "touch {{file}}")]),
    ("history", "Show shell command history",
        &[("Show command history", "history")]),
    ("date",  "Display or set the date and time",
        &[("Show current date and time", "date")]),
    ("uptime", "Show how long the system has been running",
        &[("Show system uptime and load", "uptime")]),
    ("free",  "Display memory usage",
        &[("Show free memory", "free -h")]),
    ("watch", "Monitor a command's output by re-running it periodically",
        &[("Run a command every 2 seconds", "watch {{command}}")]),
    ("tail",  "View the end of a file or follow new appends",
        &[("Follow a log file", "tail -f {{file}}"),
          ("Show the last 50 lines", "tail -n 50 {{file}}")]),
    ("head",  "View the beginning of a file",
        &[("Show first 20 lines", "head -n 20 {{file}}")]),
    ("sort",  "Sort lines of text",
        &[("Sort a file", "sort {{file}}")]),
    ("uniq",  "Filter duplicate adjacent lines",
        &[("Show unique lines (sort first)", "sort {{file}} | uniq")]),
    ("wc",    "Count lines, words, and characters",
        &[("Count lines in a file", "wc -l {{file}}")]),
    ("xargs", "Build and execute commands from input",
        &[("Run a command per input line", "echo {{items}} | xargs -n 1 {{command}}")]),
    ("bluetoothctl", "Bluetooth control",
        &[("List paired devices", "bluetoothctl devices"),
          ("Connect to a device by MAC", "bluetoothctl connect {{MAC}}")]),
    ("nmcli", "Network Manager command line",
        &[("List wifi networks", "nmcli device wifi list"),
          ("Connect to a network", "nmcli device wifi connect {{ssid}} password {{password}}")]),
    ("systemctl", "Control systemd services",
        &[("Check service status", "systemctl status {{service}}"),
          ("Restart a service", "sudo systemctl restart {{service}}")]),
    ("journalctl", "Query the systemd journal",
        &[("View recent logs", "journalctl -xe"),
          ("Follow logs for a service", "journalctl -fu {{service}}")]),
];


// ─── Scoring ──────────────────────────────────────────────────────────────

/// Score a search query against the entire index, returning the top N matches.
///
/// Scoring strategy:
///   1. First-word match against `name` is a strong signal (+0.6 to score)
///   2. Token overlap with `summary` (+0.3 per matching token, capped)
///   3. Token overlap with any example description (+0.4 per matching token, capped)
///      The best-matching example becomes the suggested command template
///   4. Synonym expansion: each query token also tries its synonyms
///   5. on_path is +0.1, not on path is a soft penalty (-0.05) — we still want
///      to surface "you could install dust to do this"
pub fn search(query: &str, top_n: usize) -> Vec<CommandMatch> {
    let docs = index();
    if docs.is_empty() { return Vec::new(); }

    let q_tokens: Vec<String> = tokenize(query);
    if q_tokens.is_empty() { return Vec::new(); }

    // Augment with any online-cached pages for words in the query.
    // Each token gets a quick existence check against the online
    // cache; cache hits add a CommandDoc to the search corpus for
    // this call only (not added to the in-memory index — these are
    // ephemeral additions). On the NEXT call to seed_from_utterance,
    // the same fetch is a no-op; on the next process restart, we
    // rebuild the in-memory index from scratch and these aren't
    // included unless the user does a full --update.
    let online_extras: Vec<CommandDoc> = q_tokens.iter()
        .filter_map(|tok| load_online_cached(tok))
        .collect();
    let combined_docs: Vec<&CommandDoc> = docs.iter()
        .chain(online_extras.iter())
        .collect();

    // Build expanded vocabulary: each query token + its synonyms
    let mut q_vocab: HashMap<String, f32> = HashMap::new();
    for tok in &q_tokens {
        q_vocab.insert(tok.clone(), 1.0);
        for syn in expand_token(tok) {
            // Synonyms count slightly less than direct hits
            q_vocab.entry(syn.to_string()).or_insert(0.6);
        }
    }

    let first_word = q_tokens.first().map(|s| s.as_str()).unwrap_or("");

    let mut scored: Vec<CommandMatch> = Vec::new();
    for doc in combined_docs.iter().copied() {
        let mut score = 0.0f32;
        let mut reasons: Vec<String> = Vec::new();
        let mut best_example: Option<(usize, f32)> = None;

        // Direct name match
        if first_word == doc.name {
            score += 0.7;
            reasons.push(format!("'{}' is the command", first_word));
        } else if doc.name.starts_with(first_word) && first_word.len() >= 3 {
            score += 0.4;
            reasons.push(format!("'{}' prefix-matches '{}'", first_word, doc.name));
        }

        // Summary match
        let summary_tokens = tokenize(&doc.summary);
        let mut summary_hits = 0;
        for st in &summary_tokens {
            if let Some(&w) = q_vocab.get(st) {
                summary_hits += 1;
                score += 0.25 * w;
            }
        }
        if summary_hits > 0 {
            reasons.push(format!("{} term{} in summary", summary_hits,
                if summary_hits == 1 { "" } else { "s" }));
        }

        // Example match — find the best example
        for (idx, (desc, _cmd)) in doc.examples.iter().enumerate() {
            let desc_tokens = tokenize(desc);
            let mut ex_score = 0.0f32;
            for dt in &desc_tokens {
                if let Some(&w) = q_vocab.get(dt) {
                    ex_score += 0.4 * w;
                }
            }
            if ex_score > 0.0 {
                if best_example.map(|(_, s)| ex_score > s).unwrap_or(true) {
                    best_example = Some((idx, ex_score));
                }
            }
        }
        if let Some((_, ex_score)) = best_example {
            score += ex_score.min(0.6);  // cap example contribution
        }

        // Path adjustment
        if doc.on_path { score += 0.1; }
        else { score -= 0.05; }

        // Need a minimum score to be a candidate
        if score < 0.3 { continue; }

        // Build the command line — use best matching example's template if any,
        // else the bare command name.
        let command = if let Some((idx, _)) = best_example {
            doc.examples[idx].1.clone()
        } else {
            doc.name.clone()
        };

        let reason = if reasons.is_empty() {
            format!("matched in {}", doc.name)
        } else {
            reasons.join(", ")
        };

        scored.push(CommandMatch {
            name: doc.name.clone(),
            command,
            score: score.min(1.0).max(0.0),
            reason,
            installed: doc.on_path,
        });
    }

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_n);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tldr_minimal() {
        let content = "# dust\n\n> Give an instant overview of which directories are using disk space.\n> See also: `du`, `ncdu`.\n\n- Display information for the current directory:\n\n`dust`\n\n- Display 30 directories:\n\n`dust [-n|--number-of-lines] 30`\n";
        let doc = parse_tldr_page(content, "dust", true);
        assert_eq!(doc.name, "dust");
        assert!(doc.summary.contains("disk space"));
        assert_eq!(doc.examples.len(), 2);
        assert_eq!(doc.examples[0].1, "dust");
    }

    #[test]
    fn synonym_expansion() {
        let exp: Vec<&'static str> = expand_token("show");
        assert!(exp.contains(&"show"));
        assert!(exp.contains(&"display"));
        assert!(exp.contains(&"list"));
    }

    #[test]
    fn serialize_roundtrip() {
        let docs = vec![CommandDoc {
            name: "test".into(),
            summary: "test cmd".into(),
            examples: vec![("desc".into(), "cmd".into())],
            on_path: true,
        }];
        let bytes = serialize_index(&docs);
        let back = deserialize_index(&bytes).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "test");
        assert_eq!(back[0].examples[0].1, "cmd");
    }
}

// ─── Online tldr fetch ────────────────────────────────────────────
//
// When the local tldr cache is missing OR a specific command isn't
// indexed, we can fetch its tldr page from the upstream repository
// on demand. Pages live at:
//
//   https://raw.githubusercontent.com/tldr-pages/tldr/main/pages/{linux|common}/<name>.md
//
// We hit linux/ first (more specific) then common/. Successful fetches
// cache locally at ~/.cache/web2qml/tldr-online/<name>.md so subsequent
// hits are free. Failed fetches store an empty marker so we don't
// pound the network on a 404'ing command name.
//
// Disabled by default — opt-in via WEB2QML_ONLINE_TLDR=1 OR via the
// new "Fetch tldr online" toggle (TODO in QML). When enabled, the
// fetch happens on a background thread; the result is consumed by
// the next call to lookup() that comes through. This is fire-and-
// forget — we don't block voice flow on network.

const TLDR_BASE: &str = "https://raw.githubusercontent.com/tldr-pages/tldr/main/pages";
const ONLINE_CACHE_DIR: &str = ".cache/web2qml/tldr-online";

/// Returns the cached online-tldr path for a given command, or None
/// if not cached. None means "not yet fetched" — it doesn't say
/// whether the command exists.
pub fn online_cache_path(name: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    // Sanitize name to avoid path traversal — only allow alphanumeric +
    // dash + underscore. Anything else: refuse.
    if name.is_empty() || name.len() > 64 { return None; }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let dir = std::path::Path::new(&home).join(ONLINE_CACHE_DIR);
    Some(dir.join(format!("{}.md", name)))
}

/// Ensure the cache directory exists. Idempotent.
fn ensure_online_cache_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::Path::new(&home).join(ONLINE_CACHE_DIR);
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Read a previously-cached online page if one exists.
/// Returns Some(parsed_doc) if cache hit AND the cache isn't a
/// negative-result marker (zero-length file).
pub fn load_online_cached(name: &str) -> Option<CommandDoc> {
    let path = online_cache_path(name)?;
    let content = std::fs::read_to_string(&path).ok()?;
    if content.is_empty() { return None; }
    let path_bins = path_set();
    let on_path = path_bins.contains(name);
    let doc = parse_tldr_page(&content, name, on_path);
    if doc.summary.is_empty() && doc.examples.is_empty() {
        return None;
    }
    Some(doc)
}

/// Trigger an async fetch of the tldr page for `name`. Returns
/// immediately. The result lands in the online cache; lookup callers
/// that come back later (e.g. on the next voice utterance) will find
/// it via load_online_cached.
///
/// No-op if WEB2QML_ONLINE_TLDR is not set OR the cache already has
/// an entry (positive or negative — we don't re-fetch failures
/// within the same session).
pub fn try_fetch_online(name: &str) {
    if std::env::var("WEB2QML_ONLINE_TLDR").is_err() {
        return;
    }
    let path = match online_cache_path(name) {
        Some(p) => p,
        None => return,
    };
    if path.exists() {
        // Already fetched (positive or negative). Don't re-hit.
        return;
    }
    let _ = ensure_online_cache_dir();
    let name_owned = name.to_string();
    let path_owned = path.clone();
    std::thread::spawn(move || {
        // Try platform-specific dir first, then common.
        let dirs = if cfg!(target_os = "linux") { ["linux", "common"] }
            else if cfg!(target_os = "macos") { ["osx", "common"] }
            else { ["common", "common"] };
        let mut fetched: Option<String> = None;
        for dir in dirs.iter() {
            let url = format!("{}/{}/{}.md", TLDR_BASE, dir, name_owned);
            // ureq is already a dep — short timeout, modest size cap.
            let result = ureq::get(&url)
                .timeout(std::time::Duration::from_secs(4))
                .call();
            match result {
                Ok(resp) => {
                    if resp.status() == 200 {
                        if let Ok(body) = resp.into_string() {
                            // tldr pages are typically <2KB
                            if body.len() < 16_000 && !body.is_empty() {
                                fetched = Some(body);
                                break;
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        match fetched {
            Some(body) => {
                let _ = std::fs::write(&path_owned, body);
                crate::clog!("cmd_index", "online tldr fetched: {}", name_owned);
            }
            None => {
                // Negative cache: empty file marks "we tried".
                let _ = std::fs::write(&path_owned, "");
                crate::clog!("cmd_index", "online tldr 404: {}", name_owned);
            }
        }
    });
}

/// Verb-analysis seed: given a voice utterance text, extract candidate
/// command names mentioned by verb patterns and trigger fetches for
/// any that aren't already indexed locally OR in the online cache.
/// Cheap (regex-style word matching) and fire-and-forget.
///
/// Examples of triggered fetches:
///   "use ripgrep to search" → triggers ripgrep
///   "show me with bat"      → triggers bat
///   "I want fzf to find"    → triggers fzf
///
/// The verb hints are common voice patterns. Nouns immediately after
/// "use" / "with" / "via" / "running" are the candidate command names.
pub fn seed_from_utterance(utterance: &str) {
    if std::env::var("WEB2QML_ONLINE_TLDR").is_err() {
        return;
    }
    let lower = utterance.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let triggers = ["use", "using", "with", "via", "run", "running", "try", "install"];
    for (i, w) in words.iter().enumerate() {
        if !triggers.contains(w) { continue; }
        // Next non-stopword
        let mut j = i + 1;
        while j < words.len() {
            let candidate = words[j].trim_matches(|c: char|
                !c.is_ascii_alphanumeric() && c != '-' && c != '_'
            );
            if candidate.len() >= 2 && candidate.len() <= 30
                && candidate.chars().all(|c| c.is_ascii_alphanumeric()
                    || c == '-' || c == '_')
            {
                // Skip articles / fillers
                if !["the", "a", "an", "to", "and", "or", "for", "this", "that"]
                    .contains(&candidate)
                {
                    try_fetch_online(candidate);
                    break;  // one fetch per trigger
                }
            }
            j += 1;
        }
    }
}

