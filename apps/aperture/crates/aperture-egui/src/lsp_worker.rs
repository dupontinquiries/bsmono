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

use aperture_lsp::{CompletionItem, LspClient};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

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
        std::thread::Builder::new()
            .name("aperture-lsp-worker".into())
            .spawn(move || worker(client, worker_rx, out_tx))
            .expect("spawn LSP worker thread");
        Ok(LspWorker { tx, rx })
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
}

fn worker(client: LspClient, rx: Receiver<Msg>, tx: Sender<LspOutcome>) {
    // Drain-to-newest before acting, same shape as aperture_complete's own
    // worker loop: Open/Change messages all apply in order (each is a real,
    // needed state transition the server must see), but if several
    // Complete requests piled up while a previous completion call was
    // in flight, only the LAST one is worth answering — the caret has
    // moved on.
    let mut pending_complete: Option<(String, u32, u32, u64)> = None;
    loop {
        let msg = match rx.recv() {
            Ok(m) => m,
            Err(_) => return, // sender dropped: the AppState (and its LspWorker) is gone
        };
        let mut msg = Some(msg);
        loop {
            match msg.take() {
                Some(Msg::Open { uri, text }) => {
                    let _ = client.did_open(&uri, "rust", &text, 1);
                }
                Some(Msg::Change { uri, text, version }) => {
                    let _ = client.did_change(&uri, &text, version);
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
    }
}
