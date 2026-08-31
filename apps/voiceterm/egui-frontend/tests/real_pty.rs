//! A real shell, through the real pipeline.
//!
//! Every other test of the terminal path uses a stand-in, and correctly:
//! a unit test cannot spawn a shell, and one that tried would be slow and
//! machine-dependent. But stand-ins share one blind spot — they produce
//! exactly what the test author expected — and driving an actual pty for
//! the first time found a defect none of them could:
//!
//! **A shell echoes what you type**, so every block held a copy of its own
//! command before its output. A fake prints only what it is told to.
//!
//! This test exists so that stops being a one-off. It spawns `/bin/sh`,
//! types three commands, and checks what came back through
//! `CommandLines`, `BlockTracker` and `Vt` — the same four pieces the app
//! wires together.
//!
//! ## It skips rather than fails where there is no pty
//!
//! A build machine without `/dev/ptmx`, or a sandbox that refuses to fork,
//! is not a broken terminal. A test that failed there would be turned off,
//! and a test that is off finds nothing.

#![cfg(unix)]

use std::time::{Duration, Instant};

use uikit_term::blocks::{BlockTracker, CommandLines};
use voiceterm_egui::pty_host::PtyHost;
use voiceterm_egui::term_grid::{TermGrid, Vt};

/// Everything the app holds for one terminal.
struct Rig {
    host: PtyHost,
    grid: TermGrid,
    vt: Vt,
    blocks: BlockTracker,
    lines: CommandLines,
}

impl Rig {
    fn start() -> Option<Rig> {
        let host = PtyHost::spawn(100, 30).ok()?;
        Some(Rig {
            host,
            grid: TermGrid::new(100, 30),
            vt: Vt::new(),
            blocks: BlockTracker::new(),
            lines: CommandLines::new(),
        })
    }

    /// Type, exactly as the app does: the bytes go to the shell AND
    /// through the line splitter, and a completed line becomes a block.
    fn type_line(&mut self, s: &str) {
        let _ = self.host.write(s.as_bytes());
        for cmd in self.lines.feed(s.as_bytes()) {
            self.blocks.submit_command(&cmd, 0);
        }
    }

    /// Drain whatever the shell said, into both the tracker and the grid.
    fn drain(&mut self) {
        let out = self.host.drain();
        if out.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        for line in text.split_inclusive(['\n', '\r']) {
            let clean = line.trim_end_matches(['\n', '\r']);
            if !clean.is_empty() {
                self.blocks.feed_line(clean);
            }
        }
        self.vt.feed(&mut self.grid, &out);
    }

    /// Wait for output to settle. Polled rather than slept once: a shell's
    /// timing varies with the machine, and a fixed sleep is either slow
    /// everywhere or flaky somewhere.
    fn settle(&mut self, ms: u64) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            self.drain();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn row(&self, y: usize) -> String {
        let mut s = String::new();
        for x in 0..self.grid.cols {
            if let Some(c) = self.grid.cell(x, y) {
                s.push(c.ch);
            }
        }
        s.trim_end().to_string()
    }

    fn screen(&self) -> String {
        (0..self.grid.rows).map(|y| self.row(y)).collect::<Vec<_>>().join("\n")
    }
}

#[test]
fn a_real_shell_produces_blocks_holding_only_their_own_output() {
    let Some(mut rig) = Rig::start() else {
        eprintln!("no pty here — skipping");
        return;
    };
    rig.settle(400); // the prompt

    rig.type_line("echo one\r");
    rig.settle(700);
    // `\\t` and `\\n` in the Rust literal, so the SHELL receives the
    // backslashes and printf interprets them. Written with real ones
    // first, which sent a newline mid-quote and left the shell sitting at
    // its continuation prompt — visible in the failure as a `>` line.
    rig.type_line("printf 'a\\tb\\n'\r");
    rig.settle(700);

    let cmds: Vec<&str> = rig.blocks.blocks.iter().map(|b| b.command.as_str()).collect();
    assert!(cmds.contains(&"echo one"), "commands recorded: {cmds:?}");

    let one = rig.blocks.blocks.iter().find(|b| b.command == "echo one")
        .expect("no block for the first command");
    // THE DEFECT THIS FILE EXISTS FOR. The shell echoes what you type, so
    // this used to read "echo one\none\n" — the command duplicated into
    // its own output.
    assert!(!one.output.starts_with("echo one"),
            "the shell's echo is in the output: {:?}", one.output);
    assert!(one.output.contains("one"), "the real output is missing: {:?}", one.output);
}

#[test]
fn colour_codes_from_a_real_program_leave_nothing_on_the_screen() {
    let Some(mut rig) = Rig::start() else {
        eprintln!("no pty here — skipping");
        return;
    };
    rig.settle(400);
    // `ls --color=always` emits SGR sequences and ends with a bare reset,
    // which is the case an empty-parameter reader gets wrong.
    rig.type_line("ls --color=always /etc/hostname\r");
    rig.settle(900);

    let screen = rig.screen();
    assert!(screen.contains("/etc/hostname"), "no output on screen:\n{screen}");
    assert!(!screen.contains("\u{1b}"), "an escape character reached the grid");
    assert!(!screen.contains("[0m") && !screen.contains("[01;"),
            "escape text was printed as characters:\n{screen}");
}

#[test]
fn a_tab_lands_on_its_column_stop() {
    let Some(mut rig) = Rig::start() else {
        eprintln!("no pty here — skipping");
        return;
    };
    rig.settle(400);
    // `\\t` and `\\n` in the Rust literal, so the SHELL receives the
    // backslashes and printf interprets them. Written with real ones
    // first, which sent a newline mid-quote and left the shell sitting at
    // its continuation prompt — visible in the failure as a `>` line.
    rig.type_line("printf 'a\\tb\\n'\r");
    rig.settle(700);

    let hit = (0..rig.grid.rows).map(|y| rig.row(y))
        .find(|r| r.starts_with('a') && r.contains('b') && !r.contains("printf"));
    let row = hit.expect(&format!("no output row:\n{}", rig.screen()));
    // Column 8, because a tab advances to the next multiple of eight.
    // Off by one here means every indented program output is misaligned.
    assert_eq!(row.find('b'), Some(8), "tab stop wrong in {row:?}");
}
