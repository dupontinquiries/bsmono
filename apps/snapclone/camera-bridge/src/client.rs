//! camera-client — receive a camera stream from camera-server, expose it in PipeWire.
//!
//! Runs on the desktop. Connects to camera-server on the phone over TLS,
//! authenticates with an Ed25519 key, then receives video frames and exposes
//! them as a PipeWire Video/Source node — visible in pw-cli, wpctl, OBS,
//! Firefox, Zoom, and any app that uses the camera picker.
//!
//! The PipeWire node is created immediately on startup and persists even when
//! the phone is disconnected (shows a black frame). It reconnects automatically.
//!
//! Setup:
//!   camera-client keygen                        # generate keypair
//!   camera-client trust <phone-ip> <fp>         # pin phone's TLS cert
//!   camera-server add-key "<pubkey> my-desktop" # on the phone
//!   camera-client connect <phone-ip>            # stream!
//!
//! The node appears in pw-cli as:
//!   node.name = "phone-camera"          (or --pw-node to customize)
//!   node.description = "Phone Camera (192.168.x.x)"
//!   media.class = "Video/Source"
//!   media.role = "Camera"

use camera_bridge_lib::auth;
use gstreamer;
use gstreamer_app;

use anyhow::{anyhow, Context, Result};
use auth::{
    AuthResponse, AuthResult, Challenge, FrameHeader,
    generate_identity, load_identity, pubkey_to_string, sign_challenge,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use log::{error, info, warn};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

// ── CLI ────────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "camera-client",
    about = "Connect to a camera-server and expose it as a PipeWire camera on this machine",
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,

    /// Config directory override
    #[arg(long)]
    config_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Generate a new Ed25519 identity keypair (run once per machine).
    /// Prints the public key to add to the server's authorized_keys.
    Keygen {
        #[arg(long, default_value = "")]
        comment: String,
    },

    /// Print the public key for the current identity.
    Pubkey,

    /// Trust a camera-server's TLS certificate by fingerprint.
    /// Get the fingerprint from: camera-server show-fingerprint
    Trust {
        server:      String,
        fingerprint: String,
    },

    /// Show all trusted servers.
    ListServers,

    /// List streams available on a server.
    ListStreams {
        server: String,
        #[arg(long, default_value = "7878")]
        port: u16,
    },

    /// Connect to a camera-server and expose it as a local PipeWire camera.
    ///
    /// The camera node is visible immediately in pw-cli list-objects, wpctl status,
    /// and any app's camera picker. It persists and auto-reconnects if the
    /// phone goes away.
    Connect {
        /// Phone's hostname or IP address
        server: String,

        #[arg(long, default_value = "7878")]
        port: u16,

        /// Which virtual camera stream to request
        #[arg(long, default_value = "virtual-cam-rear")]
        stream: String,

        /// PipeWire node name (default: phone-camera-<server>)
        #[arg(long)]
        pw_node: Option<String>,

        /// Human-readable label shown in camera pickers
        #[arg(long)]
        pw_label: Option<String>,

        /// Write to a v4l2loopback device instead of PipeWire
        #[arg(long)]
        v4l2: bool,

        /// v4l2loopback device path [default: /dev/video30]
        #[arg(long, default_value = "/dev/video30")]
        device: String,

        /// Seconds between reconnect attempts [default: 3]
        #[arg(long, default_value = "3")]
        retry_secs: u64,
    },
}

// ── Config paths ──────────────────────────────────────────────────────────────

fn config_dir(cli_override: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = cli_override { return p.clone(); }
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".config/camera-bridge");
    }
    ProjectDirs::from("", "", "camera-bridge")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".config/camera-bridge"))
}

// ── Trusted servers ───────────────────────────────────────────────────────────

fn trusted_servers_path(cfg: &PathBuf) -> PathBuf { cfg.join("trusted_servers") }

fn load_trusted_fingerprint(cfg: &PathBuf, host: &str) -> Option<String> {
    let text = std::fs::read_to_string(trusted_servers_path(cfg)).ok()?;
    text.lines().find_map(|l| {
        let mut p = l.splitn(2, ' ');
        match (p.next(), p.next()) {
            (Some(h), Some(fp)) if h == host => Some(fp.trim().to_string()),
            _ => None,
        }
    })
}

fn save_trusted_fingerprint(cfg: &PathBuf, host: &str, fp: &str) -> Result<()> {
    let path = trusted_servers_path(cfg);
    let existing = if path.exists() {
        std::fs::read_to_string(&path)?
            .lines()
            .filter(|l| !l.starts_with(host))
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    } else { String::new() };
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, format!("{existing}{host} {fp}\n"))?;
    Ok(())
}

