//! `demo` — the shell. Prints what the core is doing; no window, no GPU.
//!
//! Deliberately thin. Everything interesting is in lib.rs and is tested there;
//! this exists so you can watch a fold happen without launching a game.
use blackstar_demo::*;

fn main() {
    let car = moire();
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
}
