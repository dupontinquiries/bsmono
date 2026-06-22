// src/entity_extract.rs
//
// PHASE 0: Terminal-output-as-menu.
//
// When the user runs a structured command (dust, git status/log/branch, rg,
// bluetoothctl devices) and then issues a natural-language voice reference
// ("connect to headphones", "checkout the bug fix one"), this module:
//
//   1. Identifies which extractor matches the command
//   2. Pulls structured entities out of the output
//   3. Resolves the user's reference against those entities
//   4. Composes a follow-up command using the matched entity
//
// Direct-mode only in this phase (substring/fuzzy/ordinal matching). No filter
// mode (e.g. "all rust files"), no sub-query mode (e.g. "the last file
// changed"). Those are Phase 1+.
//
// The composed command is always surfaced via the candidate picker for user
// confirmation. Nothing auto-executes.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum EntityKind {
    /// Bluetooth device
    BluetoothDevice,
    /// Git commit
    GitCommit,
    /// Git branch
    GitBranch,
    /// File touched by git status (staged, unstaged, untracked)
    GitStatusFile,
    /// File path with size from dust
    DustEntry,
    /// File or directory in a `tree` listing. We track the full path
    /// reconstructed from the indentation so "open colorchooser.cpp" can
    /// resolve to "src/components/colorchooser.cpp" even when only the
    /// basename was voiced.
    TreeFile,
    /// Match line from ripgrep
    RgMatch,
    /// Process row from top/htop. Primary is the PID; aliases include
    /// the command/process name. Metadata carries cpu_pct, mem_pct,
    /// user, state.
    ProcessRow,
    /// Generic file (fallback)
    File,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub kind: EntityKind,
    /// Canonical identifier — what we substitute into the composed command.
    /// MAC for BT, hash for commit, branch name for branch, path for file, etc.
    pub primary: String,
    /// Human-readable label shown in the picker.
    pub display: String,
    /// Alt names / tokens we'll match user references against.
    pub aliases: Vec<String>,
    /// 0.0..1.0 confidence the extractor is in this entity.
    pub confidence: f32,
    /// Tool-specific extras (e.g. ps PID, rg line number, dust size).
    pub metadata: HashMap<String, String>,
}

impl Entity {
    /// Returns all strings worth scoring against a user reference (display
    /// + aliases). For filename-shaped entities, ALSO appends a
    /// separator-stripped variant: "colorchooser.cpp" gets a "colorchoosercpp"
    /// alongside, AND a space-separated "color chooser cpp" so that whisper's
    /// `color chooser .cpp` and `color_chooser.cpp` both have a chance of
    /// matching. This is how we work around whisper not knowing how to
    /// transcribe coder filenames.
    fn search_corpus(&self) -> String {
        let mut s = self.display.to_lowercase();
        for a in &self.aliases {
            s.push(' ');
            s.push_str(&a.to_lowercase());
        }
        // Add separator-collapsed and separator-spaced variants of any
        // alias that looks like a filename (has a dot extension or
        // contains _-./). Both directions help: "colorchooser" matches
        // a voiced "color chooser", AND a stored "color_chooser.cpp"
        // matches a voiced "colorchooser".
        for a in std::iter::once(&self.display).chain(self.aliases.iter()) {
            let lc = a.to_lowercase();
            // Despaced/dehyphenated/de-underscored form: "color_chooser.cpp" → "colorchoosercpp"
            let collapsed: String = lc.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if collapsed.len() >= 4 && collapsed != lc.replace(' ', "") {
                s.push(' ');
                s.push_str(&collapsed);
            }
            // Space-split form: "color_chooser.cpp" → "color chooser cpp"
            let spaced: String = lc.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
                .collect();
            let normalized = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized != lc && normalized.len() >= 4 {
                s.push(' ');
                s.push_str(&normalized);
            }
        }
        s
    }
}

/// Result of resolving a user reference against a set of entities.
#[derive(Clone, Debug)]
pub struct ResolvedMatch {
    pub entity: Entity,
    /// 0.0..1.0 — how confident we are about the user→entity mapping.
    pub score: f32,
    /// Human-readable explanation ("matched 'headphones'", "second commit", etc.)
    pub reason: String,
}

/// Extracted block: command + entities + which extractor produced them.
#[derive(Clone, Debug)]
pub struct ExtractedBlock {
    pub extractor_name: String,
    pub command: String,
    pub entities: Vec<Entity>,
}

// ─── Extractor trait ─────────────────────────────────────────────────────

pub trait Extractor: Send + Sync {
    fn name(&self) -> &'static str;
    /// Does this command line look like ours? Match on the COMMAND, not output.
    fn matches_command(&self, cmd: &str) -> bool;
    /// Extract entities from output. Returns empty vec on parse failure.
    fn extract(&self, command: &str, output: &str) -> Vec<Entity>;
    /// What verbs can the user say after this command? (verb -> composer template)
    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)];
}

