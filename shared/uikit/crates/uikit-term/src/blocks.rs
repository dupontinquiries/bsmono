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
    /// The command just submitted, waiting to be recognised as the
    /// shell's echo of it. Armed by `submit_command`, consumed by the
    /// first `feed_line` after it. See there for why.
    pending_echo: Option<String>,
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
        // Armed for one line — see `feed_line`.
        self.pending_echo = Some(command.trim().to_string());
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

    /// Feed one line of shell output, closing the block when the line is a
    /// prompt. Returns true when it closed one, so a caller can avoid
    /// rendering the prompt inside the output it terminated.
    ///
    /// **This is what gives a block an end.** `finish` exists and needs
    /// somebody to notice the command is over; a shell reports an exit code
    /// only if it has been configured to, and most have not. Without a
    /// prompt heuristic every block stays open forever, so the next command
    /// closes the previous one with an unknown status and the output of two
    /// commands runs together.
    ///
    /// Ported from voiceterm's own `blocks.rs`, which had this and whose
    /// copy of the tracker the app does not use — the method sat on the
    /// dead-API list for exactly that reason. Living here means the tracker
    /// that IS used can close a block.
    pub fn feed_line(&mut self, line: &str) -> bool {
        // THE SHELL ECHOES WHAT YOU TYPE, and that echo comes back as
        // output. Every block began with a copy of its own command:
        //
        //     cmd "echo one"  output "echo one\none\n"
        //
        // Invisible against a stand-in, because a fake player prints only
        // what it is told to; found the first time a real pty was driven.
        //
        // Only the FIRST line after a command is eligible, and only once,
        // so `echo echo one` keeps the "echo one" that is genuinely its
        // output. Matched with `ends_with` as well as equality, because a
        // prompt and the command often arrive on one line.
        if let Some(echo) = self.pending_echo.take() {
            let t = line.trim();
            if t == echo || (t.ends_with(&echo) && t.len() < echo.len() + 64) {
                return false;
            }
        }
        if self.capturing && looks_like_prompt(line) {
            self.finish(None);
            return true;
        }
        self.append_output(line);
        self.append_output("\n");
        false
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

/// The longest line still considered a prompt.
///
/// A prompt is short. Without a cap, any output line ending in `>` — a
/// diff, an XML dump, a quoted email — closes the block, and the command's
/// remaining output lands in the next one.
pub const MAX_PROMPT_LEN: usize = 200;

/// Does this line look like a shell prompt?
///
/// Deliberately a heuristic over the LAST character rather than a regex
/// over prompt formats: people set `PS1` to anything, and a pattern that
/// tried to enumerate them would match fewer real prompts than this does,
/// while looking more precise.
pub fn looks_like_prompt(line: &str) -> bool {
    let t = line.trim_end();
    if t.is_empty() || t.len() > MAX_PROMPT_LEN {
        return false;
    }
    matches!(t.chars().last(), Some('$') | Some('#') | Some('>') | Some('❯'))
}

/// Accumulate bytes bound for a shell and yield the commands they contain.
///
/// A terminal sends a command one keystroke at a time, so "what command
/// was that" is a question about a byte STREAM rather than a line — and
/// every caller that needs it was going to reinvent the same three rules.
/// It lives here, beside the tracker it feeds.
#[derive(Debug, Clone, Default)]
pub struct CommandLines {
    buf: Vec<u8>,
}

/// A pathological paste with no line ending must not grow the buffer
/// without limit.
pub const LINE_CAP: usize = 8 * 1024;

impl CommandLines {
    pub fn new() -> CommandLines { CommandLines::default() }

    /// Feed bytes going to the shell; return any completed commands.
    ///
    /// Three rules, each from something that goes wrong without it:
    ///
    /// * **Backspace edits the pending line** before anything reads it.
    ///   Without this, the control-byte rule below throws away every
    ///   command that was corrected while typing — which is most of them.
    /// * **A line containing control bytes is not a command.** An arrow
    ///   key or a Ctrl-C leaves an escape sequence in the buffer, and
    ///   recording that fills the history with entries nobody can re-run.
    /// * **`\r` and `\n` both end a line.** A terminal sends `\r` for
    ///   Enter; a paste arrives with `\n`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        while let Some(i) = self.buf.iter().position(|b| *b == 0x7f) {
            self.buf.remove(i);
            if i > 0 { self.buf.remove(i - 1); }
        }
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|b| *b == b'\r' || *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let cmd = String::from_utf8_lossy(&line[..line.len() - 1]).trim().to_string();
            if cmd.is_empty() || cmd.bytes().any(|b| b < 0x20) {
                continue;
            }
            out.push(cmd);
        }
        if self.buf.len() > LINE_CAP {
            let drop = self.buf.len() - LINE_CAP;
            self.buf.drain(..drop);
        }
        out
    }

    /// What has been typed but not yet entered.
    pub fn pending(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    #[test]
    fn a_command_is_recorded_when_the_line_ends_and_not_before() {
        let mut c = CommandLines::new();
        assert!(c.feed(b"ls -").is_empty(), "half a command was recorded");
        assert!(c.feed(b"la").is_empty());
        assert_eq!(c.feed(b"\r"), vec!["ls -la".to_string()]);
        assert_eq!(c.pending(), "");
    }

    #[test]
    fn a_correction_survives_rather_than_losing_the_command() {
        // Backspace leaves 0x7f in the stream. Applied before the
        // control-byte rule, this is `ls`; applied after, the whole command
        // is discarded — and typing a mistake is not rare.
        let mut c = CommandLines::new();
        assert_eq!(c.feed(b"lxs\r"), vec!["ls".to_string()]);
    }

    #[test]
    fn an_escape_sequence_is_not_a_command() {
        // An arrow key mid-line. Recording it puts an entry in the history
        // that cannot be re-run and reads as corruption.
        let mut c = CommandLines::new();
        assert!(c.feed(b"ls [A\r").is_empty());
        // And the buffer is clear afterwards, so the NEXT command is not
        // poisoned by the one that was rejected.
        assert_eq!(c.feed(b"pwd\r"), vec!["pwd".to_string()]);
    }

    #[test]
    fn several_commands_in_one_write_all_arrive() {
        // A paste, or a voice command sent with its own newline.
        let mut c = CommandLines::new();
        assert_eq!(c.feed(b"cd /tmp\nls\n"), vec!["cd /tmp".to_string(), "ls".to_string()]);
    }

    #[test]
    fn a_bare_enter_records_nothing() {
        let mut c = CommandLines::new();
        assert!(c.feed(b"\r\r  \r").is_empty());
    }

    #[test]
    fn a_paste_with_no_line_ending_cannot_grow_the_buffer_without_limit() {
        let mut c = CommandLines::new();
        c.feed(&vec![b'x'; LINE_CAP * 2]);
        assert!(c.pending().len() <= LINE_CAP, "{}", c.pending().len());
    }
}

