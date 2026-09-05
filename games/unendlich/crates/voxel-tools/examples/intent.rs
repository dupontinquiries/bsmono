// The gate: what authorizes changing the world, and what does not.
use voxel_tools::intent::{authorize, corroborate, WorldIntent};

fn main() {
    println!("{:<42} {}", "player said", "gate");
    println!("{}", "-".repeat(78));
    for prose in [
        "make it warmer here",
        "flood this valley",
        "freeze the lake",
        "colder here",
        "a garden",
        "build a garden here",
        "a warm cosy cabin",
        "it is freezing, build me a shelter",
        "flood the moat around my castle",
        "it's too cold here",
        "I wish it were warmer",
        "flood the valley, then a castle on the island",
    ] {
        match authorize(prose) {
            Some(i) => println!("{prose:<42} AUTHORIZED -> world {}", i.verb()),
            None => println!("{prose:<42} build only"),
        }
    }

    println!("\nand when the model proposes one anyway:");
    for (prose, proposed) in [
        ("a garden", WorldIntent::Hotter),
        ("make it colder", WorldIntent::Hotter),
    ] {
        match corroborate(prose, proposed) {
            Ok(i) => println!("  ok: {}", i.verb()),
            Err(r) => println!("  refused: {}", r.message),
        }
    }
}
