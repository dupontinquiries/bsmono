//! blocks — Warp/Fig-style command blocks, the terminal's semantic unit.
//!
//! `src/term_blocks.rs` is 1,302 lines with no reference in the port. A terminal
//! that only knows "a stream of bytes" cannot copy one command's output, jump to
//! the last failure, or say "show that as a table" — all of which need to know
//! **where one command ended and the next began**.
//!
//! ## The boundary detection is a heuristic and says so
//!
//! The original is honest about it: *"This is approximate without OSC 133 shell
//! integration but works for most shells."* Boundaries come from Enter being
//! pressed, a new command starting at column zero, and a prompt reappearing —
//! recognised as a short line ending in `$ # > ❯`.
//!
//! That honesty is worth porting along with the code. A block model that
//! pretended to be exact would invite features that break on the first shell
//! with an unusual prompt, and the failure would look like the terminal losing
//! output rather than like a guess that missed.
//!
//! ## Kind detection is ordered, not scored
//!
//! Email first (a very specific header signature), then Git (a command prefix),
//! then Table (a shape heuristic), then Plain. Ordered because the tests are of
//! wildly different confidence: `From:` at the top of the output is nearly
//! conclusive, while "consistent column counts" is a guess that `git log
//! --oneline` would also pass. Scoring them against each other would let the
//! weakest test outvote the strongest on a long output.

/// What shape the output has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Plain,
    /// Mail headers at the top: `mail`, `mutt -e display`, `cat ~/Maildir/…`.
    Email,
    /// Git output, for clickable hashes and follow-ups.
    Git,
    /// Tabular, for "show that as a table".
    Table,
}

/// Minimum rows before uniform columns mean anything.
///
/// Three. Two lines with the same column count is a coincidence that ordinary
/// prose hits constantly; three starts to be evidence.
pub const MIN_TABLE_ROWS: usize = 3;

/// Longest line that can still be a prompt.
///
/// A prompt is short by nature. Without a length bound, any output line ending
/// in `>` — a quoted email, a diff, an XML tag — would close the block early and
/// the rest of the command's output would land in the next one.
pub const MAX_PROMPT_LEN: usize = 120;

/// Does this line look like a shell prompt?
pub fn looks_like_prompt(line: &str) -> bool {
    let t = line.trim_end();
    if t.is_empty() || t.len() > MAX_PROMPT_LEN { return false; }
    matches!(t.chars().last(), Some('$') | Some('#') | Some('>') | Some('❯'))
}

/// Do these lines start with mail headers?
pub fn looks_like_email(text: &str) -> bool {
    // Leading blank lines are skipped: a mail dump often starts with one, and
    // requiring the header on line zero would miss most real cases.
    for line in text.lines().skip_while(|l| l.trim().is_empty()).take(4) {
        let l = line.trim_start();
        if l.starts_with("From:") || l.starts_with("To:")
            || l.starts_with("Subject:") || l.starts_with("Date:") {
            return true;
        }
    }
    false
}

/// Does this output have consistent column structure?
pub fn looks_like_table(text: &str) -> bool {
    let rows: Vec<usize> = text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split_whitespace().count())
        .collect();
    if rows.len() < MIN_TABLE_ROWS { return false; }
    // A single column is a list, not a table, and calling `ls` output tabular
    // would offer a "show as table" that does nothing.
    let first = rows[0];
    first >= 2 && rows.iter().all(|c| *c == first)
}

/// One executed command and its output.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandBlock {
    pub id: u32,
    pub command: String,
    pub output: String,
    pub cwd: Option<String>,
    pub started_at: u64,
    /// `None` when unknown — distinct from `Some(0)`, which is a finished
    /// success. A UI showing a running command as succeeded would be lying for
    /// as long as it took.
    pub exit_code: Option<i32>,
    /// Whether the block has ended.
    ///
    /// Separate from `exit_code` because "still running" and "closed, status
    /// unknown" are BOTH `None` and are not the same thing — the prompt
    /// heuristic closes blocks without ever learning a code. I conflated them
    /// first and a test caught it: a block closed by a following command stayed
    /// "running" forever, so a spinner would spin for the rest of the session.
    closed: bool,
}