#[cfg(test)]
mod feed_tests {
    use super::*;

    #[test]
    fn a_prompt_closes_the_block_and_output_before_it_belongs_to_it() {
        let mut t = BlockTracker::new();
        t.submit_command("ls", 0);
        assert!(!t.feed_line("Cargo.toml  src"));
        assert!(t.feed_line("user@host:~$"), "the prompt did not close the block");
        let b = t.blocks.last().expect("a block");
        assert!(b.output.contains("Cargo.toml"));
        assert!(!b.output.contains("user@host"),
                "the prompt was recorded as output of the command it ended");
        // Closed: output after it belongs to nobody rather than to `ls`.
        t.feed_line("stray");
        assert!(!t.blocks.last().unwrap().output.contains("stray"));
    }

    #[test]
    fn a_long_line_ending_in_an_angle_bracket_is_not_a_prompt() {
        // A diff, an XML dump, a quoted email. Without the length cap each
        // of these ends the block and the rest of the output lands in the
        // next one.
        let xml = format!("<{}>", "a".repeat(MAX_PROMPT_LEN));
        assert!(!looks_like_prompt(&xml));
        assert!(looks_like_prompt("$"));
        assert!(looks_like_prompt("~/proj ❯"));
        assert!(!looks_like_prompt(""), "a blank line is not a prompt");
        assert!(!looks_like_prompt("total 48"));
    }

    #[test]
    fn feeding_with_nothing_open_does_not_invent_a_block() {
        // Output before any command — a shell banner, a motd — must not
        // become a block with no command in it.
        let mut t = BlockTracker::new();
        assert!(!t.feed_line("Welcome to Ubuntu"));
        assert!(t.blocks.is_empty(), "output with no command opened a block");
    }
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

#[cfg(test)]
mod echo_tests {
    use super::*;

    #[test]
    fn the_shells_echo_of_the_command_is_not_its_output() {
        // Found by driving a real pty for the first time: a shell echoes
        // what you type, so every block began with a copy of its own
        // command — `cmd "echo one"` with `output "echo one\none\n"`. A
        // stand-in prints only what it is told to, so nothing showed it.
        let mut t = BlockTracker::new();
        t.submit_command("echo one", 0);
        t.feed_line("echo one");
        t.feed_line("one");
        let b = t.blocks.last().unwrap();
        assert_eq!(b.output, "one\n", "{:?}", b.output);
    }

