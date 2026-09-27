//! client — spawn a language server, speak just enough LSP to it to ask
//! "what could go here" at a cursor.
//!
//! Deliberately BLOCKING, unlike `aperture_complete::engine::Engine`'s
//! worker/inbox/outbox shape. That pattern exists so a slow AI provider
//! never stalls the UI thread; wiring the same shape here — before this
//! crate has ever been driven against a real server — would be building
//! the concurrency story before knowing which calls are actually slow
//! (`initialize` against rust-analyzer on a large workspace, unopened
//! files needing analysis, or a genuinely quick warm completion). A
//! blocking client with a bounded per-call timeout is the honest first
//! shape: real, testable against a real server, and the seam where an
//! `Engine`-style worker would go later is exactly `LspClient`'s public
//! methods — a caller on aperture's UI thread would spawn one worker
//! thread and call these from it, not call them directly.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::rpc::{read_message, write_message};

#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    /// `insertText` when the server provides one (often differs from
    /// `label` — a function's label commonly includes `(...)` the insert
    /// text omits), else `label` itself.
    pub insert_text: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    /// Flattened to plain text regardless of which of `hover`'s three legal
    /// response shapes the server used (see `parse_hover_result`'s own doc)
    /// — this client has no renderer that would benefit from keeping
    /// Markdown structure or a per-block language tag separate.
    pub contents: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    /// Covers both LSP's `Information` and `Hint` — this client has no
    /// caller yet that needs to tell them apart, and inventing a
    /// distinction nothing uses would just be two variants to keep in
    /// sync with the spec for no behavioral difference.
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    /// Zero-indexed, LSP's own convention — NOT yet char-column-converted;
    /// see `client.rs`'s completion doc for the same UTF-16-vs-char caveat.
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

pub struct LspClient {
    child: Child,
    stdin: Mutex<ChildStdin>,
    next_id: Mutex<i64>,
    /// One-shot reply channels for in-flight requests, keyed by request id.
    /// The reader thread looks a reply's `id` up here and sends the result
    /// down that specific channel — the same "one request, one answer,
    /// delivered exactly once" contract a direct function call would give,
    /// over a protocol that otherwise interleaves replies and notifications
    /// on one stream in arbitrary order.
    pending: Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>,
    /// Latest `publishDiagnostics` per URI. The server sends a FULL
    /// replacement list each time (LSP's own contract, not a delta), so
    /// this map only ever needs the newest one per URI, not history —
    /// keyed exactly on the URI the notification names, which the server
    /// may not always match byte-for-byte against a URI a caller
    /// constructed itself (see `diagnostics`'s own doc).
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    /// The server's advertised `textDocumentSync.change` kind from its
    /// `initialize` response: `0` = none, `1` = full-document, `2` =
    /// incremental (range-based). `None` if the server's response didn't
    /// have a usable value (some servers omit `textDocumentSync` entirely,
    /// which per spec means the client shouldn't sync changes at all, but
    /// every real server this crate has been run against sends SOMETHING).
    /// `did_change_range` checks this before sending a range delta — a
    /// server that only understands full-document sync receiving a range
    /// update would silently desync its copy of the file, which is a far
    /// worse failure than this client falling back to full sync.
    sync_kind: Mutex<Option<i64>>,
    /// Set by the reader when the server's stdout closes. A request made
    /// after that fails at once instead of waiting out `CALL_TIMEOUT`.
    closed: Arc<AtomicBool>,
    _reader: std::thread::JoinHandle<()>,
}

// 90s, not a shorter "reasonable" default: found empirically by this
// crate's own end-to-end test (aperture-egui's lsp_popup_e2e.rs) timing
// out at 30s on a completion that specifically needs std/crate-graph
// indexing (a prelude type like `String`, as opposed to a same-file local
// symbol, which answers in well under a second — see client.rs's own doc
// on why THAT case doesn't need this much room). A caller that wants a
// snappier UI timeout for its own purposes should apply it on top of this
// one, not assume this constant already reflects the fast path.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);

