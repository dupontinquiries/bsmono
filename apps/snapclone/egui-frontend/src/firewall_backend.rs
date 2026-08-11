//! firewall_backend — opening a port without asking the user to run anything,
//! from `FirewallPortManager`.
//!
//! Eight backends, *"tried in order, first success wins"*, because the same app
//! ships to Fedora, Ubuntu, Debian, Alpine, Arch, a Flatpak sandbox and a
//! container — and no two of them agree on how a firewall is configured.
//!
//! ## The order is by cost to the user, not by preference
//!
//! D-Bus services first (silent), then CLI tools (silent), then direct
//! `iptables` when already root (silent), and **`pkexec` last** — because that
//! one *"shows a Polkit authentication prompt"*. A password dialog during
//! startup is the most expensive possible outcome, so it is what you fall back
//! to, never what you try first.
//!
//! Getting this order wrong is not a performance problem. It is an app that
//! demands a password on a machine where `firewall-cmd` would have worked
//! silently, and users do not give that app a second chance.
//!
//! ## Detection is by presence, not by trying
//!
//! Same reasoning `power_tier` records: probing a D-Bus service nobody offers
//! costs a full timeout, and doing that eight times in sequence at startup is a
//! stall long enough to look like a hang.
//!
//! ## Failure is a state the app must survive
//!
//! There is no guarantee any backend exists. The port not opening is not fatal
//! — the peer may still reach us through the NAT punch — so `None` is a
//! reportable outcome rather than an error, and the app carries on.

/// How a port might be opened.
///
/// Ordered by cost to the user: earlier variants are silent, later ones are
/// increasingly intrusive.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Backend {
    /// firewalld over D-Bus — Fedora/RHEL/openSUSE/Arch.
    Firewalld,
    /// UFW over D-Bus — Ubuntu 24.04+.
    UfwDbus,
    /// XDG portal — inside a Flatpak sandbox.
    XdgPortal,
    /// `firewall-cmd` — firewalld without D-Bus, e.g. in containers.
    FirewalldCli,
    /// `ufw` — installed but no D-Bus service.
    UfwCli,
    /// Direct `iptables`, only when already root.
    IptablesDirect,
    /// `nft` — Debian 10+, Fedora 32+, Arch default.
    Nftables,
    /// `pkexec iptables` — shows a Polkit prompt. Last resort.
    Pkexec,
}

impl Backend {
    /// Does using this interrupt the user?
    pub fn is_silent(self) -> bool { self != Backend::Pkexec }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Firewalld => "firewalld (D-Bus)",
            Backend::UfwDbus => "UFW (D-Bus)",
            Backend::XdgPortal => "XDG portal",
            Backend::FirewalldCli => "firewalld (CLI)",
            Backend::UfwCli => "ufw (CLI)",
            Backend::IptablesDirect => "iptables (direct/root)",
            Backend::Nftables => "nftables (CLI)",
            Backend::Pkexec => "pkexec + iptables",
        }
    }
}

/// What the environment offers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Environment {
    pub has_firewalld_dbus: bool,
    pub has_ufw_dbus: bool,
    pub in_flatpak: bool,
    pub has_firewall_cmd: bool,
    pub has_ufw_cli: bool,
    pub has_iptables: bool,
    pub has_nft: bool,
    pub has_pkexec: bool,
    pub is_root: bool,
}

/// Pick a backend, or `None` if nothing is available.
///
/// The order is the whole function. Each check is a cheap presence test rather
/// than an attempt, because probing a D-Bus service nobody offers costs a full
/// timeout and doing that eight times at startup looks like a hang.
pub fn detect(env: &Environment) -> Option<Backend> {
    if env.has_firewalld_dbus { return Some(Backend::Firewalld); }
    if env.has_ufw_dbus { return Some(Backend::UfwDbus); }
    if env.in_flatpak { return Some(Backend::XdgPortal); }
    if env.has_firewall_cmd { return Some(Backend::FirewalldCli); }
    if env.has_ufw_cli { return Some(Backend::UfwCli); }
    // Direct iptables only when we already have the privilege. Without root it
    // fails with a permission error rather than prompting, which would burn the
    // attempt and land on pkexec anyway — one wasted process spawn.
    if env.is_root && env.has_iptables { return Some(Backend::IptablesDirect); }
    if env.has_nft { return Some(Backend::Nftables); }
    if env.has_pkexec && env.has_iptables { return Some(Backend::Pkexec); }
    None
}

