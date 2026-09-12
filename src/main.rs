fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--screenshot") {
        if let Err(e) = clawbound::headless::run() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    clawbound::app::run();
}