    #[test]
    fn a_prompt_and_the_command_on_one_line_is_still_the_echo() {
        // Which is how it usually arrives: the prompt is written, then
        // the typed characters, then the newline.
        let mut t = BlockTracker::new();
        t.submit_command("ls", 0);
        t.feed_line("user@host:~$ ls");
        t.feed_line("file.txt");
        assert_eq!(t.blocks.last().unwrap().output, "file.txt\n");
    }

    #[test]
    fn output_that_repeats_the_command_is_kept() {
        // `echo echo one` prints "echo one", and that is content. Only the
        // FIRST line after a command is eligible, and only once.
        let mut t = BlockTracker::new();
        t.submit_command("echo echo one", 0);
        t.feed_line("echo echo one");   // the echo
        t.feed_line("echo one");        // the output
        assert_eq!(t.blocks.last().unwrap().output, "echo one\n");
    }

    #[test]
    fn a_command_with_no_echo_loses_no_output() {
        // A shell with echo off, or output arriving before it. The guard
        // must not eat a real first line.
        let mut t = BlockTracker::new();
        t.submit_command("date", 0);
        t.feed_line("Mon Aug 31 20:00:00 UTC 2026");
        assert!(t.blocks.last().unwrap().output.starts_with("Mon"),
                "{:?}", t.blocks.last().unwrap().output);
    }

    #[test]
    fn a_long_line_ending_in_the_command_text_is_not_swallowed() {
        // The `ends_with` match is bounded, or a page of output that
        // happens to end with the command name would vanish.
        let mut t = BlockTracker::new();
        t.submit_command("ls", 0);
        let long = format!("{} ls", "x".repeat(200));
        t.feed_line(&long);
        assert!(t.blocks.last().unwrap().output.contains("xxx"),
                "a long line was taken for an echo");
    }
}

// ── block kind detection ─────────────────────────────────────────────────────
//
// Ported VERBATIM from voiceterm's `src/term_blocks.rs`, which is where this
// was written and tested (44 tests) and which nothing had ever called. There
// were three implementations of command blocks in this fleet when this landed:
// this one (639 lines, the LIVE one -- BlockTracker/CommandLines are what
// `voiceterm_egui` actually runs), voiceterm's `egui-frontend/src/blocks.rs`
// (415 lines, 19 tests, and exactly ONE caller using ONE function), and
// `src/term_blocks.rs` (1,302 lines, 44 tests, no callers at all).
//
// The live one had the tracker and no kind detection; the other two had kind
// detection and no users. So the detection comes here, taken from the
// best-tested of the three rather than from the reimplementation.
//
// ORDERED, NOT SCORED, and the original explains why: Email is checked first
// because `From:` at the top of the output is nearly conclusive, while
// "consistent column counts" is a guess that `git log --oneline` would also
// pass. Scoring them against each other would let the weakest test outvote the
// strongest on a long output.

/// The shape of a block's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockKind {
    /// No specific shape detected.
    #[default]
    Plain,
    /// Output starts with mail headers — `mail`, `mutt -e display`, a
    /// `cat ~/Maildir/...`.
    Email,
    /// A `git` command with output. Useful for clickable-hash follow-ups.
    Git,
    /// Rows of consistent column counts, for a future "show as table".
    Table,
}

impl CommandBlock {
    /// Classify this block's output. Ordered, not scored -- see the section
    /// header above.
    ///
    /// THIS FUNCTION WAS MISSING. The section says the detection was "ported
    /// VERBATIM from voiceterm's src/term_blocks.rs", and the enum, both
    /// predicates and the tests all came across -- but not this, the one
    /// function that turns them into a `BlockKind`. Nothing in the crate
    /// produced a BlockKind at all, and the tests calling `detected_kind()`
    /// never compiled, so `cargo test -p uikit-term` has been failing to build
    /// since the port. I claimed the port worked without running it. Taken
    /// verbatim from term_blocks.rs:93 now.
    pub fn detected_kind(&self) -> BlockKind {
        // Email: output starts with one of the standard mail
        // headers (after any leading blank lines).
        let trimmed_out = self.output.trim_start();
        if Self::looks_like_email(trimmed_out) {
            return BlockKind::Email;
        }
        // Git: command starts with `git ` (or is exactly `git`).
        // Output must also exist (some git commands like
        // `git config --global ...` produce nothing).
        let cmd = self.command.trim_start();
        if (cmd == "git" || cmd.starts_with("git "))
            && !self.output.trim().is_empty()
        {
            return BlockKind::Git;
        }
        // Table: 3+ lines with consistent column counts (split
        // on multi-space). Cheap heuristic; future feature ships
        // can refine.
        if Self::looks_like_table(&self.output) {
            return BlockKind::Table;
        }
        BlockKind::Plain
    }

