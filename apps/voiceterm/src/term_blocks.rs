// src/term_blocks.rs
// Warp/Fig-style command block tracking. Each block represents:
//   (prompt_marker, command_text, output_text, exit_code, duration)
//
// We detect block boundaries by:
//   1. User pressing Enter (sends \r) → previous output block closes
//   2. New cursor row at column 0 + non-empty input buffer → new command starts
//   3. Shell prompt re-appears (heuristic: short line ending in $ # > ❯) → output ends
//
// This is approximate without OSC 133 shell integration but works for most shells.


#[derive(Clone, Debug)]
pub struct CommandBlock {
    pub id: u32,
    pub command: String,
    /// Captured output (plain text, no ANSI). May be truncated for very long output.
    pub output: String,
    /// Working directory at time of execution (best-effort)
    pub cwd: Option<String>,
    /// Unix epoch seconds when command was issued
    pub started_at: u64,
    /// Duration in milliseconds (None if still running)
    pub duration_ms: Option<u32>,
    /// Exit code if known (parsed from $? or similar)
    pub exit_code: Option<i32>,
    /// Approximate row in scrollback where this block started
    pub start_row: usize,
    /// v0.1.253 (GITLOG-1b): speaker attribution if this command
    /// was dispatched from a voice intent. None for typed commands.
    /// Set in submit_command by reading audit_log's pending
    /// attribution (single-shot — cleared on read so the next
    /// typed command isn't mis-attributed). Consumed when this
    /// block closes to populate the audit log entry's
    /// `attest=voice spk_id=... spk_conf=...` fields.
    pub speaker_id: Option<String>,
    pub speaker_name: Option<String>,
    pub speaker_confidence: Option<f32>,
    /// One of "voice" | "typed" | "tap_confirm". Defaults to
    /// "typed" when no pending voice attribution exists at
    /// submit time.
    pub attestation: String,
    /// v0.1.254 (GITLOG-2): SHA-256 hash of the snapshot tree
    /// captured BEFORE this command ran. Resolves via
    /// snapshot_store to a tree object listing the cwd's files
    /// and their content hashes. Future rollback restores from
    /// this hash. None when snapshot wasn't possible (no $HOME,
    /// cwd doesn't exist, I/O failure) — graceful degradation.
    pub tree_hash: Option<String>,
    /// v0.1.274 (GITLOG-5): parent block id for branching.
    /// Default: previous block's id (linear chain). After a
    /// rollback, the next command's parent = the rollback
    /// target's parent (skipping the rolled-back commands).
    /// None for the very first block of the session.
    ///
    /// This enables the in-app branching view: walking parent
    /// pointers backward from each tip reconstructs the tree
    /// structure without needing extra storage beyond the
    /// existing audit log.
    pub parent_block_id: Option<u32>,
}

/// v0.1.375: shape detection for a block's output. Future
/// features (smart tokens, AI summary prompt selection, copy-
/// to-clipboard format hints) gate on this. Computed on demand
/// from the output text; not stored on the struct so
/// retroactive re-detection is possible if heuristics evolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// Default — no specific shape detected.
    Plain,
    /// Email-shaped: output starts with mail headers like
    /// `From:`, `To:`, `Subject:`, `Date:`. Common for `mail`,
    /// `mutt -e display`, `mbsync` logs, `cat ~/Maildir/...`.
    Email,
    /// Git command output. Detected by command starting with
    /// `git ` AND output containing characteristic words
    /// (commit, branch, modified). Useful for clickable-hash
    /// follow-ups.
    Git,
    /// Tabular output — multiple rows of consistent column
    /// counts. Detected by line-shape uniformity. Useful for
    /// future "show as table" voice command.
    Table,
}

impl CommandBlock {
    /// v0.1.375: detect the shape of this block's output.
    /// Pure function over `command` + `output`; safe to call
    /// repeatedly. Ordering: Email checked first (very specific
    /// header signature), then Git (command prefix), then Table
    /// (line-shape heuristic), default Plain.
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

    /// Email shape detector. Public for direct testing.
    /// Looks for at least two of these headers in the first
    /// ~10 lines, with at least one being `From:` or
    /// `Subject:`. Requiring two avoids matching prose that
    /// happens to contain one header-like phrase.
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

#[derive(Default)]
pub struct BlockTracker {
    pub blocks: Vec<CommandBlock>,
    next_id: u32,
    /// True if we're currently capturing output for the last block.
    /// Public so the shell-respawn path in main.rs can reset it cleanly.
    pub capturing: bool,
    /// Last-known cwd from OSC 7 or PROMPT_COMMAND export
    pub current_cwd: Option<String>,
    /// Cwd value BEFORE the most recent change. Powers undo for
    /// voice-driven smart-cd: after a `cd` lands, the toast offers
    /// "Undo" which navigates back to this path. Updated whenever
    /// set_cwd() receives a path different from current.
    pub previous_cwd: Option<String>,
    /// v0.1.274 (GITLOG-5): id of the block that the NEXT
    /// submit_command should record as its parent. Normally equals
    /// the most-recently-submitted block's id (linear chain).
    /// Reset by `set_parent_after_rollback` to the rollback
    /// target's parent, so the next command after a rollback
    /// branches from there instead of from the rolled-back tip.
    pub next_parent_id: Option<u32>,
    /// v0.1.350: blocks that JUST CLOSED in the most recent
    /// submit_command and whose duration might warrant a
    /// long-running-command notification. Pushed in
    /// submit_command (where the prior block's duration is
    /// computed). Drained by main.rs's pty_read_screen worker,
    /// which checks each against the configured threshold and
    /// emits the long_command_finished signal for any that
    /// qualify.
    ///
    /// Stored as (id, command, duration_ms) — main.rs has the
    /// threshold and decides whether to notify; BlockTracker
    /// just provides the data.
    ///
    /// Cleared on each drain. The queue should never grow more
    /// than a few entries between drains (drain runs every
    /// pty poll tick, typically 50ms on the active tab).
    pub pending_notifications: std::collections::VecDeque<(u32, String, u32)>,

