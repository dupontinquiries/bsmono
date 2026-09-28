//! The references / call-hierarchy client against `examples/lsp_stub.rs`,
//! over real stdio. What a real rust-analyzer answers is still
//! `rust_analyzer_smoke.rs`'s job; this proves the request shapes, the
//! parsing, the capability gate and the server-request guard on any machine.
//!
//! Run with a plain `cargo test -p aperture-lsp`: that builds the example.
//! `cargo test --test stub_server` alone does NOT, and runs whatever stub
//! binary an earlier build left behind — a stale stub fails in confusing
//! ways after the stub itself changes.

use aperture_lsp::LspClient;

fn stub() -> std::path::PathBuf {
    // target/<profile>/deps/<this test> -> target/<profile>/examples/lsp_stub
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join("examples").join("lsp_stub");
    assert!(p.exists(), "{} missing: `cargo test` builds examples, run it that way", p.display());
    p
}

fn log(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("aperture-lsp-stub-{name}-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn references_come_back_without_the_declaration() {
    let l = log("refs");
    let c = LspClient::spawn(stub().to_str().unwrap(), &[l.to_str().unwrap()], "file:///tmp").unwrap();
    assert!(c.supports_references());
    let refs = c.references("file:///a.rs", 1, 3).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!((refs[0].uri.as_str(), refs[0].line, refs[0].character), ("file:///a.rs", 10, 4));
    assert_eq!(refs[1].uri, "file:///other.rs");
    c.shutdown();
}

#[test]
fn incoming_calls_name_the_caller_and_every_call_site() {
    let l = log("calls");
    let c = LspClient::spawn(stub().to_str().unwrap(), &[l.to_str().unwrap()], "file:///tmp").unwrap();
    let calls = c.incoming_calls("file:///a.rs", 1, 3).unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!((call.name.as_str(), call.kind, call.uri.as_str()), ("caller", 12, "file:///other.rs"));
    assert_eq!((call.line, call.character), (5, 3), "the caller's NAME, not the top of its body");
    assert_eq!(call.sites, vec![(6, 4), (8, 4)]);
    c.shutdown();
}

#[test]
fn a_server_that_did_not_offer_them_is_never_asked() {
    let l = log("bare");
    let c = LspClient::spawn(stub().to_str().unwrap(), &[l.to_str().unwrap(), "--bare"], "file:///tmp").unwrap();
    assert!(!c.supports_references() && !c.supports_call_hierarchy());
    assert_eq!(c.references("file:///a.rs", 0, 0).unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(c.incoming_calls("file:///a.rs", 0, 0).unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    c.shutdown();
    let seen = std::fs::read_to_string(&l).unwrap_or_default();
    assert!(!seen.contains("references") && !seen.contains("CallHierarchy"), "sent anyway:\n{seen}");
}

#[test]
fn a_server_request_sharing_an_id_does_not_answer_ours() {
    // rust-analyzer sends requests of its own; one whose id happened to
    // equal an in-flight request of ours was taken as the reply to it.
    let l = log("srvreq");
    let c = LspClient::spawn(stub().to_str().unwrap(), &[l.to_str().unwrap(), "--server-request"], "file:///tmp").unwrap();
    let refs = c.references("file:///a.rs", 1, 3).unwrap();
    assert_eq!(refs.len(), 2, "the real reply, not the server's request params");
    c.shutdown();
}

#[test]
fn callers_prefers_calls_and_falls_back_to_references() {
    let c = LspClient::spawn(stub().to_str().unwrap(), &[log("pref").to_str().unwrap()], "file:///tmp").unwrap();
    let a = c.callers("file:///a.rs", 1, 3);
    assert_eq!((a.calls.len(), a.refs.len(), a.error), (1, 0, None));
    c.shutdown();

    let c = LspClient::spawn(stub().to_str().unwrap(), &[log("nocalls").to_str().unwrap(), "--no-calls"], "file:///tmp").unwrap();
    let a = c.callers("file:///a.rs", 1, 3);
    assert_eq!((a.calls.len(), a.refs.len(), a.error), (0, 2, None), "empty call hierarchy -> references");
    c.shutdown();

    let c = LspClient::spawn(stub().to_str().unwrap(), &[log("bare2").to_str().unwrap(), "--bare"], "file:///tmp").unwrap();
    let a = c.callers("file:///a.rs", 1, 3);
    assert!(a.calls.is_empty() && a.refs.is_empty() && a.error.unwrap().contains("neither"));
    c.shutdown();
}
