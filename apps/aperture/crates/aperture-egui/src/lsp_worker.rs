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

use aperture_lsp::{CompletionItem, Diagnostic, Hover, IncomingCall, Location, LspClient};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

enum Msg {
    Open { uri: String, text: String },
    /// `edit`, when present, is a candidate for incremental sync — the
    /// worker sends it via `did_change_range` ONLY if the server actually
    /// advertised support (checked here, not by the caller, since only the
    /// worker thread has the `LspClient` that knows). `text` is always the
    /// full current document regardless, used as the fallback when `edit`
    /// is `None` (no diff could be computed, or the caller chose not to)
    /// or when the server doesn't support incremental sync.
    ///
    /// The edit's positions are ALREADY UTF-16-converted (line, utf16_col)
    /// pairs — this worker has no line-text access to do that conversion
    /// itself (it only ever sees a whole document string, not a buffer
    /// with per-line structure), so the caller (which has the real
    /// `Buffer`) must convert before sending, the same way `request_
    /// completion`'s caller in `state.rs` already does for its own
    /// position.
    Change {
        uri: String,
        text: String,
        edit: Option<(u32, u32, u32, u32, String)>,
        version: i64,
    },
    Complete { uri: String, line: u32, character: u32, generation: u64 },
    Definition { uri: String, line: u32, character: u32, generation: u64 },
    HoverAt { uri: String, line: u32, character: u32, generation: u64 },
    Callers { uri: String, line: u32, character: u32, generation: u64 },
}

/// A completed (or failed-and-empty) completion request, tagged with the
/// generation it was requested under so a caller can discard a stale answer
/// the same way `aperture_complete::CompletionOutcome` already does.
pub struct LspOutcome {
    pub generation: u64,
    pub items: Vec<CompletionItem>,
}

/// Same generation-gating shape as `LspOutcome`, for `textDocument/
/// definition` — a separate type rather than folding into `LspOutcome`
/// because the two requests answer different questions and a caller
/// checking "did MY request land" gets confused fastest when two unrelated
/// answers share one Option<T> and one generation counter.
pub struct DefinitionOutcome {
    pub generation: u64,
    pub locations: Vec<Location>,
}

/// Who calls / mentions the symbol at the caret. `calls` when the server
/// has call hierarchy (every entry is a CALL, with its caller named);
/// otherwise `refs` from `textDocument/references` (every mention, callers
/// unnamed). `error` when the server offers neither — said, not silent.
pub struct CallersOutcome {
    pub generation: u64,
    pub calls: Vec<IncomingCall>,
    pub refs: Vec<Location>,
    pub error: Option<String>,
}

/// Same shape again, for `textDocument/hover`.
pub struct HoverOutcome {
    pub generation: u64,
    pub hover: Option<Hover>,
}

pub struct LspWorker {
    tx: Sender<Msg>,
    rx: Receiver<LspOutcome>,
    diag_rx: Receiver<Vec<Diagnostic>>,
    def_rx: Receiver<DefinitionOutcome>,
    hover_rx: Receiver<HoverOutcome>,
    callers_rx: Receiver<CallersOutcome>,
    /// The handshake's result, once: `Ok` when the server is up, `Err`
    /// when it could not be started or never answered `initialize`.
    ready_rx: Receiver<Result<(), String>>,
}

