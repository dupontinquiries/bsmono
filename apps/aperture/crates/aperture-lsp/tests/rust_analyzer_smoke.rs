//! Drives a REAL `rust-analyzer` over its actual stdio, not a fixture — the
//! unit tests in `rpc.rs`/`client.rs` prove the framing and parsing logic in
//! isolation; this is the only thing that proves the whole client actually
//! talks to a real server, which is the entire point of an LSP client.
//!
//! Skips (prints a message, returns without failing) rather than fails when
//! `rust-analyzer` isn't on `PATH` — a missing optional external tool is not
//! this crate's own regression, and failing the whole suite over it would
//! make CI red on machines that simply haven't installed it, which teaches
//! people to ignore CI. `cargo test -p aperture-lsp` documents the
//! requirement; this file enforces "if it's there, it must actually work."

/// Build a real (if minimal) cargo package on disk. rust-analyzer's
/// completions for a file's OWN locally-defined items work even before
/// full crate-graph indexing finishes, but only inside a package it
/// recognizes as one — a bare loose `.rs` file with no `Cargo.toml` puts it
/// in a reduced "detached file" mode that isn't representative of how
/// aperture would actually use this crate against a person's real project.
/// `name` must be distinct PER TEST, not just per process: `cargo test`
/// runs multiple tests from one binary concurrently on separate threads
/// by default, and `std::process::id()` alone is the same for all of them
/// — the bug this comment exists because of. Two tests sharing a
/// directory raced on the same `src/main.rs` and the same rust-analyzer
/// workspace root, and the symptom looked like a real protocol bug (a
/// diagnostic that should have cleared but didn't) until the actual cause
/// — directory collision, not server behavior — was found.
fn scratch_package(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aperture-lsp-smoke-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir); // a previous run's leftovers, if any
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"aperture_lsp_smoke_{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    dir
}

