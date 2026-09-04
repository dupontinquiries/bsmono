//! End-to-end through real threads and a real socket, the same discipline
//! `completion_loop.rs` already applies to the Demo provider: proves the
//! shell actually renders STREAMING deltas as they arrive over the wire,
//! not just that `note_outcome` reacts correctly to hand-built outcome
//! structs (that half is covered by `state.rs`'s own unit tests). The
//! lower layers — `OllamaProvider::complete_streaming` parsing a real
//! chunked HTTP response, `Engine`'s worker sending one outcome per
//! delta — are proven in `aperture-complete`'s own test suite; this is
//! the one piece that was genuinely untested: that AppState, wired
//! through a real `Engine` to a real socket, ends up with ghost text
//! that visibly grew, not just a single sudden final value.

use aperture_egui::state::{AppState, Ev, ProviderKind};
use aperture_complete::providers::OllamaConfig;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// A minimal fake ollama `/api/generate` streaming endpoint: replies with
/// exactly the newline-delimited JSON chunks given, over one connection,
/// via chunked transfer encoding — the real wire shape `OllamaProvider`
/// parses, not a canned in-process value.
fn fake_ollama_stream_server(body_chunks: Vec<&'static str>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf); // drain the request; content doesn't matter here
        let header = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let _ = sock.write_all(header.as_bytes());
        for chunk in &body_chunks {
            let framed = format!("{:x}\r\n{chunk}\r\n", chunk.len());
            let _ = sock.write_all(framed.as_bytes());
            // A REAL model streams tokens over real wall-clock time; a
            // localhost loopback with everything already buffered sends
            // all three lines before this test's poll loop gets a chance
            // to call poll_completions even once, since poll_completions
            // drains every queued outcome per call — a growth assertion
            // needs at least one call to land BETWEEN two chunks to have
            // anything to observe. 40ms is generous relative to the
            // poll loop's own 5ms cadence without making the test slow.
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let _ = sock.write_all(b"0\r\n\r\n");
    });
    port
}

#[test]
fn streaming_ghost_text_grows_through_a_real_engine_and_a_real_socket() {
    let port = fake_ollama_stream_server(vec![
        "{\"response\":\"fn \",\"done\":false}\n",
        "{\"response\":\"main\",\"done\":false}\n",
        "{\"response\":\"() {}\",\"done\":true}\n",
    ]);

    let mut s = AppState::new(None, false);
    s.provider_kind = ProviderKind::Ollama;
    s.ollama_cfg = OllamaConfig {
        host: format!("127.0.0.1:{port}"),
        model: "test".into(),
        num_predict: 32,
        temperature: 0.2,
        use_suffix: false,
    };
    s.engine = Some(aperture_complete::engine::Engine::new(s.build_provider()));

    let mut t = 0u64;
    for ch in "fn main".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 40;
    }
    s.tick_completion(t + 300); // debounce elapsed -> request goes out

    // Poll like the view does, watching the ghost text over time — the
    // whole point of this test is seeing it GROW, not just land once.
    let mut seen_lengths = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        s.poll_completions(t);
        if let aperture_egui::ghost::Ghost::Shown { text, .. } = &s.ghost {
            let len = text.len();
            if seen_lengths.last() != Some(&len) {
                seen_lengths.push(len);
            }
        }
        if s.loop_log.len() >= 1 {
            // Give it a moment past the final outcome in case a stray
            // extra delta is still in flight, then stop.
            std::thread::sleep(Duration::from_millis(50));
            s.poll_completions(t);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        seen_lengths.len() >= 2,
        "expected to observe the ghost text at more than one length as deltas arrived; saw: {seen_lengths:?}"
    );
    assert!(
        seen_lengths.windows(2).all(|w| w[1] >= w[0]),
        "ghost text should only grow, never shrink, across a stream: {seen_lengths:?}"
    );
    let final_text = match &s.ghost {
        aperture_egui::ghost::Ghost::Shown { text, .. } => text.clone(),
        other => panic!("expected a shown ghost at the end, got {other:?}"),
    };
    assert_eq!(final_text, "fn main() {}");

    // The real point of is_final: one log row for the whole generation,
    // not one per delta that arrived over the wire.
    assert_eq!(s.loop_log.len(), 1, "one streamed generation should be one loop-pane row");
}