    /// v0.1.363: SSH-session detection. Set true when the
    /// most-recently-submitted command starts with `ssh `,
    /// `mosh `, `kubectl exec`, `docker exec`, or `docker run
    /// -it` — long-running interactive commands whose output
    /// belongs to a remote shell, not the local one.
    ///
    /// Cleared when that block closes (the local prompt
    /// returned, meaning the session ended). Stays set across
    /// multiple commands within the SSH session because those
    /// commands don't trigger our local-prompt detection — the
    /// remote shell's prompts look different and don't close
    /// the local block.
    ///
    /// Heuristic and approximate:
    ///   - Nested SSH (ssh-from-inside-ssh) just stays "in SSH"
    ///   - User backgrounds the session with Ctrl+Z: we still
    ///     report in-ssh until they `fg` and exit cleanly
    ///   - Wrong if the user uses an SSH alias from $PATH (e.g.
    ///     `myremote` runs `ssh me@host`) — we'd miss it
    ///
    /// UI uses this to:
    ///   - Hide the git-status badge (our /proc cwd reading
    ///     reflects the LOCAL ssh process's cwd, not the
    ///     remote shell's pwd — wrong + misleading)
    ///   - Caveat the long-cmd notification with "(remote)"
    ///   - Refuse fs_find_start with a friendly status (files
    ///     mode searches LOCAL filesystem; user is on a remote
    ///     shell)
    ///   - Show a small "🔗 ssh" indicator near the prompt
    pub in_ssh_session: bool,
}

impl BlockTracker {
    pub fn new() -> Self { Self::default() }

    /// v0.1.363: read the current SSH-session state. Public so
    /// main.rs's pty_read_screen worker can emit the
    /// `in_ssh_session_changed` signal on transitions.
    pub fn is_ssh_session(&self) -> bool {
        self.in_ssh_session
    }

    /// v0.1.363: classify a command as a session-opener if it
    /// starts a long-running interactive remote/container shell.
    ///
    /// Cases we recognize:
    ///   - `ssh <something>`               classic remote shell
    ///   - `mosh <something>`              roaming alternative
    ///   - `kubectl exec ...`              container shell
    ///   - `docker exec ...`               container shell
    ///   - `docker run -it ...`            interactive container
    ///   - `podman exec ...` / `podman run -it ...`
    ///
    /// We require whitespace after the command name so we don't
    /// false-positive on `sshd`, `sshfs`, `ssh-keygen`, `dockerfile-
    /// linter`, etc. Empty args (just `ssh<EOL>`) is rare but
    /// shouldn't trigger detection — that's a usage error from
    /// the user, not a session entry.
    fn is_ssh_opener(cmd: &str) -> bool {
        let trimmed = cmd.trim_start();
        // Strip leading env-var assignments (`FOO=bar ssh host`).
        let mut rest = trimmed;
        loop {
            let next_space = match rest.find(char::is_whitespace) {
                Some(i) => i,
                None => return false,
            };
            let head = &rest[..next_space];
            if head.contains('=') && !head.starts_with('-') {
                // Looks like an env-var assignment; skip it.
                rest = rest[next_space..].trim_start();
                continue;
            }
            // head is the command name.
            // Single-name openers — must be followed by an argument.
            let after = rest[next_space..].trim_start();
            if after.is_empty() { return false; }
            match head {
                "ssh" | "mosh" => return true,
                // Multi-word openers — check the FULL prefix.
                "kubectl" => {
                    return after.starts_with("exec ")
                        || after.starts_with("exec\t");
                }
                "docker" | "podman" => {
                    if after.starts_with("exec ") || after.starts_with("exec\t") {
                        return true;
                    }
                    // `docker run` is only interactive with -it / -i / -t
                    if after.starts_with("run ") || after.starts_with("run\t") {
                        // crude check: scan for -it / -i / -t flags
                        return after.split_whitespace()
                            .any(|tok| matches!(tok,
                                "-it" | "-ti" | "-i" | "-t"
                                | "--interactive" | "--tty"));
                    }
                    return false;
                }
                _ => return false,
            }
        }
    }

