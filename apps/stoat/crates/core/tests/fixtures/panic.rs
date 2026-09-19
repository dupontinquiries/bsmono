fn inner(v: &[u32], i: usize) -> u32 {
    v[i]
}

fn main() {
    let v = vec![1, 2, 3];
    let idx = std::env::args().count() + 6;
    println!("{}", inner(&v, idx));
}
