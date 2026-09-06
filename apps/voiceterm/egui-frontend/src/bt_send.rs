//! bt_send — sends bytes to a trusted peer over classic Bluetooth (RFCOMM),
//! once `peer_handshake` has produced a `HandshakePayload` with a non-empty
//! `bt_addr`.
//!
//! ## Why this file, not a method on the app
//!
//! Same reasoning as `pty_host.rs`: `bluer` requires an async runtime (it's
//! built on `dbus-tokio`; there is no synchronous BlueZ API), and this app
//! has none — `pty_host.rs` reads a shell over a blocking fd on its own
//! thread precisely so the rest of the app never has to think about async.
//! This does the same thing for the one other place in this app that needs
//! it: one spawned thread runs a small, private `tokio` runtime just long
//! enough to drive one transfer, then exits. The app's concurrency model
//! stays "std::thread + shared state", not "some of it is async now".
//!
//! ## Progress reporting: shared state + Waker, not a channel
//!
//! `pty_host.rs`'s own header explains why a channel alone isn't enough for
//! an immediate-mode GUI: egui only redraws when asked to, and a background
//! thread finishing a chunk is not an input event. Same fix, same shape:
//! progress lands in a shared `Mutex`, and `wake()` is what actually gets
//! the next frame drawn to show it.
//!
//! ## What is NOT verified here
//!
//! Everything in `bt_kit::protocol` is unit-tested against no hardware at
//! all (`shared/bt-kit/src/protocol.rs`). This file's connect-and-send path
//! is the one part of the whole handshake→transfer chain that cannot be
//! exercised without an actual Bluetooth adapter and a peer to talk to —
//! there is no simulator for BlueZ's D-Bus surface in this sandbox. It
//! type-checks against the real `bluer` 0.16 API (confirmed the same way
//! `bt-kit`'s own `rfcomm_io.rs` was: `cargo check --features rfcomm`
//! green) — that is the honest ceiling on what's confirmed here.

use bt_kit::{Manifest, DEFAULT_CHUNK_SIZE};
use std::sync::{Arc, Mutex};

pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum SendProgress {
    Connecting,
    Sending { sent_chunks: u32, total_chunks: u32 },
    Done,
    Failed(String),
}

/// Validates a handshake's `bt_addr` field is actually a Bluetooth address
/// before anything tries to connect to it — pure, so the "peer offered no
/// Bluetooth transport" and "peer's address is garbage" cases are testable
/// without a radio. An empty string (the documented "no transport offered"
/// value) and an unparseable one both return `None`: neither is a mistake
/// worth distinguishing to the UI, both mean "no send button here".
pub fn parseable_address(bt_addr: &str) -> Option<bt_kit::bluer::Address> {
    if bt_addr.is_empty() {
        return None;
    }
    bt_addr.parse().ok()
}

/// Spawns the transfer on its own thread and returns shared state the UI
/// can read every frame. `data`/`filename` are already-prepared bytes —
/// this module has no opinion on what they are (a single file, an archived
/// workspace, anything else); deciding that format is a separate, real
/// decision left to the caller rather than guessed at here.
pub fn spawn_send(
    addr: bt_kit::bluer::Address,
    channel: u8,
    filename: String,
    data: Vec<u8>,
    wake: Waker,
) -> Arc<Mutex<SendProgress>> {
    let progress = Arc::new(Mutex::new(SendProgress::Connecting));
    let progress_bg = Arc::clone(&progress);

    std::thread::spawn(move || {
        // A private runtime for exactly this transfer, not a shared one the
        // rest of the app would need to know exists. Built with only what's
        // needed (`rt` + `io-util`, matching bt-kit's own Cargo.toml — no
        // reason to differ) rather than `rt-multi-thread`: one connection,
        // one thread, no work to spread across others.
        let rt = match bt_kit::tokio::runtime::Builder::new_current_thread().enable_io().build() {
            Ok(rt) => rt,
            Err(e) => {
                *progress_bg.lock().unwrap() = SendProgress::Failed(format!("runtime: {e}"));
                wake();
                return;
            }
        };
        rt.block_on(async {
            let result = send(addr, channel, filename, &data, &progress_bg, &wake).await;
            if let Err(e) = result {
                *progress_bg.lock().unwrap() = SendProgress::Failed(e);
                wake();
            }
        });
    });

    progress
}

async fn send(
    addr: bt_kit::bluer::Address,
    channel: u8,
    filename: String,
    data: &[u8],
    progress: &Arc<Mutex<SendProgress>>,
    wake: &Waker,
) -> Result<(), String> {
    use bt_kit::bluer::rfcomm::{SocketAddr, Stream};

    let sa = SocketAddr::new(addr, channel);
    let mut stream = Stream::connect(sa).await.map_err(|e| format!("connect: {e}"))?;

    let manifest = Manifest::for_file(filename, data, DEFAULT_CHUNK_SIZE);
    let total_chunks = manifest.chunk_count();
    bt_kit::rfcomm_io::send_frame(&mut stream, &bt_kit::Frame::Manifest(manifest.clone()))
        .await
        .map_err(|e| format!("manifest: {e:?}"))?;

    for chunk in bt_kit::protocol::chunks_for(data, manifest.chunk_size) {
        let seq = chunk.seq;
        bt_kit::rfcomm_io::send_frame(&mut stream, &bt_kit::Frame::Chunk(chunk))
            .await
            .map_err(|e| format!("chunk {seq}: {e:?}"))?;
        *progress.lock().unwrap() = SendProgress::Sending { sent_chunks: seq + 1, total_chunks };
        wake();
    }
    bt_kit::rfcomm_io::flush(&mut stream).await.map_err(|e| format!("flush: {e:?}"))?;
    bt_kit::rfcomm_io::send_frame(&mut stream, &bt_kit::Frame::Complete)
        .await
        .map_err(|e| format!("complete: {e:?}"))?;
    bt_kit::rfcomm_io::flush(&mut stream).await.map_err(|e| format!("flush: {e:?}"))?;

    *progress.lock().unwrap() = SendProgress::Done;
    wake();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_address_is_not_a_send_target() {
        // The documented meaning of bt_addr == "" (peer offered no
        // Bluetooth transport) must not be treated as a parse failure worth
        // surfacing differently — both cases mean "no send button here".
        assert_eq!(parseable_address(""), None);
    }

    #[test]
    fn garbage_is_not_a_send_target_either() {
        assert_eq!(parseable_address("not-a-bluetooth-address"), None);
    }

    #[test]
    fn a_well_formed_address_parses() {
        assert!(parseable_address("AA:BB:CC:DD:EE:FF").is_some());
    }

    #[test]
    fn progress_variants_compare_by_value_for_ui_change_detection() {
        // The UI's job is "did progress change since last frame, if so
        // repaint" — that only works if these compare structurally, not by
        // identity. Pinning it here means a future refactor that removes
        // PartialEq gets caught by a test, not by a launcher that stops
        // updating its progress bar.
        assert_eq!(
            SendProgress::Sending { sent_chunks: 3, total_chunks: 10 },
            SendProgress::Sending { sent_chunks: 3, total_chunks: 10 }
        );
        assert_ne!(
            SendProgress::Sending { sent_chunks: 3, total_chunks: 10 },
            SendProgress::Sending { sent_chunks: 4, total_chunks: 10 }
        );
    }
}
