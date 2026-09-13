fn main() {
    let opts = clawbound::launch::LaunchOptions::from_process();
    if opts.screenshot {
        if let Err(e) = clawbound::headless::run() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    clawbound::app::run(opts);
}