/// How to compose a command from a verb + entity. Hardcoded per extractor.
#[derive(Clone, Copy, Debug)]
pub enum VerbTemplate {
    /// Run a shell command. Placeholders: {primary}, {display}, {meta:KEY}.
    Shell(&'static str),
    /// Always require explicit confirmation (destructive verb).
    ShellConfirm(&'static str),
}

impl VerbTemplate {
    fn render(&self, e: &Entity) -> String {
        let tmpl = match self {
            VerbTemplate::Shell(s) | VerbTemplate::ShellConfirm(s) => *s,
        };
        let mut out = String::with_capacity(tmpl.len() + e.primary.len());
        let mut chars = tmpl.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                let mut key = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc == '}' { chars.next(); break; }
                    key.push(nc);
                    chars.next();
                }
                match key.as_str() {
                    "primary" => out.push_str(&e.primary),
                    "display" => out.push_str(&e.display),
                    k if k.starts_with("meta:") => {
                        let mk = &k[5..];
                        if let Some(v) = e.metadata.get(mk) { out.push_str(v); }
                    }
                    _ => { out.push('{'); out.push_str(&key); out.push('}'); }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    pub fn is_destructive(&self) -> bool {
        matches!(self, VerbTemplate::ShellConfirm(_))
    }
}

// ─── Extractor: bluetoothctl devices ──────────────────────────────────────

pub struct BluetoothctlExtractor;

impl Extractor for BluetoothctlExtractor {
    fn name(&self) -> &'static str { "bluetoothctl-devices" }

    fn matches_command(&self, cmd: &str) -> bool {
        let c = cmd.trim_start();
        c.starts_with("bluetoothctl devices")
            || c.starts_with("bluetoothctl paired-devices")
            || c == "bluetoothctl"
    }

    fn extract(&self, _command: &str, output: &str) -> Vec<Entity> {
        // bluetoothctl devices output:
        //   Device AA:BB:CC:DD:EE:FF Sony WH-1000XM4
        //   Device 11:22:33:44:55:66 AirPods Pro
        let mut out = Vec::new();
        for line in output.lines() {
            let line = line.trim();
            // Strip ANSI codes and possible "[bluetooth]#" prompt prefix
            let line = strip_ansi(line);
            let line = line.trim_start_matches("[bluetooth]#").trim();
            if let Some(rest) = line.strip_prefix("Device ") {
                let mut sp = rest.splitn(2, ' ');
                let mac = sp.next().unwrap_or("").trim().to_string();
                let name = sp.next().unwrap_or("").trim().to_string();
                if mac.len() == 17 && mac.matches(':').count() == 5 && !name.is_empty() {
                    let aliases = name.split_whitespace().map(|s| s.to_string()).collect();
                    out.push(Entity {
                        kind: EntityKind::BluetoothDevice,
                        primary: mac.clone(),
                        display: name.clone(),
                        aliases,
                        confidence: 0.95,
                        metadata: HashMap::new(),
                    });
                }
            }
        }
        out
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        &[
            ("connect",    VerbTemplate::Shell("bluetoothctl connect {primary}")),
            ("disconnect", VerbTemplate::Shell("bluetoothctl disconnect {primary}")),
            ("pair",       VerbTemplate::Shell("bluetoothctl pair {primary}")),
            ("trust",      VerbTemplate::Shell("bluetoothctl trust {primary}")),
            ("untrust",    VerbTemplate::Shell("bluetoothctl untrust {primary}")),
            ("info",       VerbTemplate::Shell("bluetoothctl info {primary}")),
            ("remove",     VerbTemplate::ShellConfirm("bluetoothctl remove {primary}")),
            ("forget",     VerbTemplate::ShellConfirm("bluetoothctl remove {primary}")),
        ]
    }
}

// ─── Extractor: dust ──────────────────────────────────────────────────────

pub struct DustExtractor;

impl Extractor for DustExtractor {
    fn name(&self) -> &'static str { "dust" }

    fn matches_command(&self, cmd: &str) -> bool {
        let first = cmd.trim().split_whitespace().next().unwrap_or("");
        first == "dust"
    }

    fn extract(&self, _command: &str, output: &str) -> Vec<Entity> {
        // dust output looks roughly like:
        //   1.2G ┌── ./node_modules
        //   800M │ ┌── ./.git
        //   ...
        //   3.5G ┴── ./
        //
        // The size is at the start, the path is at the end of the line. Tree-drawing
        // characters in between (varies by terminal width and dust version).
        //
        // Diagnostic mode (v0.1.74): when we extract zero entries from
        // non-empty output, dump the first 5 lines to stderr so we can
        // see WHY the parse is failing. Common modern-dust regressions:
        //   - new tree characters not in our strip set
        //   - lowercase size suffixes ("234k" instead of "234K")
        //   - column reordering
        let mut out = Vec::new();
        let mut diag_size_failed = 0;
        let mut diag_path_failed = 0;
        let mut diag_short_lines = 0;
        let mut diag_failed_examples: Vec<String> = Vec::new();
        for line in output.lines() {
            let stripped = strip_ansi(line);
            let trimmed = stripped.trim();
            if trimmed.is_empty() { continue; }

            // Find size at start: any leading whitespace + size token (e.g. "1.2G", "800M", "234K", "0B")
            let mut tokens = trimmed.split_whitespace();
            let size_tok = match tokens.next() { Some(s) => s, None => continue };
            // Validate it looks like a size: digits + optional . + suffix letter
            if !looks_like_size(size_tok) {
                diag_size_failed += 1;
                if diag_failed_examples.len() < 5 {
                    diag_failed_examples.push(format!("size? {:?}", trimmed));
                }
                continue;
            }

            // Path is somewhere after the size, but NOT the last token — dust appends
            // bar-chart and percentage columns after the path. Walk backwards skipping
            // tokens that look like:
            //   - "100%" / "59%" — percentage
            //   - "|████|" / "▏▎▍▌▋▊▉█" — bar chart fragments (any combination of these chars + |)
            // Then take the first token after that as the candidate path.
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 { diag_short_lines += 1; continue; }
            // Find the rightmost "real" token by stepping backward past junk
            let mut idx = parts.len() - 1;
            while idx > 1 {
                let tok = parts[idx];
                let is_pct = tok.ends_with('%') &&
                    tok.trim_end_matches('%').chars().all(|c| c.is_ascii_digit() || c == '.');
                // Bar / progress fragment characters. Modern dust uses
                // a mix of:
                //   '│'   U+2502  box-drawing pipe (the bar's vertical edges)
                //   '█'   U+2588  full block (filled portion)
                //   '░▒▓' shaded blocks (variant fill chars)
                //   '▏▎▍▌▋▊▉' eighth-blocks (sub-cell fill, fractional)
                //   '|'   ASCII pipe (some terminals use this instead)
                //
                // The U+2502 was MISSING from this list before v0.1.83 —
                // breaking dust extraction on output that contains the
                // bar columns dust now emits by default. Without it,
                // walkback stopped at the first '│' and used it as the
                // "path" token, which then rejected as empty after
                // tree-char stripping. Result: every dust block had
                // `extracted=0` and voice flows like "navigate to
                // thumbnails" couldn't find the matching directory.
                let is_bar = tok.chars().all(|c|
                    c == '|' || c == '│'
                        || c == '█' || c == '▏' || c == '▎' || c == '▍'
                        || c == '▌' || c == '▋' || c == '▊' || c == '▉'
                        || c == '░' || c == '▒' || c == '▓');
                if is_pct || is_bar { idx -= 1; } else { break; }
            }
            let path_tok = parts[idx];
            // Strip tree characters that may have leaked into the path token
            let path = path_tok.trim_start_matches(|c: char|
                "│├└┬┴┼─┐┘┌"
                    .contains(c) || c.is_whitespace()
            ).to_string();

            // Reject obviously-not-a-path tokens (percentage, bar fragment, empty)
            if path.is_empty() || path.ends_with('%') {
                diag_path_failed += 1;
                if diag_failed_examples.len() < 5 {
                    diag_failed_examples.push(format!("path? {:?} from {:?}", path, trimmed));
                }
                continue;
            }
            if !path.contains('/') && !path.starts_with('.') && path.len() < 2 {
                diag_path_failed += 1;
                if diag_failed_examples.len() < 5 {
                    diag_failed_examples.push(format!("path-len? {:?} from {:?}", path, trimmed));
                }
                continue;
            }
            // Skip the synthetic "." total row at the bottom of dust output — it's
            // the sum, not a useful target for cd/open/etc.
            if path == "." || path == "./" {
                continue;
            }

            let display = format!("{} ({})", path, size_tok);
            let basename = path.rsplit('/').next().unwrap_or(&path).to_string();
            let mut metadata = HashMap::new();
            metadata.insert("size".to_string(), size_tok.to_string());
            metadata.insert("size_bytes".to_string(), size_to_bytes(size_tok).to_string());

            out.push(Entity {
                kind: EntityKind::DustEntry,
                primary: path.clone(),
                display,
                aliases: vec![basename, path.clone()],
                confidence: 0.85,
                metadata,
            });
        }
        // dust prints in increasing size order; reverse so largest is first (more useful for "the biggest")
        out.reverse();
        crate::clog!("dust", "size_failed={} short_lines={} path_failed={} extracted={}",
            diag_size_failed, diag_short_lines, diag_path_failed, out.len());
        // If we got nothing useful AND there were rejection signals,
        // dump the first few rejected lines so we can see WHY parsing
        // failed. Diagnostic that surfaces dust-format regressions to
        // stderr (where we'll see them in the user's bug report)
        // instead of needing an interactive debug session.
        if out.is_empty() && (diag_size_failed + diag_path_failed) > 0 {
            for ex in &diag_failed_examples {
                crate::clog!("dust", "  reject: {}", ex);
            }
        }
        out
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        &[
            ("open",   VerbTemplate::Shell("$EDITOR {primary}")),
            ("edit",   VerbTemplate::Shell("$EDITOR {primary}")),
            // cd + natural-language synonyms (matches TreeExtractor).
            ("cd",          VerbTemplate::Shell("cd {primary}")),
            ("cd to",       VerbTemplate::Shell("cd {primary}")),
            ("navigate to", VerbTemplate::Shell("cd {primary}")),
            ("navigate",    VerbTemplate::Shell("cd {primary}")),
            ("go to",       VerbTemplate::Shell("cd {primary}")),
            ("go into",     VerbTemplate::Shell("cd {primary}")),
            ("move to",     VerbTemplate::Shell("cd {primary}")),
            ("change to",   VerbTemplate::Shell("cd {primary}")),
            ("enter",       VerbTemplate::Shell("cd {primary}")),
            ("size",   VerbTemplate::Shell("du -sh {primary}")),
            ("show",   VerbTemplate::Shell("ls -lah {primary}")),
            ("delete", VerbTemplate::ShellConfirm("rm -rf {primary}")),
            ("remove", VerbTemplate::ShellConfirm("rm -rf {primary}")),
        ]
    }
}

// ─── TreeExtractor ────────────────────────────────────────────────────────
// Parses output of GNU `tree` and reconstructs full paths from the
// box-drawing indentation. After a user runs `tree -L 2`, voice "open
// colorchooser.cpp" should resolve to the FULL path the file lives at,
// not just the basename — because `hx colorchooser.cpp` from the project
// root won't find a file in src/components/colorchooser.cpp.

pub struct TreeExtractor;

impl Extractor for TreeExtractor {
    fn name(&self) -> &'static str { "tree" }

    fn matches_command(&self, cmd: &str) -> bool {
        let first = cmd.trim().split_whitespace().next().unwrap_or("");
        first == "tree"
    }

    fn extract(&self, _command: &str, output: &str) -> Vec<Entity> {
        // tree's default output looks like:
        //   .
        //   ├── README.md
        //   ├── src
        //   │   ├── main.rs
        //   │   └── lib.rs
        //   └── tests
        //       └── test.rs
        //   N directories, M files
        //
        // The depth is encoded by repeated `│   ` (4 chars) indentation
        // before a `├── ` or `└── ` marker. We walk lines, derive depth from
        // leading characters, and maintain a stack of path components.
        let mut out = Vec::new();
        // Path stack: stack[i] is the directory name at depth i.
        let mut stack: Vec<String> = Vec::new();
        // The first line (top of tree) might be a path like "." or "/some/dir"
        let mut root: Option<String> = None;

        for line in output.lines() {
            let line = strip_ansi(line);
            // Bottom summary like "12 directories, 45 files" — stop here.
            if line.trim_start().starts_with(|c: char| c.is_ascii_digit())
                && (line.contains("directories") || line.contains("directory")
                    || line.contains("files") || line.contains("file"))
                && !line.contains('/') && !line.contains('.')
            {
                if root.is_some() { break; }
            }

            // Find the marker position. tree uses `├── ` and `└── ` (with
            // a trailing space) before the actual name. If neither marker
            // is present this is either the root line or a blank/garbage
            // line.
            let marker_pos = line.find("├── ").or_else(|| line.find("└── "));

            if let Some(pos) = marker_pos {
                // Depth = number of "│   " or "    " 4-char indent units
                // BEFORE the marker. Count chars (not bytes) so multi-byte
                // box-drawing glyphs work.
                let prefix = &line[..pos];
                // Each indent unit is 4 chars wide. Count by iterating chars.
                let prefix_chars = prefix.chars().count();
                let depth = prefix_chars / 4;
                // Name is everything after the 4-char marker. Strip trailing
                // tree annotations like " -> target" (symlinks) and " [error opening dir]"
                // by taking only the name part.
                let after = &line[pos + "├── ".len()..];
                let name = after.split(" -> ").next().unwrap_or(after).trim();
                if name.is_empty() { continue; }

                // Truncate stack to current depth (we've gone back up the tree)
                stack.truncate(depth);

                let path = if let Some(r) = &root {
                    let mut p = String::new();
                    if r != "." && !r.is_empty() {
                        p.push_str(r);
                        if !p.ends_with('/') { p.push('/'); }
                    }
                    for s in &stack { p.push_str(s); p.push('/'); }
                    p.push_str(name);
                    p
                } else {
                    let mut p = String::new();
                    for s in &stack { p.push_str(s); p.push('/'); }
                    p.push_str(name);
                    p
                };

                // Heuristic for "is this a directory": tree itself doesn't
                // tell us; we infer from the next-line's indentation. For
                // simplicity, push current name onto stack — if the next
                // line is at a deeper depth, it'll stay; otherwise it gets
                // truncated. This means leaf files momentarily live on the
                // stack but get popped on the next iteration. That's fine
                // for output construction.
                stack.push(name.to_string());

                let basename = name.to_string();
                let mut metadata = HashMap::new();
                metadata.insert("depth".to_string(), depth.to_string());

                out.push(Entity {
                    kind: EntityKind::TreeFile,
                    primary: path.clone(),
                    display: path.clone(),
                    aliases: vec![basename, path.clone()],
                    confidence: 0.85,
                    metadata,
                });
            } else {
                // No marker — could be the root line. tree prints the root
                // as the very first line of its output, e.g. ".", "..",
                // "/some/abs/path", or "relative/path". When the user pipes
                // tree through a filter (e.g. `tree -L 2 | grep color`), the
                // root line gets stripped and we instead see filtered marker
                // lines starting from arbitrary depth. We must NOT accept
                // garbage as the root — specifically:
                //   - Command-echo bytes that leaked through capture race
                //     (lines containing "|", spaces, command flags like "-L")
                //   - Prompt or shell-decoration output
                //   - Anything that doesn't look like a filesystem path
                //
                // Heuristics for "real tree root line":
                //   - Exactly "." or ".."
                //   - Starts with "/" (absolute path)
                //   - A single token (no spaces) that looks like a path or
                //     directory name (alphanumeric + path chars only)
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }
                if root.is_none() && looks_like_tree_root(trimmed) {
                    root = Some(trimmed.to_string());
                }
                // If the line doesn't look like a root AND contains no marker,
                // it's just noise — skip it without setting root. This prevents
                // command echoes (which contain spaces, pipes, flags) from
                // being treated as a path prefix.
            }
        }

        crate::clog!("tree", "extracted {} entries", out.len());
        out
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        &[
            ("open",   VerbTemplate::Shell("$EDITOR {primary}")),
            ("edit",   VerbTemplate::Shell("$EDITOR {primary}")),
            ("cat",    VerbTemplate::Shell("cat {primary}")),
            ("less",   VerbTemplate::Shell("less {primary}")),
            ("show",   VerbTemplate::Shell("cat {primary}")),
            // cd + natural-language synonyms. Voice users say
            // "navigate to X" / "go to X" / "move to X" far more
            // often than "cd X" — without these aliases the parser
            // failed to recognize the intent and surfaced no
            // candidates. User reported in v0.1.83 testing.
            //
            // "go" alone doesn't match — too overloaded ("go fetch"
            // / "go ahead"). The two-word variants are unambiguous.
            ("cd",          VerbTemplate::Shell("cd {primary}")),
            ("cd to",       VerbTemplate::Shell("cd {primary}")),
            ("navigate to", VerbTemplate::Shell("cd {primary}")),
            ("navigate",    VerbTemplate::Shell("cd {primary}")),
            ("go to",       VerbTemplate::Shell("cd {primary}")),
            ("go into",     VerbTemplate::Shell("cd {primary}")),
            ("move to",     VerbTemplate::Shell("cd {primary}")),
            ("change to",   VerbTemplate::Shell("cd {primary}")),
            ("enter",       VerbTemplate::Shell("cd {primary}")),
            ("delete", VerbTemplate::ShellConfirm("rm -i {primary}")),
            ("remove", VerbTemplate::ShellConfirm("rm -i {primary}")),
        ]
    }
}

fn looks_like_size(s: &str) -> bool {
    // Accept all of these formats (case-insensitive on the unit):
    //   "1.2G", "800M", "234K", "0B", "12T"        — single-letter suffix
    //   "1.2GB", "800MB", "234KB", "12TB"          — two-letter suffix
    //   "1.2g", "800m", "234k"                     — lowercase variant
    //   "1234"                                      — plain bytes (rare but
    //                                                 some dust versions)
    if s.is_empty() { return false; }
    // Strip a trailing 'B' first (so "1.2GB" → "1.2G", "234KB" → "234K").
    let s = s.strip_suffix(|c: char| c == 'B' || c == 'b').unwrap_or(s);
    if s.is_empty() { return false; }
    let bytes = s.as_bytes();
    let last = bytes[bytes.len() - 1];
    // Recognized unit chars (upper or lowercase). 'P'=peta added for
    // very large filesystems some dust versions report.
    let unit_match = matches!(last,
        b'K' | b'k' | b'M' | b'm' | b'G' | b'g'
        | b'T' | b't' | b'P' | b'p');
    let num_part: &str = if unit_match {
        &s[..s.len() - 1]
    } else {
        // No unit suffix — could be plain digit count.
        s
    };
    if num_part.is_empty() { return false; }
    num_part.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn size_to_bytes(s: &str) -> u64 {
    if s.is_empty() { return 0; }
    // Strip trailing 'B' / 'b' first (so "1.2GB" matches "1.2G")
    let s = s.strip_suffix(|c: char| c == 'B' || c == 'b').unwrap_or(s);
    if s.is_empty() { return 0; }
    let bytes = s.as_bytes();
    let last = bytes[bytes.len() - 1];
    let unit_match = matches!(last,
        b'K' | b'k' | b'M' | b'm' | b'G' | b'g'
        | b'T' | b't' | b'P' | b'p');
    let num_str: &str = if unit_match { &s[..s.len() - 1] } else { s };
    let num: f64 = num_str.parse().unwrap_or(0.0);
    let mult: u64 = if !unit_match {
        1
    } else {
        match last.to_ascii_uppercase() {
            b'K' => 1_024,
            b'M' => 1_024 * 1_024,
            b'G' => 1_024 * 1_024 * 1_024,
            b'T' => 1_024_u64.pow(4),
            b'P' => 1_024_u64.pow(5),
            _ => 1,
        }
    };
    (num * mult as f64) as u64
}

// ─── Extractor: git (status, log, branch) ─────────────────────────────────

pub struct GitExtractor;

impl GitExtractor {
    fn subcommand(cmd: &str) -> Option<&'static str> {
        let trimmed = cmd.trim();
        let after = trimmed.strip_prefix("git ")?;
        let first = after.split_whitespace().next()?;
        match first {
            "log" => Some("log"),
            "branch" => Some("branch"),
            "status" => Some("status"),
            _ => None,
        }
    }
}

impl Extractor for GitExtractor {
    fn name(&self) -> &'static str { "git" }

    fn matches_command(&self, cmd: &str) -> bool {
        Self::subcommand(cmd).is_some()
    }

    fn extract(&self, command: &str, output: &str) -> Vec<Entity> {
        let sub = match Self::subcommand(command) { Some(s) => s, None => return Vec::new() };
        match sub {
            "log" => extract_git_log(output),
            "branch" => extract_git_branch(output),
            "status" => extract_git_status(output),
            _ => Vec::new(),
        }
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        // Combined dictionary; resolver picks based on entity kind.
        //
        // We use `git --no-pager` for read-only display verbs (show, diff) so
        // that chained voice commands work without the user having to press `q`
        // between each one. Without this, `git show <hash>` invokes less by
        // default and the next composed command is buffered as input to less.
        // Side effect: lose less's scrollback for that command — but voice flow
        // is interactive enough that the scrollback in our terminal scrollbar
        // covers most needs, and the user can always type `git show <hash>`
        // by hand if they want pager behavior.
        &[
            // Commit verbs
            ("show",      VerbTemplate::Shell("git --no-pager show {primary}")),
            ("checkout",  VerbTemplate::Shell("git checkout {primary}")),
            ("revert",    VerbTemplate::ShellConfirm("git revert {primary}")),
            ("diff",      VerbTemplate::Shell("git --no-pager diff {primary}")),
            ("cherrypick",VerbTemplate::Shell("git cherry-pick {primary}")),
            ("copy",      VerbTemplate::Shell("printf '%s' {primary} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            // Branch verbs
            ("switch",    VerbTemplate::Shell("git checkout {primary}")),
            ("rename",    VerbTemplate::Shell("git branch -m {primary}")),
            ("delete",    VerbTemplate::ShellConfirm("git branch -d {primary}")),
            // Status file verbs
            ("add",       VerbTemplate::Shell("git add {primary}")),
            ("stage",     VerbTemplate::Shell("git add {primary}")),
            ("unstage",   VerbTemplate::Shell("git reset {primary}")),
            ("reset",     VerbTemplate::ShellConfirm("git checkout -- {primary}")),
            ("open",      VerbTemplate::Shell("$EDITOR {primary}")),
            ("edit",      VerbTemplate::Shell("$EDITOR {primary}")),
        ]
    }
}

fn extract_git_log(output: &str) -> Vec<Entity> {
    // Two formats handled:
    //   --oneline: "abc1234 commit message"
    //   default: "commit abc12345...\nAuthor: ...\nDate: ...\n\n    message\n"
    let mut out = Vec::new();
    let mut in_default = false;
    let mut current_hash = String::new();
    let mut current_msg = String::new();

    for line in output.lines() {
        let line = strip_ansi(line);
        let trimmed = line.trim();

        // --oneline form: starts with 7+ hex chars then space
        if let Some((hash, msg)) = split_oneline_commit(trimmed) {
            in_default = false;
            push_commit(&mut out, hash, msg);
            continue;
        }

        // default form
        if let Some(rest) = trimmed.strip_prefix("commit ") {
            // flush previous
            if !current_hash.is_empty() {
                push_commit(&mut out, &current_hash, current_msg.trim());
                current_msg.clear();
            }
            // hash may be followed by " (tag: ...)" — take just the hash
            current_hash = rest.split_whitespace().next().unwrap_or("").to_string();
            in_default = true;
        } else if in_default && line.starts_with("    ") && current_msg.is_empty() {
            // First indented body line is the subject
            current_msg = trimmed.to_string();
        }
    }
    if !current_hash.is_empty() {
        push_commit(&mut out, &current_hash, current_msg.trim());
    }
    out
}

fn split_oneline_commit(line: &str) -> Option<(&str, &str)> {
    let mut sp = line.splitn(2, ' ');
    let hash = sp.next()?;
    if hash.len() < 7 || hash.len() > 40 { return None; }
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
    let msg = sp.next().unwrap_or("");
    Some((hash, msg))
}

fn push_commit(out: &mut Vec<Entity>, hash: &str, msg: &str) {
    if hash.is_empty() { return; }
    let short_hash = &hash[..hash.len().min(7)];
    let display = if msg.is_empty() {
        short_hash.to_string()
    } else {
        format!("{} {}", short_hash, msg)
    };
    let aliases: Vec<String> = msg.split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|s| s.to_string()).collect();
    let mut metadata = HashMap::new();
    metadata.insert("short_hash".to_string(), short_hash.to_string());
    metadata.insert("message".to_string(), msg.to_string());
    out.push(Entity {
        kind: EntityKind::GitCommit,
        primary: hash.to_string(),
        display,
        aliases,
        confidence: 0.9,
        metadata,
    });
}

fn extract_git_branch(output: &str) -> Vec<Entity> {
    // Lines of the form:
    //   * main
    //     feature/foo
    //     remotes/origin/HEAD -> origin/main
    let mut out = Vec::new();
    for line in output.lines() {
        let line = strip_ansi(line);
        let mut s = line.as_str();
        let mut current = false;
        if let Some(rest) = s.strip_prefix("* ") { s = rest; current = true; }
        else if let Some(rest) = s.strip_prefix("  ") { s = rest; }
        else { continue; }

        let name = s.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() { continue; }
        // Skip detached-HEAD pseudo-entries like "(HEAD detached at abc)"
        if name.starts_with('(') { continue; }
        // For "remotes/origin/HEAD -> origin/main" we just take the first token (less useful but valid)

        let display = if current { format!("{} (current)", name) } else { name.clone() };
        let mut aliases: Vec<String> = name.split('/').map(|s| s.to_string()).collect();
        aliases.push(name.clone());
        let mut metadata = HashMap::new();
        if current { metadata.insert("current".to_string(), "true".to_string()); }
        out.push(Entity {
            kind: EntityKind::GitBranch,
            primary: name.clone(),
            display,
            aliases,
            confidence: 0.95,
            metadata,
        });
    }
    out
}

fn extract_git_status(output: &str) -> Vec<Entity> {
    // Two forms:
    //   short:  " M file.rs", "?? newfile", "MM both.rs"
    //   long:   "Changes not staged for commit:\n  modified:   file.rs"
    //
    // We try short-form patterns first (works for `git status -s`/`--porcelain`).
    let mut out = Vec::new();

    for line in output.lines() {
        let line = strip_ansi(line);
        if line.is_empty() { continue; }

        // Short form: first 2 chars are status, then space, then path
        if line.len() >= 4 {
            let bytes = line.as_bytes();
            let s1 = bytes[0] as char;
            let s2 = bytes[1] as char;
            let space = bytes[2] as char;
            if (s1 == ' ' || "MARCDU?!".contains(s1))
                && (s2 == ' ' || "MARCDU?!".contains(s2))
                && space == ' '
                && (s1 != ' ' || s2 != ' ')
            {
                let path = line[3..].trim().to_string();
                if !path.is_empty() {
                    push_status_file(&mut out, &path, &format!("{}{}", s1, s2));
                    continue;
                }
            }
        }

        // Long form: "  modified:   path", "  new file:   path", "  deleted:   path"
        let trimmed = line.trim();
        for (prefix, code) in &[
            ("modified:", "M"),
            ("new file:", "A"),
            ("deleted:", "D"),
            ("renamed:", "R"),
            ("typechange:", "T"),
        ] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let path = rest.trim().to_string();
                if !path.is_empty() {
                    push_status_file(&mut out, &path, code);
                    break;
                }
            }
        }
        // Untracked section: lines under "Untracked files:" are bare paths
        // We don't track section state here in Phase 0; relying on short-form is sufficient
        // for users who run `git status -s` or `git status --porcelain`.
    }

    out
}

fn push_status_file(out: &mut Vec<Entity>, path: &str, status: &str) {
    let basename = path.rsplit('/').next().unwrap_or(path).to_string();
    let display = format!("{}  {}", status, path);
    let mut metadata = HashMap::new();
    metadata.insert("status".to_string(), status.to_string());
    metadata.insert("basename".to_string(), basename.clone());
    out.push(Entity {
        kind: EntityKind::GitStatusFile,
        primary: path.to_string(),
        display,
        aliases: vec![basename, path.to_string()],
        confidence: 0.9,
        metadata,
    });
}

// ─── Extractor: ripgrep (rg) ──────────────────────────────────────────────

pub struct RipgrepExtractor;

impl Extractor for RipgrepExtractor {
    fn name(&self) -> &'static str { "rg" }

