// Tablet (e-ink) builds set EINK_SHIM=1 (gitmind does); then eframe is the
// eink-kit shim, which has no glow, and glow_scene switches to its CPU-side
// stand-in with the same API.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(eink)");
    println!("cargo:rerun-if-env-changed=EINK_SHIM");
    if std::env::var("EINK_SHIM").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=eink");
    }
}
