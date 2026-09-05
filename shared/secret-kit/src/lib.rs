//! secret-kit — WHERE a secret lives on this machine, and who can read it.
//!
//! The problem this exists for: an app that invents its own key file (say
//! `~/.config/<app>/mail/machine.key`) is invisible to the desktop. KDE's
//! wallet does not know about it, GNOME's keyring does not know about it,
//! and nothing on a phone shell does either — so the user is asked to sign
//! in again on every desktop they own, and there is no way for a wallet
//! prompt, a fingerprint reader, or a screen-lock policy to protect the
//! thing. That is the weakness in bridge-style mail clients generally.
//!
//! What is pure, and therefore lives here:
//!
//! * [`Desktop::detect_from`] — which shell is running, from the
//!   environment alone. KDE and GNOME are the easy ones; the phone shells
//!   (Phosh, Plasma Mobile, SXMO) matter because they are the case that is
//!   usually got wrong, and they are NOT their desktop parents: Phosh runs
//!   gnome-keyring, Plasma Mobile runs kwallet, SXMO usually runs neither.
//! * [`Plan::for_desktop`] — the ordered list of stores to TRY, with a
//!   reason for each. An ordered plan rather than one answer, because the
//!   right store is the first one that actually responds, and a wizard has
//!   to be able to say what it tried and why it fell back.
//! * [`file_store_path`] — where the fallback file goes, honouring
//!   `XDG_DATA_HOME`, plus [`legacy_paths`] so an existing key is found and
//!   migrated instead of silently re-prompting.
//!
//! What is NOT here: any D-Bus call, any actual read or write. Talking to
//! `org.freedesktop.secrets` needs a live session bus, which cannot be
//! tested in a sandbox and would drag a D-Bus dependency into every caller.
//! The transport implements [`SecretStore`]; this crate decides what to ask
//! and in what order, and that decision is what keeps getting made wrong.

pub mod wizard;

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The shell the user is actually running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    /// GNOME's mobile shell. Runs gnome-keyring like its parent.
    Phosh,
    /// KDE's mobile shell. Runs kwallet like its parent.
    PlasmaMobile,
    /// A tiling/minimal phone environment. Usually has no wallet at all,
    /// which is a real answer and not a failure to detect.
    Sxmo,
    /// Anything else with a session: Sway, XFCE, a bare WM. Secret Service
    /// may still be present — gnome-keyring is commonly installed alone.
    OtherSession,
    /// No graphical session: ssh, a container, a service. Never prompt.
    Headless,
}

impl Desktop {
    /// Detect from environment variables. Takes a map rather than reading
    /// the process environment so it is testable and so a caller can probe
    /// a different session (a systemd unit inspecting a user's).
    pub fn detect_from(env: &BTreeMap<String, String>) -> Desktop {
        let get = |k: &str| env.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
        // A graphical session at all? Checked FIRST: XDG_CURRENT_DESKTOP is
        // often inherited into ssh sessions and cron jobs by shell profiles,
        // and prompting a headless process for a wallet password hangs it.
        let graphical = get("WAYLAND_DISPLAY").is_some() || get("DISPLAY").is_some();
        if !graphical {
            return Desktop::Headless;
        }
        // The phone shells before their parents: Phosh sets
        // XDG_CURRENT_DESKTOP=GNOME too, and Plasma Mobile sets KDE.
        if get("PHOSH_RUNNING").is_some() {
            return Desktop::Phosh;
        }
        let current = get("XDG_CURRENT_DESKTOP").unwrap_or("").to_ascii_lowercase();
        let session = get("XDG_SESSION_DESKTOP").unwrap_or("").to_ascii_lowercase();
        let both = format!("{current}:{session}");
        if both.contains("phosh") {
            return Desktop::Phosh;
        }
        if both.contains("sxmo") {
            return Desktop::Sxmo;
        }
        if both.contains("plasmamobile") || both.contains("plasma-mobile") {
            return Desktop::PlasmaMobile;
        }
        if both.contains("kde") || both.contains("plasma") || get("KDE_FULL_SESSION").is_some() {
            return Desktop::Kde;
        }
        if both.contains("gnome") || get("GNOME_DESKTOP_SESSION_ID").is_some() {
            return Desktop::Gnome;
        }
        Desktop::OtherSession
    }