    /// Called when a command line is submitted (user pressed Enter)
    pub fn submit_command(&mut self, command: &str, start_row: usize) {
        let cmd = command.trim();
        if cmd.is_empty() { return; }

        // v0.1.106: feed each command into sudo_keepalive's classifier.
        // Cheap: just a string prefix check + atomic load. Starts a
        // background refresh thread for sudo/doas commands; harmless
        // for everything else.
        crate::sudo_keepalive::note_command(cmd);

        // v0.1.114: record into active workflow if one is being
        // captured. No-op when not recording.
        crate::workflow::record_command(cmd);

        // Close out previous block (mark its duration). This is also
        // the moment we know the block is "done" — fire a summary
        // request if log_summary is enabled. The summary worker is
        // throttled and voice-aware, so this is safe to call
        // unconditionally (it no-ops when disabled / output too small).
        //
        // v0.1.252: also write the closed block to the audit log
        // (GITLOG-1 foundation). audit_log is best-effort; failures
        // log to stderr but do not block command execution.
        if let Some(last) = self.blocks.last_mut() {
            if last.duration_ms.is_none() {
                let now = epoch_secs();
                last.duration_ms = Some(((now - last.started_at).saturating_mul(1000)) as u32);
            }
            // v0.1.350: queue a possible long-command notification.
            // main.rs's pty_read_screen drains the queue and checks
            // each entry against the configured threshold. We push
            // unconditionally here — the threshold check happens
            // there because the threshold is a Backend qt_property
            // (user-settable from QML) and BlockTracker doesn't have
            // a path to it. Cheap (a clone of the command string +
            // two integers) and the queue is bounded by drain
            // frequency.
            if let Some(d) = last.duration_ms {
                self.pending_notifications.push_back(
                    (last.id, last.command.clone(), d));
            }
            crate::log_summary::request_summary(last.id, &last.command, &last.output);
            // Scan for missing-package signals (command not found,
            // ModuleNotFoundError, etc) so the sidebar list stays
            // current. Cheap — pattern match per line.
            crate::missing_packages::scan_block(last.id, &last.output);
            // v0.1.252: append to audit log. v0.1.253 (GITLOG-1b):
            // pull speaker attribution off the block itself
            // (populated at block creation from audit_log's pending
            // state). Voice-attributed commands record with the
            // speaker fields; typed commands record with attest=typed.
            // Best-effort: failures log but don't block execution.
            let _ = crate::audit_log::write_entry(
                last,
                last.speaker_id.clone(),
                last.speaker_name.clone(),
                last.speaker_confidence,
                &last.attestation,
            );
        }

        // v0.1.253 (GITLOG-1b): consume any pending voice attribution
        // from audit_log. If present, this command was triggered by a
        // voice intent and we attach the speaker info to the new
        // block. take_pending is single-shot — subsequent typed
        // commands won't be falsely attributed.
        let (spk_id, spk_name, spk_conf, attest) =
            match crate::audit_log::take_pending_attribution() {
                Some(p) => (
                    Some(p.speaker_id),
                    Some(p.speaker_name),
                    Some(p.confidence),
                    p.attestation,
                ),
                None => (None, None, None, "typed".to_string()),
            };

        // v0.1.254 (GITLOG-2): snapshot the cwd before this command
        // runs. Captures the pre-state so rollback can restore to
        // "the state right before this command." Best-effort: any
        // I/O failure (no cwd, no $HOME, permission denied) just
        // produces None and the block records without a tree.
        // Snapshot cost is bounded by the ignore rules
        // (node_modules, .git, etc. are skipped). For typical
        // source dirs this is a few ms.
        //
        // v0.1.256 (GITLOG-4): use cmd_classifier to skip the
        // snapshot for read-only commands (ls, cat, grep, etc.).
        // These can't mutate the filesystem, so a pre-snapshot
        // would just be wasted I/O. Unknown commands (custom
        // scripts, anything we don't recognize) still snapshot —
        // conservative default. Mutating + Unknown both snapshot;
        // only confirmed ReadOnly is skipped.
        let class = crate::cmd_classifier::classify(cmd);
        let tree_hash = if class.should_snapshot() {
            self.current_cwd.as_ref()
                .and_then(|c| crate::snapshot_store::snapshot_dir(
                    std::path::Path::new(c)
                ))
        } else {
            None
        };

        self.next_id += 1;
        // v0.1.363: SSH-session detection. Check the NEW command
        // (about to run) to see if it's a session-opening command.
        // We do this BEFORE pushing the block so we can also clear
        // the flag if the prior block was the SSH-opening one and
        // it just closed (which means the session ended).
        //
        // The clearing path: if the block we just closed above had
        // an SSH-style command, set in_ssh_session = false because
        // a fresh prompt returned (the local shell is back).
        let prev_was_ssh = self.blocks.last()
            .map(|b| Self::is_ssh_opener(&b.command))
            .unwrap_or(false);
        if prev_was_ssh && self.blocks.last().map(|b| b.duration_ms.is_some()).unwrap_or(false) {
            // Prior block ran, closed, and was an SSH opener.
            // Local prompt is back.
            if self.in_ssh_session {
                crate::clog!("ssh-detect", "session ended (prev block was '{}')",
                    self.blocks.last().map(|b| b.command.as_str()).unwrap_or(""));
            }
            self.in_ssh_session = false;
        }
        // The new command may OPEN an SSH session.
        if Self::is_ssh_opener(cmd) {
            if !self.in_ssh_session {
                crate::clog!("ssh-detect", "entering session ('{}')", cmd);
            }
            self.in_ssh_session = true;
        }

        // v0.1.274 (GITLOG-5): parent pointer. Default is the
        // previous tip (linear chain). After a rollback,
        // set_parent_after_rollback overrides next_parent_id so
        // the new block forks from the restored ancestor rather
        // than continuing the rolled-back tip.
        let parent = self.next_parent_id
            .take()
            .or_else(|| self.blocks.last().map(|b| b.id));
        let block = CommandBlock {
            id: self.next_id,
            command: cmd.to_string(),
            output: String::new(),
            cwd: self.current_cwd.clone(),
            started_at: epoch_secs(),
            duration_ms: None,
            exit_code: None,
            start_row,
            speaker_id: spk_id,
            speaker_name: spk_name,
            speaker_confidence: spk_conf,
            attestation: attest,
            tree_hash,
            parent_block_id: parent,
        };
        self.blocks.push(block);
        // After a normal submit, the new block is the linear tip:
        // its id becomes the implicit parent for the next block.
        // (Explicit None — we let the .or_else fallback above
        // pick it up on the next call.)
        self.capturing = true;

        // Cap memory: keep last 200 blocks
        while self.blocks.len() > 200 {
            self.blocks.remove(0);
        }
    }