impl CommandBlock {
    pub fn new(id: u32, command: &str, started_at: u64) -> CommandBlock {
        CommandBlock { id, command: command.to_string(), output: String::new(),
                       cwd: None, started_at, exit_code: None, closed: false }
    }

    pub fn is_running(&self) -> bool { !self.closed }
    /// Ended without a code — the prompt heuristic fired and nothing else did.
    pub fn ended_unknown(&self) -> bool { self.closed && self.exit_code.is_none() }
    pub fn failed(&self) -> bool { matches!(self.exit_code, Some(c) if c != 0) }

    /// The shape of this block's output. Pure, safe to call repeatedly.
    pub fn detected_kind(&self) -> BlockKind {
        if looks_like_email(&self.output) { return BlockKind::Email; }
        if self.command.trim_start().starts_with("git ") {
            let o = self.output.to_lowercase();
            if o.contains("commit") || o.contains("branch") || o.contains("modified") {
                return BlockKind::Git;
            }
        }
        if looks_like_table(&self.output) { return BlockKind::Table; }
        BlockKind::Plain
    }
}

/// How much output one block keeps.
///
/// Bounded because a runaway `yes` or a verbose build will happily produce
/// gigabytes, and a terminal that stored all of it would die holding text
/// nobody will read. The TAIL is kept rather than the head: when something goes
/// wrong the error is at the end.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Tracks blocks as commands run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockTracker {
    blocks: Vec<CommandBlock>,
    next_id: u32,
    open: Option<usize>,
}

