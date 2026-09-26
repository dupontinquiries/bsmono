//! cosmic_setup -- put the COSMIC session on a postmarketOS phone over the
//! network, beside Plasma Mobile, and never leave it half-done.
//!
//! What is verified (build.postmarketos.org, the postmarketOS 2025-03 update):
//! the UI metapackage `postmarketos-ui-cosmic` exists for aarch64 and pulls in
//! `cosmic-session`. What is NOT verified from here: how a given image's login
//! manager picks its default session -- so this installs COSMIC ALONGSIDE the
//! current UI and stops there; the person picks COSMIC at the login screen.
//! Plasma Mobile stays installed, so the way back is always there.
//!
//! A plan is patchset's shape -- `Step { on, what, cmd, gate, rollback }` --
//! run by `run_plan`: each step's gate must pass, and on a failure the
//! rollbacks of the steps already done run, newest first.
//!
//! Root: `apk add` needs it. gitmind's fleet account (`gitmind`, made by the
//! join) is key-only and has NO root rights; root work runs as the phone's
//! bootstrap account (`user`) with its password fed to `sudo -S` on ssh's
//! stdin -- as gitmind's own provisioning does (fleet::provision_remote_command)
//! -- never on the command line. Without a password, `as_root` tries already
//! root, `doas -n`, `sudo -n` (all non-interactive) and fails plainly.

use crate::patchset::{Gate, Step, Where};

/// The UI metapackage (verified for aarch64).
pub const PACKAGE: &str = "postmarketos-ui-cosmic";

/// How a plan gets root on the phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    /// Already root, or passwordless doas / sudo.
    NoPrompt,
    /// The bootstrap account's password, fed to `sudo -S` on stdin.
    PasswordOnStdin,
}

/// `cmd` as root, the way `root` says.
pub fn root_cmd(cmd: &str, root: Root) -> String {
    match root {
        Root::NoPrompt => as_root(cmd),
        Root::PasswordOnStdin => format!("sudo -S -p '' sh -c {}", crate::deploy::shell_quote(cmd)),
    }
}

/// `cmd` run as root without prompting: already root, else `doas -n`, else
/// `sudo -n` -- whichever works; a clear failure when none does.
pub fn as_root(cmd: &str) -> String {
    let q = crate::deploy::shell_quote(cmd);
    format!(
        "if [ \"$(id -u)\" -eq 0 ]; then sh -c {q}; \
         elif command -v doas >/dev/null 2>&1 && doas -n true 2>/dev/null; then doas -n sh -c {q}; \
         elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then sudo -n sh -c {q}; \
         else echo 'needs root without a password prompt (doas -n / sudo -n), or ssh in as root' >&2; exit 1; fi"
    )
}

/// The install plan.
pub fn install_plan(root: Root) -> Vec<Step> {
    vec![
        Step { on: Where::Device, what: "postmarketOS on aarch64",
               cmd: "grep -qi postmarketos /etc/os-release && uname -m".into(),
               gate: Gate::Contains("aarch64".into()), rollback: None },
        Step { on: Where::Device, what: "COSMIC is offered by this release",
               cmd: root_cmd(&format!("apk update >/dev/null 2>&1; apk search -x {PACKAGE}"), root),
               gate: Gate::Contains(PACKAGE.into()), rollback: None },
        Step { on: Where::Device, what: "record the current UI and sessions",
               cmd: "apk info 2>/dev/null | grep '^postmarketos-ui-' ; ls /usr/share/wayland-sessions/ 2>/dev/null; true".into(),
               gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "install COSMIC alongside the current UI",
               cmd: root_cmd(&format!("apk add {PACKAGE}"), root),
               gate: Gate::ExitZero, rollback: Some(root_cmd(&format!("apk del {PACKAGE}"), root)) },
        Step { on: Where::Device, what: "the COSMIC session is registered",
               cmd: "ls /usr/share/wayland-sessions/".into(),
               gate: Gate::Contains("cosmic".into()), rollback: Some(root_cmd(&format!("apk del {PACKAGE}"), root)) },
    ]
}

/// The way back: remove COSMIC (the other UI was never touched).
pub fn remove_plan(root: Root) -> Vec<Step> {
    vec![Step { on: Where::Device, what: "remove COSMIC",
                cmd: root_cmd(&format!("apk del {PACKAGE}"), root), gate: Gate::ExitZero, rollback: None }]
}

/// Did a step's output pass its gate?
pub fn passes(gate: &Gate, ok: bool, output: &str) -> bool {
    match gate {
        Gate::ExitZero => ok,
        Gate::Contains(s) => ok && output.contains(s.as_str()),
    }
}

/// Run `steps` with `run` (a command -> Ok(stdout) / Err(message); ssh in the
/// app, a fake in tests). On the first failing gate, run the rollbacks of the
/// failing step and every step already done, newest first, and return the
/// failure. `on_line` hears each step and its outcome.
pub fn run_plan(steps: &[Step], run: &mut dyn FnMut(&str) -> Result<String, String>, on_line: &mut dyn FnMut(&str)) -> Result<(), String> {
    for (i, s) in steps.iter().enumerate() {
        on_line(&format!("• {}", s.what));
        let (ok, out) = match run(&s.cmd) { Ok(o) => (true, o), Err(e) => (false, e) };
        if passes(&s.gate, ok, &out) {
            on_line("  ok");
            continue;
        }
        on_line(&format!("  FAILED: {}", out.trim()));
        for r in steps[..=i].iter().rev().filter_map(|s| s.rollback.as_deref()) {
            on_line("  rolling back");
            if let Err(e) = run(r) { on_line(&format!("  rollback failed too: {e}")); }
        }
        return Err(format!("{}: {}", s.what, out.trim()));
    }
    Ok(())
}