fn rust_analyzer_available() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn completes_a_locally_defined_function_through_a_real_rust_analyzer() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH — this test needs a real server, see lib.rs doc");
        return;
    }

    let dir = scratch_package("completes_a_locally_defined_function");
    let main_rs = dir.join("src/main.rs");
    // Two locally-defined functions with a common prefix, and a caller that
    // has typed the prefix but not finished the name — this is exactly what
    // aperture's own ghost-completion trigger point looks like, and it is
    // answerable from THIS FILE's syntax tree alone, no std/crate indexing
    // required, so it should resolve promptly even right after spawn.
    let source = "fn compute_total() -> i32 { 42 }\nfn compute_tax() -> i32 { 1 }\n\nfn main() {\n    let x = comp\n}\n";
    std::fs::write(&main_rs, source).unwrap();

    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", source, 1).expect("didOpen");

    // Cursor right after "comp" on the `let x = comp` line — that's line 4
    // (0-indexed), character 12 (4 spaces + "let x = " is 8 chars + "comp"
    // is 4 chars = 12).
    let line = 4u32;
    let character = 12u32;

    // rust-analyzer's first response after opening a file can legitimately
    // be empty while it finishes an initial parse; retry with backoff
    // rather than treat one empty answer as failure. This is polling a
    // real, slow, external process — not something to fake with a fixed
    // sleep that either wastes time or flakes on a slower machine.
    let mut items = Vec::new();
    for attempt in 0..20 {
        items = client.completion(&uri, line, character).expect("completion request");
        if items.iter().any(|i| i.label.contains("compute_total")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("compute_total")),
        "expected 'compute_total' among completions at comp|, got {labels:?}"
    );
    // The OTHER same-prefix function should also be offered — proves this
    // is genuine prefix completion against the file's real symbols, not a
    // coincidental single match.
    assert!(
        labels.iter().any(|l| l.contains("compute_tax")),
        "expected 'compute_tax' too — both share the 'comp' prefix, got {labels:?}"
    );

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_prefix_position_still_gets_a_clean_reply_not_an_error() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("empty_prefix");
    let main_rs = dir.join("src/main.rs");
    let source = "fn main() {\n    \n}\n";
    std::fs::write(&main_rs, source).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", source, 1).expect("didOpen");
    // Blank line inside main() — a real position with nothing typed yet.
    // Should return SOME list (even if just keyword/snippet completions)
    // without erroring, not panic on an edge position.
    let result = client.completion(&uri, 1, 4);
    assert!(result.is_ok(), "{result:?}");

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn did_change_is_reflected_in_the_next_completion() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("did_change");
    let main_rs = dir.join("src/main.rs");
    let v1 = "fn main() {\n    let x = 1;\n}\n";
    std::fs::write(&main_rs, v1).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", v1, 1).expect("didOpen");

    // Add a new function, tell the server via didChange, then ask for
    // completions of ITS name — if didChange didn't actually take effect,
    // the server would still be answering against v1's contents, which has
    // no such function at all.
    let v2 = "fn freshly_added_function() {}\nfn main() {\n    let x = 1;\n    freshly_ad\n}\n";
    std::fs::write(&main_rs, v2).unwrap(); // keep disk and server state consistent
    client.did_change(&uri, v2, 2).expect("didChange");

    let mut items = Vec::new();
    for attempt in 0..20 {
        items = client.completion(&uri, 3, 15).unwrap_or_default(); // end of "freshly_ad"
        if items.iter().any(|i| i.label.contains("freshly_added_function")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("freshly_added_function")),
        "didChange's new function should be completable; got {labels:?}"
    );

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_real_type_error_produces_a_real_diagnostic() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("type_error");
    let main_rs = dir.join("src/main.rs");
    // A genuine, unambiguous type error: assigning a &str to an i32.
    let source = "fn main() {\n    let x: i32 = \"not a number\";\n    let _ = x;\n}\n";
    std::fs::write(&main_rs, source).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", source, 1).expect("didOpen");

    // Diagnostics are PUSHED, not requested — poll the client's own cache
    // (populated by the reader thread off publishDiagnostics) rather than
    // sending anything.
    let mut diags = Vec::new();
    for attempt in 0..30 {
        diags = client.diagnostics(&uri);
        if !diags.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    assert!(!diags.is_empty(), "expected at least one diagnostic for a real type mismatch");
    assert!(
        diags.iter().any(|d| d.severity == aperture_lsp::DiagnosticSeverity::Error),
        "a &str-to-i32 mismatch should be an Error, not a lesser severity; got {diags:?}"
    );
    // Line 1 (0-indexed) is where the mismatched assignment is.
    assert!(
        diags.iter().any(|d| d.start_line == 1),
        "expected the diagnostic anchored on the assignment's line; got {diags:?}"
    );

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goto_definition_finds_a_locally_defined_function_through_a_real_rust_analyzer() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("gotodef");
    let main_rs = dir.join("src/main.rs");
    // The call site is on line 2 (0-indexed); the definition is line 0.
    let source = "fn compute_total() -> i32 { 42 }\n\nfn main() {\n    let x = compute_total();\n    let _ = x;\n}\n";
    std::fs::write(&main_rs, source).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", source, 1).expect("didOpen");

    // Caret inside "compute_total(" on the call site, line 3, somewhere
    // within the identifier (character 14, inside "compute_total").
    let mut locations = Vec::new();
    for attempt in 0..20 {
        locations = client.goto_definition(&uri, 3, 14).unwrap_or_default();
        if !locations.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    assert!(!locations.is_empty(), "expected at least one definition location for compute_total");
    assert_eq!(locations[0].uri, uri, "the definition is in the same file");
    assert_eq!(locations[0].line, 0, "compute_total is defined on line 0");

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hover_reports_a_type_signature_for_a_real_function_through_rust_analyzer() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("hover");
    let main_rs = dir.join("src/main.rs");
    let source = "fn compute_total() -> i32 { 42 }\n\nfn main() {\n    let x = compute_total();\n    let _ = x;\n}\n";
    std::fs::write(&main_rs, source).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", source, 1).expect("didOpen");

    // Caret inside "compute_total(" on the call site, line 3, character 14.
    let mut hover = None;
    for attempt in 0..20 {
        hover = client.hover(&uri, 3, 14).unwrap_or(None);
        if hover.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    let hover = hover.expect("expected a hover result for compute_total's call site");
    assert!(
        hover.contents.contains("i32"),
        "expected the return type in the hover text; got: {:?}",
        hover.contents
    );

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_did_change_is_reflected_when_the_server_supports_it() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("incremental");
    let main_rs = dir.join("src/main.rs");
    let v1 = "fn main() {\n    let x = 1;\n}\n";
    std::fs::write(&main_rs, v1).unwrap();
    let uri = format!("file://{}", main_rs.display());
    let root_uri = format!("file://{}", dir.display());

    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &root_uri)
        .expect("spawn rust-analyzer");
    client.did_open(&uri, "rust", v1, 1).expect("didOpen");

    assert!(
        client.supports_incremental_sync(),
        "rust-analyzer is expected to advertise incremental sync"
    );

    // Insert "freshly_added_function() -> i32 { 0 }\n" as a new line 0,
    // via a RANGE replace of the empty range at (0,0) — a pure insertion,
    // the simplest real incremental edit — then check completion sees it,
    // the same way did_change_is_reflected_in_the_next_completion already
    // proves full-document sync does.
    let insertion = "fn freshly_added_function() -> i32 { 0 }\n";
    client
        .did_change_range(&uri, 0, 0, 0, 0, insertion, 2)
        .expect("did_change_range");

    let v2 = format!("{insertion}{v1}");
    // Also append a call site so there's something to complete against.
    let v3 = v2.replace("let x = 1;", "let x = 1;\n    let y = freshly_ad");
    client
        .did_change_range(&uri, 2, 0, 2, 0, "    let y = freshly_ad\n", 3)
        .expect("second did_change_range");

    let mut items = Vec::new();
    for attempt in 0..20 {
        items = client.completion(&uri, 2, 22).unwrap_or_default(); // end of "freshly_ad"
        if items.iter().any(|i| i.label.contains("freshly_added_function")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("freshly_added_function")),
        "the incrementally-inserted function should be completable; got {labels:?}. v3 for reference: {v3:?}"
    );

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cross-FILE callers through a real rust-analyzer — the capability the
/// entity graph lacked. Written without a rust-analyzer to run it against
/// (the sandbox it was written in could not install one); the request
/// shapes are proven by `stub_server.rs`, and this is what proves a real
/// server answers them. If it fails on first real run, suspect indexing
/// time before suspecting the client: the retry loop is the same one the
/// F12 test needed.
#[test]
fn callers_finds_a_cross_file_caller_through_a_real_rust_analyzer() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }
    let dir = scratch_package("callers");
    let main_rs = dir.join("src/main.rs");
    let util_rs = dir.join("src/util.rs");
    let util = "pub fn compute_total() -> i32 {\n    42\n}\n";
    let main = "mod util;\n\nfn report() {\n    let _ = util::compute_total();\n}\n\nfn main() {\n    report();\n}\n";
    std::fs::write(&util_rs, util).unwrap();
    std::fs::write(&main_rs, main).unwrap();
    let util_uri = format!("file://{}", util_rs.display());
    let main_uri = format!("file://{}", main_rs.display());
    let client = aperture_lsp::LspClient::spawn("rust-analyzer", &[], &format!("file://{}", dir.display()))
        .expect("spawn rust-analyzer");
    client.did_open(&main_uri, "rust", main, 1).expect("didOpen main");
    client.did_open(&util_uri, "rust", util, 1).expect("didOpen util");
    assert!(client.supports_references(), "rust-analyzer advertises references");

    // Inside the name `compute_total` in its definition (line 0).
    let mut answer = aperture_lsp::CallersAnswer::default();
    for attempt in 0..20 {
        answer = client.callers(&util_uri, 0, 10);
        if !answer.calls.is_empty() || !answer.refs.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1).min(4)));
    }
    if client.supports_call_hierarchy() {
        let caller = answer.calls.first().unwrap_or_else(|| panic!("no incoming calls: {answer:?}"));
        assert_eq!(caller.name, "report");
        assert_eq!(caller.uri, main_uri, "the caller is in the OTHER file");
        assert_eq!(caller.sites.first().map(|s| s.0), Some(3), "called on main.rs line 3");
    } else {
        let r = answer.refs.first().unwrap_or_else(|| panic!("no references: {answer:?}"));
        assert_eq!((r.uri.as_str(), r.line), (main_uri.as_str(), 3));
    }
    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
