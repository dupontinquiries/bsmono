// Drop into examples/ to demo box-plot output.
use unendlich_shared::zone_stats::{ZoneStats, render_zone_rich, render_zone_boxplot};

fn main() {
    let label_width = 12;

    println!("=== Realistic bimodal (95% cheap, 5% expensive) ===");
    let mut bimodal = ZoneStats::default();
    for _ in 0..950 { bimodal.push(8_000); }
    for _ in 0..50  { bimodal.push(200_000); }
    println!("{}", render_zone_rich("shadow", label_width, &bimodal, Some(8_333)));
    println!("{}", render_zone_boxplot("shadow", label_width, &bimodal));

    println!();
    println!("=== Mostly tight, mild variability ===");
    let mut tight = ZoneStats::default();
    for i in 0..1000 { tight.push(3_000 + (i % 200) * 5); }  // 3.0..4.0ms
    println!("{}", render_zone_rich("cull", label_width, &tight, Some(5_000)));
    println!("{}", render_zone_boxplot("cull", label_width, &tight));

    println!();
    println!("=== Flat / steady-state ===");
    let mut flat = ZoneStats::default();
    for _ in 0..500 { flat.push(8_333); }
    println!("{}", render_zone_rich("vsync", label_width, &flat, Some(8_333)));
    println!("{}", render_zone_boxplot("vsync", label_width, &flat));

    println!();
    println!("=== Heavy tail (10% expensive, gradient) ===");
    let mut tail = ZoneStats::default();
    for _ in 0..900 { tail.push(8_000); }
    for i in 0..100 { tail.push(20_000 + i * 1000); }
    println!("{}", render_zone_rich("main", label_width, &tail, Some(8_333)));
    println!("{}", render_zone_boxplot("main", label_width, &tail));
}