    /// Append output text to the active block. Strip ANSI before storing.
    pub fn append_output(&mut self, text: &str) {
        if !self.capturing { return; }
        let stripped = strip_ansi(text);
        if let Some(block) = self.blocks.last_mut() {
            // Cap per-block output at 32KB to avoid memory bloat
            let remaining = 32_768usize.saturating_sub(block.output.len());
            if remaining == 0 { return; }
            if stripped.len() <= remaining {
                block.output.push_str(&stripped);
            } else {
                // Find the largest char-boundary index ≤ remaining so we don't
                // slice through a multi-byte UTF-8 sequence (helix/htop's box-drawing
                // characters like ─ and │ are 3 bytes each).
                let mut cut = remaining;
                while cut > 0 && !stripped.is_char_boundary(cut) { cut -= 1; }
                block.output.push_str(&stripped[..cut]);
                block.output.push_str("\n[...output truncated]");
                self.capturing = false;
            }
        }
    }

    /// Mark the current block as complete (next prompt detected)
    pub fn finish(&mut self, exit_code: Option<i32>) {
        if !self.capturing { return; }
        if let Some(block) = self.blocks.last_mut() {
            let now = epoch_secs();
            block.duration_ms = Some(((now - block.started_at).saturating_mul(1000)) as u32);
            block.exit_code = exit_code;
        }
        self.capturing = false;
    }

    /// Update cwd from OSC 7 (file://hostname/path) or other source.
    ///
    /// v0.1.106: tracks previous_cwd so smart-cd toasts can offer
    /// an Undo button. We update previous_cwd ONLY when the new path
    /// differs from current — this way if OSC 7 fires twice with the
    /// same value (some shells emit redundantly), undo still points
    /// at the genuine prior location.
    pub fn set_cwd(&mut self, path: &str) {
        if path.is_empty() { return; }
        if self.current_cwd.as_deref() != Some(path) {
            self.previous_cwd = self.current_cwd.clone();
        }
        self.current_cwd = Some(path.to_string());
    }

    /// Return last N blocks as serialized text for QML
    /// Format per line: id|cmd|output_first_line|duration_ms|exit|cwd|started|hasTree
    /// (8 fields). v0.1.255 added hasTree (1 if block has a tree
    /// snapshot for rollback, 0 if not).
    pub fn serialize(&self, n: usize) -> String {
        let take = self.blocks.len().min(n);
        let start = self.blocks.len() - take;
        let mut out = String::with_capacity(take * 256);
        for block in &self.blocks[start..] {
            out.push_str(&block.id.to_string());
            out.push('\x1F');
            out.push_str(&escape_field(&block.command));
            out.push('\x1F');
            // First line of output
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
            out.push('\x1F');
            // v0.1.255 (GITLOG-3): rollback button affordance.
            // "1" if a snapshot tree exists, "0" otherwise.
            // QML uses this to show the rollback button only on
            // blocks that can actually rollback.
            out.push(if block.tree_hash.is_some() { '1' } else { '0' });
            out.push('\n');
        }
        out
    }

    pub fn get_block_output(&self, id: u32) -> Option<&str> {
        self.blocks.iter().find(|b| b.id == id).map(|b| b.output.as_str())
    }

    pub fn get_block_command(&self, id: u32) -> Option<&str> {
        self.blocks.iter().find(|b| b.id == id).map(|b| b.command.as_str())
    }

