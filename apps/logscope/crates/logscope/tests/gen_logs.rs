/// gen_logs — test log file generator
///
/// Run with:  cargo run --example gen_logs -- [output_dir]
/// Generates several realistic log files in tests/logs/:
///
///   helix.log       — editor LSP traffic (like the real 165GB file)
///   nginx.log       — web server access + errors
///   systemd.log     — journald-style mixed services
///   rust_build.log  — cargo build JSON output
///   sparse.log      — mostly quiet with occasional bursts

use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};

fn main() {
    let out_dir = std::env::args().nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/logs")));

    std::fs::create_dir_all(&out_dir).expect("cannot create output dir");

    gen_helix(&out_dir.join("helix.log"),         5_000);
    gen_nginx(&out_dir.join("nginx.log"),          3_000);
    gen_systemd(&out_dir.join("systemd.log"),      4_000);
    gen_rust_build(&out_dir.join("rust_build.log"));
    gen_sparse(&out_dir.join("sparse.log"),        8_000);

    println!("Generated logs in {}", out_dir.display());
}

// ── generators ────────────────────────────────────────────────────────────────

fn gen_helix(path: &Path, n: usize) {
    let components = [
        "helix_lsp::transport",
        "helix_lsp::client",
        "helix_core::syntax",
        "helix_view::editor",
        "helix_term::application",
    ];
    let lsp_servers = ["rust-analyzer", "pylsp", "tsserver", "clangd"];
    let levels = ["DEBUG", "INFO", "WARN", "ERROR"];
    let level_weights = [60, 25, 10, 5]; // percentages

    let mut out = String::with_capacity(n * 120);
    let mut ts = Timestamp::new(2025, 3, 11, 16, 0, 0);

    for i in 0..n {
        let comp  = components[i % components.len()];
        let lsp   = lsp_servers[i % lsp_servers.len()];
        let level = weighted_pick(&levels, &level_weights);
        ts.advance_ms(fastrand_range(1, 800));

        // Simulate run of repeated LSP messages (the key pattern Logscope compresses)
        let msg = if i % 47 < 8 {
            format!("{lsp} err <- 'info: downloading component \\'{}\\''",
                ["cargo","clippy","rust-docs","rust-src","rustc","rustfmt"][i % 6])
        } else if i % 31 == 0 {
            format!("{lsp} stderr <- 'error[E{:04}]: {}'",
                i % 600,
                ["cannot borrow as mutable", "type mismatch", "use of moved value",
                 "missing lifetime specifier"][i % 4])
        } else if level == "ERROR" {
            format!("{lsp} connection error: {}", ["timeout", "broken pipe", "EOF"][i % 3])
        } else {
            format!("{lsp} <- {:?}", serde_like_value(i))
        };

        writeln!(out, "{} {} [{}] {}", ts, comp, level, msg).unwrap();
    }
    std::fs::write(path, out).unwrap();
}

fn gen_nginx(path: &Path, n: usize) {
    let ips = ["10.0.0.1","192.168.1.42","172.16.0.99","203.0.113.7","198.51.100.3"];
    let paths = ["/", "/api/v1/users", "/static/app.js", "/health", "/api/v1/logs",
                 "/favicon.ico", "/api/v1/metrics", "/.env", "/wp-admin/", "/phpinfo.php"];
    let codes = [(200,70),(304,10),(404,8),(500,5),(302,4),(403,2),(429,1)];
    let methods = ["GET","GET","GET","POST","GET","DELETE","PUT"];
    let mut out = String::with_capacity(n * 100);
    let mut ts = Timestamp::new(2025, 3, 11, 0, 0, 0);

    for i in 0..n {
        ts.advance_ms(fastrand_range(10, 3000));
        let ip     = ips[i % ips.len()];
        let method = methods[i % methods.len()];
        let path   = paths[i % paths.len()];
        let code   = weighted_code(&codes);
        let bytes  = fastrand_range(200, 48000);
        let ms     = fastrand_range(1, 2000);

        if code >= 500 {
            writeln!(out,
                "{ts} [error] *{} connect() failed (111: Connection refused) \
                 while connecting to upstream, client: {ip}, request: \"{method} {path} HTTP/1.1\"",
                i).unwrap();
        } else {
            writeln!(out,
                "{ip} - - [{ts}] \"{method} {path} HTTP/1.1\" {code} {bytes} \"-\" \
                 \"Mozilla/5.0\" {ms}ms").unwrap();
        }
    }
    std::fs::write(path, out).unwrap();
}