    pub fn looks_like_email(text: &str) -> bool {
        let mut has_anchor = false; // From: or Subject:
        let mut total_headers = 0;
        for (i, line) in text.lines().enumerate() {
            if i >= 10 { break; }
            // Header pattern: starts with one of the known
            // header names + ":". Case-insensitive on the
            // name to tolerate some forwarded-mail
            // variations.
            let lower = line.to_ascii_lowercase();
            // Anchor headers (strong signal).
            if lower.starts_with("from: ") || lower.starts_with("subject: ") {
                has_anchor = true;
                total_headers += 1;
                continue;
            }
            // Supporting headers (weaker on their own).
            if lower.starts_with("to: ")
                || lower.starts_with("cc: ")
                || lower.starts_with("bcc: ")
                || lower.starts_with("date: ")
                || lower.starts_with("reply-to: ")
                || lower.starts_with("message-id: ")
                || lower.starts_with("in-reply-to: ")
                || lower.starts_with("references: ")
            {
                total_headers += 1;
                continue;
            }
            // First non-empty non-header line ends the
            // header block.
            if !line.trim().is_empty() && total_headers > 0 {
                break;
            }
        }
        has_anchor && total_headers >= 2
    }

    /// Table shape detector. Looks for 3+ consecutive lines
    /// whose split-on-multi-whitespace column counts agree
    /// (within ±1). Excludes header-rule lines like `----`.
    pub fn looks_like_table(text: &str) -> bool {
        let mut col_counts: Vec<usize> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }
            // Skip ruler lines.
            if trimmed.chars().all(|c| c == '-' || c == '=' || c == '+' || c.is_whitespace()) {
                continue;
            }
            // Multi-space split — table columns are usually
            // separated by 2+ spaces or tabs.
            let cols = if trimmed.contains('\t') {
                trimmed.split('\t').filter(|s| !s.is_empty()).count()
            } else {
                // Split on runs of 2+ spaces.
                let mut count = 0;
                let mut in_word = false;
                let mut prev_space_run = 0;
                for c in trimmed.chars() {
                    if c == ' ' {
                        prev_space_run += 1;
                    } else {
                        if !in_word || prev_space_run >= 2 {
                            count += 1;
                            in_word = true;
                        }
                        prev_space_run = 0;
                    }
                }
                count
            };
            // 2+ columns to be table-shaped.
            if cols >= 2 {
                col_counts.push(cols);
            }
        }
        if col_counts.len() < 3 { return false; }
        let first = col_counts[0];
        col_counts.iter().all(|&c| c.abs_diff(first) <= 1)
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    fn block(cmd: &str, out: &str) -> CommandBlock {
        let mut t = BlockTracker::new();
        t.submit_command(cmd, 0);
        t.append_output(out);
        t.finish(Some(0));
        t.blocks.last().unwrap().clone()
    }

    /// Email is checked FIRST because its signature is nearly conclusive.
    #[test]
    fn mail_headers_are_recognised() {
        let b = block("cat mail.txt", "From: a@b.c\nSubject: hi\nTo: d@e.f\n\nbody");
        assert_eq!(b.detected_kind(), BlockKind::Email);
    }

    /// One header is not enough -- prose mentioning "From:" must not become an
    /// email, which is why the original requires two with an anchor.
    #[test]
    fn a_single_header_like_line_is_not_an_email() {
        let b = block("echo", "From: the docs, we learn that\nthis is prose\n");
        assert_ne!(b.detected_kind(), BlockKind::Email);
    }

    #[test]
    fn a_git_command_with_output_is_git() {
        let b = block("git status", "On branch main\nnothing to commit\n");
        assert_eq!(b.detected_kind(), BlockKind::Git);
        // ...and one with no output is not, since some git commands are silent.
        let quiet = block("git config --global x y", "");
        assert_eq!(quiet.detected_kind(), BlockKind::Plain);
    }

    /// ORDERING, NOT SCORING. `git log --oneline` passes the table shape test;
    /// Git must win because a command prefix is the stronger signal.
    #[test]
    fn git_output_that_looks_tabular_is_still_git() {
        let b = block("git log --oneline",
                      "a1b2c3d  first  thing\ne4f5g6h  second  thing\ni7j8k9l  third  thing\n");
        assert_eq!(b.detected_kind(), BlockKind::Git);
    }

    #[test]
    fn consistent_columns_are_a_table() {
        let b = block("ls -l", "a  1  x\nb  2  y\nc  3  z\n");
        assert_eq!(b.detected_kind(), BlockKind::Table);
    }

    #[test]
    fn ordinary_output_is_plain() {
        assert_eq!(block("echo hi", "hi\n").detected_kind(), BlockKind::Plain);
        assert_eq!(block("true", "").detected_kind(), BlockKind::Plain);
    }
}
