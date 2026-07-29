fn main() {
    use logscope::cratecheck::*;
    use std::path::Path;
    let cached = scan_cache(Path::new("/home/claude/.cargo"));
    println!("local cache: {} crate names", cached.len());
    let empty = Default::default();
    for name in ["ratatui", "ratatouille", "serde", "wasm-bindgen-futures", "totally-made-up-xyz"] {
        let k = check_local(name, None, &empty, &cached);
        println!("  {name:24} {k:?}  needs_network={}", k.needs_network());
    }
    println!("\nlookup URL for ratatui: {:?}", sparse_index_url("ratatui"));
}
