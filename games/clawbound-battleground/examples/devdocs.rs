// Regenerate every document in dev/ from the game's data (src/devdocs.rs).
//   cargo run --example devdocs
fn main() {
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dev");
    let n = clawbound::devdocs::write_all(&dev).expect("write dev docs");
    println!("{n} documents -> {}", dev.display());
}