// ── TLS with fingerprint pinning ──────────────────────────────────────────────

struct PinningVerifier { expected_fp: String }

impl rustls::client::danger::ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fp = auth::cert_fingerprint(end_entity.as_ref());
        if fp == self.expected_fp {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "Fingerprint mismatch\n  got:      {fp}\n  expected: {}",
                self.expected_fp
            )))
        }
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct)
        -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct)
        -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

fn make_tls_connector(fingerprint: &str) -> Result<TlsConnector> {
    let verifier = Arc::new(PinningVerifier { expected_fp: fingerprint.to_string() });
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

// ── Auth handshake ────────────────────────────────────────────────────────────

async fn do_auth(
    reader:  &mut BufReader<impl AsyncReadExt + Unpin>,
    writer:  &mut (impl AsyncWriteExt + Unpin),
    key:     &ed25519_dalek::SigningKey,
    stream:  &str,
) -> Result<(AuthResult, Vec<auth::StreamInfo>)> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let challenge: Challenge = serde_json::from_str(line.trim())
        .context("Invalid challenge JSON from server")?;
    let streams = challenge.streams.clone();

    let response = AuthResponse {
        pubkey:    pubkey_to_string(&key.verifying_key()),
        signature: sign_challenge(key, &challenge.nonce)?,
        stream:    stream.to_string(),
    };
    writer.write_all((serde_json::to_string(&response)? + "\n").as_bytes()).await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let result: AuthResult = serde_json::from_str(line.trim())
        .context("Invalid auth result JSON")?;

    Ok((result, streams))
}

// ── Persistent PipeWire output node ──────────────────────────────────────────
//
// The node is created ONCE and lives for the process lifetime.
// While connected it receives real frames; while disconnected it gets
// black frames at a slow rate so consumers see "no signal" rather than
// the node disappearing from pw-cli.

struct PwOutput {
    appsrc:   gstreamer_app::AppSrc,
    pipeline: gstreamer::Pipeline,
    width:    u32,
    height:   u32,
}

impl PwOutput {
    fn new(node_name: &str, label: &str, width: u32, height: u32) -> Result<Self> {
        use gstreamer::prelude::*;

        gstreamer::init()?;

        let pipeline_str = format!(
            "appsrc name=src format=time is-live=true stream-type=0 block=false \
               caps=\"video/x-raw,format=RGB,width={width},height={height},framerate=30/1\" ! \
             videoconvert ! video/x-raw,format=I420 ! \
             pipewiresink name=pw sync=false"
        );

        let pipeline = gstreamer::parse::launch(&pipeline_str)?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("Not a pipeline"))?;

        let pw_sink = pipeline.by_name("pw")
            .ok_or_else(|| anyhow!("No pipewiresink"))?;

        let props = gstreamer::Structure::builder("props")
            .field("media.class",      "Video/Source")
            .field("node.name",        node_name)
            .field("node.description", label)
            .field("media.role",       "Camera")
            .build();
        pw_sink.set_property("stream-properties", &props);

        let appsrc = pipeline.by_name("src")
            .ok_or_else(|| anyhow!("No appsrc"))?
            .downcast::<gstreamer_app::AppSrc>()
            .map_err(|_| anyhow!("Not an AppSrc"))?;

        appsrc.set_format(gstreamer::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);
        appsrc.set_max_bytes((width * height * 3) as u64);
        appsrc.set_block(false);

        pipeline.set_state(gstreamer::State::Playing)?;
        pipeline.state(gstreamer::ClockTime::from_seconds(5)).0
            .map_err(|_| anyhow!("PipeWire output pipeline did not reach PLAYING"))?;

        info!("PipeWire node '{node_name}' visible in pw-cli ({}×{})", width, height);
        info!("  label: '{label}'");
        info!("  media.class: Video/Source");

        Ok(Self { appsrc, pipeline, width, height })
    }

    fn push_frame(&self, rgb: &[u8]) -> Result<()> {
        use gstreamer::prelude::*;
        let mut buf = gstreamer::Buffer::with_size(rgb.len())
            .map_err(|_| anyhow!("Buffer alloc failed"))?;
        {
            let r = buf.get_mut().unwrap();
            let mut m = r.map_writable().map_err(|_| anyhow!("Map failed"))?;
            m.copy_from_slice(rgb);
        }
        let _ = self.appsrc.push_buffer(buf);
        Ok(())
    }

    /// Push a solid black frame (used while disconnected).
    fn push_black(&self) {
        let black = vec![0u8; (self.width * self.height * 3) as usize];
        let _ = self.push_frame(&black);
    }
}