fn gen_systemd(path: &Path, n: usize) {
    let units = [
        ("NetworkManager",  "INFO",  "connection 'eth0' activated"),
        ("sshd",            "INFO",  "Accepted publickey for user from 192.168.1.10 port 52341"),
        ("kernel",          "WARN",  "possible circular locking dependency detected"),
        ("systemd",         "INFO",  "Started Session 42 of user alice"),
        ("dbus-daemon",     "DEBUG", "Successfully activated service 'org.freedesktop.NetworkManager'"),
        ("cups",            "WARN",  "Printer 'HP_LaserJet' is not responding"),
        ("bluetoothd",      "ERROR", "Failed to set mode: Blocked through rfkill (0x12)"),
        ("avahi-daemon",    "INFO",  "Registering new address record for 192.168.1.5 on eth0"),
        ("thermald",        "WARN",  "CPU temperature 87C exceeds threshold 85C"),
        ("upower",          "INFO",  "battery: battery 79% (charging)"),
    ];
    let mut out = String::with_capacity(n * 100);
    let mut ts = Timestamp::new(2025, 3, 11, 6, 0, 0);

    for i in 0..n {
        ts.advance_ms(fastrand_range(50, 5000));
        let (unit, level, msg) = units[i % units.len()];
        writeln!(out, "{ts} {unit}[{}]: [{}] {msg}", 1000 + i, level).unwrap();
    }
    std::fs::write(path, out).unwrap();
}

fn gen_rust_build(path: &Path) {
    // Realistic cargo JSON message output
    let messages = [
        r#"{"reason":"compiler-message","package_id":"logscope 0.1.0","manifest_path":"/home/user/logscope/Cargo.toml","target":{"kind":["bin"],"name":"logscope"},"message":{"rendered":"warning: unused variable: `x`\n --> src/main.rs:42:9\n  |\n42 |     let x = 5;\n  |         ^ help: if this is intentional, prefix it with an underscore: `_x`","level":"warning","spans":[{"file_name":"src/main.rs","line_start":42,"line_end":42,"column_start":9,"column_end":10}]}}"#,
        r#"{"reason":"compiler-message","package_id":"logscope 0.1.0","manifest_path":"/home/user/logscope/Cargo.toml","target":{"kind":["bin"],"name":"logscope"},"message":{"rendered":"error[E0502]: cannot borrow `pol` as immutable because it is also borrowed as mutable\n --> src/ui/rotation_tab.rs:503:63\n  |\n503|                     let cfg = pol.pipeline.get_or_insert_with(|| PipelineConfig {\n  |                               ---          ------------------ ^^ immutable borrow occurs here","level":"error","spans":[{"file_name":"src/ui/rotation_tab.rs","line_start":503,"line_end":504}]}}"#,
        r#"{"reason":"compiler-artifact","package_id":"gtk4 0.9.0","manifest_path":"/home/user/.cargo/registry/src/gtk4-0.9.0/Cargo.toml","target":{"kind":["lib"],"name":"gtk4"},"filenames":["/home/user/logscope/target/release/deps/libgtk4.rlib"],"fresh":false}"#,
        r#"{"reason":"build-script-executed","package_id":"gtk4-sys 0.9.0","linked_libs":["gtk-4"],"linked_paths":["/usr/lib/x86_64-linux-gnu"]}"#,
        r#"{"reason":"compiler-message","package_id":"logscope 0.1.0","manifest_path":"/home/user/logscope/Cargo.toml","target":{"kind":["bin"],"name":"logscope"},"message":{"rendered":"error[E0308]: mismatched types\n --> src/analyzer.rs:88:12\n  |\n88 |         Ok(result)\n  |            ^^^^^^ expected `String`, found `&str`","level":"error","spans":[{"file_name":"src/analyzer.rs","line_start":88}]}}"#,
        r#"{"reason":"build-finished","success":false}"#,
    ];
    let out = messages.join("\n") + "\n";
    std::fs::write(path, out).unwrap();
}

