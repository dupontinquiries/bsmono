#[cfg(feature = "app")]
fn main() -> eframe::Result<()> {
    use netcode::profile::Profile;
    use netcode::store::{ProfileService, SnapcoreStore};

    // identity + snapcore-DHT service (OpenDHT primary swaps into the first slot behind its feature)
    let me = Profile::new("me@local", whoami_name());
    let svc = ProfileService::new(SnapcoreStore::new(), SnapcoreStore::new());
    let mut centre = bsgames::ui::GameCentre::new(me, svc);

    eframe::run_native(
        "BigSilicon Games",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| Ok(Box::new(App { centre: std::mem::replace(&mut centre, dummy()) }))),
    )
}

#[cfg(feature = "app")]
fn whoami_name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "player".into())
}

#[cfg(feature = "app")]
fn dummy() -> bsgames::ui::GameCentre {
    use netcode::profile::Profile;
    use netcode::store::{ProfileService, SnapcoreStore};
    bsgames::ui::GameCentre::new(
        Profile::new("x", "x"),
        ProfileService::new(SnapcoreStore::new(), SnapcoreStore::new()),
    )
}

#[cfg(feature = "app")]
struct App {
    centre: bsgames::ui::GameCentre,
}
#[cfg(feature = "app")]
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        self.centre.ui(ctx);
    }
}

#[cfg(not(feature = "app"))]
fn main() {
    eprintln!("build with --features app for the eframe window (the lib builds headless)");
}
