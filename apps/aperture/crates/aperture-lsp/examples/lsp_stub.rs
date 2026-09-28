//! lsp_stub — a scripted language server for `tests/stub_server.rs`.
//!
//! Not a server anyone should run: it answers the handful of requests the
//! tests make with fixed data, so the client's references / call-hierarchy
//! code is exercised over real stdio framing on a machine with no
//! rust-analyzer (the sandbox this was written in has none, and
//! `rust_analyzer_smoke.rs` skips there). An example rather than a bin so it
//! is built by `cargo test` and never installed.
//!
//! Args: `LOG [--bare] [--server-request]`
//! * LOG: every method received is appended here, one per line.
//! * --bare: advertise no references / call hierarchy.
//! * --no-calls: answer `prepareCallHierarchy` with nothing.
//! * --server-request: before answering `textDocument/references`, send a
//!   server->client REQUEST reusing that request's id, as rust-analyzer's
//!   `workspace/configuration` and progress requests can.

use aperture_lsp::rpc::{read_message, write_message};
use serde_json::{json, Value};
use std::io::{BufReader, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let log = args.first().cloned().unwrap_or_else(|| "/dev/null".into());
    let bare = args.iter().any(|a| a == "--bare");
    let server_request = args.iter().any(|a| a == "--server-request");
    let no_calls = args.iter().any(|a| a == "--no-calls");
    let mut out = std::io::stdout().lock();
    let mut inp = BufReader::new(std::io::stdin().lock());
    let loc = |uri: &str, l: u32, c: u32| json!({ "uri": uri, "range": { "start": { "line": l, "character": c }, "end": { "line": l, "character": c + 3 } } });
    while let Ok(Some(msg)) = read_message(&mut inp) {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{method}");
        }
        let Some(id) = msg.get("id").cloned() else {
            if method == "exit" {
                return;
            }
            continue; // a notification
        };
        let uri = msg.pointer("/params/textDocument/uri").and_then(Value::as_str).unwrap_or("file:///x.rs").to_string();
        let result = match method.as_str() {
            "initialize" => {
                let mut caps = json!({ "textDocumentSync": 1 });
                if !bare {
                    caps["referencesProvider"] = json!(true);
                    caps["callHierarchyProvider"] = json!({});
                }
                json!({ "capabilities": caps })
            }
            "textDocument/references" => {
                if server_request {
                    let _ = write_message(&mut out, &json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/configuration", "params": { "items": [] } }));
                }
                assert_eq!(msg.pointer("/params/context/includeDeclaration"), Some(&json!(false)));
                json!([loc(&uri, 10, 4), loc("file:///other.rs", 3, 8)])
            }
            "textDocument/prepareCallHierarchy" if no_calls => json!([]),
            "textDocument/prepareCallHierarchy" => json!([{
                "name": "target", "kind": 12, "uri": uri,
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 3, "character": 1 } },
                "selectionRange": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 9 } },
                "data": { "opaque": 42 }
            }]),
            "callHierarchy/incomingCalls" => {
                // The item must come back verbatim, server data included.
                assert_eq!(msg.pointer("/params/item/data/opaque"), Some(&json!(42)));
                json!([{
                    "from": {
                        "name": "caller", "kind": 12, "uri": "file:///other.rs",
                        "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 9, "character": 1 } },
                        "selectionRange": { "start": { "line": 5, "character": 3 }, "end": { "line": 5, "character": 9 } }
                    },
                    "fromRanges": [
                        { "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 10 } },
                        { "start": { "line": 8, "character": 4 }, "end": { "line": 8, "character": 10 } }
                    ]
                }])
            }
            _ => Value::Null,
        };
        let _ = write_message(&mut out, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }
}
