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
    _reader: std::thread::JoinHandle<()>,
}

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

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
        let reader_pending = Arc::clone(&pending);
        let reader = std::thread::Builder::new()
            .name("aperture-lsp-reader".into())
            .spawn(move || reader_loop(BufReader::new(stdout), reader_pending))
            .expect("spawn LSP reader thread");

        let client = LspClient {
            child,
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(1),
            pending,
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
        self.request("initialize", params)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("initialize failed: {e}")))?;
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
        self.pending.lock().unwrap().insert(id, tx);
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

fn reader_loop<R: BufRead>(mut r: R, pending: Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>) {
    loop {
        match read_message(&mut r) {
            Ok(Some(msg)) => {
                // A reply has "id" and either "result" or "error"; a
                // notification (diagnostics, log messages, ...) has no
                // "id" the client originated and is dropped here — this
                // crate doesn't consume server-initiated notifications
                // yet. That is a real gap (diagnostics are useful!), not
                // an oversight: see the crate-level TODO note.
                if let Some(id) = msg.get("id").and_then(Value::as_i64) {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let outcome = if let Some(err) = msg.get("error") {
                            Err(err.clone())
                        } else {
                            Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = tx.send(outcome);
                    }
                }
            }
            Ok(None) => return, // server closed its stdout — clean exit
            Err(_) => return,   // malformed stream; nothing more to salvage
        }
    }
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
    use super::*;

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
}
