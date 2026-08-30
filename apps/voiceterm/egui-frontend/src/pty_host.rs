//! pty_host — the missing half of the terminal: an actual shell.
//!
//! ## What was wrong
//!
//! Every other piece was here and correct. `pty_input::drain_egui`
//! encodes keystrokes (arrows, Ctrl-C, Backspace, Alt word motions) into
//! the right bytes. `Session::pty_out` collects them. `term_grid` parses
//! VT/ANSI into a screen and draws it. What did not exist anywhere in
//! this crate was a PROCESS: no `openpty`, no `CommandBuilder`, no
//! `spawn`. The bytes were encoded perfectly and written to a `Vec<u8>`
//! that nothing read, and nothing ever wrote bytes back.
//!
//! So "keys do not appear in the terminal and commands do not run" was
//! not one bug. It was the absence of the component that would have made
//! either possible, behind a UI complete enough to look like it should
//! work.
//!
//! ## Why a separate module rather than a method on the app
//!
//! The Qt side's `pty_term.rs` is 2,944 lines because it also holds the
//! grid, the parser, block tracking and shell-completion capture. This
//! port already has those elsewhere, so what is left is small: spawn,
//! two pipes, resize, reap. Keeping it separate means the app owns no
//! threads and no file descriptors directly, and the whole thing can be
//! absent — `Option<PtyHost>` — on a build or platform without one.
//!
//! ## Threading
//!
//! One reader thread, because a blocking read on the master fd is the
//! only way to notice output promptly and `poll` in the UI loop would
//! either spin or lag. It appends to a shared buffer the UI drains each
//! frame; the UI never blocks on the child.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// A running shell attached to a pty.
pub struct PtyHost {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Bytes the shell has produced and the UI has not yet drained.
    incoming: Arc<Mutex<Vec<u8>>>,
    /// Set by the reader thread when the child's output ends, which is
    /// how the UI learns the shell exited without reaping it itself.
    ended: Arc<std::sync::atomic::AtomicBool>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// What we last told the pty, so `resize` can skip no-op ioctls —
    /// a resize on every frame would send SIGWINCH to the shell
    /// continuously and make full-screen programs redraw forever.
    size: (u16, u16),
}

impl PtyHost {
    /// Spawn the user's login shell.
    ///
    /// `SHELL` with `/bin/sh` as the fallback, matching the Qt side. `-l`
    /// so the shell reads its profile: a terminal whose PATH differs from
    /// every other terminal on the machine is a subtle, lasting
    /// annoyance.
    ///
    /// `TERM=xterm-256color` because that is what this grid actually
    /// implements. Claiming more (`xterm-kitty`, say) invites programs to
    /// send sequences the parser will render as garbage.
    pub fn spawn(cols: u16, rows: u16) -> Result<PtyHost, String> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        let sys = native_pty_system();
        let pair = sys
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("openpty: {e}"))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("spawn {shell}: {e}"))?;
        // Drop our copy of the slave: while any handle to it is open the
        // reader never sees EOF, so the terminal would look alive
        // forever after the shell exits.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().map_err(|e| format!("reader: {e}"))?;
        let writer = pair.master.take_writer().map_err(|e| format!("writer: {e}"))?;

        let incoming = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let incoming = Arc::clone(&incoming);
            let ended = Arc::clone(&ended);
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            ended.store(true, std::sync::atomic::Ordering::SeqCst);
                            return;
                        }
                        Ok(n) => {
                            // Poison ignored: if the UI thread panicked
                            // while holding this, the reader carrying on
                            // is harmless and the alternative is losing
                            // the shell's output too.
                            let mut g = match incoming.lock() {
                                Ok(g) => g,
                                Err(e) => e.into_inner(),
                            };
                            g.extend_from_slice(&buf[..n]);
                        }
                    }
                }
            });
        }

        Ok(PtyHost {
            writer: Arc::new(Mutex::new(writer)),
            incoming,
            ended,
            master: pair.master,
            child,
            size: (cols, rows),
        })
    }

    /// Send bytes to the shell. This is where `Session::pty_out` goes.
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut w = match self.writer.lock() {
            Ok(w) => w,
            Err(e) => e.into_inner(),
        };
        w.write_all(bytes).map_err(|e| format!("write: {e}"))?;
        // Flushed every time. A keystroke that sits in a buffer waiting
        // for more is indistinguishable from a terminal that ignored it,
        // which is the exact symptom being fixed.
        w.flush().map_err(|e| format!("flush: {e}"))
    }

    /// Take everything the shell has produced since the last call.
    ///
    /// Drains rather than copies, so output is delivered exactly once and
    /// the buffer cannot grow without bound while a long build runs.
    pub fn drain(&self) -> Vec<u8> {
        let mut g = match self.incoming.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        std::mem::take(&mut *g)
    }

    /// Tell the pty its new size, if it changed.
    ///
    /// Skipping the no-op matters: a resize every frame sends SIGWINCH
    /// continuously, and a full-screen program would redraw forever
    /// instead of drawing once.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == self.size || cols == 0 || rows == 0 {
            return;
        }
        self.size = (cols, rows);
        let _ = self.master.resize(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Has the shell exited?
    ///
    /// Reads the reader thread's flag rather than calling `try_wait`
    /// every frame: EOF on the master is the earliest signal, and polling
    /// the child adds a syscall per frame to learn the same thing later.
    pub fn ended(&self) -> bool {
        self.ended.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Reap the child, so a closed pane does not leave a zombie.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PtyHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point, end to end: type a command, get its output.
    ///
    /// Uses `sh` directly rather than the login shell so the test does
    /// not depend on the developer's profile, and looks for a marker
    /// rather than an exact transcript because a prompt is unpredictable.
    #[test]
    #[ignore = "spawns a real shell; run explicitly"]
    fn a_command_runs_and_its_output_comes_back() {
        std::env::set_var("SHELL", "/bin/sh");
        let host = PtyHost::spawn(80, 24).expect("spawn");
        host.write(b"echo VOICETERM_MARKER\n").expect("write");
        let mut seen = String::new();
        for _ in 0..100 {
            seen.push_str(&String::from_utf8_lossy(&host.drain()));
            if seen.contains("VOICETERM_MARKER") {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("no output from the shell in 5s; got {seen:?}");
    }

    #[test]
    #[ignore = "spawns a real shell; run explicitly"]
    fn an_exited_shell_is_reported_rather_than_looking_alive() {
        std::env::set_var("SHELL", "/bin/sh");
        let host = PtyHost::spawn(80, 24).expect("spawn");
        host.write(b"exit\n").expect("write");
        for _ in 0..100 {
            if host.ended() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the shell exited and the host never noticed");
    }
}
