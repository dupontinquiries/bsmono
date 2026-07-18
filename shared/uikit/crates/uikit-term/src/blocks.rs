//! blocks — Warp/Fig-style command block tracking, the lean core of voiceterm's
//! `src/term_blocks.rs` adopted into the shared terminal kit for the egui port's blocks drawer.
//!
//! Each block is `(command, output, cwd, exit, duration)`. Boundaries are approximate without
//! OSC 133 shell integration (Enter closes the previous block; the exit code arrives via
//! [`BlockTracker::note_exit_code`] when integration IS present and attaches to the block that
//! just closed), matching the reference's behavior.
//!
//! Adopted **verbatim** from the Qt backend (with its unit tests): `strip_ansi`, `escape_field`,
//! the 32 KB per-block output cap with UTF-8 char-boundary truncation (helix/htop's box-drawing
//! glyphs are 3 bytes each), the `\x1F`-separated / `\n`-record `serialize` wire format (7 core
//! fields, so existing consumers of the format parse this output unchanged), and the
//! cwd/previous-cwd tracking that powers smart-cd undo.
//!
//! Deliberately NOT lifted (they are backend concerns wired through the Qt app's audit/PTY
//! stack, and the seams are marked there): speaker attestation, snapshot tree hashes + rollback,
//! branch-tree serialization, sudo-keepalive/workflow hooks, and the summary worker. When the
//! egui app grows a real PTY + audit stack those return as a layer above this tracker.

/// One command's block: what ran, what it printed, where, and how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandBlock {
    pub id: u32,
    pub command: String,
    /// Captured output (plain text, ANSI stripped). Capped at 32 KB — see
    /// [`BlockTracker::append_output`].
    pub output: String,
    /// Working directory at time of execution (best-effort, OSC 7 or prompt export).
    pub cwd: Option<String>,
    /// Unix epoch seconds when the command was issued.
    pub started_at: u64,
    /// Duration in milliseconds (`None` while still running).
    pub duration_ms: Option<u32>,
    /// Exit code if known (OSC 133 or prompt heuristics). `None` = unknown.
    pub exit_code: Option<i32>,
    /// Approximate row in scrollback where this block started.
    pub start_row: usize,
}

/// Segments a terminal session into [`CommandBlock`]s.
#[derive(Default)]
pub struct BlockTracker {
    pub blocks: Vec<CommandBlock>,
    next_id: u32,
    /// True while output is being captured into the last block. Public so a shell-respawn
    /// path can reset it cleanly (as the reference backend does).
    pub capturing: bool,
    /// Last-known cwd from OSC 7 or PROMPT_COMMAND export.
    pub current_cwd: Option<String>,
    /// Cwd BEFORE the most recent change — powers "Undo" for smart-cd. Updated only when
    /// `set_cwd` sees a path different from current, so a repeated OSC 7 doesn't clobber it.
    pub previous_cwd: Option<String>,
    /// v0.1.883: exit code captured (via shell integration) since the last block finished;
    /// `take()`n when the next command closes that block so it can't leak onto a later one.
    pending_exit_code: Option<i32>,
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl BlockTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A command was submitted (Enter). Closes the previous block — stamping its duration and
    /// attaching any pending shell-integration exit code — then opens a new capturing block.
    /// Empty/whitespace commands are ignored, as in the reference.
    pub fn submit_command(&mut self, command: &str, start_row: usize) {
        let cmd = command.trim();
        if cmd.is_empty() {
            return;
        }
        if let Some(last) = self.blocks.last_mut() {
            if last.duration_ms.is_none() {
                let now = epoch_secs();
                last.duration_ms = Some(((now - last.started_at).saturating_mul(1000)) as u32);
            }
            let exit = self.pending_exit_code.take();
            if last.exit_code.is_none() {
                last.exit_code = exit;
            }
        }
        self.next_id += 1;
        self.blocks.push(CommandBlock {
            id: self.next_id,
            command: cmd.to_string(),
            output: String::new(),
            cwd: self.current_cwd.clone(),
            started_at: epoch_secs(),
            duration_ms: None,
            exit_code: None,
            start_row,
        });
        self.capturing = true;
    }

    /// Shell integration reported an exit code; it attaches to the block that closes next.
    pub fn note_exit_code(&mut self, code: i32) {
        self.pending_exit_code = Some(code);
    }