fn gen_sparse(path: &Path, n: usize) {
    // Mostly heartbeats with occasional error bursts
    let mut out = String::with_capacity(n * 80);
    let mut ts = Timestamp::new(2025, 3, 11, 0, 0, 0);

    for i in 0..n {
        ts.advance_ms(if i % 100 < 3 { fastrand_range(1, 50) } else { 60_000 });

        let line = if i % 100 < 3 {
            // Error burst
            format!("[ERROR] database connection pool exhausted (pool_size=10, waiting={})", i % 50)
        } else if i % 200 == 0 {
            "[WARN] slow query detected: SELECT * FROM events WHERE ts > ? (847ms)".into()
        } else {
            format!("[INFO] heartbeat ok (uptime={}s, rss={}MB)", i * 60, 128 + i % 64)
        };
        writeln!(out, "{ts} {line}").unwrap();
    }
    std::fs::write(path, out).unwrap();
}

// ── helpers ───────────────────────────────────────────────────────────────────

struct Timestamp { y:u32,mo:u32,d:u32,h:u32,mi:u32,s:u32,ms:u32 }
impl Timestamp {
    fn new(y:u32,mo:u32,d:u32,h:u32,mi:u32,s:u32) -> Self {
        Timestamp{y,mo,d,h,mi,s,ms:0}
    }
    fn advance_ms(&mut self, ms: u64) {
        let total = self.ms as u64 + ms;
        self.ms  = (total % 1000) as u32;
        let secs = total / 1000;
        self.s  += secs as u32;
        self.mi += self.s / 60;  self.s  %= 60;
        self.h  += self.mi / 60; self.mi %= 60;
        self.d  += self.h / 24;  self.h  %= 24;
    }
}
impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
               self.y,self.mo,self.d,self.h,self.mi,self.s,self.ms)
    }
}

fn fastrand_range(lo: u64, hi: u64) -> u64 {
    // Deterministic-ish pseudo-random (no dep needed)
    static SEED: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0x517cc1b727220a95);
    let s = SEED.fetch_add(0x9e3779b97f4a7c15, std::sync::atomic::Ordering::Relaxed);
    let v = s.wrapping_mul(0x6c62272e07bb0142).wrapping_add(0x62b821756295c58d);
    lo + (v >> 32) % (hi - lo + 1)
}

fn weighted_pick<'a>(items: &[&'a str], weights: &[u32]) -> &'a str {
    let total: u32 = weights.iter().sum();
    let mut r = fastrand_range(0, total as u64 - 1) as u32;
    for (item, &w) in items.iter().zip(weights) {
        if r < w { return item; }
        r -= w;
    }
    items[0]
}

fn weighted_code(codes: &[(u32, u32)]) -> u32 {
    let total: u32 = codes.iter().map(|(_, w)| w).sum();
    let mut r = fastrand_range(0, total as u64 - 1) as u32;
    for &(code, w) in codes {
        if r < w { return code; }
        r -= w;
    }
    200
}

fn serde_like_value(i: usize) -> &'static str {
    [
        "{ \"jsonrpc\": \"2.0\", \"method\": \"textDocument/didChange\" }",
        "{ \"jsonrpc\": \"2.0\", \"method\": \"textDocument/completion\" }",
        "{ \"jsonrpc\": \"2.0\", \"result\": null }",
        "Notification { method: 'window/logMessage', params: Object { ... } }",
        "{ \"id\": 42, \"result\": { \"capabilities\": {} } }",
    ][i % 5]
}
