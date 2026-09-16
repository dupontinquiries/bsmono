//! `demo` — the shell. Prints what the core is doing; no window, no GPU.
//!
//! Deliberately thin. Everything interesting is in lib.rs and is tested there;
//! this exists so you can watch a fold happen without launching a game.
use blackstar_demo::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let spin = args.iter().any(|a| a == "--spin");
    let car = moire();
    if spin {
        // Turntable: the cheapest way to check a model from every side, and
        // the thing that catches a hole you would only see from one angle.
        let v = View { w: 92, h: 34, ..Default::default() };
        let m = car.mesh(24, 8);
        for i in 0..24 {
            let yaw = std::f32::consts::TAU * i as f32 / 24.0;
            print!("\x1b[H\x1b[2J{}", v.render(&m, yaw, 0.30));
            println!("  {} — yaw {:>3.0}°   [ctrl-c]", car.name, yaw.to_degrees());
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        return;
    }
    println!("{} — {:.2} x {:.2} x {:.2} m, wheelbase {:.2}",
             car.name, car.length(), car.width(), car.height(), car.wheelbase());

    let mut fold = Fold::new(1200);
    let mut t: Ms = 0;
    fold.toggle(t);
    while fold.busy() {
        t += 100;
        fold.tick(t);
        let bar: String = std::iter::repeat('#')
            .take((fold.eased() * 28.0) as usize).collect();
        println!("  {:>5} ms  {:<28}  {:?}", t, bar, fold.form());
    }
    println!("  drivable={} walkable={}", fold.drivable(), fold.walkable());
    println!();
    print!("{}", View::default().render(&car.mesh(20, 7), 0.62, 0.30));
}
