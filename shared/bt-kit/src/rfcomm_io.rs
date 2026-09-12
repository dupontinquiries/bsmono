//! rfcomm_io — the one part of this crate that touches an actual Bluetooth
//! radio, and deliberately the smallest, most-audited part of it.
//!
//! Everything that can be tested without hardware lives in `protocol.rs` and
//! is tested there. This file cannot be exercised in CI — there's no
//! Bluetooth adapter in a build sandbox — so it stays thin on purpose: read
//! bytes, feed them to `FrameReader`; take a `Frame`, `encode_frame` it,
//! write the bytes. No protocol logic of its own to get subtly wrong in a
//! way tests can't catch.
//!
//! Not exercised against real hardware anywhere in this repo yet. Typechecks
//! against bluer 0.16's real `rfcomm::Stream` API (confirmed: it implements
//! `tokio::io::{AsyncRead, AsyncWrite}` directly, `src/rfcomm/mod.rs:558-564`
//! in that crate version) — that's the extent of the verification possible
//! without a device in hand.

use crate::protocol::{Frame, FrameReader};
use bluer::rfcomm::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Bluetooth's Object Push service class — registering under it means a
/// receiving phone's native "incoming file" prompt can pick this up like
/// any other OBEX push, not just another instance of this same app. Whether
/// to actually speak OBEX's own request/response framing on top of this
/// (for that interop) or keep both ends on the simpler `protocol.rs` framing
/// (for fleet-to-fleet transfers, where OBEX's negotiation buys nothing) is
/// an open question — this constant is here so the decision has a name to
/// attach to rather than a magic UUID appearing later with no context.
pub const OBEX_OBJECT_PUSH_UUID: &str = "00001105-0000-1000-8000-00805f9b34fb";

#[derive(Debug)]
pub enum SendError {
    Io(std::io::Error),
}

impl From<std::io::Error> for SendError {
    fn from(e: std::io::Error) -> Self {
        SendError::Io(e)
    }
}

/// Writes one frame to an open RFCOMM stream. Does not flush after every
/// frame — callers sending many chunks in a row should flush once at the
/// end (or let `write_all` batching do it), since a flush per ~900-byte
/// chunk is exactly the kind of overhead this exists to avoid.
pub async fn send_frame(stream: &mut Stream, frame: &Frame) -> Result<(), SendError> {
    let bytes = crate::protocol::encode_frame(frame);
    stream.write_all(&bytes).await?;
    Ok(())
}

pub async fn flush(stream: &mut Stream) -> Result<(), SendError> {
    stream.flush().await?;
    Ok(())
}

/// Reads from the stream until at least one complete frame is available, or
/// the connection closes. Returns every frame `FrameReader` decoded out of
/// this read — which can be more than one (its own test proves a single
/// `feed` call can yield several frames if they arrived back-to-back) — so
/// callers must drain the whole `Vec`, not just look at one entry. An
/// earlier version of this function returned only the first and silently
/// dropped the rest; `FrameReader::feed` already removes decoded bytes from
/// its internal buffer when it returns them, so anything not handed back
/// here is gone, not "still buffered". Fixed before it shipped as a bug
/// nobody would have seen until two frames happened to arrive in one read.
///
/// An empty `Vec` means only a partial frame arrived so far — call again.
/// `None` means the peer closed the connection, which, mid-transfer, is
/// exactly the "walked out of range" case `Reassembler::resume_from` exists
/// for; this function has no opinion on reconnecting, that's the caller's
/// loop to run.
pub async fn read_frames(stream: &mut Stream, reader: &mut FrameReader) -> Result<Option<Vec<Frame>>, SendError> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(None); // peer closed
    }
    Ok(Some(reader.feed(&buf[..n])))
}