impl LspWorker {
    /// Start `cmd` for `root_uri` WITHOUT waiting for it.
    ///
    /// The handshake runs on the worker thread. It used to run here, on the
    /// caller's thread — the UI thread, from `open_file` — and
    /// `initialize` may wait up to `CALL_TIMEOUT` (90 s): a server that is
    /// alive but slow or wedged froze the editor for that long every time a
    /// .rs file was opened. Requests sent before the handshake finishes wait
    /// in the inbox and are answered after it; [`Self::poll_ready`] reports
    /// how it went, and on failure the worker thread exits.
    pub fn spawn(cmd: &str, args: &[&str], root_uri: &str) -> std::io::Result<Self> {
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (cmd, args, root_uri) = (cmd.to_string(), args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), root_uri.to_string());
        let (tx, worker_rx) = channel::<Msg>();
        let (out_tx, rx) = channel::<LspOutcome>();
        let (diag_tx, diag_rx) = channel::<Vec<Diagnostic>>();
        let (def_tx, def_rx) = channel::<DefinitionOutcome>();
        let (hover_tx, hover_rx) = channel::<HoverOutcome>();
        let (callers_tx, callers_rx) = channel::<CallersOutcome>();
        std::thread::Builder::new()
            .name("aperture-lsp-worker".into())
            .spawn(move || {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let client = match LspClient::spawn(&cmd, &argv, &root_uri) {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                worker(client, worker_rx, out_tx, diag_tx, def_tx, hover_tx, callers_tx)
            })
            .expect("spawn LSP worker thread");
        Ok(LspWorker { tx, rx, diag_rx, def_rx, hover_rx, callers_rx, ready_rx })
    }

    /// The handshake's outcome, reported once. `None` while it is still
    /// running (and every time after it has been read).
    pub fn poll_ready(&self) -> Option<Result<(), String>> {
        self.ready_rx.try_recv().ok()
    }

    pub fn open(&self, uri: String, text: String) {
        let _ = self.tx.send(Msg::Open { uri, text });
    }

    /// `edit`, when `Some`, is `(start_line, start_utf16_col, end_line,
    /// end_utf16_col, replacement_text)` — see `Msg::Change`'s own doc for
    /// why the caller must do the UTF-16 conversion, and why the worker
    /// (not this call site) decides whether the edit is actually usable.
    pub fn change(&self, uri: String, text: String, edit: Option<(u32, u32, u32, u32, String)>, version: i64) {
        let _ = self.tx.send(Msg::Change { uri, text, edit, version });
    }

    /// Queue a completion request. Never blocks; a request superseded by a
    /// newer one before the worker gets to it is coalesced away, same rule
    /// as `Engine::request`.
    pub fn request_completion(&self, uri: String, line: u32, character: u32, generation: u64) {
        let _ = self.tx.send(Msg::Complete { uri, line, character, generation });
    }

    /// Queue a go-to-definition request. Same coalesce-to-newest rule as
    /// completions — a definition lookup superseded by a later keystroke
    /// or another lookup isn't worth answering.
    pub fn request_definition(&self, uri: String, line: u32, character: u32, generation: u64) {
        let _ = self.tx.send(Msg::Definition { uri, line, character, generation });
    }