    pub fn label(self) -> &'static str {
        match self {
            Desktop::Kde => "KDE Plasma",
            Desktop::Gnome => "GNOME",
            Desktop::Phosh => "Phosh",
            Desktop::PlasmaMobile => "Plasma Mobile",
            Desktop::Sxmo => "SXMO",
            Desktop::OtherSession => "a graphical session",
            Desktop::Headless => "no graphical session",
        }
    }
}

/// A place a secret can be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// `org.freedesktop.secrets` over D-Bus. BOTH gnome-keyring and modern
    /// kwallet implement this, which is why it is first nearly everywhere:
    /// one code path serves both desktops.
    SecretService,
    /// KDE's own `org.kde.kwalletd6` interface. Only worth trying where
    /// kwallet is present but its Secret Service bridge is not — some
    /// distributions ship kwallet without `kwallet-secrets`.
    KWallet,
    /// An encrypted file under the app's data dir. Always available, never
    /// protected by the session lock; the honest last resort.
    EncryptedFile,
}

impl Store {
    pub fn label(self) -> &'static str {
        match self {
            Store::SecretService => "Secret Service (GNOME Keyring / KWallet)",
            Store::KWallet => "KWallet",
            Store::EncryptedFile => "encrypted file",
        }
    }

    /// Whether the session lock protects it. The wizard says this out loud:
    /// it is the difference the user is actually choosing between.
    pub fn locks_with_session(self) -> bool {
        !matches!(self, Store::EncryptedFile)
    }
}

/// One step of the plan: what to try, and why this one here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub store: Store,
    pub why: &'static str,
}

/// The ordered stores to try on a given desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub desktop: Desktop,
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn for_desktop(desktop: Desktop) -> Plan {
        let ss = Step {
            store: Store::SecretService,
            why: "the shared interface both GNOME Keyring and KWallet implement",
        };
        let kw = Step {
            store: Store::KWallet,
            why: "some distributions ship KWallet without its Secret Service bridge",
        };
        let file = Step {
            store: Store::EncryptedFile,
            why: "always available, but not protected by the session lock",
        };
        let steps = match desktop {
            // KDE: Secret Service first (one path for both desktops), then
            // kwallet directly for the installs missing the bridge.
            Desktop::Kde | Desktop::PlasmaMobile => vec![ss, kw, file],
            // GNOME and Phosh run gnome-keyring; kwallet will not be there.
            Desktop::Gnome | Desktop::Phosh => vec![ss, file],
            // SXMO typically has no wallet. Still TRY Secret Service — a user
            // may have installed gnome-keyring — but expect the file.
            Desktop::Sxmo | Desktop::OtherSession => vec![ss, file],
            // Never touch D-Bus without a session: a wallet prompt with no
            // desktop to draw it hangs the process.
            Desktop::Headless => vec![file],
        };
        Plan { desktop, steps }
    }

    pub fn detect(env: &BTreeMap<String, String>) -> Plan {
        Plan::for_desktop(Desktop::detect_from(env))
    }

    pub fn first(&self) -> Store {
        self.steps.first().map(|s| s.store).unwrap_or(Store::EncryptedFile)
    }

    /// True when every step is the unprotected fallback — the case worth
    /// warning about, because the user's secret will sit in a file that the
    /// screen lock does not cover.
    pub fn unprotected_only(&self) -> bool {
        self.steps.iter().all(|s| !s.store.locks_with_session())
    }

    /// One sentence for the wizard.
    pub fn explain(&self) -> String {
        let names: Vec<&str> = self.steps.iter().map(|s| s.store.label()).collect();
        format!("On {}: try {}.", self.desktop.label(), names.join(", then "))
    }
}

/// Where the fallback file goes. `XDG_DATA_HOME` if set, else
/// `~/.local/share`, then `<app>/<name>`. Data, not config: a key is not
/// something a user hand-edits or syncs to another machine as text.
pub fn file_store_path(env: &BTreeMap<String, String>, app: &str, name: &str) -> PathBuf {
    let base = env
        .get("XDG_DATA_HOME")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && s.starts_with('/'))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = env.get("HOME").map(|s| s.as_str()).unwrap_or("/tmp");
            PathBuf::from(home).join(".local/share")
        });
    base.join(app).join(name)
}