/// `capabilities.textDocumentSync` in an `initialize` response is legal in
/// TWO shapes: a bare integer (the older, simpler convention some servers
/// still use) or a `TextDocumentSyncOptions` object with a `change` field
/// carrying that same integer (the modern shape, which also carries
/// `openClose`/`save` this client doesn't need). Handling only one shape
/// would silently treat a server using the other as advertising `None`,
/// falling back to full-document sync unnecessarily rather than actually
/// missing anything correctness-critical — but "unnecessarily conservative"
/// is still a bug worth not having when it's this cheap to avoid.
fn parse_sync_kind(initialize_result: &Value) -> Option<i64> {
    let sync = initialize_result.get("capabilities")?.get("textDocumentSync")?;
    sync.as_i64().or_else(|| sync.get("change").and_then(Value::as_i64))
}

impl LspClient {
    /// Spawn `cmd` and complete the `initialize`/`initialized` handshake.
    /// `root_uri` is a `file://` URI — the workspace root a real server
    /// uses to decide what else it may need to index.
    pub fn spawn(cmd: &str, args: &[&str], root_uri: &str) -> std::io::Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // most servers log diagnostics noise here; not this crate's job to surface it
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        let pending: Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = Arc::clone(&pending);
        let reader_diagnostics = Arc::clone(&diagnostics);
        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = Arc::clone(&closed);
        let reader = std::thread::Builder::new()
            .name("aperture-lsp-reader".into())
            .spawn(move || {
                reader_loop(BufReader::new(stdout), Arc::clone(&reader_pending), reader_diagnostics);
                // THE SERVER IS GONE: fail every waiting request now. Without
                // this a server that exits during `initialize` -- rustup's
                // `rust-analyzer` proxy when the component is not installed
                // does exactly that, printing an error and exiting 1 -- left
                // the handshake blocked for the full 90s CALL_TIMEOUT, on the
                // UI thread, every time a .rs file was opened.
                reader_closed.store(true, Ordering::SeqCst);
                reader_pending.lock().unwrap().clear();
            })
            .expect("spawn LSP reader thread");

        let client = LspClient {
            child,
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(1),
            pending,
            diagnostics,
            sync_kind: Mutex::new(None),
            closed,
            _reader: reader,
        };

