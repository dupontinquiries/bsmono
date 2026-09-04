//! lsp_worker — the concurrency wrapper `aperture_lsp`'s own doc comment
//! points at: `LspClient`'s methods are blocking (deliberately, until a real
//! caller existed to prove which calls are actually slow); this is that
//! real caller, and it answers the question the same way `aperture_complete
//! ::engine::Engine` already did for AI completions — one worker thread,
//! one inbox, one outbox, newest-request-wins for completions specifically
//! (an LSP round trip a person has already kept typing past is not worth
//! showing).
//!
//! Optional and best-effort throughout: no `rust-analyzer` on `PATH`, or a
//! language with no server configured, means `spawn` returns `Err` and the
//! shell simply doesn't get this tier — `workspace_symbols`'s local outline
//! stays the popup's baseline either way (see `aperture_lsp`'s crate doc for
//! why LSP is an ADDITIONAL tier, not a replacement).

use aperture_lsp::{CompletionItem, Diagnostic, LspClient};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

enum Msg {
    Open { uri: String, text: String },
    Change { uri: String, text: String, version: i64 },
    Complete { uri: String, line: u32, character: u32, generation: u64 },
}

/// A completed (or failed-and-empty) completion request, tagged with the
/// generation it was requested under so a caller can discard a stale answer
/// the same way `aperture_complete::CompletionOutcome` already does.
pub struct LspOutcome {
    pub generation: u64,
    pub items: Vec<CompletionItem>,
}

pub struct LspWorker {
    tx: Sender<Msg>,
    rx: Receiver<LspOutcome>,
    diag_rx: Receiver<Vec<Diagnostic>>,
}

impl LspWorker {
    /// Spawn `cmd` and hand it `root_uri`. Fails exactly when
    /// `LspClient::spawn` does (the command isn't on `PATH`, or the
    /// `initialize` handshake itself errors) — there is no separate
    /// "worker started but client didn't" state to reason about.
    pub fn spawn(cmd: &str, args: &[&str], root_uri: &str) -> std::io::Result<Self> {
        let client = LspClient::spawn(cmd, args, root_uri)?;
        let (tx, worker_rx) = channel::<Msg>();
        let (out_tx, rx) = channel::<LspOutcome>();
        let (diag_tx, diag_rx) = channel::<Vec<Diagnostic>>();
        std::thread::Builder::new()
            .name("aperture-lsp-worker".into())
            .spawn(move || worker(client, worker_rx, out_tx, diag_tx))
            .expect("spawn LSP worker thread");
        Ok(LspWorker { tx, rx, diag_rx })
    }

    pub fn open(&self, uri: String, text: String) {
        let _ = self.tx.send(Msg::Open { uri, text });
    }

    pub fn change(&self, uri: String, text: String, version: i64) {
        let _ = self.tx.send(Msg::Change { uri, text, version });
    }

    /// Queue a completion request. Never blocks; a request superseded by a
    /// newer one before the worker gets to it is coalesced away, same rule
    /// as `Engine::request`.
    pub fn request_completion(&self, uri: String, line: u32, character: u32, generation: u64) {
        let _ = self.tx.send(Msg::Complete { uri, line, character, generation });
    }

    /// Non-blocking; call once per frame.
    pub fn poll(&self) -> Option<LspOutcome> {
        self.rx.try_recv().ok()
    }

    /// Non-blocking; the worker pushes a new list only when it actually
    /// changed (see `worker`'s own loop), so every value received here is
    /// a real transition, not a repeat of the same diagnostics each frame.
    pub fn poll_diagnostics(&self) -> Option<Vec<Diagnostic>> {
        self.diag_rx.try_recv().ok()
    }
}

fn worker(client: LspClient, rx: Receiver<Msg>, tx: Sender<LspOutcome>, diag_tx: Sender<Vec<Diagnostic>>) {
    // Drain-to-newest before acting, same shape as aperture_complete's own
    // worker loop: Open/Change messages all apply in order (each is a real,
    // needed state transition the server must see), but if several
    // Complete requests piled up while a previous completion call was
    // in flight, only the LAST one is worth answering — the caret has
    // moved on.
    let mut pending_complete: Option<(String, u32, u32, u64)> = None;
    // The URI diagnostics get checked against — whatever the last Open or
    // Change named. `publishDiagnostics` is a PUSH, not a reply to
    // anything this worker asked for, so there is no message to drain for
    // it; the periodic recv_timeout below is what notices a new one.
    let mut current_uri: Option<String> = None;
    let mut last_diagnostics: Vec<Diagnostic> = Vec::new();
    loop {
        let msg = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return, // AppState (and its LspWorker) is gone
        };
        let mut msg = msg;
        loop {
            match msg.take() {
                Some(Msg::Open { uri, text }) => {
                    let _ = client.did_open(&uri, "rust", &text, 1);
                    current_uri = Some(uri);
                }
                Some(Msg::Change { uri, text, version }) => {
                    let _ = client.did_change(&uri, &text, version);
                    current_uri = Some(uri);
                }
                Some(Msg::Complete { uri, line, character, generation }) => {
                    pending_complete = Some((uri, line, character, generation));
                }
                None => break,
            }
            match rx.try_recv() {
                Ok(next) => msg = Some(next),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some((uri, line, character, generation)) = pending_complete.take() {
            let items = client.completion(&uri, line, character).unwrap_or_default();
            if tx.send(LspOutcome { generation, items }).is_err() {
                return; // receiver dropped
            }
        }
        if let Some(uri) = &current_uri {
            let latest = client.diagnostics(uri);
            if latest != last_diagnostics {
                last_diagnostics = latest.clone();
                if diag_tx.send(latest).is_err() {
                    return; // receiver dropped
                }
            }
        }
    }
}