    /// Queue a hover request. Same coalesce-to-newest rule.
    pub fn request_hover(&self, uri: String, line: u32, character: u32, generation: u64) {
        let _ = self.tx.send(Msg::HoverAt { uri, line, character, generation });
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

    /// Non-blocking; call once per frame.
    pub fn poll_definition(&self) -> Option<DefinitionOutcome> {
        self.def_rx.try_recv().ok()
    }

    /// Non-blocking; call once per frame.
    pub fn request_callers(&self, uri: String, line: u32, character: u32, generation: u64) {
        let _ = self.tx.send(Msg::Callers { uri, line, character, generation });
    }

    pub fn poll_callers(&self) -> Option<CallersOutcome> {
        self.callers_rx.try_recv().ok()
    }

    pub fn poll_hover(&self) -> Option<HoverOutcome> {
        self.hover_rx.try_recv().ok()
    }
}

fn worker(
    client: LspClient,
    rx: Receiver<Msg>,
    tx: Sender<LspOutcome>,
    diag_tx: Sender<Vec<Diagnostic>>,
    def_tx: Sender<DefinitionOutcome>,
    hover_tx: Sender<HoverOutcome>,
    callers_tx: Sender<CallersOutcome>,
) {
    // Drain-to-newest before acting, same shape as aperture_complete's own
    // worker loop: Open/Change messages all apply in order (each is a real,
    // needed state transition the server must see), but if several
    // Complete, Definition, or HoverAt requests piled up while a previous
    // call was in flight, only the LAST one of each kind is worth
    // answering — the caret has moved on.
    let mut pending_complete: Option<(String, u32, u32, u64)> = None;
    let mut pending_definition: Option<(String, u32, u32, u64)> = None;
    let mut pending_hover: Option<(String, u32, u32, u64)> = None;
    let mut pending_callers: Option<(String, u32, u32, u64)> = None;
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
                Some(Msg::Change { uri, text, edit, version }) => {
                    // Incremental IF both the caller offered a computable
                    // edit AND the server actually advertised support —
                    // checked here, once, right before sending, since this
                    // is the only place that has both pieces of
                    // information at the same time.
                    let sent = match &edit {
                        Some((start_line, start_col, end_line, end_col, replacement))
                            if client.supports_incremental_sync() =>
                        {
                            client.did_change_range(
                                &uri, *start_line, *start_col, *end_line, *end_col, replacement, version,
                            )
                        }
                        _ => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no incremental edit")),
                    };
                    if sent.is_err() {
                        let _ = client.did_change(&uri, &text, version);
                    }
                    current_uri = Some(uri);
                }
                Some(Msg::Complete { uri, line, character, generation }) => {
                    pending_complete = Some((uri, line, character, generation));
                }
                Some(Msg::Definition { uri, line, character, generation }) => {
                    pending_definition = Some((uri, line, character, generation));
                }
                Some(Msg::HoverAt { uri, line, character, generation }) => {
                    pending_hover = Some((uri, line, character, generation));
                }
                Some(Msg::Callers { uri, line, character, generation }) => {
                    pending_callers = Some((uri, line, character, generation));
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
        if let Some((uri, line, character, generation)) = pending_definition.take() {
            let locations = client.goto_definition(&uri, line, character).unwrap_or_default();
            if def_tx.send(DefinitionOutcome { generation, locations }).is_err() {
                return; // receiver dropped
            }
        }
        if let Some((uri, line, character, generation)) = pending_hover.take() {
            let hover = client.hover(&uri, line, character).unwrap_or(None);
            if hover_tx.send(HoverOutcome { generation, hover }).is_err() {
                return; // receiver dropped
            }
        }
        if let Some((uri, line, character, generation)) = pending_callers.take() {
            let outcome = callers(&client, &uri, line, character, generation);
            if callers_tx.send(outcome).is_err() {
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


fn callers(client: &LspClient, uri: &str, line: u32, character: u32, generation: u64) -> CallersOutcome {
    let a = client.callers(uri, line, character);
    CallersOutcome { generation, calls: a.calls, refs: a.refs, error: a.error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ready(w: &LspWorker) -> Result<(), String> {
        let t = Instant::now();
        loop {
            if let Some(r) = w.poll_ready() {
                return r;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "no handshake result");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_wedged_server_does_not_block_the_caller() {
        // Alive, reads nothing, answers nothing: `initialize` would wait
        // out the full 90 s CALL_TIMEOUT.
        let t = Instant::now();
        let w = LspWorker::spawn("sh", &["-c", "sleep 30"], "file:///tmp").unwrap();
        assert!(t.elapsed() < Duration::from_millis(500), "spawn waited {:?}", t.elapsed());
        assert!(w.poll_ready().is_none(), "still handshaking, and that is fine");
        w.request_definition("file:///x.rs".into(), 0, 0, 1); // queues, does not block
    }

    #[test]
    fn a_missing_server_reports_failure_through_poll_ready() {
        let w = LspWorker::spawn("aperture-no-such-language-server", &[], "file:///tmp").unwrap();
        assert!(ready(&w).is_err());
    }
}