impl BlockTracker {
    pub fn blocks(&self) -> &[CommandBlock] { &self.blocks }
    pub fn len(&self) -> usize { self.blocks.len() }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() }
    pub fn open_block(&self) -> Option<&CommandBlock> {
        self.blocks.get(self.open?)
    }

    /// The user pressed Enter on a command.
    pub fn submit_command(&mut self, command: &str, started_at: u64) -> u32 {
        // An empty Enter is not a command. Recording one would fill the history
        // with blank blocks that the "jump to previous command" key then walks
        // through.
        if command.trim().is_empty() { return self.next_id; }
        // A previous block still open is closed with an unknown status rather
        // than left running forever — the shell moved on, so we did too.
        if self.open.is_some() { self.finish(None); }
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(CommandBlock::new(id, command, started_at));
        self.open = Some(self.blocks.len() - 1);
        id
    }

    /// Output arrived.
    pub fn append_output(&mut self, text: &str) {
        let Some(i) = self.open else { return };
        let b = &mut self.blocks[i];
        b.output.push_str(text);
        if b.output.len() > MAX_OUTPUT_BYTES {
            // Trim from the front to a char boundary — slicing bytes blindly
            // would panic on any multi-byte character, which build output is
            // full of.
            let cut = b.output.len() - MAX_OUTPUT_BYTES;
            let at = (cut..b.output.len())
                .find(|i| b.output.is_char_boundary(*i))
                .unwrap_or(b.output.len());
            b.output = b.output[at..].to_string();
        }
    }

    /// A prompt reappeared, or an exit code arrived.
    pub fn finish(&mut self, exit_code: Option<i32>) {
        let Some(i) = self.open.take() else { return };
        // Only overwrite an unknown code: a real code that arrived before the
        // prompt heuristic fired is better evidence than the heuristic.
        if self.blocks[i].exit_code.is_none() {
            self.blocks[i].exit_code = exit_code;
        }
        self.blocks[i].closed = true;
    }

    /// Feed a line of terminal output, closing the block if it is a prompt.
    ///
    /// Returns true when the line closed a block, so a caller can avoid
    /// rendering the prompt inside the output it terminated.
    pub fn feed_line(&mut self, line: &str) -> bool {
        if self.open.is_some() && looks_like_prompt(line) {
            self.finish(None);
            return true;
        }
        self.append_output(line);
        self.append_output("\n");
        false
    }

    pub fn set_cwd(&mut self, path: &str) {
        if let Some(i) = self.open { self.blocks[i].cwd = Some(path.to_string()); }
    }

    /// The most recent failure, for "jump to the last error".
    pub fn last_failure(&self) -> Option<&CommandBlock> {
        self.blocks.iter().rev().find(|b| b.failed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_with(cmd: &str, output: &str) -> BlockTracker {
        let mut t = BlockTracker::default();
        t.submit_command(cmd, 0);
        t.append_output(output);
        t
    }

    #[test]
    fn a_command_opens_a_block_and_a_prompt_closes_it() {
        let mut t = BlockTracker::default();
        t.submit_command("ls", 0);
        assert!(t.open_block().is_some());
        assert!(t.feed_line("user@host:~$ "), "the prompt closed it");
        assert!(t.open_block().is_none());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn a_long_line_ending_in_a_bracket_does_not_close_a_block() {
        // Without a length bound, a quoted email, a diff or an XML tag would
        // close the block early and the rest of the output would land in the
        // next one.
        let long = format!("{}>", "x".repeat(MAX_PROMPT_LEN));
        assert!(!looks_like_prompt(&long));
        assert!(looks_like_prompt("~$"));
        assert!(looks_like_prompt("❯"));
    }

    #[test]
    fn an_empty_line_is_not_a_prompt() {
        assert!(!looks_like_prompt(""));
        assert!(!looks_like_prompt("   "));
    }

    #[test]
    fn an_empty_enter_does_not_create_a_block() {
        // Otherwise the history fills with blank blocks that "jump to previous
        // command" then walks through.
        let mut t = BlockTracker::default();
        t.submit_command("   ", 0);
        assert!(t.is_empty());
    }

    #[test]
    fn a_new_command_closes_the_previous_block() {
        // The shell moved on, so we do too, rather than leaving a block running
        // forever.
        let mut t = BlockTracker::default();
        t.submit_command("first", 0);
        t.submit_command("second", 1);
        assert_eq!(t.len(), 2);
        assert!(!t.blocks()[0].is_running());
    }

    #[test]
    fn a_block_closed_without_a_code_is_not_still_running() {
        // "Still running" and "closed, status unknown" are both exit_code None
        // and are not the same thing. I conflated them and this caught it: a
        // block closed by a following command stayed "running" forever, so a
        // spinner would spin for the rest of the session.
        let mut t = BlockTracker::default();
        t.submit_command("first", 0);
        t.submit_command("second", 1);
        assert!(!t.blocks()[0].is_running());
        assert!(t.blocks()[0].ended_unknown());
        assert!(!t.blocks()[0].failed(), "unknown is not failed");
    }

    #[test]
    fn a_running_command_is_distinct_from_a_successful_one() {
        // None and Some(0) are different states, and a UI showing a running
        // command as succeeded would be lying for as long as it took.
        let t = tracker_with("sleep 10", "");
        assert!(t.blocks()[0].is_running());
        assert!(!t.blocks()[0].failed());
    }

    #[test]
    fn a_real_exit_code_outranks_the_prompt_heuristic() {
        // The heuristic is a guess; an actual code is not.
        let mut t = BlockTracker::default();
        t.submit_command("false", 0);
        t.finish(Some(1));
        t.finish(None);
        assert_eq!(t.blocks()[0].exit_code, Some(1));
        assert!(t.blocks()[0].failed());
    }

    #[test]
    fn the_last_failure_is_findable() {
        let mut t = BlockTracker::default();
        t.submit_command("ok", 0);
        t.finish(Some(0));
        t.submit_command("bad", 1);
        t.finish(Some(2));
        t.submit_command("ok again", 2);
        t.finish(Some(0));
        assert_eq!(t.last_failure().map(|b| b.command.as_str()), Some("bad"));
    }

    #[test]
    fn no_failures_means_none_rather_than_the_last_block() {
        let mut t = BlockTracker::default();
        t.submit_command("ok", 0);
        t.finish(Some(0));
        assert!(t.last_failure().is_none());
    }

    #[test]
    fn mail_headers_are_detected_past_leading_blank_lines() {
        // A mail dump often starts with a blank line, and requiring the header
        // on line zero would miss most real cases.
        let t = tracker_with("cat msg", "\n\nFrom: ada@x.y\nSubject: hi\n\nbody");
        assert_eq!(t.blocks()[0].detected_kind(), BlockKind::Email);
    }

    #[test]
    fn git_output_needs_both_the_command_and_the_content() {
        // The command alone is not enough — `git --version` is not git output
        // worth linking hashes in.
        let good = tracker_with("git status", "On branch main\nmodified: a.rs\nx y");
        assert_eq!(good.blocks()[0].detected_kind(), BlockKind::Git);
        let bare = tracker_with("git --version", "git version 2.43.0");
        assert_ne!(bare.blocks()[0].detected_kind(), BlockKind::Git);
    }

    #[test]
    fn kind_detection_is_ordered_so_the_weakest_test_cannot_outvote_the_strongest() {
        // Uniform column counts is a guess; `From:` at the top is nearly
        // conclusive. A mail dump whose lines happen to be uniform must still
        // read as Email.
        let t = tracker_with("cat msg", "From: a@b.c\nTo: d@e.f\nDate: now x\n");
        assert_eq!(t.blocks()[0].detected_kind(), BlockKind::Email);
    }

    #[test]
    fn a_table_needs_several_rows_of_consistent_columns() {
        // Two matching lines is a coincidence ordinary prose hits constantly.
        assert!(!looks_like_table("a b\nc d"));
        assert!(looks_like_table("a b\nc d\ne f"));
    }

    #[test]
    fn a_single_column_list_is_not_a_table() {
        // Calling `ls` output tabular would offer a "show as table" that does
        // nothing.
        assert!(!looks_like_table("one\ntwo\nthree\nfour"));
    }

    #[test]
    fn ragged_output_is_plain() {
        let t = tracker_with("echo", "a b c\nd\ne f");
        assert_eq!(t.blocks()[0].detected_kind(), BlockKind::Plain);
    }

    #[test]
    fn runaway_output_is_bounded_and_keeps_the_tail() {
        // A `yes` or a verbose build produces gigabytes, and when something goes
        // wrong the error is at the END.
        let mut t = BlockTracker::default();
        t.submit_command("yes", 0);
        for _ in 0..40 { t.append_output(&"x".repeat(16 * 1024)); }
        t.append_output("THE ERROR");
        assert!(t.blocks()[0].output.len() <= MAX_OUTPUT_BYTES);
        assert!(t.blocks()[0].output.ends_with("THE ERROR"));
    }

    #[test]
    fn trimming_does_not_split_a_multi_byte_character() {
        // Build output is full of them, and slicing bytes blindly panics.
        let mut t = BlockTracker::default();
        t.submit_command("build", 0);
        for _ in 0..(MAX_OUTPUT_BYTES / 3 + 100) { t.append_output("→"); }
        assert!(t.blocks()[0].output.len() <= MAX_OUTPUT_BYTES);
        assert!(t.blocks()[0].output.starts_with('→'));
    }

    #[test]
    fn output_with_no_open_block_is_dropped_rather_than_panicking() {
        // Shell noise arrives before the first command and after the last.
        let mut t = BlockTracker::default();
        t.append_output("motd banner");
        assert!(t.is_empty());
    }
}
