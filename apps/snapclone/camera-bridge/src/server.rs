//! camera-server — stream virtual camera nodes to trusted desktops.
//!
//! Runs on the FairPhone 5. Reads from the virtual PipeWire camera nodes
//! written by camera-bridge and streams them over TLS to authenticated clients.
//!
//! Auth: Ed25519 public key challenge/response, same model as SSH authorized_keys.
//! Each trusted desktop's public key is listed in:
//!   ~/.config/camera-bridge/authorized_keys
//!
//! TLS: self-signed cert generated on first run. Clients pin the fingerprint.
//! The fingerprint is printed on first run and stored in the client config.
//!
//! Usage:
//!   camera-server [OPTIONS]
//!   camera-server --add-key "<pubkey-base64> desktop-name"   # trust a new client
//!   camera-server --show-fingerprint                          # print TLS fingerprint
//!   camera-server --list-keys                                 # show trusted clients

use camera_bridge_lib::auth;
use gstreamer;
use gstreamer_app;

use anyhow::{anyhow, Context, Result};
use auth::{
    AuthResponse, AuthResult, Challenge, FrameHeader, StreamInfo,
    is_authorized, load_authorized_keys, make_nonce, pubkey_to_string,
    sign_challenge, verify_response,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use log::{error, info, warn};
use serde_json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

// ── CLI ────────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "camera-server", about = "Stream FP5 cameras to trusted desktops")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Bind address [default: 0.0.0.0:7878]
    #[arg(long, default_value = "0.0.0.0:7878")]
    bind: String,

    /// Config directory override
    #[arg(long)]
    config_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Add a client public key to authorized_keys.
    /// Generate one on the client with: camera-client keygen
    AddKey {
        /// "<base64-pubkey> optional-comment"
        key_line: String,
    },
    /// Remove a key by comment or pubkey prefix.
    RemoveKey {
        /// Comment or pubkey prefix to remove
        pattern: String,
    },
    /// List all trusted client keys.
    ListKeys,
    /// Print the TLS certificate fingerprint clients should pin.
    ShowFingerprint,
    /// Generate a new TLS certificate (replaces existing, all clients must re-pin).
    RegenCert,
}

// ── Config paths ──────────────────────────────────────────────────────────────

fn config_dir(cli_override: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = cli_override { return p.clone(); }
    // Respect existing camera-bridge config location
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".config/camera-bridge");
    }
    ProjectDirs::from("", "", "camera-bridge")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".config/camera-bridge"))
}

// ── TLS ───────────────────────────────────────────────────────────────────────

fn load_or_generate_tls(cert_path: &PathBuf, key_path: &PathBuf)
    -> Result<(Vec<rustls::pki_types::CertificateDer<'static>>,
               rustls::pki_types::PrivateKeyDer<'static>)>
{
    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read(cert_path)?;
        let key_pem  = std::fs::read(key_path)?;
        let certs = rustls_pemfile::certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse cert PEM")?;
        let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
            .context("Failed to parse key PEM")?
            .ok_or_else(|| anyhow!("No private key in key PEM"))?;
        return Ok((certs, key));
    }

    info!("Generating self-signed TLS certificate...");
    let subject = rcgen::generate_simple_self_signed(vec![
        "camera-server".to_string(),
        "localhost".to_string(),
    ]).context("Failed to generate TLS certificate")?;

    let cert_pem = subject.cert.pem();
    let key_pem  = subject.key_pair.serialize_pem();

    std::fs::create_dir_all(cert_path.parent().unwrap())?;
    std::fs::write(cert_path, &cert_pem)?;
    std::fs::write(key_path,  &key_pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    let cert_der = subject.cert.der().to_vec();
    let fp = auth::cert_fingerprint(&cert_der);
    info!("TLS certificate generated.");
    info!("Fingerprint: {fp}");
    info!("Share this with desktop clients: camera-client trust <server-ip> {fp}");

    // Re-load from PEM to get the right types
    let certs = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow!("Key parse failed"))?;
    Ok((certs, key))
}