impl Drop for PwOutput {
    fn drop(&mut self) {
        use gstreamer::prelude::*;
        let _ = self.appsrc.end_of_stream();
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}

// ── RGB → YUYV ────────────────────────────────────────────────────────────────

fn rgb_to_yuyv(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixels = (width * height) as usize;
    let mut out = Vec::with_capacity(pixels * 2);
    let mut i = 0;
    while i + 6 <= rgb.len() {
        let (r0,g0,b0) = (rgb[i] as f32, rgb[i+1] as f32, rgb[i+2] as f32);
        let (r1,g1,b1) = (rgb[i+3] as f32, rgb[i+4] as f32, rgb[i+5] as f32);
        let y0 = ( 0.299*r0+0.587*g0+0.114*b0).clamp(0.0,255.0) as u8;
        let y1 = ( 0.299*r1+0.587*g1+0.114*b1).clamp(0.0,255.0) as u8;
        let u  = (-0.14713*r0-0.28886*g0+0.436*b0+128.0).clamp(0.0,255.0) as u8;
        let v  = ( 0.615*r0-0.51499*g0-0.10001*b0+128.0).clamp(0.0,255.0) as u8;
        out.extend_from_slice(&[y0,u,y1,v]);
        i += 6;
    }
    while out.len() < pixels * 2 { out.extend_from_slice(&[16u8,128]); }
    out
}

// ── Connect loop (auto-reconnects) ────────────────────────────────────────────

async fn run_connect_loop(
    server:     String,
    port:       u16,
    stream:     String,
    key:        ed25519_dalek::SigningKey,
    connector:  TlsConnector,
    pw_out:     Option<Arc<PwOutput>>,   // None = v4l2 mode
    v4l2_dev:   Option<String>,          // Some = v4l2 mode
    retry_secs: u64,
) {
    let server_name: rustls::pki_types::ServerName = {
        let s = server.as_str();
        match s.parse::<std::net::IpAddr>() {
            Ok(ip) => rustls::pki_types::ServerName::IpAddress(ip.into()),
            Err(_) => rustls::pki_types::ServerName::try_from(s.to_owned())
                .expect("invalid server name"),
        }
    };

    let mut first = true;
    loop {
        if !first {
            // Push black frames while disconnected so the PipeWire node stays alive
            if let Some(ref pw) = pw_out {
                pw.push_black();
            }
            tokio::time::sleep(Duration::from_secs(retry_secs)).await;
        }
        first = false;

        let addr = match format!("{server}:{port}").to_socket_addrs() {
            Ok(mut a) => match a.next() {
                Some(a) => a,
                None    => { warn!("Cannot resolve {server}"); continue; }
            },
            Err(e) => { warn!("DNS error: {e}"); continue; }
        };

        let tcp = match TcpStream::connect(addr).await {
            Ok(t)  => t,
            Err(e) => { warn!("Cannot connect to {server}:{port}: {e}"); continue; }
        };

        let tls = match connector.connect(server_name.clone(), tcp).await {
            Ok(t)  => t,
            Err(e) => { warn!("TLS failed: {e}"); continue; }
        };

        let (r, mut w) = tokio::io::split(tls);
        let mut reader = BufReader::new(r);

        let (result, _streams) = match do_auth(&mut reader, &mut w, &key, &stream).await {
            Ok(r)  => r,
            Err(e) => { warn!("Auth error: {e}"); continue; }
        };

        if !result.ok {
            error!("Auth rejected: {}", result.message);
            error!("Check that your public key is in the server's authorized_keys.");
            error!("  camera-client pubkey   → copy this");
            error!("  camera-server add-key \"<pubkey> <comment>\"   → on the phone");
            return; // don't retry auth failures
        }

        let (w, h) = (result.width, result.height);
        info!("Connected. Receiving '{}' ({}×{} {}fps)", stream, w, h, result.fps);

        if let Err(e) = receive_frames(&mut reader, &pw_out, &v4l2_dev, w, h).await {
            warn!("Stream ended: {e}");
        }
        info!("Disconnected — retrying in {retry_secs}s...");
    }
}

async fn receive_frames(
    reader:   &mut BufReader<impl AsyncReadExt + Unpin>,
    pw_out:   &Option<Arc<PwOutput>>,
    v4l2_dev: &Option<String>,
    width:    u32,
    height:   u32,
) -> Result<()> {
    // Open v4l2 device if needed
    let mut v4l2_file = match v4l2_dev {
        Some(dev) => Some(
            std::fs::OpenOptions::new().write(true).open(dev)
                .with_context(|| format!("Cannot open {dev} — is v4l2loopback loaded?"))?
        ),
        None => None,
    };

    loop {
        // 5-second timeout per frame: catches network stalls and malformed headers
        // where read_line would otherwise block indefinitely waiting for '\n'.
        let mut header_line = String::new();
        let n = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            reader.read_line(&mut header_line),
        ).await
            .map_err(|_| anyhow!("Frame header timeout — server stalled?"))?
            .context("read_line error")?;
        if n == 0 { return Err(anyhow!("EOF")); }

        let header: FrameHeader = serde_json::from_str(header_line.trim())
            .context("Invalid frame header")?;

        if header.data_bytes > 10 * 1024 * 1024 {
            return Err(anyhow!("Frame too large: {} bytes — rejecting", header.data_bytes));
        }
        let mut buf = vec![0u8; header.data_bytes as usize];
        tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            reader.read_exact(&mut buf),
        ).await
            .map_err(|_| anyhow!("Frame data timeout"))?
            .context("read_exact error")?;

