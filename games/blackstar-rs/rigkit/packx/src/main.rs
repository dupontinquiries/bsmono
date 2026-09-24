//! packx <chassis-dir> <pose-index> <out.bin>
//! Pull one pose's scene.bin out of a chassis's bots.pack.
mod botpack;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 { eprintln!("packx <chassis-dir> <pose> <out.bin>"); std::process::exit(2); }
    let blob = std::fs::read(std::path::Path::new(&a[1]).join("bots.pack"))
        .expect("read bots.pack");
    let i: usize = a[2].parse().unwrap();
    match botpack::decode_pose(&blob, i) {
        Some(b) => { std::fs::write(&a[3], &b).unwrap(); println!("pose {i}: {} bytes", b.len()); }
        None => { eprintln!("pose {i}: decode failed"); std::process::exit(1); }
    }
}
