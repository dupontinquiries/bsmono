//! join — first contact with a phone that was wiped an hour ago.
//!
//! A fresh postmarketOS image has `user` / `147147` and no sshd running. The
//! existing [`fleet::provision_user_script`](crate::fleet::provision_user_script)
//! handles everything *after* you can log in; this is the part before that,
//! and it is the step the operator actually repeats.
//!
//! ## Why phone-side, and what it does not fix
//!
//! Enabling sshd is a command typed on the phone. So is
//! `curl -sS http://laptop:9-something/join/CODE | sh`. Neither removes the
//! typing, and pretending otherwise would be selling a convenience that does
//! not exist. What the join script buys is that the typing happens ONCE and
//! does everything: sshd up and persistent, fleet user created, key installed,
//! and — the part that matters for a fleet — the device reports its hostname
//! and host-key fingerprint back, so the laptop's roster registers it without
//! a LAN scan and pins the key it will later check for reflashes.
//!
//! ## The code is a bound on exposure, not a cryptographic handshake
//!
//! Be plain about this. The payload is served over HTTP on a local network.
//! The code authenticates the *request to the server*; it does not
//! authenticate the *server to the phone*. Somebody already on your LAN who
//! wins a race can serve a different script. That is the same trust model as
//! typing a password into an ssh prompt on that LAN, which is what this
//! replaces — it is not weaker, and it is not magic.
//!
//! What the code does buy is a short, single-use window instead of a standing
//! service: the join endpoint answers once per code and then stops.
//!
//! ## The default credentials are the real hazard
//!
//! `user` / `147147` is in the published documentation, so between imaging and
//! enrolment the device is open to anyone on the network who knows that — which
//! is everyone. The join script therefore locks the fleet account to key-only
//! auth and does NOT leave a password on it. It deliberately does not disable
//! password auth globally: that would strand `user`, which is the only way back
//! in when a join goes wrong, and locking yourself out of a test phone is worse
//! than the window you closed.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a code is good for. Long enough to walk to the phone and type it,
/// short enough that a forgotten one is not a standing invitation.
pub const CODE_TTL: Duration = Duration::from_secs(10 * 60);

/// Digits in a join code. Six is what a person will retype off a screen
/// without resenting it; the TTL and single use are what make six enough.
pub const CODE_LEN: usize = 6;

/// A pairing code, issued by the laptop and typed on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCode {
    pub code: String,
    /// Unix seconds when it stops being valid.
    pub expires_at: u64,
    /// Set once the phone has used it. Single-use: a code that still works
    /// after the phone joined is a code sitting on a screen somebody else can
    /// read.
    pub used: bool,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Format a code from raw entropy. Split out from generation so the digits are
/// testable without a random source.
pub fn code_from_bytes(bytes: &[u8]) -> String {
    let mut n: u64 = 0;
    for b in bytes.iter().take(8) {
        n = n.wrapping_mul(31).wrapping_add(*b as u64);
    }
    let modulo = 10u64.pow(CODE_LEN as u32);
    format!("{:0width$}", n % modulo, width = CODE_LEN)
}

impl JoinCode {
    /// Issue a code valid from `now`.
    pub fn issue(code: impl Into<String>, now: u64) -> JoinCode {
        JoinCode { code: code.into(), expires_at: now + CODE_TTL.as_secs(), used: false }
    }

    pub fn issue_now(code: impl Into<String>) -> JoinCode {
        JoinCode::issue(code, now_secs())
    }

    /// Would this code be accepted at `now`?
    pub fn valid_at(&self, now: u64) -> bool {
        !self.used && now < self.expires_at
    }

    /// Redeem it. Returns false — and changes nothing — if it was already used
    /// or has expired, so a replay cannot succeed by arriving twice.
    pub fn redeem(&mut self, offered: &str, now: u64) -> bool {
        if !self.valid_at(now) || offered.trim() != self.code {
            return false;
        }
        self.used = true;
        true
    }
}

/// What the phone sends back when the script has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinReport {
    pub hostname: String,
    /// The device's ssh host key fingerprint, so the roster can pin it and
    /// notice the next reflash.
    pub host_key: String,
    /// Architecture as the phone reports it (`uname -m`), mapped to a rust
    /// target by the caller — the phone does not know about triples.
    pub arch: String,
}