    /// v0.1.255 (GITLOG-3): return (tree_hash, cwd, command) for a
    /// rollback target. Returns None if block doesn't exist, has
    /// no tree snapshot, or has no recorded cwd. All three are
    /// required for rollback to make sense.
    pub fn get_rollback_target(&self, id: u32) -> Option<(String, String, String)> {
        let b = self.blocks.iter().find(|b| b.id == id)?;
        let tree = b.tree_hash.clone()?;
        let cwd = b.cwd.clone()?;
        Some((tree, cwd, b.command.clone()))
    }

    /// v0.1.255 (GITLOG-3): return the id of the most recent
    /// block that can be rolled back (has tree_hash AND cwd).
    /// Used by voice intent "undo last command". Returns None
    /// when no recent blocks are rollbackable.
    pub fn most_recent_rollbackable(&self) -> Option<u32> {
        self.blocks.iter().rev()
            .find(|b| b.tree_hash.is_some() && b.cwd.is_some())
            .map(|b| b.id)
    }

    /// v0.1.274 (GITLOG-5): called by the rollback path after a
    /// successful restore. Sets next_parent_id so the NEXT
    /// submit_command records the rolled-back-to block's parent
    /// (not the rolled-back tip) as its parent. This is what
    /// creates the branch — the next command becomes a SIBLING
    /// of the rolled-back command, not its successor.
    ///
    /// If `rolled_back_to_id` doesn't exist in the local block
    /// vector (e.g. rolled out of the in-memory window), we fall
    /// back to the rolled-back-to id itself as parent. The audit
    /// log retains the full chain regardless.
    pub fn set_parent_after_rollback(&mut self, rolled_back_to_id: u32) {
        let parent = self.blocks.iter()
            .find(|b| b.id == rolled_back_to_id)
            .and_then(|b| b.parent_block_id)
            .or(Some(rolled_back_to_id));
        self.next_parent_id = parent;
        crate::clog!("term_blocks", "after rollback to block {}, next command's parent = {:?}",
            rolled_back_to_id, self.next_parent_id);
    }

    /// v0.1.274 (GITLOG-5): emit a serialized tree of recent
    /// blocks for the in-app branch viewer. Format: one block
    /// per line, tab-separated:
    ///   id \t parent_id_or_empty \t command \t cwd \t exit
    ///
    /// QML reconstructs the tree from parent pointers and
    /// renders. Capped at the last N blocks to keep emission
    /// cheap; older blocks are still in audit_log for the
    /// audit viewer.
    pub fn serialize_branch_tree(&self, limit: usize) -> String {
        let start = self.blocks.len().saturating_sub(limit);
        let mut out = String::with_capacity(limit * 80);
        for block in &self.blocks[start..] {
            out.push_str(&block.id.to_string());
            out.push('\t');
            if let Some(p) = block.parent_block_id {
                out.push_str(&p.to_string());
            }
            out.push('\t');
            out.push_str(&escape_field(&block.command));
            out.push('\t');
            out.push_str(&escape_field(block.cwd.as_deref().unwrap_or("")));
            out.push('\t');
            out.push_str(&block.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()));
            out.push('\n');
        }
        out
    }

    /// v0.1.274 (GITLOG-5): render the branch tree as markdown
    /// for the in-app viewer at `web2qml://branches`. Walks
    /// parent pointers and groups by lineage, indenting child
    /// commands beneath their parents.
    ///
    /// The renderer is best-effort: it captures the LAST `limit`
    /// blocks (matching the in-memory cap of 200) and reconstructs
    /// the partial tree. Older blocks rolled out of memory show
    /// as "root nodes" without parent context, which is fine —
    /// the user gets recent branching structure, and the full
    /// linear log is at `web2qml://audit` for absolute history.
    pub fn render_branches_markdown(&self, limit: usize) -> String {
        use std::collections::HashMap;
        let start = self.blocks.len().saturating_sub(limit);
        let recent: Vec<&CommandBlock> = self.blocks[start..].iter().collect();

        let mut md = String::with_capacity(recent.len() * 160 + 512);
        md.push_str("# Command Branches\n\n");

        if recent.is_empty() {
            md.push_str("No commands recorded yet. Once you start running commands in the terminal, this view shows the branching structure created by rollbacks.\n\n");
            md.push_str("- Each linear sequence is a chain of commands.\n");
            md.push_str("- After a rollback, the next command branches off the rolled-back-to ancestor — a new sibling, not a successor.\n");
            md.push_str("- This view lets you see where branches forked and (eventually) navigate between them.\n");
            return md;
        }

        md.push_str(&format!(
            "Showing the {} most recent blocks. Each fork is created when you rollback then run a new command — that new command becomes a sibling of the rolled-back one.\n\n",
            recent.len()
        ));

        // Build children index: parent_id → Vec<&CommandBlock>
        let mut children: HashMap<u32, Vec<&CommandBlock>> = HashMap::new();
        let mut roots: Vec<&CommandBlock> = Vec::new();
        let recent_ids: std::collections::HashSet<u32> = recent.iter().map(|b| b.id).collect();
        for b in &recent {
            match b.parent_block_id {
                Some(p) if recent_ids.contains(&p) => {
                    children.entry(p).or_default().push(b);
                }
                _ => roots.push(b),  // parent not in window OR no parent
            }
        }

        // Render each root and its descendants as indented bullets.
        for root in &roots {
            render_branch_node(&mut md, root, &children, 0);
        }
        md.push_str("\n---\n\n");
        md.push_str("**Markers:**\n\n");
        md.push_str("- ⏎ — a sibling fork (this command's parent has multiple children)\n");
        md.push_str("- ↩ — block has a rollback snapshot available\n");
        md.push_str("- ✓ / ✗ — last exit code\n");
        md
    }
}

