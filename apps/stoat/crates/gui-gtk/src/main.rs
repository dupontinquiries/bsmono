//! GTK shell. Same role as stoat-cli: own the main loop, render `Change`s, turn
//! input into calls on the session. No debugging logic lives here.
//!
//! Deliberately no composite templates, no GObject subclassing and no
//! `glib::clone!`: those are the three places gtk-rs has churned most between
//! releases (clone!'s syntax was replaced wholesale in 0.20). Plain builders and
//! `Rc::downgrade` compile unchanged across the versions I know of.

mod app;
mod pages;

use adw::prelude::*;

pub struct Cli {
    pub marker: Option<String>,
    pub page: Option<String>,
    pub program: Option<(std::path::PathBuf, Vec<String>)>,
}

fn main() -> gtk::glib::ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take = |name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        (i < args.len()).then(|| args.remove(i))
    };
    let cli = std::rc::Rc::new(Cli {
        marker: take("--marker"),
        page: take("--page"),
        program: (!args.is_empty()).then(|| (args.remove(0).into(), args)),
    });

    // Placeholder id; pick a reverse-DNS name you control before shipping a
    // .desktop file, since it also names the settings and cache directories.
    let application = adw::Application::builder().application_id("dev.local.Stoat").build();
    application.connect_activate(move |a| app::App::build(a, cli.clone()));
    // We parsed argv ourselves; do not let GApplication try to open the binary
    // path as a document.
    application.run_with_args::<&str>(&[])
}
