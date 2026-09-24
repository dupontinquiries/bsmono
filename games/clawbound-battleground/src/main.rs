fn main() {
    let opts = clawbound::launch::LaunchOptions::from_process();
    // Before anything builds geometry, headless or windowed.
    clawbound::map::set_active(opts.map);
    if opts.screenshot {
        let res = if opts.rig_sheet { clawbound::headless::run_rig_sheet() } else { clawbound::headless::run() };
        if let Err(e) = res {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    clawbound::app::run(opts);
}