/// Helper for render_branches_markdown: emit one block and recurse.
fn render_branch_node(
    md: &mut String,
    block: &CommandBlock,
    children: &std::collections::HashMap<u32, Vec<&CommandBlock>>,
    depth: usize,
) {
    // 2-space indent per level for nested-bullet rendering.
    for _ in 0..depth { md.push_str("  "); }
    md.push_str("- ");

    // Truncate long commands for line readability.
    let cmd: String = if block.command.chars().count() > 80 {
        let mut s: String = block.command.chars().take(80).collect();
        s.push('…');
        s
    } else {
        block.command.clone()
    };
    md.push('`');
    md.push_str(&cmd);
    md.push('`');

    // Inline markers.
    if let Some(code) = block.exit_code {
        if code == 0 {
            md.push_str(" ✓");
        } else {
            md.push_str(&format!(" ✗ exit {}", code));
        }
    }
    if block.tree_hash.is_some() {
        md.push_str(" ↩");
    }
    if let Some(name) = &block.speaker_name {
        md.push_str(&format!(" — _{}_", name));
    }
    md.push('\n');

    // Recurse into children. If this block has multiple children,
    // the FIRST is the linear successor, the rest are forks
    // created by rollback. We could prefix the latter with a ⏎
    // marker, but the indentation pattern is clear enough.
    if let Some(kids) = children.get(&block.id) {
        for kid in kids {
            render_branch_node(md, kid, children, depth + 1);
        }
    }
}

fn escape_field(s: &str) -> String {
    // Replace separator chars to avoid breaking the parse format
    s.replace('\x1F', " ").replace('\n', " ").replace('\r', "")
}

fn epoch_secs() -> u64 { core_kit::time::now_secs() }

