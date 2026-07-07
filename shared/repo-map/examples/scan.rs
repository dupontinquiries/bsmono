use repo_map::{scan_repo, CrateKind};
use std::path::Path;
fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let crates = scan_repo(Path::new(&root), &["apps", "shared", "games"]);
    let (mut members, mut standalone, mut cmake) = (0, 0, 0);
    for c in &crates {
        match c.kind {
            CrateKind::WorkspaceMember { .. } => members += 1,
            CrateKind::Standalone => standalone += 1,
            CrateKind::Cmake => cmake += 1,
        }
    }
    println!("crates indexed: {}  (members={members} standalone={standalone} cmake={cmake})", crates.len());
    println!("\nsample — the crates I added this session + one cmake game:");
    for want in ["netcode", "bsgames", "repo-map", "voice-kit", "cockpit", "blackstar"] {
        if let Some(c) = crates.iter().find(|c| c.name == want) {
            println!("  {:<12} {:<34} {}", c.name, c.path, format!("{:?}", c.kind).split('{').next().unwrap().trim());
            println!("               test: {}", c.test_argv().join(" "));
        } else {
            println!("  {:<12} NOT FOUND", want);
        }
    }
}
