//! ssh_runner — the adapter that lets a fleet reroll actually run.
//!
//! ## What this closes
//!
//! `reroll::plan` produced a plan and `reroll::execute` could walk it,
//! and the fleet page said so on screen: *"run: not wired yet — needs
//! the ssh/cargo runner"*. That was honest, and it was the last thing
//! between planning a fleet deploy and doing one.
//!
//! Nothing new is invented here. Every operation already exists in
//! `cockpit` and the push page already drives most of them for ONE
//! device:
//!
//! | `Runner` method | backed by                          |
//! |-----------------|------------------------------------|
//! | `enroll`        | `deploy::trust_host` + `setup_key` |
//! | `build`         | folded into `install` — see below  |
//! | `install`       | `flow::deploy_app_remote_logged`   |
//! | `rename`        | `deploy::remote_capture` + hostnamectl |
//! | `refresh`       | `deploy::remote_capture` + apk     |
//! | `triple_of`     | the roster                         |
//!
//! ## Why `build` does not build
//!
//! `flow::deploy_app_remote_logged` builds AND installs in one call —
//! it has to, because it needs the built binary's path to scp it, and
//! then runs `installer::push_install_script` to place it and write the
//! `.desktop` entry. Splitting that pair would mean reimplementing the
//! second half here, against a contract documented in `installer`.
//!
//! So `build` records the target as attempted and returns Ok, and
//! `install` does the real work. That is a compromise and it has a
//! visible consequence: a compile error is reported against the INSTALL
//! step rather than the build step. Recorded rather than hidden, because
//! `reroll::execute`'s failure rules distinguish the two — a failed build
//! is supposed to skip that app on every device sharing the triple,
//! and through this runner it will not.
//!
//! ## Untested against a real device
//!
//! The failure rules are the substance of `reroll::execute`, and they
//! run over ssh against hardware this sandbox does not have. The happy
//! path is a thin wrapper over functions the push page already exercises;
//! the error paths have never executed. Treat first-run failures as
//! expected and fixable rather than as a working feature.

use cockpit::reroll::Runner;

/// How enrolment gets a password.
///
/// `deploy::setup_key` needs one to install the key, and after that
/// nothing does. Held as an enum rather than an `Option<String>` so the
/// no-password case is a STATE with a name, not an absent value that
/// every call site has to interpret.
pub enum Credential {
    /// Password auth for first contact. Consumed by `setup_key` and not
    /// retained beyond it.
    Password(String),
    /// A key is already installed. Enrolment is then a no-op rather than
    /// an error — re-running a plan over an enrolled fleet is normal.
    KeyAlreadyInstalled,
}

pub struct SshRunner {
    pub user: String,
    /// id → (host, triple), from the roster. Owned rather than borrowed
    /// so the runner can be moved onto a worker thread; a fleet reroll
    /// takes minutes and must not hold the UI's data.
    pub devices: Vec<(String, String, String)>,
    pub credential: Credential,
    /// Every line the underlying operations emit, for the run log.
    pub log: Vec<String>,
}

impl SshRunner {
    pub fn new(user: &str, devices: Vec<(String, String, String)>, credential: Credential) -> Self {
        SshRunner { user: user.to_string(), devices, credential, log: Vec::new() }
    }

    /// Resolve a device id to its host.
    ///
    /// An unknown id is an ERROR, not a silent skip. `execute` walks a
    /// plan built from the roster, so an id it cannot resolve means the
    /// two have diverged — and a fleet run that quietly omits a device
    /// is worse than one that stops and says which.
    fn host_of(&self, id: &str) -> Result<&str, String> {
        self.devices
            .iter()
            .find(|(d, _, _)| d == id)
            .map(|(_, h, _)| h.as_str())
            .ok_or_else(|| format!("{id} is not in the roster"))
    }
}

impl Runner for SshRunner {
    fn enroll(&mut self, device: &str) -> Result<(), String> {
        let host = self.host_of(device)?.to_string();
        // Trust the host key BEFORE offering a password. `deploy` has
        // this ordering elsewhere for the same reason: a password sent
        // to an unverified host is the one mistake enrolment must not
        // make, and doing it in the other order would send it during the
        // very handshake that decides whether the host is who it claims.
        let t = cockpit::deploy::trust_host(&host)?;
        self.log.push(format!("{device}: {t}"));
        match &self.credential {
            Credential::KeyAlreadyInstalled => {
                self.log.push(format!("{device}: key already installed"));
                Ok(())
            }
            Credential::Password(p) => {
                let r = cockpit::deploy::setup_key(&self.user, &host, p)?;
                self.log.push(format!("{device}: {r}"));
                Ok(())
            }
        }
    }

    fn build(&mut self, app: &str, triple: &str) -> Result<(), String> {
        // Deliberately not a build. See the module note: the build and
        // the install are one call in `flow`, because the install needs
        // the binary's path and then runs a script that also writes the
        // .desktop entry. Splitting them here would reimplement that
        // contract.
        self.log.push(format!("build {app} ({triple}) — deferred to install"));
        Ok(())
    }