    /// Append PTY output to the current block: ANSI stripped, capped at 32 KB per block with a
    /// UTF-8 char-boundary-safe truncation marker (verbatim from the reference).
    pub fn append_output(&mut self, text: &str) {
        if !self.capturing {
            return;
        }
        let stripped = strip_ansi(text);
        if let Some(block) = self.blocks.last_mut() {
            let remaining = 32_768usize.saturating_sub(block.output.len());
            if remaining == 0 {
                return;
            }
            if stripped.len() <= remaining {
                block.output.push_str(&stripped);
            } else {
                let mut cut = remaining;
                while cut > 0 && !stripped.is_char_boundary(cut) {
                    cut -= 1;
                }
                block.output.push_str(&stripped[..cut]);
                block.output.push_str("\n[...output truncated]");
                self.capturing = false;
            }
        }
    }

    /// Mark the current block complete (next prompt detected).
    pub fn finish(&mut self, exit_code: Option<i32>) {
        if !self.capturing {
            return;
        }
        if let Some(block) = self.blocks.last_mut() {
            let now = epoch_secs();
            block.duration_ms = Some(((now - block.started_at).saturating_mul(1000)) as u32);
            block.exit_code = exit_code;
        }
        self.capturing = false;
    }

    /// Update cwd (OSC 7 `file://host/path` already resolved by the caller, or a prompt
    /// export). Tracks `previous_cwd` only on actual change — smart-cd undo semantics.
    pub fn set_cwd(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        if self.current_cwd.as_deref() != Some(path) {
            self.previous_cwd = self.current_cwd.clone();
        }
        self.current_cwd = Some(path.to_string());
    }

    pub fn get_block_output(&self, id: u32) -> Option<&str> {
        self.blocks.iter().find(|b| b.id == id).map(|b| b.output.as_str())
    }

    pub fn get_block_command(&self, id: u32) -> Option<&str> {
        self.blocks.iter().find(|b| b.id == id).map(|b| b.command.as_str())
    }

    /// The reference wire format, 7 core fields (`id␟cmd␟preview␟duration␟exit␟cwd␟started`),
    /// one block per line, most recent last — parsers of the Qt backend's serialize read this
    /// unchanged (that format appends optional fields after the 7th; absent here).
    pub fn serialize(&self, n: usize) -> String {
        let take = self.blocks.len().min(n);
        let start = self.blocks.len() - take;
        let mut out = String::with_capacity(take * 256);
        for block in &self.blocks[start..] {
            out.push_str(&block.id.to_string());
            out.push('\x1F');
            out.push_str(&escape_field(&block.command));
            out.push('\x1F');
            let first_line: String = block.output.lines().take(3).collect::<Vec<_>>().join(" / ");
            out.push_str(&escape_field(&first_line.chars().take(200).collect::<String>()));
            out.push('\x1F');
            out.push_str(&block.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "-".into()));
            out.push('\x1F');
            out.push_str(&block.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()));
            out.push('\x1F');
            out.push_str(&block.cwd.clone().unwrap_or_default());
            out.push('\x1F');
            out.push_str(&block.started_at.to_string());
            out.push('\n');
        }
        out
    }
}

/// Replace separator chars so a field can't break the `\x1F`/`\n` parse format — verbatim.
fn escape_field(s: &str) -> String {
    s.replace('\x1F', " ").replace('\n', " ").replace('\r', "")
}