/// Map `uname -m` to the rust target this fleet builds for.
///
/// Unknown machines return `None` rather than a guess: a wrong triple produces
/// a binary that installs cleanly and then refuses to start, which is a much
/// worse failure than being asked which architecture this is.
pub fn triple_for_arch(arch: &str) -> Option<&'static str> {
    match arch.trim() {
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-gnu"),
        "x86_64" | "amd64" => Some("x86_64-unknown-linux-gnu"),
        "armv7l" | "armv7" => Some("armv7-unknown-linux-gnueabihf"),
        _ => None,
    }
}

/// The script the phone runs: `curl -sS <base>/join/<code> | sh`.
///
/// Idempotent throughout, because the second run is the common one — a join
/// that failed at the last step should be fixable by running it again rather
/// than by reflashing.
pub fn join_script(base_url: &str, code: &str, fleet_user: &str, pubkey: &str) -> String {
    let key = pubkey.trim();
    format!(
        "set -e\n\
         # sshd, now and after the next boot. Alpine/pmOS uses OpenRC; the\n\
         # systemd branch is there for images that are not.\n\
         if command -v rc-update >/dev/null 2>&1; then\n\
         \x20 sudo rc-update add sshd default 2>/dev/null || true\n\
         \x20 sudo rc-service sshd start 2>/dev/null || true\n\
         elif command -v systemctl >/dev/null 2>&1; then\n\
         \x20 sudo systemctl enable --now sshd 2>/dev/null || true\n\
         fi\n\
         # The fleet account: created if absent, never given a password, and\n\
         # locked so it is key-only. `user` keeps its password — it is the way\n\
         # back in when a join goes wrong.\n\
         if ! id -u '{fleet_user}' >/dev/null 2>&1; then\n\
         \x20 if command -v useradd >/dev/null 2>&1; then sudo useradd -m -s /bin/sh '{fleet_user}';\n\
         \x20 else sudo adduser -D -s /bin/sh '{fleet_user}'; fi\n\
         fi\n\
         sudo passwd -l '{fleet_user}' >/dev/null 2>&1 || true\n\
         H=$(getent passwd '{fleet_user}' | cut -d: -f6); [ -n \"$H\" ] || H=/home/{fleet_user}\n\
         sudo mkdir -p \"$H/.ssh\"; sudo chmod 700 \"$H/.ssh\"\n\
         # Appended only if absent, so re-running does not stack duplicates.\n\
         if ! sudo grep -qF '{key}' \"$H/.ssh/authorized_keys\" 2>/dev/null; then\n\
         \x20 printf '%s\\n' '{key}' | sudo tee -a \"$H/.ssh/authorized_keys\" >/dev/null\n\
         fi\n\
         sudo chmod 600 \"$H/.ssh/authorized_keys\"\n\
         sudo chown -R '{fleet_user}':'{fleet_user}' \"$H/.ssh\"\n\
         # Report back: hostname, host key, arch. This is what registers the\n\
         # device on the roster without a LAN scan.\n\
         FP=$(sudo ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub 2>/dev/null | awk '{{print $2}}')\n\
         curl -sS -X POST '{base_url}/joined/{code}' \\\n\
         \x20 -d \"hostname=$(hostname)\" -d \"host_key=$FP\" -d \"arch=$(uname -m)\" >/dev/null || true\n\
         echo gitmind-join-ok\n"
    )
}