/// Over ssh, as gitmind reaches a device. With `password`, each command gets
/// it on stdin (for `sudo -S`); it never appears in an argument.
pub fn run_over_ssh(steps: &[Step], user: &str, host: &str, password: Option<&str>, on_line: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut run = |cmd: &str| -> Result<String, String> {
        match password {
            None => crate::deploy::remote_capture(user, host, cmd),
            Some(pw) => {
                use std::io::Write;
                let mut child = std::process::Command::new("ssh")
                    .args(crate::deploy::ssh_argv(user, host, cmd))
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn().map_err(|e| format!("ssh failed: {e}"))?;
                if let Some(mut si) = child.stdin.take() { let _ = writeln!(si, "{pw}"); }
                let out = child.wait_with_output().map_err(|e| format!("ssh failed: {e}"))?;
                if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) }
                else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
            }
        }
    };
    run_plan(steps, &mut run, on_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake phone: answers by substring, records every command.
    fn phone(answers: Vec<(&'static str, Result<&'static str, &'static str>)>) -> (impl FnMut(&str) -> Result<String, String>, std::rc::Rc<std::cell::RefCell<Vec<String>>>) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let l = log.clone();
        (move |cmd: &str| {
            l.borrow_mut().push(cmd.to_string());
            for (k, a) in &answers {
                if cmd.contains(k) { return a.map(str::to_string).map_err(str::to_string); }
            }
            Ok(String::new())
        }, log)
    }

    #[test]
    fn a_ready_phone_gets_cosmic_beside_its_ui() {
        let (mut run, log) = phone(vec![
            ("uname -m", Ok("aarch64")),
            ("apk search -x", Ok("postmarketos-ui-cosmic-2-r4")),
            ("ls /usr/share/wayland-sessions/", Ok("cosmic.desktop\nplasma-mobile.desktop")),
        ]);
        let mut lines = Vec::new();
        assert!(run_plan(&install_plan(Root::NoPrompt), &mut run, &mut |l| lines.push(l.to_string())).is_ok(), "{lines:?}");
        let cmds = log.borrow();
        assert!(cmds.iter().any(|c| c.contains("apk add postmarketos-ui-cosmic")));
        assert!(!cmds.iter().any(|c| c.contains("apk del")), "nothing rolled back");
        assert!(!cmds.iter().any(|c| c.contains("plasma") && c.contains("del")), "the current UI is never removed");
    }

    #[test]
    fn a_release_without_cosmic_stops_before_installing_anything() {
        let (mut run, log) = phone(vec![("uname -m", Ok("aarch64")), ("apk search -x", Ok(""))]);
        let err = run_plan(&install_plan(Root::NoPrompt), &mut run, &mut |_| {}).unwrap_err();
        assert!(err.contains("offered"), "{err}");
        assert!(!log.borrow().iter().any(|c| c.contains("apk add")), "no install attempted");
    }

    #[test]
    fn a_session_that_never_appears_is_rolled_back() {
        let (mut run, log) = phone(vec![
            ("uname -m", Ok("aarch64")),
            ("apk search -x", Ok("postmarketos-ui-cosmic-2-r4")),
            ("ls /usr/share/wayland-sessions/", Ok("plasma-mobile.desktop")),
        ]);
        assert!(run_plan(&install_plan(Root::NoPrompt), &mut run, &mut |_| {}).is_err());
        let cmds = log.borrow();
        let dels = cmds.iter().filter(|c| c.contains("apk del postmarketos-ui-cosmic")).count();
        assert!(dels >= 1, "COSMIC removed again: {cmds:?}");
        assert!(cmds.last().unwrap().contains("apk del"), "the rollback is the last thing done");
    }

    #[test]
    fn not_a_postmarketos_arm_phone_is_refused_first() {
        let (mut run, log) = phone(vec![("uname -m", Ok("x86_64"))]);
        assert!(run_plan(&install_plan(Root::NoPrompt), &mut run, &mut |_| {}).is_err());
        assert_eq!(log.borrow().len(), 1, "nothing after the check");
    }

    #[test]
    fn with_the_bootstrap_password_root_is_sudo_s_and_the_password_is_not_in_the_command() {
        let plan = install_plan(Root::PasswordOnStdin);
        let add = plan.iter().find(|s| s.cmd.contains("apk add")).unwrap();
        assert!(add.cmd.starts_with("sudo -S -p ''"), "{}", add.cmd);
        assert!(add.rollback.as_deref().unwrap().starts_with("sudo -S"), "the rollback gets root the same way");
        assert!(!plan.iter().any(|s| s.cmd.contains("147147")), "no password in any command");
    }

    #[test]
    fn root_is_asked_for_without_a_prompt() {
        let c = as_root("apk add x");
        assert!(c.contains("id -u") && c.contains("doas -n") && c.contains("sudo -n"), "{c}");
        assert!(!c.contains("sudo apk"), "never an interactive sudo");
    }
}
