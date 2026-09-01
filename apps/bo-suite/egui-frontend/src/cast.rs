//! cast — playing to a networked B&O speaker rather than out of this
//! device's own audio hardware.
//!
//! `playback.rs`'s `mpv`-to-a-PipeWire-sink model already covers LOCAL
//! output: Bluetooth buds and USB-C DACs both show up there as ordinary
//! sinks, so nothing new was needed for those. This module is the other
//! half — a B&O speaker on the LAN can't be a PipeWire sink; it only
//! understands "fetch this URL and play it", so the file has to be
//! served over HTTP first (`bo_cast::track_server`) and then the device
//! told to fetch it (`bo_cast::play_uri`).
//!
//! Both are async (reqwest/tokio) and network calls can hang for seconds
//! on a device that's asleep or gone, so this runs them on a dedicated
//! thread with its own single-threaded runtime — the same
//! spawn-a-thread-and-poll-a-channel shape `real_catalog.rs`'s worker
//! uses, for the same reason: a UI frame must never block on the network.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

/// What the UI asked for.
pub enum CastCmd {
    /// Serve `local_path` and tell the device at `device_base`
    /// (`"http://192.168.1.50"`, no trailing slash) to play it.
    PlayFile { device_base: String, local_path: PathBuf, title: String },
}

/// What came back. Owned copies of `bo_cast::play_uri::PlayOutcome`'s
/// fields rather than that type itself — it isn't `Clone`/`Debug`, and a
/// UI toast needs only the yes/no and a short reason, not the full
/// per-endpoint attempt log (that still goes to stderr, same as before).
pub struct CastResult {
    pub device_base: String,
    pub title: String,
    pub ok: bool,
    /// The endpoint that worked, or the last failure's note.
    pub detail: String,
}

/// Runs on its own thread. `cmd_rx`/`result_tx` are the two channel
/// halves; the caller keeps the other ends.
pub fn run_cast_worker(cmd_rx: Receiver<CastCmd>, result_tx: Sender<CastResult>, server_port: u16) {
    // A single-thread runtime is enough — casts are rare, user-initiated,
    // one at a time in practice, and this avoids pulling in a thread pool
    // for what's essentially "one blocking HTTP POST, occasionally".
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            let _ = result_tx.send(CastResult {
                device_base: String::new(), title: String::new(),
                ok: false, detail: format!("cast worker: no tokio runtime: {e}"),
            });
            return;
        }
    };
    let server = match bo_cast::track_server::TrackServer::bind(server_port) {
        Ok(s) => s,
        Err(e) => {
            let _ = result_tx.send(CastResult {
                device_base: String::new(), title: String::new(),
                ok: false, detail: format!("cast worker: track server: {e}"),
            });
            return;
        }
    };
    let client = reqwest::Client::new();

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            CastCmd::PlayFile { device_base, local_path, title } => {
                let outcome = rt.block_on(cast_one(&server, &client, &device_base, &local_path, &title));
                let _ = result_tx.send(outcome);
            }
        }
    }
}

async fn cast_one(
    server: &bo_cast::track_server::TrackServer,
    client: &reqwest::Client,
    device_base: &str,
    local_path: &std::path::Path,
    title: &str,
) -> CastResult {
    let peer_ip = match bo_cast::dlna_discover::host_of(device_base)
        .and_then(|h| h.parse::<std::net::IpAddr>().ok())
    {
        Some(ip) => ip,
        None => return CastResult {
            device_base: device_base.to_string(), title: title.to_string(),
            ok: false, detail: "could not parse device address".into(),
        },
    };
    let path_segment = server.expose(local_path.to_path_buf());
    let track_url = server.url_for(peer_ip, &path_segment);
    let outcome = bo_cast::play_uri::play_url_on_device(device_base, &track_url, title, client).await;
    match outcome.used_endpoint {
        Some(endpoint) => CastResult {
            device_base: device_base.to_string(), title: title.to_string(),
            ok: true, detail: endpoint,
        },
        None => CastResult {
            device_base: device_base.to_string(), title: title.to_string(),
            ok: false,
            detail: outcome.attempts.last()
                .map(|a| a.note.clone())
                .unwrap_or_else(|| "no endpoint accepted the track".into()),
        },
    }
}
