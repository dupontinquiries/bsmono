//! phys_probe (r201) — exercises the phys helper libraries end-to-end so
//! their behavior is observable without an in-game screen. Prints a few
//! physically-meaningful scenarios and asserts the outputs match the
//! textbook expectations. Run: cargo run --example phys_probe --release
use unendlich_engine::phys::{acoustics, fluid_forces, hydro, rope};

fn main() {
    let mut ok = true;
    let check = |label: &str, cond: bool, ok: &mut bool| {
        println!("  [{}] {}", if cond { "PASS" } else { "FAIL" }, label);
        if !cond { *ok = false; }
    };

    println!("ACOUSTICS:");
    let d = acoustics::propagation_delay(686.0);
    println!("    thunder 686 blocks away heard after {d:.2}s");
    check("distant thunder delayed ~2s", (d - 2.0).abs() < 0.05, &mut ok);
    let near_g = acoustics::distance_gain(5.0, 1.0, 0.01);
    let far_g = acoustics::distance_gain(80.0, 1.0, 0.01);
    println!("    gain 5b={near_g:.3}  80b={far_g:.4} (spreading + air absorption)");
    check("far source much quieter", far_g < near_g * 0.2, &mut ok);
    let cave = acoustics::box_room_rt60(30.0, 12.0, 30.0, 0.08); // big stone cavern
    let closet = acoustics::box_room_rt60(3.0, 3.0, 3.0, 0.4);   // small soft room
    println!("    RT60 cavern={cave:.2}s  closet={closet:.2}s");
    check("cavern rings far longer than closet", cave > closet * 5.0, &mut ok);

    println!("HYDRO:");
    let jet = hydro::torricelli_speed(5.0, hydro::GRAVITY);
    println!("    breach 5b below surface jets at {jet:.1} b/s");
    check("torricelli jet ~14.1 b/s at head 5", (jet - 14.142).abs() < 0.1, &mut ok);
    let q = hydro::orifice_flow_rate(5.0, 1.0, 0.6, hydro::GRAVITY);
    println!("    1b² orifice outflow {q:.1} b³/s");
    check("outflow positive and below ideal", q > 0.0 && q < jet, &mut ok);

    println!("FLUID FORCES:");
    let cork = fluid_forces::buoyant_accel(1.0, 0.3, 20.0);
    let stone = fluid_forces::buoyant_accel(1.0, 3.0, 20.0);
    println!("    cork net accel {cork:+.1}  stone net accel {stone:+.1} (b/s²)");
    check("cork rises, stone sinks", cork > 0.0 && stone < 0.0, &mut ok);

    println!("ROPE:");
    let mut r = rope::Rope::hanging([0.0, 20.0, 0.0], 10, 1.0);
    for _ in 0..800 { r.step([0.0, -20.0, 0.0], 1.0 / 60.0); }
    let drop = r.nodes[0].pos[1] - r.nodes[9].pos[1];
    println!("    10-node rope settled: drop {drop:.2}b, stretch err {:.4}", r.stretch_error());
    check("rope hangs ~9 blocks, taut", (drop - 9.0).abs() < 0.3 && r.stretch_error() < 0.1, &mut ok);

    println!("phys_probe: {}", if ok { "ALL PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