fn make_tls_acceptor(cert_path: &PathBuf, key_path: &PathBuf) -> Result<TlsAcceptor> {
    let (certs, key) = load_or_generate_tls(cert_path, key_path)?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("Failed to build TLS config")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

// ── Available streams (from camera-bridge virtual nodes) ──────────────────────

fn available_streams() -> Vec<StreamInfo> {
    let w = std::env::var("OUTPUT_WIDTH").ok()
        .and_then(|v| v.parse().ok()).unwrap_or(480u32);
    let h = std::env::var("OUTPUT_HEIGHT").ok()
        .and_then(|v| v.parse().ok()).unwrap_or(640u32);
    vec![
        StreamInfo { name: "virtual-cam-rear".into(),     width: w, height: h, fps: 30 },
        StreamInfo { name: "virtual-cam-front".into(),    width: w, height: h, fps: 30 },
        StreamInfo { name: "virtual-cam-tracking".into(), width: w, height: h, fps: 30 },
    ]
}

// ── Client handler ────────────────────────────────────────────────────────────

async fn handle_client(
    stream:     tokio_rustls::server::TlsStream<TcpStream>,
    peer:       SocketAddr,
    auth_keys:  Arc<Vec<ed25519_dalek::VerifyingKey>>,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    info!("[{peer}] Connected");
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // ── 1. Send challenge ─────────────────────────────────────────────────────
    let nonce = make_nonce();
    let nonce_b64 = B64.encode(nonce);
    let challenge = Challenge {
        nonce:   nonce_b64.clone(),
        streams: available_streams(),
    };
    let challenge_json = serde_json::to_string(&challenge)? + "\n";
    writer.write_all(challenge_json.as_bytes()).await?;

    // ── 2. Read auth response ─────────────────────────────────────────────────
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let response: AuthResponse = serde_json::from_str(line.trim())
        .context("Invalid auth response JSON")?;

    // ── 3. Verify signature ───────────────────────────────────────────────────
    let sig_ok = verify_response(&response.pubkey, &nonce_b64, &response.signature).is_ok();
    let authorized = sig_ok && is_authorized(&response.pubkey, &auth_keys);

    if !authorized {
        warn!("[{peer}] Auth failed (pubkey: {}...)", &response.pubkey[..8.min(response.pubkey.len())]);
        let result = AuthResult {
            ok: false,
            message: "Not authorized. Add your public key to the server's authorized_keys.".into(),
            width: 0, height: 0, fps: 0, codec: String::new(),
        };
        writer.write_all((serde_json::to_string(&result)? + "\n").as_bytes()).await?;
        return Ok(());
    }
    info!("[{peer}] Authenticated (stream: {})", response.stream);

    // ── 4. Find requested stream ──────────────────────────────────────────────
    let streams = available_streams();
    let stream_info = streams.iter()
        .find(|s| s.name == response.stream)
        .cloned()
        .unwrap_or_else(|| streams[0].clone());

    let result = AuthResult {
        ok:      true,
        message: "Authenticated. Stream starting.".into(),
        width:   stream_info.width,
        height:  stream_info.height,
        fps:     stream_info.fps,
        codec:   "raw-rgb".into(),
    };
    writer.write_all((serde_json::to_string(&result)? + "\n").as_bytes()).await?;

    // ── 5. Stream frames from the PipeWire virtual node ───────────────────────
    // Channel capacity = 1: the GStreamer thread produces frames; the async
    // writer consumes them.  Capacity 1 means at most ONE frame is queued
    // between producer and consumer — if the TLS write is slow, the GStreamer
    // thread sees a full channel and drops the frame rather than accumulating
    // an unbounded backlog of Vec<u8> allocations.
    //
    // This is intentional: for a live camera stream, a dropped frame is
    // always preferable to OOM.  The client will see a momentary stutter
    // rather than the phone running out of memory.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
    let node_name   = stream_info.name.clone();
    let frame_bytes = (stream_info.width * stream_info.height * 3) as usize;

    // Spawn a blocking thread for GStreamer.
    // The thread uses try_send (non-blocking) — if the channel is full it
    // drops the frame and moves on.  This prevents the GStreamer thread from
    // ever blocking on a slow TLS client.
    std::thread::spawn(move || {
        if let Err(e) = stream_frames_gst(&node_name, frame_bytes, tx) {
            error!("GStreamer error: {e}");
        }
    });

    // Forward frames to the TLS client
    let mut pts_ms = 0u64;
    while let Some(frame_data) = rx.recv().await {
        let header = FrameHeader {
            pts_ms,
            data_bytes: frame_data.len() as u32,
            keyframe:   true,
        };
        let header_json = serde_json::to_string(&header)? + "\n";
        writer.write_all(header_json.as_bytes()).await?;
        writer.write_all(&frame_data).await?;
        pts_ms += 1000 / stream_info.fps as u64;
    }

    info!("[{peer}] Stream ended");
    Ok(())
}

// ── GStreamer frame source ────────────────────────────────────────────────────

fn stream_frames_gst(
    node_name:   &str,
    _frame_bytes: usize,
    tx:          tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    use gstreamer::prelude::*;
    use gstreamer_app::AppSink;
    use gstreamer_video::VideoInfo;

    gstreamer::init()?;

    let pipeline_str = format!(
        "pipewiresrc target-object=\"{node_name}\" ! \
         videoconvert ! \
         video/x-raw,format=RGB ! \
         appsink name=sink max-buffers=2 drop=true sync=false"
    );

    let pipeline = gstreamer::parse::launch(&pipeline_str)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow!("Not a pipeline"))?;

    let appsink = pipeline.by_name("sink")
        .ok_or_else(|| anyhow!("No appsink"))?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("Not an AppSink"))?;

    appsink.set_caps(Some(&gstreamer::Caps::builder("video/x-raw").field("format","RGB").build()));
    appsink.set_sync(false);

    pipeline.set_state(gstreamer::State::Playing)?;
    pipeline.state(gstreamer::ClockTime::from_seconds(10)).0
        .map_err(|_| anyhow!("Pipeline did not reach PLAYING"))?;

    info!("Streaming from {node_name}");

    loop {
        let sample = match appsink.try_pull_sample(gstreamer::ClockTime::from_mseconds(200)) {
            Some(s) => s,
            None    => {
                if tx.is_closed() { break; }
                continue;
            }
        };

        let caps   = match sample.caps() { Some(c) => c, None => continue };
        let buffer = match sample.buffer() { Some(b) => b, None => continue };
        let map    = match buffer.map_readable() { Ok(m) => m, Err(_) => continue };
        let data   = map.as_slice().to_vec();

        // try_send: if the channel is full (slow TLS client), drop this frame.
        // Never block here — blocking would let Vec<u8> allocations accumulate
        // in the GStreamer thread's stack while downstream is stalled.
        match tx.try_send(data) {
            Ok(_)  => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                // Channel full → slow client. Drop frame, keep streaming.
                log::debug!("Client too slow — dropping frame");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
        }
    }

    let _ = pipeline.set_state(gstreamer::State::Null);
    Ok(())
}