        client.handshake(root_uri)?;
        Ok(client)
    }

    fn handshake(&self, root_uri: &str) -> std::io::Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            // The one capability this crate actually uses. Advertising more
            // than that would be a promise about handling responses this
            // client doesn't yet parse — a server is entitled to lean on
            // whatever capabilities it's told a client has.
            "capabilities": {
                "textDocument": {
                    "completion": { "completionItem": { "snippetSupport": false } }
                }
            }
        });
        let result = self
            .request("initialize", params)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("initialize failed: {e}")))?;
        *self.sync_kind.lock().unwrap() = parse_sync_kind(&result);
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    pub fn did_open(&self, uri: &str, language_id: &str, text: &str, version: i64) -> std::io::Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        )
    }

    /// Tell the server the file changed. Sends the WHOLE new text rather
    /// than an incremental delta — `TextDocumentSyncKind::Full` is the one
    /// every server supports, and this crate has no buffer-diff of its own
    /// to build a `range`-based delta from (that machinery already exists
    /// in `aperture-core::op` for undo; reusing it here is exactly the kind
    /// of premature layering this crate's own doc comment warns against
    /// before a real caller shows up needing the bandwidth savings).
    pub fn did_change(&self, uri: &str, text: &str, version: i64) -> std::io::Result<()> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        )
    }

    /// `true` if the server's `initialize` response advertised
    /// `TextDocumentSyncKind.Incremental` (`2`) — the only thing
    /// `did_change_range` checks before a caller relies on it, since
    /// sending a range delta to a server that only understands
    /// full-document sync would silently desync its copy of the file.
    pub fn supports_incremental_sync(&self) -> bool {
        *self.sync_kind.lock().unwrap() == Some(2)
    }

    /// Send a RANGE-based incremental change: `new_text` replaces exactly
    /// `[start, end)` (both zero-indexed `(line, character)` pairs, UTF-16
    /// code units per LSP convention — same caveat `completion`'s own doc
    /// states) rather than replacing the whole document. Returns an error
    /// without sending anything if the server never advertised incremental
    /// support — see `supports_incremental_sync`'s doc for why silently
    /// sending one anyway would be worse than refusing.
    pub fn did_change_range(
        &self,
        uri: &str,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        new_text: &str,
        version: i64,
    ) -> std::io::Result<()> {
        if !self.supports_incremental_sync() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "server did not advertise TextDocumentSyncKind.Incremental",
            ));
        }
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{
                    "range": {
                        "start": { "line": start_line, "character": start_character },
                        "end": { "line": end_line, "character": end_character },
                    },
                    "text": new_text,
                }],
            }),
        )
    }

    /// Ask what could go at `(line, character)`, both zero-indexed per the
    /// LSP spec (NOT this crate's own convention elsewhere — `aperture_
    /// core::buffer::Pos` is also zero-indexed, so a caller passing a `Pos`
    /// straight through needs no translation, but this is worth stating
    /// explicitly since `aperture_core::voice`'s `GoToLine` is deliberately
    /// ONE-indexed for the opposite reason — matching how a person says it,
    /// not how a protocol defines it).
    pub fn completion(&self, uri: &str, line: u32, character: u32) -> std::io::Result<Vec<CompletionItem>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        let result = self
            .request("textDocument/completion", params)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("completion failed: {e}")))?;
        Ok(parse_completion_result(&result))
    }

    /// The most recent `publishDiagnostics` for `uri`, or an empty list if
    /// the server hasn't sent any yet (either it's still analyzing, or the
    /// file genuinely has none — this method can't distinguish those two,
    /// same as a caller polling before the FIRST answer arrives can't). NOT
    /// a request/response — diagnostics are server-PUSHED, so this just
    /// reads whatever the reader thread has already captured; there is no
    /// "ask for diagnostics now" message in the protocol to send.
    pub fn diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        self.diagnostics.lock().unwrap().get(uri).cloned().unwrap_or_default()
    }

    /// `textDocument/definition` at `(line, character)`. Returns every
    /// location the server names — usually one, but a trait method's
    /// definition can legitimately resolve to several implementations, and
    /// collapsing to "just the first" would silently pick one arbitrarily
    /// on a caller's behalf.
    pub fn goto_definition(&self, uri: &str, line: u32, character: u32) -> std::io::Result<Vec<Location>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        let result = self
            .request("textDocument/definition", params)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("goto_definition failed: {e}")))?;
        Ok(parse_definition_result(&result))
    }

    /// `textDocument/hover` at `(line, character)` — usually a type
    /// signature and/or doc comment for whatever's under the caret.
    /// `Ok(None)` for a `null` response (nothing hoverable there, a normal
    /// answer) — distinguished from `Err` (the request itself failed) the
    /// same way `goto_definition`'s empty `Vec` distinguishes "no
    /// definition" from a request error, just with `Option` instead of an
    /// empty collection since at most one `Hover` can ever come back.
    pub fn hover(&self, uri: &str, line: u32, character: u32) -> std::io::Result<Option<Hover>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        let result = self
            .request("textDocument/hover", params)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("hover failed: {e}")))?;
        Ok(parse_hover_result(&result))
    }

    fn notify(&self, method: &str, params: Value) -> std::io::Result<()> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_message(&mut *self.stdin.lock().unwrap(), &msg)
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        let (tx, rx) = channel();
        {
            // Checked under the same lock the reader clears with, so a
            // request can never slip in after the clear and wait forever.
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                return Err(format!("the language server has exited ({method} not sent)"));
            }
            pending.insert(id, tx);
        }
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = write_message(&mut *self.stdin.lock().unwrap(), &msg) {
            self.pending.lock().unwrap().remove(&id);
            return Err(format!("write failed: {e}"));
        }
        match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error_obj)) => Err(format!("server returned an error: {error_obj}")),
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("timed out after {CALL_TIMEOUT:?} waiting for a reply to {method}"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("the reader thread exited — the server process likely died".to_string())
            }
        }
    }

    /// Best-effort: ask nicely (`shutdown`/`exit`, the LSP-mandated
    /// sequence), then kill if the process hasn't gone within a short
    /// grace period. A server that ignores `shutdown` should not be able to
    /// hang a caller that's trying to close the editor.
    pub fn shutdown(mut self) {
        let _ = self.request("shutdown", json!(null));
        let _ = self.notify("exit", json!(null));
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

fn reader_loop<R: BufRead>(
    mut r: R,
    pending: Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
) {
    loop {
        match read_message(&mut r) {
            Ok(Some(msg)) => {
                // A reply has "id" and either "result" or "error"; a
                // notification (diagnostics, log messages, ...) has no
                // "id" the client originated. `publishDiagnostics`
                // specifically is handled below; every other notification
                // (log messages, progress, ...) is still dropped — a real
                // gap, not an oversight, see the crate-level doc.
                if let Some(id) = msg.get("id").and_then(Value::as_i64) {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let outcome = if let Some(err) = msg.get("error") {
                            Err(err.clone())
                        } else {
                            Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = tx.send(outcome);
                    }
                } else if msg.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
                    if let Some(params) = msg.get("params") {
                        if let Some(uri) = params.get("uri").and_then(Value::as_str) {
                            let parsed = parse_diagnostics(params);
                            diagnostics.lock().unwrap().insert(uri.to_string(), parsed);
                        }
                    }
                }
            }
            Ok(None) => return, // server closed its stdout — clean exit
            Err(_) => return,   // malformed stream; nothing more to salvage
        }
    }
}