/// The line the operator reads off the laptop and types on the phone.
pub fn join_command(base_url: &str, code: &str) -> String {
    format!("curl -sS {base_url}/join/{code} | sh")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1 laptop";

    #[test]
    fn a_code_is_single_use_and_a_replay_changes_nothing() {
        let mut c = JoinCode::issue("123456", 1_000);
        assert!(c.valid_at(1_000));
        assert!(c.redeem("123456", 1_100));
        // The phone joined. A code still live on a screen somebody else can
        // read is the thing single-use exists to prevent.
        assert!(!c.valid_at(1_200));
        assert!(!c.redeem("123456", 1_200), "a replay must not succeed");
        assert!(c.used);
    }

    #[test]
    fn an_expired_code_is_refused_and_a_wrong_one_does_not_burn_the_right_one() {
        let mut c = JoinCode::issue("123456", 0);
        let after = CODE_TTL.as_secs() + 1;
        assert!(!c.valid_at(after));
        assert!(!c.redeem("123456", after));

        // A typo must not consume the code — the operator retypes it, and a
        // fat-fingered digit that invalidated the pairing would send them back
        // to the laptop for a new one every time.
        let mut c = JoinCode::issue("123456", 0);
        assert!(!c.redeem("123457", 10));
        assert!(!c.used);
        assert!(c.redeem("123456", 10));
    }

    #[test]
    fn codes_are_the_length_a_person_will_retype() {
        let c = code_from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(c.len(), CODE_LEN);
        assert!(c.chars().all(|ch| ch.is_ascii_digit()));
        // Low entropy must still pad to full width, or the code shown and the
        // code typed differ in length and the operator thinks they misread it.
        assert_eq!(code_from_bytes(&[0]).len(), CODE_LEN);
        assert_eq!(code_from_bytes(&[0]), "000000");
    }

    #[test]
    fn an_unknown_architecture_asks_rather_than_guesses() {
        assert_eq!(triple_for_arch("aarch64"), Some("aarch64-unknown-linux-gnu"));
        assert_eq!(triple_for_arch(" arm64 "), Some("aarch64-unknown-linux-gnu"));
        assert_eq!(triple_for_arch("x86_64"), Some("x86_64-unknown-linux-gnu"));
        // A wrong triple installs cleanly and then refuses to start, which is
        // far worse than a question.
        assert_eq!(triple_for_arch("riscv64"), None);
        assert_eq!(triple_for_arch(""), None);
    }

    #[test]
    fn the_script_survives_being_run_twice() {
        let s = join_script("http://10.0.0.2:8099", "123456", "gitmind", KEY);
        // The key is appended only when absent: a re-run after a partial
        // failure must not stack duplicate authorized_keys lines.
        assert!(s.contains("grep -qF"));
        // User creation is guarded, so the second run does not fail on an
        // existing account and abort the rest under `set -e`.
        assert!(s.contains("if ! id -u 'gitmind'"));
        assert!(s.contains("set -e"));
    }

    #[test]
    fn the_fleet_account_is_key_only_and_user_keeps_its_way_back_in() {
        let s = join_script("http://h", "1", "gitmind", KEY);
        assert!(s.contains("passwd -l 'gitmind'"), "no password on the fleet account");
        // Global password auth is deliberately NOT disabled: that would strand
        // `user`, which is the only route back when a join goes wrong.
        assert!(!s.contains("PasswordAuthentication no"));
    }

    #[test]
    fn sshd_is_enabled_for_this_boot_and_the_next() {
        let s = join_script("http://h", "1", "gitmind", KEY);
        // Starting it without enabling it means the phone drops off the fleet
        // at the next reboot, which looks like a flaky device.
        assert!(s.contains("rc-update add sshd default"));
        assert!(s.contains("rc-service sshd start"));
        // Images that are not OpenRC still work.
        assert!(s.contains("systemctl enable --now sshd"));
    }

    #[test]
    fn the_device_reports_enough_to_register_itself() {
        let s = join_script("http://10.0.0.2:8099", "123456", "gitmind", KEY);
        // Hostname, host key and arch: the three things the roster needs, so a
        // fresh phone appears without a LAN scan and with its key already
        // pinned for the next reflash check.
        assert!(s.contains("hostname=$(hostname)"));
        assert!(s.contains("ssh_host_ed25519_key.pub"));
        assert!(s.contains("arch=$(uname -m)"));
        assert!(s.contains("/joined/123456"));
    }

    #[test]
    fn the_command_is_short_enough_to_type_on_a_phone() {
        let cmd = join_command("http://10.0.0.2:8099", "123456");
        assert_eq!(cmd, "curl -sS http://10.0.0.2:8099/join/123456 | sh");
        assert!(cmd.len() < 60, "this gets typed on a touch keyboard: {cmd}");
    }

    #[test]
    fn a_report_becomes_a_roster_entry() {
        // The join's whole point: the device ends up on the roster with its
        // key pinned and its triple known, without anyone scanning or typing
        // an address.
        let r = JoinReport {
            hostname: "fairphone".into(),
            host_key: "SHA256:abc".into(),
            arch: "aarch64".into(),
        };
        let triple = triple_for_arch(&r.arch).unwrap();
        let entry = crate::roster::Entry {
            id: "fp5".into(),
            label: "fp5".into(),
            hostname: r.hostname.clone(),
            ip: "10.0.0.5".into(),
            triple: triple.into(),
            enrollment: crate::reroll::Enrollment::Keyed,
            host_key: Some(r.host_key.clone()),
            online: true,
            // A phone that just joined was just imaged: its apk index is old.
            needs_refresh: true,
        };
        let mut roster = crate::roster::Roster::default();
        roster.add(entry).unwrap();
        // And it needs no pairing step in the next plan, because it just did one.
        let plan = crate::reroll::plan(&roster.devices(), &["voiceterm".to_string()]);
        assert_eq!(plan.enroll_count(), 0);
        assert_eq!(plan.install_count(), 1);
    }
}
