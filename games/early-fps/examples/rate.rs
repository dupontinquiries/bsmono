fn main() {
    print!("{}", early_fps::mapeval::report(7));
    println!("\nbuilt-biome avg openness: {:.3}", early_fps::mapeval::built_openness(7));
}
