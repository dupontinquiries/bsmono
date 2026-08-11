fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let (mut ok, mut skip) = (0, 0);
    let mut unparsed = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let n = e.unwrap().file_name().to_string_lossy().to_string();
        match hand_kit::parse(&n) {
            Some(_) => ok += 1,
            None if n.starts_with("Photo from") => skip += 1,
            None => unparsed.push(n),
        }
    }
    println!("parsed {ok}, skipped {skip} unlabelled, UNPARSED {}", unparsed.len());
    for u in &unparsed { println!("  {u}"); }
}