/// Older locations the same secret may already be at, newest first. Checked
/// before prompting: finding an existing key and migrating it is the
/// difference between "it just works on this machine" and "sign in again".
pub fn legacy_paths(env: &BTreeMap<String, String>, name: &str) -> Vec<PathBuf> {
    let home = env.get("HOME").map(|s| s.as_str()).unwrap_or("/tmp");
    vec![
        // The hand-rolled path the Qt mail stack used.
        PathBuf::from(home).join(".config/web2qml/mail").join(name),
        PathBuf::from(home).join(".config/voiceterm/mail").join(name),
    ]
}

/// What a transport must provide. Implemented by a D-Bus client, by a file
/// backend, and by a fake in tests. Deliberately tiny: a secret store is a
/// map of string to bytes with a lock on it.
pub trait SecretStore {
    /// Human name for messages ("GNOME Keyring").
    fn describe(&self) -> String;
    /// Is it actually reachable right now? A plan step that fails this is
    /// skipped, and the wizard says so.
    fn available(&self) -> bool;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String>;
    fn delete(&mut self, key: &str) -> Result<(), String>;
}

/// Walk a plan and return the first available store, with the ones that
/// were skipped and why — so a wizard reports rather than silently
/// degrading to a file.
pub fn choose<'a>(plan: &Plan, stores: &'a mut [Box<dyn SecretStore>])
    -> (Option<&'a mut Box<dyn SecretStore>>, Vec<String>)
{
    let mut skipped = Vec::new();
    let mut chosen: Option<usize> = None;
    for step in &plan.steps {
        if let Some(i) = stores.iter().position(|s| s.describe() == step.store.label()) {
            if stores[i].available() {
                chosen = Some(i);
                break;
            }
            skipped.push(format!("{} is not responding", step.store.label()));
        } else {
            skipped.push(format!("{} is not built in", step.store.label()));
        }
    }
    match chosen {
        Some(i) => (Some(&mut stores[i]), skipped),
        None => (None, skipped),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_phone_shells_are_not_mistaken_for_their_desktop_parents() {
        // Phosh sets XDG_CURRENT_DESKTOP=GNOME as well; the specific marker wins.
        let phosh = env(&[("WAYLAND_DISPLAY", "wayland-0"),
                          ("XDG_CURRENT_DESKTOP", "GNOME"), ("PHOSH_RUNNING", "1")]);
        assert_eq!(Desktop::detect_from(&phosh), Desktop::Phosh);
        let pm = env(&[("WAYLAND_DISPLAY", "wayland-0"),
                       ("XDG_CURRENT_DESKTOP", "KDE:plasmamobile")]);
        assert_eq!(Desktop::detect_from(&pm), Desktop::PlasmaMobile);
        let sxmo = env(&[("WAYLAND_DISPLAY", "wayland-0"), ("XDG_CURRENT_DESKTOP", "sxmo")]);
        assert_eq!(Desktop::detect_from(&sxmo), Desktop::Sxmo);
    }

    #[test]
    fn the_desktops_are_detected_by_any_of_their_usual_markers() {
        let kde = env(&[("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", "KDE")]);
        assert_eq!(Desktop::detect_from(&kde), Desktop::Kde);
        let kde2 = env(&[("DISPLAY", ":0"), ("KDE_FULL_SESSION", "true")]);
        assert_eq!(Desktop::detect_from(&kde2), Desktop::Kde);
        let gnome = env(&[("WAYLAND_DISPLAY", "wayland-0"), ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")]);
        assert_eq!(Desktop::detect_from(&gnome), Desktop::Gnome);
        let sway = env(&[("WAYLAND_DISPLAY", "wayland-0"), ("XDG_CURRENT_DESKTOP", "sway")]);
        assert_eq!(Desktop::detect_from(&sway), Desktop::OtherSession);
    }

    #[test]
    fn a_session_variable_inherited_into_ssh_does_not_make_it_graphical() {
        // The trap: shell profiles export XDG_CURRENT_DESKTOP into ssh
        // sessions and cron. Prompting for a wallet there hangs the process.
        let ssh = env(&[("XDG_CURRENT_DESKTOP", "KDE"), ("SSH_CONNECTION", "1.2.3.4 22")]);
        assert_eq!(Desktop::detect_from(&ssh), Desktop::Headless);
        assert_eq!(Plan::for_desktop(Desktop::Headless).steps.len(), 1);
        assert_eq!(Plan::for_desktop(Desktop::Headless).first(), Store::EncryptedFile);
        assert!(Plan::for_desktop(Desktop::Headless).unprotected_only());
    }

    #[test]
    fn kde_tries_kwallet_directly_and_gnome_never_does() {
        let kde = Plan::for_desktop(Desktop::Kde);
        assert_eq!(kde.first(), Store::SecretService, "one path serves both desktops first");
        assert!(kde.steps.iter().any(|s| s.store == Store::KWallet));
        let gnome = Plan::for_desktop(Desktop::Gnome);
        assert!(!gnome.steps.iter().any(|s| s.store == Store::KWallet),
                "kwallet is not on a GNOME box");
        // Every plan ends somewhere that always works.
        for d in [Desktop::Kde, Desktop::Gnome, Desktop::Phosh, Desktop::PlasmaMobile,
                  Desktop::Sxmo, Desktop::OtherSession, Desktop::Headless] {
            let p = Plan::for_desktop(d);
            assert_eq!(p.steps.last().unwrap().store, Store::EncryptedFile, "{d:?}");
            assert!(p.explain().contains(d.label()));
        }
        assert!(!Plan::for_desktop(Desktop::Kde).unprotected_only());
    }

    #[test]
    fn the_file_path_honours_xdg_and_legacy_locations_are_offered_first() {
        let e = env(&[("HOME", "/home/kes"), ("XDG_DATA_HOME", "/home/kes/.data")]);
        assert_eq!(file_store_path(&e, "voiceterm", "mail.key"),
                   PathBuf::from("/home/kes/.data/voiceterm/mail.key"));
        // Unset, empty, or relative XDG_DATA_HOME all fall back to the spec default.
        for bad in ["", "   ", "relative/path"] {
            let e2 = env(&[("HOME", "/home/kes"), ("XDG_DATA_HOME", bad)]);
            assert_eq!(file_store_path(&e2, "voiceterm", "mail.key"),
                       PathBuf::from("/home/kes/.local/share/voiceterm/mail.key"), "{bad:?}");
        }
        let legacy = legacy_paths(&e, "machine.key");
        assert!(legacy.iter().any(|p| p.ends_with("web2qml/mail/machine.key")),
                "the Qt stack's own path must be found, or the user signs in again");
    }

    struct Fake { name: &'static str, up: bool }
    impl SecretStore for Fake {
        fn describe(&self) -> String { self.name.to_string() }
        fn available(&self) -> bool { self.up }
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, String> { Ok(None) }
        fn set(&mut self, _: &str, _: &[u8]) -> Result<(), String> { Ok(()) }
        fn delete(&mut self, _: &str) -> Result<(), String> { Ok(()) }
    }

    #[test]
    fn choosing_skips_what_is_not_answering_and_says_what_it_skipped() {
        let plan = Plan::for_desktop(Desktop::Kde);
        let mut stores: Vec<Box<dyn SecretStore>> = vec![
            Box::new(Fake { name: Store::SecretService.label(), up: false }),
            Box::new(Fake { name: Store::KWallet.label(), up: true }),
            Box::new(Fake { name: Store::EncryptedFile.label(), up: true }),
        ];
        let (chosen, skipped) = choose(&plan, &mut stores);
        assert_eq!(chosen.map(|s| s.describe()), Some(Store::KWallet.label().to_string()));
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("not responding"), "{skipped:?}");

        // Nothing available at all is reported, not silently a file.
        let mut none: Vec<Box<dyn SecretStore>> = vec![
            Box::new(Fake { name: Store::SecretService.label(), up: false }),
        ];
        let (chosen, skipped) = choose(&plan, &mut none);
        assert!(chosen.is_none());
        assert_eq!(skipped.len(), 3, "every step accounted for: {skipped:?}");
        assert!(skipped.iter().any(|s| s.contains("not built in")));
    }
}
