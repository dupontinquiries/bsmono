//! `unendlich-eink [seed]`: unendlich's world on an e-ink tablet (eink-kit).
//! `unendlich-eink --snapshot out.ppm [seed]` renders headless and exits.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = args.iter().skip(1).find_map(|a| a.parse::<u32>().ok()).unwrap_or(1337);
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "unendlich.ppm".into());
        std::process::exit(match snapshot(&out, seed) { Ok(()) => 0, Err(e) => { eprintln!("snapshot: {e}"); 1 } });
    }
    if let Err(e) = eink_kit::run(unendlich_eink::app::Unendlich::new(seed)) {
        eprintln!("unendlich-eink: {e}");
        std::process::exit(1);
    }
}

/// Run the real app on eink-kit's in-memory panel until the cache settles,
/// then write the panel as a PPM, with the refresh log eink-kit would send.
fn snapshot(out: &str, seed: u32) -> std::io::Result<()> {
    use eink_kit::present::{mem::MemPresent, Present};
    use std::time::{Duration, Instant};
    let cfg = eink_kit::config::Config::from_env().map_err(std::io::Error::other)?;
    let mut p = MemPresent::new(1620, 2160, eink_kit::format::PixelFormat::Rgb565, true, true);
    let mut e = eink_kit::Engine::new(&cfg, p.caps(), "mem");
    let mut app = unendlich_eink::app::Unendlich::new(seed);
    let t0 = Instant::now();
    let mut frames = 0;
    let mut updates = 0;
    while t0.elapsed() < Duration::from_secs(20) {
        let now = t0.elapsed();
        if e.wants_frame(now) {
            let r = e.frame(now, &mut app, &mut p)?;
            frames += 1;
            updates += r.updates.len();
        }
        updates += e.tick(now, &mut p)?.len();
        if app.cache().pending() == 0 && !e.wants_frame(t0.elapsed()) && frames > 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("snapshot: {frames} frames, {updates} panel updates, {} cached columns, last render {:.1} ms, {:.1} s",
        app.cache().len(), app.last_ms, t0.elapsed().as_secs_f32());
    p.write_ppm(std::path::Path::new(out))
}