    fn matches_command(&self, cmd: &str) -> bool {
        let first = cmd.trim().split_whitespace().next().unwrap_or("");
        first == "rg" || first == "ripgrep"
    }

    fn extract(&self, _command: &str, output: &str) -> Vec<Entity> {
        // Default rg output (without --no-heading):
        //   path/to/file.rs
        //   42:    let foo = bar;
        //   58:    other match;
        //
        //   path/to/other.rs
        //   12:match
        //
        // With --no-heading / -n forces "path:line:content" per match.
        // We handle both, but the heading form is most common.
        let mut out = Vec::new();
        let mut current_file: Option<String> = None;

        for line in output.lines() {
            let line = strip_ansi(line);
            if line.is_empty() {
                current_file = None;
                continue;
            }

            // path:line:content form (rg --no-heading or grep-like)
            if let Some((path, line_no, content)) = parse_rg_inline(&line) {
                push_rg_match(&mut out, &path, line_no, &content);
                continue;
            }

            // line_no:content form (heading mode, file already known)
            if let Some(file) = &current_file {
                if let Some((line_no, content)) = parse_lineno_content(&line) {
                    push_rg_match(&mut out, file, line_no, &content);
                    continue;
                }
            }

            // Otherwise treat as a heading (file path)
            if !line.contains(':') || line.starts_with("./") || line.starts_with('/') || !line.contains(' ') {
                // Heuristic: if line looks like a file path (contains '/' or ends in extension), treat as heading
                if looks_like_path(&line) {
                    current_file = Some(line.to_string());
                    continue;
                }
            }
        }
        out
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        &[
            ("open",        VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("edit",        VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("find",        VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("search",      VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("locate",      VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("go to",       VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("goto",        VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("jump to",     VerbTemplate::Shell("$EDITOR +{meta:line} {meta:path}")),
            ("show",        VerbTemplate::Shell("awk 'NR>={meta:start} && NR<={meta:end}' {meta:path}")),
            // Copy/yank — by default copies the matched LINE CONTENT (most useful).
            // Older "copy path:line" form is rarely what voice users want.
            ("yank",        VerbTemplate::Shell("sed -n '{meta:line}p' {meta:path} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            ("yank line",   VerbTemplate::Shell("sed -n '{meta:line}p' {meta:path} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            ("copy",        VerbTemplate::Shell("sed -n '{meta:line}p' {meta:path} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            ("copy line",   VerbTemplate::Shell("sed -n '{meta:line}p' {meta:path} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            // Path:line form is still useful when you want to share a code reference
            ("copy path",   VerbTemplate::Shell("printf '%s:%s' {meta:path} {meta:line} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
            ("copy reference", VerbTemplate::Shell("printf '%s:%s' {meta:path} {meta:line} | (wl-copy 2>/dev/null || xclip -selection clipboard 2>/dev/null || pbcopy)")),
        ]
    }
}

fn parse_rg_inline(line: &str) -> Option<(String, u32, String)> {
    // path:line:content where line is digits
    // path can contain ':' (e.g. on Windows or with colons in names) — be greedy in path,
    // first ':' after a path-shaped prefix that's followed by digits then ':'.
    let mut idx = 0;
    let bytes = line.as_bytes();
    // Find "<digits>:" pattern after a colon
    let mut last_colon = None;
    while idx < bytes.len() {
        if bytes[idx] == b':' {
            // Check if the chars after this colon up to the NEXT colon are all digits
            let mut j = idx + 1;
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() { j += 1; }
            if j > start && j < bytes.len() && bytes[j] == b':' {
                last_colon = Some((idx, start, j));
                break;
            }
        }
        idx += 1;
    }
    let (path_end, num_start, num_end) = last_colon?;
    if path_end == 0 { return None; }
    let path = line[..path_end].to_string();
    let line_no: u32 = line[num_start..num_end].parse().ok()?;
    let content = line[num_end + 1..].to_string();
    if !looks_like_path(&path) { return None; }
    Some((path, line_no, content))
}

fn parse_lineno_content(line: &str) -> Option<(u32, String)> {
    let mut sp = line.splitn(2, ':');
    let n_str = sp.next()?;
    if !n_str.chars().all(|c| c.is_ascii_digit()) || n_str.is_empty() { return None; }
    let n: u32 = n_str.parse().ok()?;
    let content = sp.next().unwrap_or("").to_string();
    Some((n, content))
}

fn looks_like_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 500 { return false; }
    // Heuristic: contains '/' OR ends with a known extension OR starts with '.'
    if s.contains('/') { return true; }
    let common_exts = [".rs", ".py", ".js", ".ts", ".jsx", ".tsx", ".go", ".c", ".h",
        ".cpp", ".cc", ".java", ".rb", ".sh", ".md", ".txt", ".toml", ".yaml", ".yml",
        ".json", ".html", ".css", ".scss", ".vue", ".svelte", ".zig", ".swift", ".kt",
        ".lua", ".pl", ".php", ".sql", ".xml", ".ini", ".conf", ".cfg", ".lock"];
    if common_exts.iter().any(|e| s.ends_with(e)) { return true; }
    false
}

fn push_rg_match(out: &mut Vec<Entity>, path: &str, line_no: u32, content: &str) {
    let basename = path.rsplit('/').next().unwrap_or(path).to_string();
    // Char-boundary-safe truncation: take first 60 chars, not bytes, so we
    // don't slice in the middle of a UTF-8 codepoint.
    let preview: String = content.chars().take(60).collect();
    let display = if content.chars().count() > 60 {
        format!("{}:{}: {}…", path, line_no, preview)
    } else {
        format!("{}:{}: {}", path, line_no, content)
    };
    let mut metadata = HashMap::new();
    metadata.insert("path".to_string(), path.to_string());
    metadata.insert("line".to_string(), line_no.to_string());
    metadata.insert("content".to_string(), content.to_string());
    metadata.insert("basename".to_string(), basename.clone());
    metadata.insert("start".to_string(), line_no.saturating_sub(3).max(1).to_string());
    metadata.insert("end".to_string(), (line_no + 5).to_string());
    out.push(Entity {
        kind: EntityKind::RgMatch,
        primary: format!("{}:{}", path, line_no),
        display,
        aliases: vec![basename, content.trim().to_string()],
        confidence: 0.85,
        metadata,
    });
}

// ─── ANSI strip helper ────────────────────────────────────────────────────

/// Iterative two-row Levenshtein distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (m, n) = (av.len(), bv.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if av[i-1] == bv[j-1] { 0 } else { 1 };
            curr[j] = (curr[j-1] + 1).min(prev[j] + 1).min(prev[j-1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Returns true if the given trimmed line looks like a plausible tree
/// root line. Tree prints the root as either ".", "..", an absolute path
/// "/foo/bar", or a single relative-path token like "src" or "myproject".
/// It's never a multi-word phrase, never contains pipes or shell metachars.
///
/// We use this to filter out command-echo leakage in the captured output
/// (race between Enter and the drain timer): when the user types
/// `tree -L 2 | grep color` and then runs it, that command string can
/// leak into the captured output before our block tracker starts capturing
/// "for real". Without this filter, that command-echo line gets used as
/// the path root and prepended to every extracted entity.
fn looks_like_tree_root(s: &str) -> bool {
    if s.is_empty() { return false; }
    // Reject anything with spaces, pipes, or other shell metachars
    for c in s.chars() {
        if c == ' ' || c == '\t' || c == '|' || c == '&' || c == ';'
            || c == '<' || c == '>' || c == '$' || c == '`' || c == '*'
            || c == '?' || c == '"' || c == '\'' {
            return false;
        }
    }
    // Reject if it looks like a flag (starts with -)
    if s.starts_with('-') { return false; }
    // Accept the special tree roots
    if s == "." || s == ".." { return true; }
    // Accept absolute paths
    if s.starts_with('/') || s.starts_with("~/") { return true; }
    // Accept multi-component relative paths (contain a slash)
    if s.contains('/') {
        return s.chars().all(|c|
            c.is_ascii_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-');
    }
    // Bare single-component names are MOSTLY command-echo artifacts:
    //   - The user runs "tree" and the echoed "tree" line is the first
    //     thing in captured output BEFORE the actual "." root
    //   - Race conditions during PTY capture leave a single character
    //     fragment ("t" leaked from "tree", "d" from "dust")
    //
    // Real bare-name tree roots DO exist (running "tree subdir/" where
    // subdir is a single token), but the common-case bug from
    // misidentifying these is way more harmful than the rare-case
    // benefit of accepting them. Reject all bare single-component
    // tree-root candidates. Tree's own canonical "." / ".." / abs
    // paths still work via the cases above.
    //
    // Bug repro before this change: user ran `tree` in kwin repo;
    // tree extractor saw "tree" as the root, then real tree marker
    // lines built paths like "ttree/autotests/integration/..." (the
    // actual path concatenated onto the command-echo "tree" root).
    false
}

fn strip_ansi(s: &str) -> String {
    // Quick-and-dirty: drop ESC [ ... letter sequences
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Eat until letter or end
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii_alphabetic() { break; }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ─── Resolver ─────────────────────────────────────────────────────────────

/// Resolve a user reference (e.g. "headphones", "the second one") against
/// a list of entities. Returns up to top_n matches, sorted by score.
///
/// Strategy:
///   - Ordinal phrases ("first", "second", "third", "last", "top", "bottom")
///     match by position with high confidence.
///   - Substring of any word in the reference appearing in display/aliases
///     gives a substring score.
///   - Token-level fuzzy match on display + aliases.
///   - "the X one" / "that X" prefixes are stripped before scoring.
pub fn resolve_reference(reference: &str, entities: &[Entity], top_n: usize)
    -> Vec<ResolvedMatch>
{
    if entities.is_empty() { return Vec::new(); }
    let raw = reference.trim()
        .trim_end_matches(|c: char| c == '.' || c == '!' || c == '?' || c == ',')
        .to_lowercase();
    let cleaned = strip_pronouns_and_articles(&raw);

    // Ordinal first
    if let Some(idx) = match_ordinal(&cleaned, entities.len()) {
        let entity = entities[idx].clone();
        return vec![ResolvedMatch {
            entity,
            score: 0.9,
            reason: format!("ordinal match (position {})", idx + 1),
        }];
    }

    // Substring + fuzzy scoring
    let ref_tokens: Vec<&str> = cleaned.split_whitespace()
        .filter(|t| t.len() >= 2 && !STOP_WORDS.contains(t))
        .collect();

    let mut scored: Vec<(usize, f32, String)> = Vec::new();
    // Compute a separator-collapsed version of the full reference.
    // "color chooser .cpp" → "colorchoosercpp"
    // "color_chooser dot cpp" → still mostly survives
    // This lets us catch filename matches where whisper split or ate
    // separators that the actual filename has — a common pain point for
    // coder filenames like colorchooser.cpp / color_chooser.cpp.
    let ref_collapsed: String = cleaned.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();

    for (i, e) in entities.iter().enumerate() {
        let corpus = e.search_corpus();
        let mut score = 0.0f32;
        let mut reasons: Vec<String> = Vec::new();

        // Filename-style match: collapsed reference appears in corpus (which
        // also has collapsed forms appended for filename-like aliases).
        // Strong signal — voicing "color chooser cpp" should match
        // "colorchooser.cpp" with high confidence.
        if ref_collapsed.len() >= 5 && corpus.contains(&ref_collapsed) {
            score += 0.5;
            reasons.push(format!("filename-shape match"));
        }

        // Each reference token
        for tok in &ref_tokens {
            let tok = tok.to_lowercase();
            // v0.1.581: EXACT word match — the token equals a whole word
            // in the corpus (split on non-alphanumerics). This must
            // outrank a mere substring containment: voicing "test"
            // should rank test.rs ABOVE latest.log / contest.txt, which
            // only CONTAIN "test". Previously both got the same +0.4
            // substring score, so the real file didn't float to the top.
            let exact_word = corpus
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w == tok);
            if exact_word {
                score += 0.6;
                reasons.push(format!("exact '{}'", tok));
                continue;
            }
            // Substring match (full token appears anywhere in corpus)
            if corpus.contains(&tok) {
                score += 0.4;
                reasons.push(format!("matched '{}'", tok));
                continue;
            }
            // Prefix match: any corpus word contains the token as a prefix
            let prefix_hit = corpus.split(|c: char| !c.is_alphanumeric())
                .any(|w| w.len() >= 3 && w.starts_with(&tok));
            if prefix_hit {
                score += 0.2;
                reasons.push(format!("prefix '{}'", tok));
                continue;
            }
            // Edit-distance fuzzy match: catch whisper mistranscriptions like
            // "smoky" → "Smokey" (one substitution). Only fires for tokens of
            // length 4+ to avoid spurious short-word matches, and only against
            // corpus words within 2 edits.
            if tok.len() >= 4 {
                let best = corpus.split(|c: char| !c.is_alphanumeric())
                    .filter(|w| w.len() >= tok.len().saturating_sub(2)
                              && w.len() <= tok.len() + 2)
                    .map(|w| (w, levenshtein(&tok, w)))
                    .min_by_key(|(_, d)| *d);
                if let Some((w, d)) = best {
                    let max_allowed = (tok.len() / 4).max(1).min(2);
                    if d <= max_allowed {
                        score += 0.3 - (d as f32 * 0.05);
                        reasons.push(format!("close to '{}' (~{})", w, d));
                    }
                }
            }
        }

        // No ref tokens at all? fall back to fuzzy of full cleaned reference
        if ref_tokens.is_empty() && !cleaned.is_empty() {
            if corpus.contains(&cleaned) {
                score += 0.3;
                reasons.push(format!("matched full reference"));
            }
        }

        if score > 0.0 {
            scored.push((i, score.min(1.0), reasons.join(", ")));
        }
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_n);

    scored.into_iter().map(|(idx, score, reason)| ResolvedMatch {
        entity: entities[idx].clone(),
        score,
        reason,
    }).collect()
}

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "to", "of", "for", "on", "in", "with", "by", "as",
    "and", "or", "but", "is", "are", "was", "were", "be", "been", "being",
    "this", "that", "these", "those", "one", "ones", "it", "its",
];

fn strip_pronouns_and_articles(s: &str) -> String {
    let s = s.trim();
    // "the X one" / "that X one" / "this X" → "X"
    let prefixes = ["the ", "that ", "this ", "those ", "these "];
    let mut t = s.to_string();
    for p in &prefixes {
        if let Some(rest) = t.strip_prefix(p) {
            t = rest.to_string();
            break;
        }
    }
    let suffixes = [" one", " ones", " thing", " things"];
    for sfx in &suffixes {
        if let Some(rest) = t.strip_suffix(sfx) {
            t = rest.to_string();
            break;
        }
    }
    t.trim().to_string()
}

fn match_ordinal(text: &str, n: usize) -> Option<usize> {
    if n == 0 { return None; }
    let phrases: &[(&str, isize)] = &[
        ("first", 0), ("1st", 0), ("number one", 0), ("top", 0),
        ("second", 1), ("2nd", 1), ("number two", 1),
        ("third", 2), ("3rd", 2), ("number three", 2),
        ("fourth", 3), ("4th", 3),
        ("fifth", 4), ("5th", 4),
        ("sixth", 5), ("seventh", 6), ("eighth", 7), ("ninth", 8), ("tenth", 9),
        ("last", -1), ("latest", -1), ("most recent", -1), ("newest", -1), ("bottom", -1),
        ("oldest", -2), // last in list (assumes log-style ordering)
        ("biggest", -3), ("largest", -3), ("smallest", -4), // size-based ordinals (handled below)
    ];
    for (p, off) in phrases {
        if text == *p || text.contains(p) {
            // Word-boundary check
            let starts = text.starts_with(p) ||
                text.contains(&format!(" {}", p)) ||
                text.contains(&format!("{} ", p));
            if !starts { continue; }
            return match *off {
                -1 => Some(0), // most recent in our extractors = first (we reverse dust, git log is newest first)
                -2 => Some(n.saturating_sub(1)),
                -3 => Some(0), // dust output is reversed; biggest is first
                -4 => Some(n.saturating_sub(1)),
                v if v >= 0 => {
                    let i = v as usize;
                    if i < n { Some(i) } else { None }
                }
                _ => None,
            };
        }
    }
    None
}

// ─── Verb parsing ─────────────────────────────────────────────────────────

/// Try to find a verb in the user's reference text that matches the extractor's
/// dictionary. Returns (verb_template, verb_word, remainder).
pub fn parse_verb(text: &str, extractor: &dyn Extractor) -> Option<(VerbTemplate, &'static str, String)> {
    let text_lc = text.trim()
        .trim_end_matches(|c: char| c == '.' || c == '!' || c == '?' || c == ',')
        .to_lowercase();
    // Whisper sometimes inserts spaces into single words ("cd" → "c d"). Make a
    // de-spaced copy for matching.
    let text_despaced: String = text_lc.split_whitespace().collect::<Vec<_>>().join(" ");
    let text_compact: String = text_lc.chars().filter(|c| !c.is_whitespace()).collect();

    let dict = extractor.verb_dictionary();
    // Sort verbs by length descending so multi-word verbs match before subwords
    let mut verbs: Vec<(&'static str, VerbTemplate)> = dict.iter().copied().collect();
    verbs.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));

    for (verb, tmpl) in &verbs {
        // Direct prefix match
        if let Some(rest) = text_despaced.strip_prefix(verb) {
            if rest.is_empty() || rest.starts_with(' ') {
                return Some((*tmpl, verb, rest.trim().to_string()));
            }
        }
        // Whisper-spaced match: e.g. "c d" should match verb "cd". Compare against
        // the compact form (whitespace removed).
        if verb.len() >= 2 && verb.len() <= 4 && !verb.contains(' ') {
            if let Some(rest) = text_compact.strip_prefix(verb) {
                // Find where the verb ends in the original text. We approximate by
                // counting chars from the start, skipping whitespace.
                let mut consumed = 0usize;
                let mut chars_left = verb.len();
                for (i, c) in text_despaced.char_indices() {
                    if chars_left == 0 { consumed = i; break; }
                    if !c.is_whitespace() { chars_left -= 1; }
                    consumed = i + c.len_utf8();
                }
                let remainder: String = text_despaced[consumed..].trim().to_string();
                let _ = rest; // not used; using remainder from original text
                return Some((*tmpl, verb, remainder));
            }
        }
    }
    None
}

// ─── Compose ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Composition {
    /// Final shell command to execute
    pub command: String,
    /// Verb that drove the composition
    pub verb: &'static str,
    /// Entity that was matched
    pub entity: Entity,
    /// Whether this is destructive (always require confirmation)
    pub destructive: bool,
    /// Score of the underlying entity match (0..1)
    pub score: f32,
    /// Reason text for UI
    pub reason: String,
}

/// Top-level: given a user reference + a list of recent extracted blocks,
/// return ranked compositions. Iterates blocks newest-first; uses each block's
/// extractor to (a) check verb match, (b) resolve reference, (c) compose command.
pub fn resolve_blocks(reference: &str, blocks: &[ExtractedBlock]) -> Vec<Composition> {
    let mut compositions: Vec<Composition> = Vec::new();

    for (block_idx, block) in blocks.iter().enumerate() {
        let extractor = match extractor_by_name(&block.extractor_name) {
            Some(e) => e,
            None => continue,
        };

        // Verb must come from this extractor's dictionary
        let (tmpl, verb, remainder) = match parse_verb(reference, extractor) {
            Some(t) => t,
            None => continue,
        };
        if remainder.is_empty() { continue; }

        // Resolve the remainder against this block's entities
        let matches = resolve_reference(&remainder, &block.entities, 3);
        for m in matches {
            // Decay score by block age (most recent block = no decay; older = penalty)
            let block_decay = 1.0 - (block_idx as f32 * 0.1).min(0.5);
            let final_score = (m.score * block_decay).min(1.0);
            let cmd = tmpl.render(&m.entity);
            compositions.push(Composition {
                command: cmd,
                verb,
                entity: m.entity.clone(),
                destructive: tmpl.is_destructive(),
                score: final_score,
                reason: format!("{} → {} ({})",
                    extractor.name(), m.reason, m.entity.display),
            });
        }
    }

    compositions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Folder boost: when the user's reference is a pure navigation
    // intent ("navigate to", "go to", "cd to" etc) AND the top
    // candidates are all cd commands (no edit/cat/delete/etc), boost
    // the relative score so they punch through other filters that
    // might otherwise bury them. User reported: "go to folder still
    // needs work and better punching through to deeper folders, can
    // increase scoring when have multiple folders with no other
    // actions?" — this is the answer.
    //
    // The boost is RELATIVE — top cd-verb candidate gets +0.15,
    // capped at 1.0. Lower-ranked cds also get +0.10. Non-cd
    // candidates unaffected. Effect: if your "navigate to thumbnails"
    // matched a tree entry at score 0.45 (dilution from many entries
    // sharing a substring), it lifts to 0.60 — above the 0.5 confidence
    // threshold the UI uses for primary candidate display.
    let cd_count = compositions.iter().filter(|c| c.verb == "cd"
        || c.verb == "cd to" || c.verb == "navigate to" || c.verb == "navigate"
        || c.verb == "go to" || c.verb == "go into" || c.verb == "move to"
        || c.verb == "change to" || c.verb == "enter").count();
    if cd_count >= 2 {
        let mut applied = 0u32;
        for c in compositions.iter_mut() {
            let is_cd = c.verb == "cd" || c.verb == "cd to"
                || c.verb == "navigate to" || c.verb == "navigate"
                || c.verb == "go to" || c.verb == "go into"
                || c.verb == "move to" || c.verb == "change to"
                || c.verb == "enter";
            if !is_cd { continue; }
            let boost = if applied == 0 { 0.15 } else { 0.10 };
            c.score = (c.score + boost).min(1.0);
            applied += 1;
            if applied >= 5 { break; }  // top-5 only
        }
        if applied > 0 {
            // Re-sort after boost
            compositions.sort_by(|a, b|
                b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            crate::clog!("entity", "folder-boost applied to {} cd candidates", applied);
        }
    }
    // Hard blacklist: drop any composed command whose first token is in BLACKLISTED_BINARIES.
    // Phase 0 testing guardrail. Catches both direct "rm ..." and bare "dd ..." regardless of
    // which extractor produced them. Pipes/subshells aren't covered — if the template puts
    // these inside `bash -c` or after `&&`, they'd slip through. Acceptable for test phase.
    compositions.retain(|c| {
        let first = c.command.trim_start().split_whitespace().next().unwrap_or("");
        // Strip leading sudo/doas prefix
        let first = if first == "sudo" || first == "doas" {
            c.command.trim_start().split_whitespace().nth(1).unwrap_or("")
        } else {
            first
        };
        if BLACKLISTED_BINARIES.contains(&first) {
            eprintln!("EntityExtract: blocked composed command '{}' (binary '{}' in blacklist)",
                c.command, first);
            false
        } else {
            true
        }
    });
    compositions.truncate(5);
    compositions
}

/// Hard-blocked first-tokens. Composition is dropped entirely if matched.
/// Test-phase safety: these are tools where a wrong entity match would do
/// real damage (rm: data loss; dd: device wipe).
const BLACKLISTED_BINARIES: &[&str] = &["rm", "dd"];

fn extractor_by_name(name: &str) -> Option<&'static dyn Extractor> {
    match name {
        "bluetoothctl-devices" => Some(&BLUETOOTHCTL),
        "dust" => Some(&DUST),
        "tree" => Some(&TREE),
        "git" => Some(&GIT),
        "rg" => Some(&RG),
        // "top" matches both htop and btop variants — the extract_top_processes
        // function above handles all three with the same TopExtractor verbs.
        "top" => Some(&TOP),
        _ => None,
    }
}

/// Run all extractors against a (command, output) pair. Returns the first
/// matching extractor's results, or None.
pub fn extract_block(command: &str, output: &str) -> Option<ExtractedBlock> {
    for ex in ALL_EXTRACTORS {
        if ex.matches_command(command) {
            let entities = ex.extract(command, output);
            if entities.is_empty() { return None; }
            return Some(ExtractedBlock {
                extractor_name: ex.name().to_string(),
                command: command.to_string(),
                entities,
            });
        }
    }
    None
}

// Statics for the extractors so we can return &'static refs from extractor_by_name
static BLUETOOTHCTL: BluetoothctlExtractor = BluetoothctlExtractor;
static DUST: DustExtractor = DustExtractor;
static TREE: TreeExtractor = TreeExtractor;
static GIT: GitExtractor = GitExtractor;
static RG: RipgrepExtractor = RipgrepExtractor;
static TOP: TopExtractor = TopExtractor;

static ALL_EXTRACTORS: &[&dyn Extractor] = &[
    &BLUETOOTHCTL, &DUST, &TREE, &GIT, &RG, &TOP,
];

// ─── TopExtractor ─────────────────────────────────────────────────────────
//
// Companion to extract_top_processes() below. Unlike normal Extractors
// which run on completed (command, output) blocks, top/htop are
// alt-screen TUIs that never produce a captured block — instead,
// main.rs::voice_parse_candidates calls extract_top_processes()
// directly on the live rendered grid and pushes the result into
// recent_blocks with extractor_name = "top".
//
// This struct exists so resolve_blocks() can find a verb_dictionary
// for those synthetic blocks. Its matches_command/extract methods
// are stubs — extract returns empty (never used), matches_command
// returns false (we don't want auto-extraction on the COMMAND
// `top` since that would need the live render path anyway).
//
// Verb design:
//   "kill"       -> kill {primary}                  (SIGTERM, polite)
//   "force kill" -> kill -9 {primary}               (SIGKILL, destructive)
//   "stop"       -> kill -STOP {primary}            (pause; less common)
//   "info"       -> ps -o pid,ppid,user,cmd -p {primary}
//
// All forms confirmed (ShellConfirm) — even SIGTERM can break
// long-running tasks; "kill the wrong second" is always recoverable
// only by Ctrl-Z'ing fast, and we'd rather make the user say yes
// once than nuke the wrong PID.

pub struct TopExtractor;

impl Extractor for TopExtractor {
    fn name(&self) -> &'static str { "top" }

    fn matches_command(&self, _cmd: &str) -> bool {
        // Top/htop don't produce capturable blocks — they're alt-
        // screen TUIs. Synthesis happens in main.rs by calling
        // extract_top_processes directly on the live grid. So we
        // never want this extractor to auto-fire from the block-
        // tracker path.
        false
    }

    fn extract(&self, _command: &str, _output: &str) -> Vec<Entity> {
        // Stub — see matches_command. Real extraction goes through
        // the standalone extract_top_processes() function below
        // because it needs the foreground command name to choose
        // a parser variant (top vs htop vs btop).
        Vec::new()
    }

    fn verb_dictionary(&self) -> &'static [(&'static str, VerbTemplate)] {
        &[
            // Polite default — most "kill" / "stop" intents the user
            // means SIGTERM. Confirmation gate prevents misfiring on
            // accidental "kill" mentions (e.g. "kill time").
            ("kill",       VerbTemplate::ShellConfirm("kill {primary}")),
            ("stop",       VerbTemplate::ShellConfirm("kill {primary}")),
            ("end",        VerbTemplate::ShellConfirm("kill {primary}")),
            ("terminate",  VerbTemplate::ShellConfirm("kill {primary}")),
            ("quit",       VerbTemplate::ShellConfirm("kill {primary}")),
            // Force-kill variants — explicit SIGKILL.
            ("force kill", VerbTemplate::ShellConfirm("kill -9 {primary}")),
            ("force",      VerbTemplate::ShellConfirm("kill -9 {primary}")),
            ("nuke",       VerbTemplate::ShellConfirm("kill -9 {primary}")),
            // Pause / resume.
            ("pause",      VerbTemplate::ShellConfirm("kill -STOP {primary}")),
            ("freeze",     VerbTemplate::ShellConfirm("kill -STOP {primary}")),
            ("resume",     VerbTemplate::Shell("kill -CONT {primary}")),
            ("continue",   VerbTemplate::Shell("kill -CONT {primary}")),
            // Information (read-only).
            ("info",       VerbTemplate::Shell("ps -o pid,ppid,user,cmd -p {primary}")),
            ("show",       VerbTemplate::Shell("ps -o pid,ppid,user,cmd -p {primary}")),
            ("inspect",    VerbTemplate::Shell("ps -o pid,ppid,user,cmd -p {primary}")),
        ]
    }
}


/// Scrape process rows from the alt-screen text of a running top/htop.
///
/// Unlike normal extractors, top/htop never produce a "command + output"
/// block — they're alt-screen TUIs that overwrite cells continuously.
/// Calling code should pass the LIVE rendered text snapshot from the
/// pty grid (via Grid::render_text) and the foreground program name
/// ("top" or "htop") for parser selection.
///
/// Returns Vec<Entity> where each entity is a process row:
///   primary = PID (string)
///   display = "<COMMAND> (PID, <CPU%>%cpu, <MEM%>%mem)"
///   aliases = [process_name_lowercase, basename_of_command]
///   metadata = {pid, cpu_pct, mem_pct, user, state, command}
///
/// Designed for queries like:
///   "kill the second"        → PID at row index 1
///   "kill chrome"            → PID matching alias "chrome"
///   "send signal to firefox" → PID matching alias "firefox"
///
/// The scraper is forgiving — if a column is missing it leaves the
/// metadata field empty. Typical top output we parse:
///
///   PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM TIME+ COMMAND
///  1234 alice     20   0  4567M   234M    34M S  12.3   3.1 1:23  firefox
///
/// htop output is similar but uses a different column order on some
/// versions. Both have PID first and COMMAND last (which is what we
/// need most). We seek the header row first to locate column indices,
/// then parse subsequent rows.
pub fn extract_top_processes(rendered_text: &str, foreground: &str) -> Vec<Entity> {
    let lower_fg = foreground.to_lowercase();
    if lower_fg != "top" && lower_fg != "htop" && lower_fg != "btop" {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Find the header row. It's the line containing "PID" AND
    // ("COMMAND" or "Command" or "CMD"). Lines above are the dashboard
    // (load avg, memory, etc) which we skip.
    let lines: Vec<&str> = rendered_text.lines().collect();
    let header_idx = match lines.iter().position(|l|
        l.contains("PID")
        && (l.contains("COMMAND") || l.contains("Command") || l.contains("CMD"))
    ) {
        Some(i) => i,
        None => return Vec::new(),
    };
    let header = lines[header_idx];
    // Find column starts: positions where a non-space follows a space
    // OR position 0. This gives us the start of each header word.
    let mut col_starts: Vec<usize> = Vec::new();
    let mut prev_space = true;
    for (i, c) in header.char_indices() {
        if !c.is_whitespace() && prev_space {
            col_starts.push(i);
        }
        prev_space = c.is_whitespace();
    }
    // Map each column header to its index for easy lookup. We're
    // generous about case — top uses "PID %CPU %MEM COMMAND", htop
    // uses "PID PRI NI VIRT RES SHR S CPU% MEM% TIME+ Command".
    let header_words: Vec<&str> = header.split_whitespace().collect();
    let find_col = |name_variants: &[&str]| -> Option<usize> {
        for (i, w) in header_words.iter().enumerate() {
            for v in name_variants {
                if w.eq_ignore_ascii_case(v) { return Some(i); }
            }
        }
        None
    };
    let pid_col = find_col(&["PID"]).unwrap_or(0);
    let user_col = find_col(&["USER"]);
    let cpu_col = find_col(&["%CPU", "CPU%"]);
    let mem_col = find_col(&["%MEM", "MEM%"]);
    let cmd_col = find_col(&["COMMAND", "Command", "CMD"]).unwrap_or(header_words.len() - 1);
    let _ = col_starts;  // currently unused; kept for future column-aligned parsing

    // Parse rows after the header until a blank line or end.
    for line in lines.iter().skip(header_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() { break; }
        // Skip rows that are clearly the footer (htop has key-binding
        // legend at the bottom; top has nothing). The footer rows
        // start with F1, F2... or contain "?Help" / "Help".
        if trimmed.starts_with("F1") || trimmed.starts_with("F2")
           || trimmed.contains("Help") {
            break;
        }
        // Strip ANSI for safety; render_text should already have
        // them stripped but defensive.
        let plain = strip_ansi(line);
        let plain = plain.trim();
        let words: Vec<&str> = plain.split_whitespace().collect();
        if words.len() < cmd_col + 1 { continue; }
        let pid = words.get(pid_col).map(|s| s.to_string()).unwrap_or_default();
        // PID must look like a number; if not, this isn't a process row.
        if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) { continue; }
        let user = user_col.and_then(|i| words.get(i)).map(|s| s.to_string()).unwrap_or_default();
        let cpu = cpu_col.and_then(|i| words.get(i)).map(|s| s.to_string()).unwrap_or_default();
        let mem = mem_col.and_then(|i| words.get(i)).map(|s| s.to_string()).unwrap_or_default();
        // Command can have spaces (e.g. "/usr/bin/firefox -P default")
        // so we glue everything from cmd_col onward back together.
        let command_parts: Vec<&str> = words.iter().skip(cmd_col).copied().collect();
        if command_parts.is_empty() { continue; }
        let command = command_parts.join(" ");
        // Aliases: the bare process name (last path segment), lower-
        // cased basename, lowercased full command.
        let basename: String = command.split('/').last()
            .unwrap_or(&command)
            .split_whitespace().next().unwrap_or(&command)
            .to_string();
        let mut aliases: Vec<String> = Vec::new();
        aliases.push(basename.clone());
        aliases.push(basename.to_lowercase());
        aliases.push(command.to_lowercase());
        aliases.sort();
        aliases.dedup();
        let display = format!("{} (PID {}, {}%cpu, {}%mem)",
            basename, pid, cpu, mem);
        let mut metadata = HashMap::new();
        metadata.insert("pid".to_string(), pid.clone());
        metadata.insert("cpu_pct".to_string(), cpu);
        metadata.insert("mem_pct".to_string(), mem);
        metadata.insert("user".to_string(), user);
        metadata.insert("command".to_string(), command);
        out.push(Entity {
            kind: EntityKind::ProcessRow,
            primary: pid,
            display,
            aliases,
            confidence: 0.85,
            metadata,
        });
        // Cap at 30 — anyone scrolling far down isn't using voice
        // selection, and we don't want to spend forever parsing.
        if out.len() >= 30 { break; }
    }
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetoothctl_basic() {
        let out = "Device 11:22:33:44:55:66 Sony WH-1000XM4\n\
                   Device AA:BB:CC:DD:EE:FF AirPods Pro";
        let e = BluetoothctlExtractor.extract("bluetoothctl devices", out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].primary, "11:22:33:44:55:66");
        assert!(e[0].aliases.iter().any(|a| a == "WH-1000XM4"));
    }

    #[test]
    fn bluetooth_resolves_headphones() {
        let out = "Device 11:22:33:44:55:66 Sony WH-1000XM4 Headphones\n\
                   Device AA:BB:CC:DD:EE:FF AirPods Pro";
        let entities = BluetoothctlExtractor.extract("bluetoothctl devices", out);
        let matches = resolve_reference("headphones", &entities, 3);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].entity.primary, "11:22:33:44:55:66");
    }

    #[test]
    fn exact_word_outranks_substring() {
        // v0.1.581: voicing "test" must rank a file whose stem IS "test"
        // ABOVE files that merely CONTAIN "test" as a substring
        // (latest, contest, attestation). Before the exact-word boost
        // these all tied at the substring score and the real file didn't
        // float up. Build File entities the way completion::
        // files_to_entities does (display=basename, aliases incl. stem).
        let mk = |path: &str| {
            let base = path.rsplit('/').next().unwrap_or(path).to_string();
            let stem = base.rsplit_once('.').map(|(s, _)| s.to_string())
                .unwrap_or_else(|| base.clone());
            Entity {
                kind: EntityKind::File,
                primary: path.to_string(),
                display: base.clone(),
                aliases: vec![base, path.to_string(), stem],
                confidence: 1.0,
                metadata: std::collections::HashMap::new(),
            }
        };
        let entities = vec![
            mk("src/latest.log"),
            mk("contest.txt"),
            mk("src/test.rs"),
        ];
        let matches = resolve_reference("test", &entities, 5);
        assert_eq!(matches[0].entity.primary, "src/test.rs",
            "exact stem 'test' must outrank substring-only matches");
        assert!(matches[0].score > matches[1].score,
            "exact match score must strictly exceed substring score");
    }

    #[test]
    fn git_log_oneline() {
        let out = "abc1234 fix: bound DTW j_start\n\
                   def5678 feat: add lowpass filter";
        let e = extract_git_log(out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].primary, "abc1234");
    }

    #[test]
    fn git_branch_current() {
        let out = "* main\n  feature/voice";
        let e = extract_git_branch(out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].primary, "main");
        assert_eq!(e[0].metadata.get("current"), Some(&"true".to_string()));
    }

    #[test]
    fn rg_inline() {
        let out = "src/main.rs:42:    let foo = bar;";
        let e = RipgrepExtractor.extract("rg foo", out);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].primary, "src/main.rs:42");
    }

    #[test]
    fn dust_reverses_for_biggest() {
        let out = "234K  ┌── ./small\n\
                   1.2G  ┴── ./big";
        let e = DustExtractor.extract("dust", out);
        assert!(!e.is_empty());
        assert_eq!(e[0].primary, "./big"); // reversed: biggest first
    }

    #[test]
    fn ordinal_match() {
        let out = "abc1234 first commit\ndef5678 second commit\n0001111 third commit";
        let entities = extract_git_log(out);
        let matches = resolve_reference("the second", &entities, 3);
        assert_eq!(matches[0].entity.primary, "def5678");
    }

    #[test]
    fn verb_parse() {
        let (tmpl, verb, rest) = parse_verb("connect to headphones", &BluetoothctlExtractor).unwrap();
        assert_eq!(verb, "connect");
        assert_eq!(rest, "to headphones");
        let _ = tmpl; // template used downstream
    }

    #[test]
    fn full_compose_bluetooth() {
        let block = ExtractedBlock {
            extractor_name: "bluetoothctl-devices".into(),
            command: "bluetoothctl devices".into(),
            entities: BluetoothctlExtractor.extract(
                "bluetoothctl devices",
                "Device 11:22:33:44:55:66 Sony Headphones"
            ),
        };
        let comps = resolve_blocks("connect to headphones", &[block]);
        assert!(!comps.is_empty());
        assert_eq!(comps[0].command, "bluetoothctl connect 11:22:33:44:55:66");
    }
}