        // Push to PipeWire
        if let Some(ref pw) = pw_out {
            pw.push_frame(&buf)?;
        }

        // Push to v4l2
        if let Some(ref mut dev) = v4l2_file {
            use std::io::Write;
            let yuyv = rgb_to_yuyv(&buf, width, height);
            dev.write_all(&yuyv)?;
        }
    }
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
    let identity_path = cfg.join("identity");

    match cli.command {
        // ── keygen ────────────────────────────────────────────────────────────
        Cmd::Keygen { comment } => {
            let hostname = gethostname();
            let comment = if comment.is_empty() { hostname.clone() } else { comment };

            let pubkey = if identity_path.exists() {
                let k = load_identity(&identity_path)?;
                println!("Identity already exists: {}", identity_path.display());
                pubkey_to_string(&k.verifying_key())
            } else {
                let pk = generate_identity(&identity_path)?;
                println!("Generated identity: {}", identity_path.display());
                pk
            };

            let line = format!("{pubkey} {comment}");
            println!("\nAdd this key to the phone:");
            println!("  camera-server add-key \"{line}\"");
            println!("\nOr directly on the phone:");
            println!("  echo '{line}' >> ~/.config/camera-bridge/authorized_keys");
        }

        // ── pubkey ────────────────────────────────────────────────────────────
        Cmd::Pubkey => {
            let k = load_identity(&identity_path)
                .context("No identity. Run: camera-client keygen")?;
            println!("{}", pubkey_to_string(&k.verifying_key()));
        }

        // ── trust ─────────────────────────────────────────────────────────────
        Cmd::Trust { server, fingerprint } => {
            let fp = fingerprint.to_lowercase().replace(':', "");
            save_trusted_fingerprint(&cfg, &server, &fp)?;
            println!("Trusted {server}");
            println!("Connect: camera-client connect {server}");
        }

        // ── list-servers ──────────────────────────────────────────────────────
        Cmd::ListServers => {
            let path = trusted_servers_path(&cfg);
            if !path.exists() {
                println!("No trusted servers. Use: camera-client trust <ip> <fingerprint>");
                return Ok(());
            }
            println!("Trusted servers:");
            for line in std::fs::read_to_string(&path)?.lines() {
                let mut p = line.splitn(2, ' ');
                if let (Some(host), Some(fp)) = (p.next(), p.next()) {
                    println!("  {:<20} fingerprint: {}", host, &fp[..16.min(fp.len())]);
                }
            }
        }

        // ── list-streams ──────────────────────────────────────────────────────
        Cmd::ListStreams { server, port } => {
            let fp = load_trusted_fingerprint(&cfg, &server)
                .ok_or_else(|| anyhow!("'{server}' not trusted. Run: camera-client trust {server} <fp>"))?;
            let key = load_identity(&identity_path)
                .context("No identity. Run: camera-client keygen")?;

            let connector = make_tls_connector(&fp)?;
            let addr = format!("{server}:{port}").to_socket_addrs()?
                .next().ok_or_else(|| anyhow!("Cannot resolve {server}"))?;
            let tcp = TcpStream::connect(addr).await?;
            let sn = match server.parse::<std::net::IpAddr>() {
                Ok(ip) => rustls::pki_types::ServerName::IpAddress(ip.into()),
                Err(_) => rustls::pki_types::ServerName::try_from(server.clone())
                    .map_err(|_| anyhow!("Invalid server name"))?,
            };
            let tls = connector.connect(sn, tcp).await?;
            let (r, mut w) = tokio::io::split(tls);
            let mut reader = BufReader::new(r);

            // Auth with any stream name — we just want the challenge/stream list
            let (result, streams) = do_auth(&mut reader, &mut w, &key, "virtual-cam-rear").await?;

            if !result.ok {
                println!("Auth failed: {}", result.message);
                return Ok(());
            }

            println!("Streams available on {server}:");
            for s in &streams {
                println!("  {:30} {}×{} @ {}fps", s.name, s.width, s.height, s.fps);
            }
            println!("\nConnect with:");
            for s in &streams {
                println!("  camera-client connect {server} --stream {}", s.name);
            }
        }

        // ── connect ───────────────────────────────────────────────────────────
        Cmd::Connect { server, port, stream, pw_node, pw_label, v4l2, device, retry_secs } => {
            let fp = load_trusted_fingerprint(&cfg, &server)
                .ok_or_else(|| anyhow!(
                    "'{server}' not trusted.\nRun: camera-client trust {server} <fingerprint>\n\
                     Get the fingerprint from: camera-server show-fingerprint  (on the phone)"
                ))?;
            let key = load_identity(&identity_path)
                .context("No identity. Run: camera-client keygen")?;

            let connector = make_tls_connector(&fp)?;

            // Derive PipeWire node name and label
            let node_name = pw_node.unwrap_or_else(|| {
                // Sanitise server IP/hostname for use as a PipeWire node name
                let safe = server.replace('.', "-").replace(':', "-");
                format!("phone-camera-{safe}")
            });
            let label = pw_label.unwrap_or_else(|| {
                format!("Phone Camera ({})", server)
            });

            if v4l2 {
                // v4l2 mode — no persistent PipeWire node
                info!("Output: v4l2loopback {device}");
                info!("Connecting to {server}:{port}, stream={stream}");
                run_connect_loop(
                    server, port, stream, key, connector,
                    None, Some(device), retry_secs,
                ).await;
            } else {
                // PipeWire mode — create the node first so it's immediately visible,
                // then start streaming. The node persists across reconnects.

                // We need to know width/height to create the node.
                // Do a quick connect to get negotiated dimensions.
                let (w, h) = probe_dimensions(&server, port, &stream, &key, &connector).await
                    .unwrap_or((480, 640)); // FP5 default

                let pw_out = Arc::new(
                    PwOutput::new(&node_name, &label, w, h)
                        .context("Failed to create PipeWire output node")?
                );

                info!("PipeWire node active. Visible in:");
                info!("  pw-cli list-objects | grep {node_name}");
                info!("  wpctl status");
                info!("Connecting to {server}:{port}, stream={stream}");
                info!("Auto-reconnects every {retry_secs}s if the phone disconnects.");

                run_connect_loop(
                    server, port, stream, key, connector,
                    Some(pw_out), None, retry_secs,
                ).await;
            }
        }
    }

    Ok(())
}

/// Quick connect just to discover width/height, then disconnect.
async fn probe_dimensions(
    server:    &str,
    port:      u16,
    stream:    &str,
    key:       &ed25519_dalek::SigningKey,
    connector: &TlsConnector,
) -> Option<(u32, u32)> {
    let addr = format!("{server}:{port}").to_socket_addrs().ok()?.next()?;
    let tcp = TcpStream::connect(addr).await.ok()?;
    let sn = match server.parse::<std::net::IpAddr>() {
        Ok(ip) => rustls::pki_types::ServerName::IpAddress(ip.into()),
        Err(_) => rustls::pki_types::ServerName::try_from(server.to_owned()).ok()?,
    };
    let tls = connector.connect(sn, tcp).await.ok()?;
    let (r, mut w) = tokio::io::split(tls);
    let mut reader = BufReader::new(r);
    let (result, _) = do_auth(&mut reader, &mut w, key, stream).await.ok()?;
    if result.ok { Some((result.width, result.height)) } else { None }
}

fn gethostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
        .split('.')
        .next()
        .unwrap_or("desktop")
        .to_string()
}
