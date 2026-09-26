// The intro's cue sheet for scoring it (src/devdocs.rs).
//   cargo run --example intro_cue_sheet [out.svg]   (default: dev/music/intro_cue_sheet.svg)
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "dev/music/intro_cue_sheet.svg".into());
    if let Some(d) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(d).expect("dir");
    }
    std::fs::write(&out, clawbound::devdocs::intro_cue_sheet()).expect("write");
    println!("cue sheet -> {out}");
}