/// A port to open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PortRequest {
    pub port: u16,
    pub tcp: bool,
}

impl PortRequest {
    pub fn protocol(&self) -> &'static str { if self.tcp { "tcp" } else { "udp" } }

    /// Is this a port we should be asking about at all?
    ///
    /// Port 0 is not a port, and a privileged port below 1024 is not something
    /// this app uses — asking for one is a bug that would prompt the user for a
    /// password to do something wrong.
    pub fn is_sane(&self) -> bool { self.port >= 1024 }
}

/// How a request ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Opened(Backend),
    /// Refused by the backend or the user.
    Denied(Backend),
    /// Nothing available to try.
    Unavailable,
    /// The request itself was wrong.
    Invalid,
}

impl Outcome {
    /// Should the app carry on regardless?
    ///
    /// Always. The port not opening is not fatal — the peer may still reach us
    /// through the NAT punch — so this is reportable rather than an error, and
    /// treating it as fatal would make a firewall the app cannot configure into
    /// an app that will not start.
    pub fn is_fatal(&self) -> bool { false }

    pub fn opened(&self) -> bool { matches!(self, Outcome::Opened(_)) }
}

/// Try to open a port.
pub fn request(env: &Environment, req: &PortRequest, succeeds: impl Fn(Backend) -> bool)
    -> Outcome {
    if !req.is_sane() { return Outcome::Invalid; }
    match detect(env) {
        None => Outcome::Unavailable,
        Some(b) if succeeds(b) => Outcome::Opened(b),
        Some(b) => Outcome::Denied(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment { Environment::default() }

    #[test]
    fn a_silent_backend_always_beats_a_password_prompt() {
        // Getting this order wrong is not a performance problem. It is an app
        // that demands a password on a machine where firewall-cmd would have
        // worked silently, and users do not give that app a second chance.
        let e = Environment { has_firewall_cmd: true, has_pkexec: true,
                              has_iptables: true, ..env() };
        assert_eq!(detect(&e), Some(Backend::FirewalldCli));
        assert!(detect(&e).unwrap().is_silent());
    }

    #[test]
    fn pkexec_is_only_reached_when_nothing_else_exists() {
        let e = Environment { has_pkexec: true, has_iptables: true, ..env() };
        assert_eq!(detect(&e), Some(Backend::Pkexec));
        assert!(!Backend::Pkexec.is_silent());
    }

    #[test]
    fn dbus_backends_come_before_their_own_cli_equivalents() {
        // Same tool, cheaper route: the D-Bus call needs no process spawn.
        let e = Environment { has_firewalld_dbus: true, has_firewall_cmd: true, ..env() };
        assert_eq!(detect(&e), Some(Backend::Firewalld));
        let e = Environment { has_ufw_dbus: true, has_ufw_cli: true, ..env() };
        assert_eq!(detect(&e), Some(Backend::UfwDbus));
    }

    #[test]
    fn a_flatpak_uses_the_portal_rather_than_a_cli_it_cannot_reach() {
        // Inside the sandbox the host's firewall-cmd is not on PATH, and even
        // if it were, running it would not affect the host.
        let e = Environment { in_flatpak: true, has_firewall_cmd: true,
                              has_nft: true, ..env() };
        assert_eq!(detect(&e), Some(Backend::XdgPortal));
    }

    #[test]
    fn direct_iptables_needs_root_and_is_skipped_without_it() {
        // Without root it fails with a permission error rather than prompting,
        // which burns the attempt and lands on pkexec anyway — one wasted
        // process spawn.
        let not_root = Environment { has_iptables: true, has_nft: true, ..env() };
        assert_eq!(detect(&not_root), Some(Backend::Nftables));
        let as_root = Environment { is_root: true, ..not_root };
        assert_eq!(detect(&as_root), Some(Backend::IptablesDirect));
    }

    #[test]
    fn pkexec_without_iptables_is_not_a_backend() {
        // The prompt would appear and then the command would fail — the worst
        // combination: interrupted the user AND achieved nothing.
        let e = Environment { has_pkexec: true, has_iptables: false, ..env() };
        assert_eq!(detect(&e), None);
    }

    #[test]
    fn a_machine_with_no_firewall_tooling_reports_none() {
        // Containers and minimal images. Not a failure.
        assert_eq!(detect(&env()), None);
    }

    #[test]
    fn failing_to_open_a_port_is_never_fatal() {
        // The peer may still reach us through the NAT punch. Treating this as
        // fatal turns a firewall the app cannot configure into an app that will
        // not start.
        assert!(!Outcome::Unavailable.is_fatal());
        assert!(!Outcome::Denied(Backend::Pkexec).is_fatal());
        assert!(!Outcome::Invalid.is_fatal());
    }

    #[test]
    fn a_denied_request_names_which_backend_denied_it() {
        // "Firewall request failed" is not a diagnosis; a user on Ubuntu needs
        // to know it was ufw that said no.
        let e = Environment { has_ufw_cli: true, ..env() };
        let out = request(&e, &PortRequest { port: 9876, tcp: true }, |_| false);
        assert_eq!(out, Outcome::Denied(Backend::UfwCli));
    }

    #[test]
    fn a_privileged_port_is_refused_before_anything_is_attempted() {
        // Asking for one is a bug, and on the pkexec path it would prompt the
        // user for a password to do something wrong.
        let e = Environment { has_pkexec: true, has_iptables: true, ..env() };
        assert_eq!(request(&e, &PortRequest { port: 80, tcp: true }, |_| true), Outcome::Invalid);
        assert_eq!(request(&e, &PortRequest { port: 0, tcp: true }, |_| true), Outcome::Invalid);
    }

    #[test]
    fn the_apps_own_port_is_sane() {
        assert!(PortRequest { port: 9876, tcp: true }.is_sane());
        assert!(PortRequest { port: 9877, tcp: false }.is_sane());
        assert!(PortRequest { port: 1024, tcp: true }.is_sane(), "the boundary is allowed");
    }

    #[test]
    fn the_protocol_string_matches_what_the_backends_expect() {
        // firewalld's AddPort and ufw both take lowercase tcp/udp; a capitalised
        // string is rejected by the backend rather than by us.
        assert_eq!(PortRequest { port: 9876, tcp: true }.protocol(), "tcp");
        assert_eq!(PortRequest { port: 9877, tcp: false }.protocol(), "udp");
    }

    #[test]
    fn a_successful_open_reports_which_backend_worked() {
        // Worth surfacing: it tells a user why the app needed a password on one
        // machine and not another.
        let e = Environment { has_firewalld_dbus: true, ..env() };
        let out = request(&e, &PortRequest { port: 9876, tcp: true }, |_| true);
        assert_eq!(out, Outcome::Opened(Backend::Firewalld));
        assert!(out.opened());
    }

    #[test]
    fn every_backend_has_a_label_a_user_could_read() {
        // These appear in a diagnostic the user is asked to report.
        for b in [Backend::Firewalld, Backend::UfwDbus, Backend::XdgPortal,
                  Backend::FirewalldCli, Backend::UfwCli, Backend::IptablesDirect,
                  Backend::Nftables, Backend::Pkexec] {
            assert!(!b.label().is_empty(), "{b:?}");
        }
    }
}