/// LSP severity is `1..=4` (Error/Warning/Information/Hint); anything
/// else (a future value the spec adds, or a nonconforming server) maps to
/// `Info` rather than being dropped — an unrecognized severity is still a
/// real diagnostic a caller should be able to show, just without
/// confidence about how loudly.
fn parse_severity(v: Option<&Value>) -> DiagnosticSeverity {
    match v.and_then(Value::as_i64) {
        Some(1) => DiagnosticSeverity::Error,
        Some(2) => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Info,
    }
}

fn parse_diagnostics(params: &Value) -> Vec<Diagnostic> {
    let Some(arr) = params.get("diagnostics").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|d| {
            let range = d.get("range")?;
            let start = range.get("start")?;
            let end = range.get("end")?;
            Some(Diagnostic {
                start_line: start.get("line")?.as_u64()? as u32,
                start_character: start.get("character")?.as_u64()? as u32,
                end_line: end.get("line")?.as_u64()? as u32,
                end_character: end.get("character")?.as_u64()? as u32,
                severity: parse_severity(d.get("severity")),
                message: d.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// `textDocument/hover`'s `contents` field is legal in THREE shapes per
/// spec, none deprecated out of existence yet: a `MarkupContent` object
/// (`{kind, value}` — the modern shape, `kind` is `"plaintext"` or
/// `"markdown"`), a single `MarkedString` (either a plain string, or the
/// older `{language, value}` code-block shape), or an array of
/// `MarkedString`. All three flatten to plain text here — see `Hover`'s
/// own doc for why keeping Markdown/language structure isn't worth it
/// without a renderer that would use it.
fn parse_hover_result(result: &Value) -> Option<Hover> {
    let contents = result.get("contents")?;
    let text = flatten_hover_contents(contents);
    if text.trim().is_empty() {
        return None; // an empty MarkupContent value is "nothing to show", not a Hover with blank text
    }
    Some(Hover { contents: text })
}

fn flatten_hover_contents(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(arr) = v.as_array() {
        return arr.iter().map(flatten_hover_contents).collect::<Vec<_>>().join("\n\n");
    }
    // MarkupContent ({kind, value}) and the older per-block MarkedString
    // ({language, value}) both carry the text in a "value" field — neither
    // shape needs `kind`/`language` for a plain-text flatten.
    v.get("value").and_then(Value::as_str).unwrap_or_default().to_string()
}


/// `textDocument/definition` replies as `null` (nothing found), a single
/// `Location`, an array of `Location`, or — for a server advertising
/// `definitionLinkSupport` (not a capability this client declares, but
/// nothing stops a server from sending it anyway) — an array of
/// `LocationLink`, which names the target differently (`targetUri`/
/// `targetSelectionRange` instead of `uri`/`range`). Handling only the
/// shape this client expects and silently returning nothing against a
/// server that chose another is exactly the bug `parse_completion_result`'s
/// own doc warns about, for the same reason, one call away.
fn parse_definition_result(result: &Value) -> Vec<Location> {
    if result.is_null() {
        return Vec::new();
    }
    let items: Vec<&Value> = if let Some(arr) = result.as_array() {
        arr.iter().collect()
    } else {
        vec![result]
    };
    items.iter().filter_map(|it| parse_one_location(it)).collect()
}

fn parse_one_location(v: &Value) -> Option<Location> {
    // Plain Location: {uri, range: {start: {line, character}, ...}}.
    if let (Some(uri), Some(range)) = (v.get("uri").and_then(Value::as_str), v.get("range")) {
        let start = range.get("start")?;
        return Some(Location {
            uri: uri.to_string(),
            line: start.get("line")?.as_u64()? as u32,
            character: start.get("character")?.as_u64()? as u32,
        });
    }
    // LocationLink: {targetUri, targetSelectionRange: {start: {...}, ...}}.
    // targetSelectionRange (the identifier itself) is preferred over
    // targetRange (the whole enclosing item, e.g. a function's full body)
    // when both are present — a caller jumping "to the definition" wants
    // the caret on the name, not the top of a possibly long block.
    if let Some(uri) = v.get("targetUri").and_then(Value::as_str) {
        let range = v.get("targetSelectionRange").or_else(|| v.get("targetRange"))?;
        let start = range.get("start")?;
        return Some(Location {
            uri: uri.to_string(),
            line: start.get("line")?.as_u64()? as u32,
            character: start.get("character")?.as_u64()? as u32,
        });
    }
    None
}


/// `textDocument/completion` replies as either `CompletionItem[]` or a
/// `CompletionList { items: CompletionItem[] }` — both legal per the spec,
/// and a client that only handles one shape will silently see zero results
/// against a server that chose the other.
fn parse_completion_result(result: &Value) -> Vec<CompletionItem> {
    let items = if let Some(arr) = result.as_array() {
        arr.as_slice()
    } else if let Some(arr) = result.get("items").and_then(Value::as_array) {
        arr.as_slice()
    } else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let label = it.get("label")?.as_str()?.to_string();
            let insert_text = it
                .get("insertText")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| label.clone());
            let detail = it.get("detail").and_then(Value::as_str).map(str::to_string);
            Some(CompletionItem { label, insert_text, detail })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// A "server" that exits during the handshake (rustup's proxy for an
    /// uninstalled rust-analyzer does) fails spawn promptly, not after
    /// CALL_TIMEOUT.
    #[test]
    fn a_server_that_exits_fails_the_handshake_at_once() {
        let t = std::time::Instant::now();
        let r = LspClient::spawn("sh", &["-c", "echo nope >&2; exit 1"], "file:///tmp");
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
    }

    use super::*;

    #[test]
    fn sync_kind_parses_the_modern_object_shape() {
        let v = json!({ "capabilities": { "textDocumentSync": { "openClose": true, "change": 2 } } });
        assert_eq!(parse_sync_kind(&v), Some(2));
    }

    #[test]
    fn sync_kind_parses_the_older_bare_integer_shape() {
        let v = json!({ "capabilities": { "textDocumentSync": 1 } });
        assert_eq!(parse_sync_kind(&v), Some(1));
    }

    #[test]
    fn sync_kind_is_none_when_the_server_omits_it_entirely() {
        let v = json!({ "capabilities": {} });
        assert_eq!(parse_sync_kind(&v), None);
    }

    #[test]
    fn hover_parses_a_markup_content_object() {
        let v = json!({ "contents": { "kind": "markdown", "value": "```rust\nfn foo() -> i32\n```" } });
        assert_eq!(parse_hover_result(&v), Some(Hover { contents: "```rust\nfn foo() -> i32\n```".into() }));
    }

    #[test]
    fn hover_parses_a_bare_marked_string() {
        let v = json!({ "contents": "fn foo() -> i32" });
        assert_eq!(parse_hover_result(&v), Some(Hover { contents: "fn foo() -> i32".into() }));
    }

    #[test]
    fn hover_parses_the_older_language_tagged_marked_string() {
        let v = json!({ "contents": { "language": "rust", "value": "fn foo() -> i32" } });
        assert_eq!(parse_hover_result(&v), Some(Hover { contents: "fn foo() -> i32".into() }));
    }

    #[test]
    fn hover_parses_an_array_of_marked_strings_joined_with_blank_lines() {
        let v = json!({ "contents": ["fn foo() -> i32", "the foo function"] });
        assert_eq!(parse_hover_result(&v), Some(Hover { contents: "fn foo() -> i32\n\nthe foo function".into() }));
    }

    #[test]
    fn hover_null_result_is_none_not_an_error() {
        assert_eq!(parse_hover_result(&Value::Null), None);
    }

    #[test]
    fn hover_an_empty_value_string_is_none_not_a_blank_hover() {
        let v = json!({ "contents": { "kind": "plaintext", "value": "" } });
        assert_eq!(parse_hover_result(&v), None);
    }

    #[test]
    fn definition_parses_a_single_plain_location() {
        let v = json!({ "uri": "file:///a.rs", "range": { "start": { "line": 3, "character": 5 }, "end": { "line": 3, "character": 10 } } });
        assert_eq!(parse_definition_result(&v), vec![Location { uri: "file:///a.rs".into(), line: 3, character: 5 }]);
    }

    #[test]
    fn definition_parses_an_array_of_locations() {
        let v = json!([
            { "uri": "file:///a.rs", "range": { "start": { "line": 1, "character": 0 } } },
            { "uri": "file:///b.rs", "range": { "start": { "line": 2, "character": 4 } } },
        ]);
        assert_eq!(parse_definition_result(&v), vec![
            Location { uri: "file:///a.rs".into(), line: 1, character: 0 },
            Location { uri: "file:///b.rs".into(), line: 2, character: 4 },
        ]);
    }

    #[test]
    fn definition_null_result_is_empty_not_an_error() {
        // No definition found is a valid, common answer (e.g. the caret
        // isn't on anything definable) — not a malformed response.
        assert_eq!(parse_definition_result(&Value::Null), Vec::<Location>::new());
    }

    #[test]
    fn definition_parses_location_link_shape_preferring_target_selection_range() {
        // A server advertising definitionLinkSupport (not a capability this
        // client declares, but nothing on the wire enforces that) may still
        // send LocationLink instead of plain Location.
        let v = json!([{
            "targetUri": "file:///a.rs",
            "targetRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
            "targetSelectionRange": { "start": { "line": 2, "character": 7 }, "end": { "line": 2, "character": 12 } },
        }]);
        assert_eq!(
            parse_definition_result(&v),
            vec![Location { uri: "file:///a.rs".into(), line: 2, character: 7 }],
            "should land on the identifier (targetSelectionRange), not the whole item (targetRange)"
        );
    }

    #[test]
    fn definition_falls_back_to_target_range_when_selection_range_is_absent() {
        let v = json!([{
            "targetUri": "file:///a.rs",
            "targetRange": { "start": { "line": 4, "character": 2 } },
        }]);
        assert_eq!(parse_definition_result(&v), vec![Location { uri: "file:///a.rs".into(), line: 4, character: 2 }]);
    }

    #[test]
    fn definition_skips_a_malformed_entry_rather_than_losing_the_whole_batch() {
        let v = json!([
            { "uri": "file:///a.rs" }, // missing range entirely
            { "uri": "file:///b.rs", "range": { "start": { "line": 9, "character": 1 } } },
        ]);
        assert_eq!(parse_definition_result(&v), vec![Location { uri: "file:///b.rs".into(), line: 9, character: 1 }]);
    }

    #[test]
    fn parses_a_bare_array_response() {
        let v = json!([{ "label": "foo", "detail": "fn foo()" }]);
        let items = parse_completion_result(&v);
        assert_eq!(items, vec![CompletionItem {
            label: "foo".into(), insert_text: "foo".into(), detail: Some("fn foo()".into()),
        }]);
    }

    #[test]
    fn parses_a_completion_list_wrapper() {
        let v = json!({ "isIncomplete": false, "items": [{ "label": "bar" }] });
        let items = parse_completion_result(&v);
        assert_eq!(items, vec![CompletionItem { label: "bar".into(), insert_text: "bar".into(), detail: None }]);
    }

    #[test]
    fn insert_text_falls_back_to_label_when_the_server_omits_it() {
        let v = json!([{ "label": "compute_total" }]);
        assert_eq!(parse_completion_result(&v)[0].insert_text, "compute_total");
    }

    #[test]
    fn insert_text_is_used_when_it_differs_from_the_label() {
        // The common real case: label shows "foo(a, b)" for a human to
        // read, insertText is just "foo" so the parens aren't duplicated
        // if the editor's own auto-pair already added them.
        let v = json!([{ "label": "foo(a, b)", "insertText": "foo" }]);
        assert_eq!(parse_completion_result(&v)[0].insert_text, "foo");
        assert_eq!(parse_completion_result(&v)[0].label, "foo(a, b)");
    }

    #[test]
    fn an_item_with_no_label_is_skipped_rather_than_panicking() {
        // Malformed per spec, but "a malformed item elsewhere in the list
        // hides every other item" is a worse failure than dropping just it.
        let v = json!([{ "detail": "no label here" }, { "label": "valid" }]);
        let items = parse_completion_result(&v);
        assert_eq!(items, vec![CompletionItem { label: "valid".into(), insert_text: "valid".into(), detail: None }]);
    }

    #[test]
    fn neither_array_nor_items_shape_returns_empty_not_an_error() {
        // A `null` result (no completions applicable here) is valid per
        // spec and must not be treated as malformed.
        assert_eq!(parse_completion_result(&Value::Null), Vec::<CompletionItem>::new());
    }

    #[test]
    fn parses_a_publish_diagnostics_params_object() {
        let params = json!({
            "uri": "file:///tmp/x.rs",
            "diagnostics": [{
                "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9}},
                "severity": 1,
                "message": "mismatched types",
            }]
        });
        let diags = parse_diagnostics(&params);
        assert_eq!(diags, vec![Diagnostic {
            start_line: 2, start_character: 4, end_line: 2, end_character: 9,
            severity: DiagnosticSeverity::Error, message: "mismatched types".to_string(),
        }]);
    }

    #[test]
    fn severity_numbers_map_per_spec_and_unknown_values_fall_back_to_info() {
        assert_eq!(parse_severity(Some(&json!(1))), DiagnosticSeverity::Error);
        assert_eq!(parse_severity(Some(&json!(2))), DiagnosticSeverity::Warning);
        assert_eq!(parse_severity(Some(&json!(3))), DiagnosticSeverity::Info);
        assert_eq!(parse_severity(Some(&json!(4))), DiagnosticSeverity::Info);
        assert_eq!(parse_severity(Some(&json!(99))), DiagnosticSeverity::Info, "unrecognized value, not dropped");
        assert_eq!(parse_severity(None), DiagnosticSeverity::Info, "missing severity, not dropped");
    }

    #[test]
    fn a_diagnostic_missing_its_range_is_skipped_rather_than_panicking() {
        let params = json!({
            "uri": "file:///tmp/x.rs",
            "diagnostics": [
                {"message": "no range here"},
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "ok"},
            ]
        });
        let diags = parse_diagnostics(&params);
        assert_eq!(diags.len(), 1, "the malformed entry is dropped, not the whole batch");
        assert_eq!(diags[0].message, "ok");
    }

    #[test]
    fn an_empty_diagnostics_array_means_the_file_is_currently_clean() {
        // The server sends this explicitly when errors are FIXED — an
        // empty list is meaningful (clean), not the same as "no answer yet".
        let params = json!({ "uri": "file:///tmp/x.rs", "diagnostics": [] });
        assert_eq!(parse_diagnostics(&params), Vec::<Diagnostic>::new());
    }
}
