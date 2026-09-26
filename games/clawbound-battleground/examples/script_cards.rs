// The voice script's cue cards and recording sheet (src/devdocs.rs).
//   cargo run --example script_cards [out dir]   (default: dev/voice_script)
fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "dev/voice_script".into()));
    std::fs::create_dir_all(&out).expect("out dir");
    let mut n = 0;
    for (path, body) in clawbound::devdocs::all() {
        if let Some(name) = path.strip_prefix("voice_script/") {
            std::fs::write(out.join(name), body).expect("write");
            n += 1;
        }
    }
    println!("{n} files -> {}", out.display());
}