// ── Key management commands ───────────────────────────────────────────────────

fn cmd_add_key(auth_keys_path: &PathBuf, key_line: &str) -> Result<()> {
    // Validate the key parses correctly
    auth::pubkey_from_str(key_line)?;

    let mut existing = if auth_keys_path.exists() {
        std::fs::read_to_string(auth_keys_path)?
    } else {
        String::new()
    };

    // Check for duplicates
    let new_key = key_line.split_whitespace().next().unwrap_or("");
    if existing.lines().any(|l| l.trim_start_matches('#').trim().starts_with(new_key)) {
        println!("Key already in authorized_keys.");
        return Ok(());
    }

    if !existing.ends_with('\n') && !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(key_line);
    existing.push('\n');

    std::fs::create_dir_all(auth_keys_path.parent().unwrap())?;
    std::fs::write(auth_keys_path, &existing)?;
    println!("Added: {key_line}");
    Ok(())
}

fn cmd_remove_key(auth_keys_path: &PathBuf, pattern: &str) -> Result<()> {
    let text = std::fs::read_to_string(auth_keys_path)?;
    let filtered: Vec<&str> = text.lines()
        .filter(|l| !l.contains(pattern))
        .collect();
    let removed = text.lines().count() - filtered.len();
    std::fs::write(auth_keys_path, filtered.join("\n") + "\n")?;
    println!("Removed {removed} key(s) matching '{pattern}'");
    Ok(())
}

