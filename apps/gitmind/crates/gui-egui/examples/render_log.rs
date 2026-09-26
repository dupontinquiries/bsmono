// Render the Fleet page holding a finished build's log to /tmp/fleet-log.ppm
// (and, with LOG_OVERLAY=1, the full-window log), using eink-kit's CPU
// rasterizer as a headless screenshot tool. LOG_W / LOG_H set the size.
use gitmind_egui::app::{App, Tab};
use eink_kit::present::{mem::MemPresent, Present};

fn sample() -> Vec<String> {
    let mut v = vec!["$ cargo build -p gitmind-egui --features app".to_string()];
    for c in ["libc v0.2.155", "cfg-if v1.0.0", "proc-macro2 v1.0.86", "unicode-ident v1.0.12", "quote v1.0.36",
              "syn v2.0.72", "serde v1.0.204", "thiserror v2.0.12", "egui v0.34.1", "eframe v0.34.1"] {
        for k in 0..6 { v.push(format!("   Compiling {c}{}", if k == 0 { String::new() } else { format!(" (unit {k})") })); }
    }
    v.extend([
        "warning: unused variable: `x`",
        "  --> crates/cockpit/src/flow.rs:212:9",
        "    |",
        "212 |     let x = plan.len();",
        "    |         ^ help: if this is intentional, prefix it with an underscore: `_x`",
        "",
        "   Compiling cockpit v0.1.0 (/home/me/c20/apps/gitmind/crates/cockpit)",
        "error[E0308]: mismatched types",
        "   --> crates/gui-egui/src/pages/fleet.rs:344:31",
        "    |",
        "344 |         let n: usize = suite.apps.len() as u32;",
        "    |                -----   ^^^^^^^^^^^^^^^^^^^^^^^ expected `usize`, found `u32`",
        "    |                |",
        "    |                expected due to this",
        "",
        "error: could not compile `gitmind-egui` (lib) due to 1 previous error",
    ].map(String::from));
    v
}

fn main() {
    let w: u32 = std::env::var("LOG_W").ok().and_then(|s| s.parse().ok()).unwrap_or(1100);
    let h: u32 = std::env::var("LOG_H").ok().and_then(|s| s.parse().ok()).unwrap_or(900);
    let overlay = std::env::var("LOG_OVERLAY").is_ok();
    let mut app = App::default();
    app.tab = Tab::Fleet;
    // LOG_RUNNING=1: the task is still going and the strip's full-window log
    // is open (what "log" on the task strip shows from another tab).
    let running = std::env::var("LOG_RUNNING").is_ok();
    if running { app.tab = Tab::Files; }
    let lines = sample();
    let n = lines.len();
    app.task.run_logged(move |sink| {
        for l in &lines { sink.push(l); }
        if running { std::thread::sleep(std::time::Duration::from_secs(30)); }
        "build failed".into()
    });
    while if running { app.task.log().len() < n } else { app.task.is_running() } { std::thread::sleep(std::time::Duration::from_millis(5)); }
    let mut cfg = eink_kit::config::Config::from_env().unwrap();
    cfg.scale = 1.0;
    cfg.eink_theme = false;
    let mut p = MemPresent::new(w, h, eink_kit::format::PixelFormat::Rgb888, true, true);
    let mut e = eink_kit::Engine::new(&cfg, p.caps(), "mem");
    let mut shell = eink_kit::FromCtx(|ctx: &egui::Context| {
        if overlay { gitmind_egui::show::open_overlay(ctx, if running { "strip" } else { "fleet" }); }
        app.ui(ctx)
    });
    for t in 0..6u64 { e.frame(std::time::Duration::from_millis(t * 200), &mut shell, &mut p).unwrap(); }
    p.write_ppm(std::path::Path::new("/tmp/fleet-log.ppm")).unwrap();
}