/// Strip ANSI CSI/OSC sequences and control chars (keeping `\n`/`\t`) — verbatim from the
/// reference, including OSC termination by BEL or ST and the two-byte ESC fallback.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' {
                            break;
                        }
                        if nc == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\x07' | '\x08' => {}
            '\r' => {}
            '\x00'..='\x06' | '\x0b'..='\x1f' if c != '\n' && c != '\t' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── escape_field / strip_ansi: the reference's own tests, adopted verbatim ───

    #[test]
    fn escape_field_pins() {
        assert_eq!(escape_field("hello world"), "hello world");
        assert_eq!(escape_field("a\x1Fb"), "a b");
        assert_eq!(escape_field("line1\nline2"), "line1 line2");
        assert_eq!(escape_field("line1\rline2"), "line1line2");
        assert_eq!(escape_field("a\x1Fb\nc\rd"), "a b cd");
    }

    #[test]
    fn strip_ansi_pins() {
        assert_eq!(strip_ansi("plain text"), "plain text");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[1;38;5;208mbold orange\x1b[m"), "bold orange");
        assert_eq!(strip_ansi("\x1b]0;title\x07after"), "after"); // OSC + BEL
        assert_eq!(strip_ansi("\x1b]7;file://h/p\x1b\\after"), "after"); // OSC + ST
        assert_eq!(strip_ansi("keep\nnewlines\tand tabs"), "keep\nnewlines\tand tabs");
        assert_eq!(strip_ansi("bell\x07back\x08space"), "bellbackspace");
    }

    // ─── tracker flow ───

    #[test]
    fn blocks_segment_and_close_on_the_next_submit() {
        let mut t = BlockTracker::new();
        t.set_cwd("/home/u/proj");
        t.submit_command("  ls -la  ", 0);
        t.append_output("\x1b[34mfile_a\x1b[0m\nfile_b\n");
        assert_eq!(t.blocks.len(), 1);
        assert_eq!(t.blocks[0].command, "ls -la"); // trimmed
        assert_eq!(t.blocks[0].output, "file_a\nfile_b\n"); // ANSI stripped
        assert_eq!(t.blocks[0].cwd.as_deref(), Some("/home/u/proj"));
        assert!(t.blocks[0].duration_ms.is_none()); // still running

        t.note_exit_code(0); // shell integration reports before the next prompt
        t.submit_command("echo hi", 3);
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.blocks[0].exit_code, Some(0)); // attached to the block that closed
        assert!(t.blocks[0].duration_ms.is_some());
        assert!(t.capturing);

        t.finish(Some(1));
        assert_eq!(t.blocks[1].exit_code, Some(1));
        assert!(!t.capturing);
        t.append_output("late output"); // not capturing — dropped
        assert_eq!(t.blocks[1].output, "");
        // empty command is a no-op
        t.submit_command("   ", 5);
        assert_eq!(t.blocks.len(), 2);
    }

    #[test]
    fn output_caps_at_32kb_on_a_char_boundary() {
        let mut t = BlockTracker::new();
        t.submit_command("yes │", 0);
        // fill close to the cap, then push multibyte box-drawing over it
        t.append_output(&"x".repeat(32_760));
        t.append_output("││││││"); // 3 bytes each — the cut must not split one
        let out = &t.blocks[0].output;
        assert!(out.ends_with("\n[...output truncated]"));
        assert!(out.len() <= 32_768 + "\n[...output truncated]".len());
        assert!(!t.capturing); // capture stops once truncated
        let before = out.clone();
        t.append_output("more");
        assert_eq!(&t.blocks[0].output, &before);
    }

    #[test]
    fn cwd_tracks_previous_only_on_change() {
        let mut t = BlockTracker::new();
        t.set_cwd("/a");
        assert_eq!(t.previous_cwd, None);
        t.set_cwd("/a"); // repeat: previous must NOT clobber
        assert_eq!(t.previous_cwd, None);
        t.set_cwd("/b");
        assert_eq!(t.previous_cwd.as_deref(), Some("/a"));
        t.set_cwd(""); // empty ignored
        assert_eq!(t.current_cwd.as_deref(), Some("/b"));
    }

    #[test]
    fn serialize_speaks_the_reference_wire_format() {
        let mut t = BlockTracker::new();
        t.set_cwd("/w");
        t.submit_command("make", 0);
        t.append_output("line one\nline two\nline three\nline four\n");
        t.finish(Some(0));
        let s = t.serialize(50);
        let fields: Vec<&str> = s.trim_end().split('\x1F').collect();
        assert_eq!(fields.len(), 7);
        assert_eq!(fields[0], "1");
        assert_eq!(fields[1], "make");
        assert_eq!(fields[2], "line one / line two / line three"); // first 3 lines joined
        assert_eq!(fields[4], "0");
        assert_eq!(fields[5], "/w");
        // n limits from the tail (most recent kept)
        t.submit_command("second", 1);
        let s2 = t.serialize(1);
        assert!(s2.starts_with("2\x1F"));
        assert_eq!(s2.lines().count(), 1);
        // lookups by id
        assert_eq!(t.get_block_command(1), Some("make"));
        assert!(t.get_block_output(1).unwrap().contains("line four"));
        assert_eq!(t.get_block_command(99), None);
    }
}
