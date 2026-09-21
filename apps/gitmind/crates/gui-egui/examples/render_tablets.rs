// Render the Tablets page holding a failed deploy to /tmp/tablets.ppm, using
// eink-kit's CPU rasterizer as a headless screenshot tool.
use gitmind_egui::app::{App, Tab};
use eink_kit::present::{mem::MemPresent, Present};
fn main() {
    use cockpit::eink_fleet as ef;
    let mut app = App::default();
    app.tab = Tab::Tablets;
    *app.tablets.found.lock().unwrap() = Some((
        ef::parse_probe("machine=reMarkable Ferrari\narch=aarch64"),
        ef::parse_displays("qtfb=1\nappload_dir=/home/root/xovi/exthome/appload\nxovi_installed=1\nglibc=2.39\n"),
    ));
    app.tablets.last_result = "eink-demo exited right after launch:\neink: model PaperPro (GUESS from architecture (device tree said \"\"); set EINK_MODEL)\neink: EINK_BACKEND=qtfb but no QTFB_KEY: launch from AppLoad".into();
    let mut cfg = eink_kit::config::Config::from_env().unwrap();
    cfg.scale = 2.0;
    cfg.eink_theme = false; // gitmind's own look, not the e-ink preset
    let mut p = MemPresent::new(760, 3200, eink_kit::format::PixelFormat::Rgb888, true, true);
    let mut e = eink_kit::Engine::new(&cfg, p.caps(), "mem");
    let mut shell = eink_kit::FromCtx(|ctx: &egui::Context| app.ui(ctx));
    for t in 0..4u64 { e.frame(std::time::Duration::from_millis(t * 200), &mut shell, &mut p).unwrap(); }
    p.write_ppm(std::path::Path::new("/tmp/tablets.ppm")).unwrap();
}