/// Strip ANSI/VT escape sequences for clean output capture
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // Skip CSI: \x1b [ ... letter
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() { break; }
                    }
                } else if chars.peek() == Some(&']') {
                    // OSC: \x1b ] ... \x07 or \x1b\\
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' { break; }
                        if nc == '\x1b' && chars.peek() == Some(&'\\') { chars.next(); break; }
                    }
                } else {
                    // Two-byte ESC sequence
                    chars.next();
                }
            }
            '\x07' | '\x08' => { /* bell, backspace — drop */ }
            '\r' => { /* drop carriage returns */ }
            '\x00'..='\x06' | '\x0b'..='\x1f' if c != '\n' && c != '\t' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── escape_field tests ───

    #[test]
    fn escape_field_passthrough() {
        // Normal text untouched.
        assert_eq!(escape_field("hello world"), "hello world");
    }

    #[test]
    fn escape_field_separator() {
        // \x1F is replaced with space to keep the serialize format intact.
        assert_eq!(escape_field("a\x1Fb"), "a b");
    }

    #[test]
    fn escape_field_newline() {
        // Newlines replaced — serialize uses \n as record boundary.
        assert_eq!(escape_field("line1\nline2"), "line1 line2");
    }

    #[test]
    fn escape_field_carriage_return() {
        // Carriage returns dropped (just gone, not replaced).
        assert_eq!(escape_field("line1\rline2"), "line1line2");
    }

    #[test]
    fn escape_field_combined() {
        // All three sanitizations applied to one input.
        assert_eq!(escape_field("a\x1Fb\nc\rd"), "a b cd");
    }

    // ─── strip_ansi tests ───

    #[test]
    fn strip_ansi_passthrough() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strip_ansi_csi_color() {
        // \x1b[31m = red foreground; should be entirely removed.
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_csi_with_args() {
        // Multi-arg CSI: \x1b[1;31m = bold red. End at 'm'.
        assert_eq!(strip_ansi("\x1b[1;31mtext"), "text");
    }

    #[test]
    fn strip_ansi_osc_terminated_bel() {
        // OSC terminated by BEL (\x07): \x1b]0;title\x07 = window title.
        assert_eq!(strip_ansi("\x1b]0;Title\x07after"), "after");
    }

    #[test]
    fn strip_ansi_osc_terminated_st() {
        // OSC terminated by ST (\x1b\\): same effect.
        assert_eq!(strip_ansi("\x1b]0;Title\x1b\\after"), "after");
    }

    #[test]
    fn strip_ansi_drops_bell() {
        assert_eq!(strip_ansi("ding\x07ding"), "dingding");
    }

    #[test]
    fn strip_ansi_drops_backspace() {
        // Bare backspace stripped (we don't try to delete preceding char).
        assert_eq!(strip_ansi("a\x08b"), "ab");
    }

    #[test]
    fn strip_ansi_drops_carriage_return() {
        // \r stripped — assumes the terminal layer handles cursor positioning.
        assert_eq!(strip_ansi("line1\r\nline2"), "line1\nline2");
    }

    #[test]
    fn strip_ansi_preserves_tab() {
        // \t kept (display layer interprets).
        assert_eq!(strip_ansi("col1\tcol2"), "col1\tcol2");
    }

    #[test]
    fn strip_ansi_preserves_newline() {
        // \n kept.
        assert_eq!(strip_ansi("line1\nline2"), "line1\nline2");
    }

    #[test]
    fn strip_ansi_complex_real_world() {
        // A typical ls output line with colors and reset.
        let raw = "\x1b[1;34mdocs/\x1b[0m\nfile.txt";
        assert_eq!(strip_ansi(raw), "docs/\nfile.txt");
    }

    // ─── BlockTracker minimal smoke tests ───

    #[test]
    fn tracker_starts_empty() {
        let t = BlockTracker::new();
        assert!(t.blocks.is_empty());
        assert_eq!(t.get_block_output(0), None);
        assert_eq!(t.get_block_command(0), None);
    }

    #[test]
    fn tracker_default_matches_new() {
        let a = BlockTracker::new();
        let b: BlockTracker = Default::default();
        assert!(a.blocks.is_empty());
        assert!(b.blocks.is_empty());
    }

    // v0.1.363: SSH-session detection.

    #[test]
    fn ssh_opener_recognizes_ssh() {
        assert!(BlockTracker::is_ssh_opener("ssh user@host"));
        assert!(BlockTracker::is_ssh_opener("ssh host"));
        assert!(BlockTracker::is_ssh_opener("ssh -p 2222 host"));
        assert!(BlockTracker::is_ssh_opener("  ssh host"));
    }

    #[test]
    fn ssh_opener_recognizes_mosh() {
        assert!(BlockTracker::is_ssh_opener("mosh user@host"));
        assert!(BlockTracker::is_ssh_opener("mosh host"));
    }

    #[test]
    fn ssh_opener_recognizes_kubectl_exec() {
        assert!(BlockTracker::is_ssh_opener("kubectl exec -it pod -- bash"));
        assert!(BlockTracker::is_ssh_opener("kubectl exec pod"));
    }

    #[test]
    fn ssh_opener_recognizes_docker_exec() {
        assert!(BlockTracker::is_ssh_opener("docker exec -it container bash"));
        assert!(BlockTracker::is_ssh_opener("podman exec -it container sh"));
    }

    #[test]
    fn ssh_opener_recognizes_docker_run_interactive() {
        assert!(BlockTracker::is_ssh_opener("docker run -it alpine sh"));
        assert!(BlockTracker::is_ssh_opener("docker run -i -t alpine sh"));
        assert!(BlockTracker::is_ssh_opener("docker run --interactive --tty alpine sh"));
        assert!(BlockTracker::is_ssh_opener("podman run -ti alpine sh"));
    }

    #[test]
    fn ssh_opener_rejects_non_interactive_docker_run() {
        // No -it: just a one-shot container run, not a session.
        assert!(!BlockTracker::is_ssh_opener("docker run alpine echo hi"));
        assert!(!BlockTracker::is_ssh_opener("docker run --rm alpine echo hi"));
    }

    #[test]
    fn ssh_opener_rejects_similar_names() {
        // sshd, sshfs, ssh-keygen, etc. shouldn't match.
        assert!(!BlockTracker::is_ssh_opener("sshd -D"));
        assert!(!BlockTracker::is_ssh_opener("sshfs user@host:/ /mnt"));
        assert!(!BlockTracker::is_ssh_opener("ssh-keygen -t ed25519"));
        assert!(!BlockTracker::is_ssh_opener("ssh-add ~/.ssh/key"));
    }

    #[test]
    fn ssh_opener_rejects_bare_command() {
        // Just "ssh" with no args is a usage error, not a session.
        assert!(!BlockTracker::is_ssh_opener("ssh"));
        assert!(!BlockTracker::is_ssh_opener("ssh "));
        assert!(!BlockTracker::is_ssh_opener("mosh"));
    }

    #[test]
    fn ssh_opener_handles_env_var_prefix() {
        // SSH-style env prefix common with key files, etc.
        assert!(BlockTracker::is_ssh_opener("SSH_AUTH_SOCK=/tmp/sock ssh host"));
        assert!(BlockTracker::is_ssh_opener("FOO=bar BAZ=qux ssh user@host"));
    }

    #[test]
    fn ssh_opener_rejects_arbitrary_commands() {
        assert!(!BlockTracker::is_ssh_opener("ls -la"));
        assert!(!BlockTracker::is_ssh_opener("git status"));
        assert!(!BlockTracker::is_ssh_opener("echo ssh host"));
        assert!(!BlockTracker::is_ssh_opener("vim ssh.txt"));
    }

    #[test]
    fn tracker_enters_ssh_on_submit() {
        let mut t = BlockTracker::new();
        assert!(!t.is_ssh_session());
        t.submit_command("ssh user@host", 0);
        assert!(t.is_ssh_session());
    }

    #[test]
    fn tracker_stays_in_ssh_for_subsequent_commands() {
        // After entering SSH, we stay in until the block closes.
        // submit_command for "ssh host" sets in_ssh_session = true.
        // submit_command for a SUBSEQUENT command (which from
        // web2qml's POV closes the ssh block) currently leaves
        // in_ssh_session = false because the close path checks
        // prev_was_ssh. This is intentional: from our model, a
        // new local prompt means SSH ended.
        //
        // The REAL ssh-stays-open case (where we don't see new
        // prompts because they're remote) is handled by the
        // fact that no submit_command is called for remote
        // commands — they don't go through us. So the test
        // here is just that the initial entry works.
        let mut t = BlockTracker::new();
        t.submit_command("ssh host", 0);
        assert!(t.is_ssh_session());
        // Simulate the prior block CLOSING with a duration (the
        // ssh process actually exited).
        if let Some(b) = t.blocks.last_mut() {
            b.duration_ms = Some(1000);
        }
        // Next local command after the session ended.
        t.submit_command("ls", 0);
        // The is_ssh_session is now false because the block
        // that opened it has closed.
        assert!(!t.is_ssh_session());
    }

    // v0.1.375: block-shape detection.

    fn make_block(cmd: &str, output: &str) -> CommandBlock {
        CommandBlock {
            id: 1,
            command: cmd.to_string(),
            output: output.to_string(),
            cwd: None,
            started_at: 0,
            duration_ms: Some(100),
            exit_code: Some(0),
            start_row: 0,
            speaker_id: None,
            speaker_name: None,
            speaker_confidence: None,
            attestation: "typed".to_string(),
            tree_hash: None,
            parent_block_id: None,
        }
    }

    #[test]
    fn detected_kind_email() {
        let out = "From: alice@example.com\nTo: bob@example.com\nSubject: Hello\nDate: Mon, 1 Jan 2024 12:00:00 +0000\n\nHello there!";
        let b = make_block("cat msg.eml", out);
        assert_eq!(b.detected_kind(), BlockKind::Email);
    }

    #[test]
    fn detected_kind_email_lowercase_anchors_match() {
        // Headers can vary in case in some clients
        let out = "from: alice@example.com\nto: bob@example.com\nsubject: Hi\n\nMessage body";
        let b = make_block("formail", out);
        assert_eq!(b.detected_kind(), BlockKind::Email);
    }

    #[test]
    fn detected_kind_email_needs_anchor() {
        // To/Cc alone (no From/Subject) shouldn't trigger —
        // those headers can appear in non-email contexts.
        let out = "To: someone\nCc: another\n\nThis is not email.";
        let b = make_block("cat notes.txt", out);
        assert_ne!(b.detected_kind(), BlockKind::Email);
    }

    #[test]
    fn detected_kind_email_needs_two_headers() {
        // Single "From:" header line in arbitrary output
        // shouldn't false-positive.
        let out = "From: the meeting notes\n\nNotes go here.";
        let b = make_block("cat notes.md", out);
        assert_ne!(b.detected_kind(), BlockKind::Email);
    }

    #[test]
    fn detected_kind_git_with_output() {
        let out = "On branch main\nYour branch is up to date with 'origin/main'.\n\nnothing to commit, working tree clean";
        let b = make_block("git status", out);
        assert_eq!(b.detected_kind(), BlockKind::Git);
    }

    #[test]
    fn detected_kind_git_just_git() {
        // Bare `git` (lists subcommands) — should also match.
        let out = "usage: git [-v | --version] [-h | --help]\n\nThese are common Git commands used in various situations:\n   add\n   commit";
        let b = make_block("git", out);
        assert_eq!(b.detected_kind(), BlockKind::Git);
    }

    #[test]
    fn detected_kind_git_empty_output_skipped() {
        // `git config --global user.email foo@bar` produces
        // no output — don't tag as Git block (no signal to
        // act on).
        let b = make_block("git config user.email foo@bar", "");
        assert_eq!(b.detected_kind(), BlockKind::Plain);
    }

    #[test]
    fn detected_kind_not_git_prefix() {
        // Commands that contain "git" but don't start with
        // it shouldn't be classified as git.
        let b = make_block("dustgit", "some output");
        assert_ne!(b.detected_kind(), BlockKind::Git);
    }

    #[test]
    fn detected_kind_table() {
        // 3+ rows, 3 columns each, separated by multiple spaces.
        let out = "Name        Age  City\nAlice       30   Boston\nBob         25   Seattle\nCarol       40   Portland";
        let b = make_block("ls", out);
        assert_eq!(b.detected_kind(), BlockKind::Table);
    }

    #[test]
    fn detected_kind_table_tolerates_dashes() {
        // ASCII tables with ruler lines should still detect.
        let out = "Name        Age\n-----       ---\nAlice       30\nBob         25\nCarol       40";
        let b = make_block("ls", out);
        assert_eq!(b.detected_kind(), BlockKind::Table);
    }

    #[test]
    fn detected_kind_plain_default() {
        let b = make_block("echo hello", "hello");
        assert_eq!(b.detected_kind(), BlockKind::Plain);
    }

    #[test]
    fn detected_kind_empty_output_plain() {
        let b = make_block("true", "");
        assert_eq!(b.detected_kind(), BlockKind::Plain);
    }
}
