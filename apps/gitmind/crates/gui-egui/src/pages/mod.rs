pub mod bluetooth;
pub mod phone_setup;
pub mod diagnostics;
pub mod share;
pub mod artifacts;
pub mod fleet;
pub mod network;
pub mod apps;
pub mod branches;
pub mod dupes;
pub mod files;
pub mod import;
pub mod push;
pub mod timeline;
pub mod settings;
pub mod tablets;

/// Every page's source, for the tests that read it.
///
/// A `TextEdit` with no explicit `id_salt` takes its id from where it lands in the layout,
/// and in a wrapping row at the wrap threshold typing one character can move it — which
/// changes the id, and egui tracks focus BY id. The field then ignores every key after the
/// first. No rendered frame shows that, so the check is a source check, and it needs the
/// sources.
pub const SOURCES: &[(&str, &str)] = &[
    ("bluetooth", include_str!("bluetooth.rs")),
    ("phone_setup", include_str!("phone_setup.rs")),
    ("diagnostics", include_str!("diagnostics.rs")),
    ("share", include_str!("share.rs")),
    ("artifacts", include_str!("artifacts.rs")),
    ("fleet", include_str!("fleet.rs")),
    ("network", include_str!("network.rs")),
    ("apps", include_str!("apps.rs")),
    ("branches", include_str!("branches.rs")),
    ("dupes", include_str!("dupes.rs")),
    ("files", include_str!("files.rs")),
    ("import", include_str!("import.rs")),
    ("push", include_str!("push.rs")),
    ("timeline", include_str!("timeline.rs")),
    ("settings", include_str!("settings.rs")),
    ("tablets", include_str!("tablets.rs")),
];
