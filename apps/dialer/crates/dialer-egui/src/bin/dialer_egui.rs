//! The deployable dialer build.
//!
//! `dialer-egui`          — ModemManager if present, else the simulated modem
//! `dialer-egui --mock`   — always the simulated modem (demo / desktop dev)
//! `dialer-egui --mm`     — ModemManager or exit 2 (for on-device debugging,
//!                          where a silent fallback would hide the real fault)

fn main() -> eframe::Result<()> {
    let mut choice = dialer_egui::BackendChoice::Auto;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--mock" => choice = dialer_egui::BackendChoice::Mock,
            #[cfg(feature = "mm")]
            "--mm" => choice = dialer_egui::BackendChoice::Mm,
            "--help" | "-h" => {
                eprintln!("dialer-egui [--mock | --mm]");
                return Ok(());
            }
            other => {
                eprintln!("dialer-egui: unknown flag {other} (try --help)");
                std::process::exit(2);
            }
        }
    }
    dialer_egui::run(choice)
}