fn cmd_list_keys(auth_keys_path: &PathBuf) -> Result<()> {
    if !auth_keys_path.exists() {
        println!("No authorized_keys file. Add a key with: camera-server add-key <pubkey>");
        return Ok(());
    }
    let text = std::fs::read_to_string(auth_keys_path)?;
    let mut n = 0;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') { continue; }
        n += 1;
        let parts: Vec<&str> = l.splitn(2, ' ').collect();
        let key_short = &parts[0][..16.min(parts[0].len())];
        let comment   = parts.get(1).copied().unwrap_or("(no comment)");
        println!("  {n}. {key_short}…  {comment}");
    }
    if n == 0 { println!("No keys in authorized_keys."); }
    Ok(())
}

fn cmd_show_fingerprint(cert_path: &PathBuf, key_path: &PathBuf) -> Result<()> {
    let (certs, _) = load_or_generate_tls(cert_path, key_path)?;
    let cert = certs.first().ok_or_else(|| anyhow!("No cert"))?;
    let fp = auth::cert_fingerprint(cert.as_ref());
    println!("TLS fingerprint: {fp}");
    println!("Clients pin this with: camera-client trust <server-ip> {fp}");
    Ok(())
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    ).init();

    let cli = Cli::parse();
    let cfg = config_dir(cli.config_dir.as_ref());
    let auth_keys_path = cfg.join("authorized_keys");
    let cert_path      = cfg.join("server.crt");
    let key_path       = cfg.join("server.key");

    // Handle subcommands
    match cli.command {
        Some(Cmd::AddKey { key_line }) => {
            return cmd_add_key(&auth_keys_path, &key_line);
        }
        Some(Cmd::RemoveKey { pattern }) => {
            return cmd_remove_key(&auth_keys_path, &pattern);
        }
        Some(Cmd::ListKeys) => {
            return cmd_list_keys(&auth_keys_path);
        }
        Some(Cmd::ShowFingerprint) => {
            return cmd_show_fingerprint(&cert_path, &key_path);
        }
        Some(Cmd::RegenCert) => {
            if cert_path.exists() { std::fs::remove_file(&cert_path)?; }
            if key_path.exists()  { std::fs::remove_file(&key_path)?; }
            return cmd_show_fingerprint(&cert_path, &key_path);
        }
        None => {}
    }

    // Load TLS and auth keys
    let acceptor = make_tls_acceptor(&cert_path, &key_path)?;

    // Print fingerprint so user knows what to give clients
    {
        let (certs, _) = load_or_generate_tls(&cert_path, &key_path)?;
        if let Some(cert) = certs.first() {
            let fp = auth::cert_fingerprint(cert.as_ref());
            info!("TLS fingerprint: {fp}");
            info!("  Add to client: camera-client trust <this-ip> {fp}");
        }
    }

    let auth_keys: Arc<Vec<_>> = Arc::new(
        if auth_keys_path.exists() {
            load_authorized_keys(&auth_keys_path)?
        } else {
            warn!("No authorized_keys — no clients will be accepted.");
            warn!("Add a client key: camera-server add-key \"<pubkey> my-desktop\"");
            vec![]
        }
    );

    let listener = TcpListener::bind(&cli.bind).await
        .with_context(|| format!("Failed to bind to {}", cli.bind))?;
    info!("camera-server listening on {} (TLS)", cli.bind);
    info!("Trusted clients: {}", auth_keys.len());

    // Limit concurrent streaming clients to prevent resource exhaustion.
    // Each client spawns a GStreamer thread + holds a TLS connection.
    // 8 clients × ~922KB/frame × 2 queued frames = ~15MB minimum; keep it reasonable.
    const MAX_CLIENTS: usize = 8;
    let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CLIENTS));
    info!("Max concurrent clients: {MAX_CLIENTS}");

    loop {
        let (stream, peer) = listener.accept().await?;

        // Reject immediately if at capacity — don't even do TLS handshake.
        let permit = match sem.clone().try_acquire_owned() {
            Ok(p)  => p,
            Err(_) => {
                warn!("[{peer}] Rejected — at max concurrent clients ({MAX_CLIENTS})");
                drop(stream);
                continue;
            }
        };

        let acceptor  = acceptor.clone();
        let auth_keys = Arc::clone(&auth_keys);

        tokio::spawn(async move {
            let _permit = permit; // held for duration of this connection
            match acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    if let Err(e) = handle_client(tls_stream, peer, auth_keys).await {
                        warn!("[{peer}] Error: {e}");
                    }
                }
                Err(e) => warn!("[{peer}] TLS handshake failed: {e}"),
            }
        });
    }
}