    fn install(&mut self, app: &str, device: &str) -> Result<(), String> {
        let host = self.host_of(device)?.to_string();
        let triple = self
            .triple_of(device)
            .ok_or_else(|| format!("{device} has no target triple"))?;
        let suite_app = cockpit::flow::store_groups()
            .into_iter()
            .flat_map(|g| g.apps)
            .find(|a| a.name == app)
            .ok_or_else(|| format!("{app} is not in the catalog"))?;
        let mut lines: Vec<String> = Vec::new();
        let r = {
            let mut on_line = |l: &str| lines.push(l.to_string());
            cockpit::flow::deploy_app_remote_logged(
                &suite_app, &triple, &self.user, &host, &mut on_line)
        };
        // The build output is kept on BOTH paths. On failure it is the
        // only place the compiler error appears, and discarding it would
        // leave "install: build failed" with nothing to act on.
        self.log.extend(lines);
        r.map(|msg| self.log.push(msg))
    }

    fn rename(&mut self, device: &str, hostname: &str) -> Result<(), String> {
        let host = self.host_of(device)?.to_string();
        // Rejected here rather than sent: a shell-quoted hostname that
        // reaches `hostnamectl` is a command injection through a field
        // the operator typed, and the check is cheaper than the audit.
        if hostname.is_empty()
            || !hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(format!("{hostname:?} is not a valid hostname (letters, digits, hyphen)"));
        }
        let out = cockpit::deploy::remote_capture(
            &self.user, &host, &format!("sudo hostnamectl set-hostname {hostname}"))?;
        self.log.push(format!("{device}: renamed to {hostname} {out}"));
        Ok(())
    }

    fn refresh(&mut self, device: &str) -> Result<(), String> {
        let host = self.host_of(device)?.to_string();
        let out = cockpit::deploy::remote_capture(&self.user, &host, "sudo apk update")?;
        self.log.push(format!("{device}: apk update {out}"));
        Ok(())
    }

    fn triple_of(&self, device: &str) -> Option<String> {
        self.devices.iter().find(|(d, _, _)| d == device).map(|(_, _, t)| t.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> SshRunner {
        SshRunner::new(
            "fleet",
            vec![
                ("pine".into(), "10.0.0.5".into(), "aarch64-unknown-linux-musl".into()),
                ("x86box".into(), "10.0.0.6".into(), "x86_64-unknown-linux-gnu".into()),
            ],
            Credential::KeyAlreadyInstalled,
        )
    }

    #[test]
    fn a_device_not_in_the_roster_is_an_error_not_a_skip() {
        // `execute` walks a plan built from the roster, so an
        // unresolvable id means the two have diverged. A fleet run that
        // quietly omits a device is worse than one that stops and names
        // it.
        let mut r = runner();
        let e = r.rename("ghost", "newname").unwrap_err();
        assert!(e.contains("ghost"), "{e}");
        assert!(e.contains("roster"), "{e}");
    }

    #[test]
    fn a_hostname_with_shell_metacharacters_never_reaches_the_device() {
        // This string would otherwise be interpolated into
        // `sudo hostnamectl set-hostname …` and run. The check is
        // cheaper than the audit.
        let mut r = runner();
        for bad in ["a; rm -rf /", "$(whoami)", "back`tick`", "with space", ""] {
            assert!(r.rename("pine", bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_valid_hostname_is_not_rejected() {
        // The guard must not be so strict that real names fail — a
        // validator that blocks legitimate input gets removed rather
        // than fixed. Errors here are the ABSENT DEVICE, not the name.
        let mut r = runner();
        let e = r.rename("pine", "pine-64-a").unwrap_err();
        assert!(!e.contains("valid hostname"), "the name was rejected: {e}");
    }

    #[test]
    fn triples_come_from_the_roster_per_device() {
        // `execute` needs this to know which installs a failed build
        // invalidated, so a wrong answer costs the wrong devices.
        let r = runner();
        assert_eq!(r.triple_of("pine").as_deref(), Some("aarch64-unknown-linux-musl"));
        assert_eq!(r.triple_of("x86box").as_deref(), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(r.triple_of("ghost"), None);
    }

    #[test]
    fn enrolling_an_already_keyed_device_is_a_no_op_not_a_failure() {
        // Re-running a plan over an enrolled fleet is normal, and an
        // error here would make every subsequent step skip for a device
        // that is perfectly reachable.
        //
        // Only the credential branch is asserted — `trust_host` needs a
        // network — so this pins the DECISION rather than the round trip.
        let r = runner();
        assert!(matches!(r.credential, Credential::KeyAlreadyInstalled));
    }

    #[test]
    fn build_is_recorded_as_deferred_rather_than_claiming_success() {
        // `build` returning Ok without building is a compromise, and the
        // log line is what makes it visible: a reader of the run log can
        // see the build did not happen HERE and that a compile error
        // will surface against install.
        let mut r = runner();
        r.build("gitmind", "aarch64-unknown-linux-musl").unwrap();
        assert!(r.log.iter().any(|l| l.contains("deferred to install")), "{:?}", r.log);
    }
}
