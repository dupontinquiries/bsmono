//! fp5_setup -- replaying the owner's Fairphone 5 customisations after a reset.
//!
//! Everything below was done BY HAND to the phone over ssh in one session; none of it survives a
//! factory reset, and none of it was written down anywhere a person could replay. Each part is a
//! plan in `cosmic_setup`'s shape -- `Step { on, what, cmd, gate, rollback }` run by
//! [`crate::cosmic_setup::run_plan`] -- so gitmind can show each gate pass or fail and undo a
//! half-finished part.
//!
//! ## What is here, and where it came from
//!
//! * [`hatch_launcher_plan`] -- tideshell's apps button runs `hatch-egui --toggle`; the gitmind
//!   deploy installs the launcher as `hatch` (apps are installed under their directory name), so
//!   the button spawned a program that did not exist. An alias fixes a phone running an older
//!   tideshell; newer tideshell finds either name.
//! * [`rusty_board_service_plan`] -- the keyboard died with `layer-shell shell failed: Broken pipe`
//!   and stayed dead, because it was started by an XDG autostart entry with no restart policy.
//!   A unit with `Restart=always` owns it instead.
//! * [`audio_packages_plan`] -- the packages the speaker chain needs. NOT the whole story: this
//!   phone's sound server is PulseAudio, and the chain is a PipeWire filter-chain, so installing
//!   these does not by itself put anything on the speakers. That decision is the owner's.
//! * [`tideshell_swap_plan`] -- replacing the running shell safely. Its first gate exists because a
//!   deploy once restarted tideshell while the phone sat at the LOGIN SCREEN: no compositor, so
//!   it failed, looped, and the health check blamed the new build for an environmental failure.
//!
//! ## Not here, on purpose
//!
//! No passwords: root runs through [`crate::cosmic_setup::root_cmd`] (the password goes on ssh's
//! stdin, never into a command). The ssh key itself is the join step's job
//! ([`crate::join_server`]). Camera profiles are not shipped because none has been measured.

use crate::cosmic_setup::{root_cmd, Root};
use crate::patchset::{Gate, Step, Where};

/// Installed so far for the speaker chain (the dry run and the install are separate steps).
pub const AUDIO_PACKAGES: &[&str] = &[
    "pipewire-filter-graph", "pipewire-filter-graph-builtin", "pipewire-filter-graph-lv2",
    "bankstown-lv2", "lsp-plugins-lv2",
];

/// The PipeWire packages upgraded to match the filter-graph ones (1.6.8-r5 -> 1.6.9-r0). Left out
/// on purpose: `pipewire-spa-libcamera`, so the camera stack is not touched by an audio install.
pub const PIPEWIRE_ALIGN: &[&str] = &[
    "pipewire", "pipewire-libs", "pipewire-lang", "pipewire-udev", "pipewire-systemd", "pipewire-tools",
];

const BIN: &str = "$HOME/.local/bin";
const UNIT_DIR: &str = "$HOME/.config/systemd/user";

pub fn hatch_launcher_plan() -> Vec<Step> {
    vec![
        Step { on: Where::Device, what: "hatch is installed",
               cmd: format!("test -x \"{BIN}/hatch\""), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "hatch-egui alias for the launcher",
               cmd: format!("[ -e \"{BIN}/hatch-egui\" ] || ln -s hatch \"{BIN}/hatch-egui\""),
               gate: Gate::ExitZero,
               rollback: Some(format!("[ -L \"{BIN}/hatch-egui\" ] && rm \"{BIN}/hatch-egui\"; true")) },
        Step { on: Where::Device, what: "the alias resolves",
               cmd: format!("test -x \"{BIN}/hatch-egui\" && echo RESOLVES"),
               gate: Gate::Contains("RESOLVES".into()), rollback: None },
    ]
}

pub fn rusty_board_service_plan() -> Vec<Step> {
    let dropin = format!("{UNIT_DIR}/rusty-board.service.d/egui.conf");
    let autostart = "$HOME/.config/autostart/rusty-board.desktop";
    // OUTSIDE autostart/. Renaming it in place to `.desktop.disabled` does NOT disable it:
    // systemd's xdg-autostart generator still turns the renamed file into
    // `app-rusty\x2dboard.desktop.disabled@autostart.service` and starts it, so the keyboard ran
    // twice (the old entry plus the new unit) and two instances fight over the one input method.
    // Found on the phone, where this exact plan step had been done by hand.
    let retired_dir = "$HOME/.config/autostart-disabled";
    let retired = format!("{retired_dir}/rusty-board.desktop");
    vec![
        Step { on: Where::Device, what: "the egui keyboard binary is installed",
               cmd: format!("test -x \"{BIN}/rusty-board-egui\""), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "a restartable unit override (Restart=always)",
               cmd: format!("mkdir -p \"{UNIT_DIR}/rusty-board.service.d\" && printf '[Service]\\nExecStart=\\nExecStart={BIN}/rusty-board-egui\\nRestart=always\\nRestartSec=1\\n' > \"{dropin}\" && systemctl --user daemon-reload"),
               gate: Gate::ExitZero,
               rollback: Some(format!("rm -f \"{dropin}\"; systemctl --user daemon-reload")) },
        Step { on: Where::Device, what: "the autostart launcher (no restart policy) is retired",
               cmd: format!("mkdir -p \"{retired_dir}\" && {{ [ ! -e \"{autostart}\" ] || mv \"{autostart}\" \"{retired}\"; }}"),
               gate: Gate::ExitZero,
               rollback: Some(format!("[ ! -e \"{retired}\" ] || mv \"{retired}\" \"{autostart}\"")) },
        Step { on: Where::Device, what: "exactly one launcher owns the keyboard",
               // Nothing under autostart/ may still name rusty-board, whatever its extension.
               cmd: "ls \"$HOME/.config/autostart/\" /etc/xdg/autostart/ 2>/dev/null | grep -ci rusty-board | grep -qx 0 && echo ONE_OWNER".into(),
               gate: Gate::Contains("ONE_OWNER".into()), rollback: None },
        Step { on: Where::Device, what: "the unit is enabled and started",
               cmd: "systemctl --user enable --now rusty-board.service".into(),
               gate: Gate::ExitZero,
               rollback: Some("systemctl --user disable --now rusty-board.service".into()) },
        Step { on: Where::Device, what: "the keyboard is running",
               // `is-active` prints "inactive" too, and "inactive" contains "active": gate on a word
               // only a running unit makes us print.
               cmd: "sleep 3; systemctl --user is-active --quiet rusty-board.service && echo RUNNING".into(),
               gate: Gate::Contains("RUNNING".into()), rollback: None },
    ]
}

pub fn audio_packages_plan(root: Root) -> Vec<Step> {
    let add = AUDIO_PACKAGES.join(" ");
    let align = PIPEWIRE_ALIGN.join(" ");
    vec![
        Step { on: Where::Device, what: "postmarketOS on aarch64",
               cmd: "grep -qi postmarketos /etc/os-release && uname -m".into(),
               gate: Gate::Contains("aarch64".into()), rollback: None },
        Step { on: Where::Device, what: "the packages resolve (dry run, nothing installed)",
               cmd: root_cmd(&format!("apk add -s {add}"), root), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "install the speaker-chain packages",
               cmd: root_cmd(&format!("apk add {add}"), root), gate: Gate::ExitZero,
               rollback: Some(root_cmd(&format!("apk del {add}"), root)) },
        Step { on: Where::Device, what: "align PipeWire with them (camera plugin left alone)",
               cmd: root_cmd(&format!("apk upgrade {align}"), root), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "the bankstown bundle is installed",
               cmd: "test -d /usr/lib/lv2/bankstown.lv2 && echo BUNDLE".into(),
               gate: Gate::Contains("BUNDLE".into()), rollback: None },
    ]
}

/// Replace the running tideshell with the binary already uploaded to `staged` on the device.
pub fn tideshell_swap_plan(staged: &str) -> Vec<Step> {
    let live = format!("{BIN}/tideshell");
    let prev = format!("{BIN}/tideshell.prev");
    let restore = format!("cp -p \"{prev}\" \"{live}\" && systemctl --user restart tideshell");
    vec![
        Step { on: Where::Device, what: "a graphical session exists (not just the login screen)",
               // The greeter has its own compositor but the USER has no wayland socket until they log in.
               cmd: "ls /run/user/$(id -u)/ 2>/dev/null | grep -q '^wayland-' && echo SESSION".into(),
               gate: Gate::Contains("SESSION".into()), rollback: None },
        Step { on: Where::Device, what: "the new build's libraries resolve on this phone",
               cmd: format!("chmod +x \"{staged}\" && ! ldd \"{staged}\" 2>&1 | grep -q 'not found' && echo LIBS_OK"),
               gate: Gate::Contains("LIBS_OK".into()), rollback: None },
        Step { on: Where::Device, what: "keep the running build as tideshell.prev",
               cmd: format!("cp -p \"{live}\" \"{prev}\""), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "install the new build",
               cmd: format!("mv \"{staged}\" \"{live}\""), gate: Gate::ExitZero,
               rollback: Some(format!("cp -p \"{prev}\" \"{live}\"")) },
        Step { on: Where::Device, what: "restart the shell",
               cmd: "systemctl --user restart tideshell".into(), gate: Gate::ExitZero, rollback: Some(restore.clone()) },
        Step { on: Where::Device, what: "the shell is running",
               cmd: "sleep 8; systemctl --user is-active --quiet tideshell && echo RUNNING".into(),
               gate: Gate::Contains("RUNNING".into()), rollback: Some(restore) },
    ]
}

/// The camera's rear main sensor (the one with the lens motor), as libcamera names it.
pub const MAIN_CAMERA: &str = "/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29";

/// Install the snapshot-cam binary already uploaded to `staged`, with its launcher entry. The entry
/// carries the two settings the phone needs: the rear main camera as the source (PipeWire is not
/// running, so the probe finds none) and the clockwise quarter turn (the sensors are mounted at 270).
pub fn camera_app_plan(staged: &str) -> Vec<Step> {
    let live = format!("{BIN}/snapshot-cam");
    let prev = format!("{BIN}/snapshot-cam.prev");
    let wrapper = format!("{BIN}/snapshot-cam-phone");
    let entry = "$HOME/.local/share/applications/snapshot-cam.desktop";
    vec![
        Step { on: Where::Device, what: "the camera build's libraries resolve on this phone",
               cmd: format!("chmod +x \"{staged}\" && ! ldd \"{staged}\" 2>&1 | grep -q 'not found' && echo LIBS_OK"),
               gate: Gate::Contains("LIBS_OK".into()), rollback: None },
        Step { on: Where::Device, what: "keep the installed camera as snapshot-cam.prev",
               cmd: format!("[ ! -e \"{live}\" ] || cp -p \"{live}\" \"{prev}\""), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "install the camera app",
               cmd: format!("mv \"{staged}\" \"{live}\""), gate: Gate::ExitZero,
               rollback: Some(format!("[ ! -e \"{prev}\" ] || cp -p \"{prev}\" \"{live}\"")) },
        // A wrapper, not `Exec=env VAR="a b" ...`: the desktop spec cannot quote part of an argument.
        Step { on: Where::Device, what: "the camera's launcher wrapper",
               cmd: format!("printf '#!/bin/sh\\nexport SNAPSHOT_CAM_TURN=90\\nexport SNAPSHOT_CAM_SOURCE=\"gst:libcamerasrc camera-name={MAIN_CAMERA}\"\\nexec \"$HOME/.local/bin/snapshot-cam\" \"$@\"\\n' > \"{wrapper}\" && chmod +x \"{wrapper}\""),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{wrapper}\"")) },
        Step { on: Where::Device, what: "the camera's launcher entry",
               cmd: format!("mkdir -p \"$(dirname \"{entry}\")\" && printf '[Desktop Entry]\\nType=Application\\nName=Camera\\nComment=Snapshot camera: tap to focus, burst, colour profiles\\nExec=%s\\nIcon=camera-photo\\nTerminal=false\\nCategories=Graphics;Photography;\\nStartupNotify=true\\n' \"{wrapper}\" > \"{entry}\""),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{entry}\"")) },
        Step { on: Where::Device, what: "the app, its wrapper and its entry are in place",
               cmd: format!("test -x \"{live}\" && grep -q 'SNAPSHOT_CAM_TURN=90' \"{wrapper}\" && grep -q 'snapshot-cam-phone' \"{entry}\" && echo CAMERA_READY"),
               gate: Gate::Contains("CAMERA_READY".into()), rollback: None },
    ]
}

/// Replace hatch with the current build uploaded to `staged`. The September import on the phone was
/// older than the repo (no resident logic); `hatch-egui` was a symlink to it, which is put back on
/// rollback.
pub fn hatch_swap_plan(staged: &str) -> Vec<Step> {
    let live = format!("{BIN}/hatch-egui");
    vec![
        Step { on: Where::Device, what: "the hatch build's libraries resolve on this phone",
               cmd: format!("chmod +x \"{staged}\" && ! ldd \"{staged}\" 2>&1 | grep -q 'not found' && echo LIBS_OK"),
               gate: Gate::Contains("LIBS_OK".into()), rollback: None },
        Step { on: Where::Device, what: "install hatch-egui (replacing the alias to the old import)",
               cmd: format!("rm -f \"{live}\" && mv \"{staged}\" \"{live}\""), gate: Gate::ExitZero,
               rollback: Some(format!("rm -f \"{live}\"; ln -s hatch \"{live}\"")) },
        Step { on: Where::Device, what: "hatch runs",
               cmd: format!("\"{live}\" --help | grep -q quit-on-hide && echo HATCH_OK"),
               gate: Gate::Contains("HATCH_OK".into()), rollback: Some(format!("rm -f \"{live}\"; ln -s hatch \"{live}\"")) },
    ]
}

/// The address the postmarketOS `usb-moded` developer profile gives the phone on its USB gadget
/// network (the `usb0` interface); the laptop side is 172.16.42.2.
pub const USB_ADDR: &str = "172.16.42.1";
const EMERGENCY_DROPIN: &str = "/etc/ssh/sshd_config.d/60-emergency-usb.conf";

/// The sshd drop-in: password login for the owner, ONLY on the USB address. Everything else keeps
/// the machine's own rules. The password is the account's existing one (not stored here); a
/// separate backup key is added when one is staged at /tmp/emergency.pub.
pub fn emergency_dropin() -> String {
    format!("# Emergency access over the USB cable (written by gitmind's fp5_setup).\nMatch LocalAddress {USB_ADDR}\n    PasswordAuthentication yes\n    KbdInteractiveAuthentication yes\n    PermitRootLogin no\n    AllowUsers user\n")
}

/// Reach the phone over a USB cable when wifi, the shell and the compositor are all gone: sshd on
/// the gadget network, password login on that address only, and an optional backup key.
pub fn emergency_access_plan(root: Root) -> Vec<Step> {
    let dropin = emergency_dropin();
    let write = format!("mkdir -p /etc/ssh/sshd_config.d && printf '%s' {} > {EMERGENCY_DROPIN}", crate::deploy::shell_quote(&dropin));
    vec![
        Step { on: Where::Device, what: "sshd starts at boot",
               cmd: "systemctl list-unit-files sshd.service | grep -q 'sshd.service *enab' && echo SSHD_BOOT".into(),
               gate: Gate::Contains("SSHD_BOOT".into()), rollback: None },
        Step { on: Where::Device, what: "the USB gadget network has its address (developer profile)",
               cmd: format!("ip -4 -o addr show 2>/dev/null | grep -q ' {USB_ADDR}/' && echo USB_NET"),
               gate: Gate::Contains("USB_NET".into()), rollback: None },
        Step { on: Where::Device, what: "password login on the USB address only",
               cmd: root_cmd(&write, root), gate: Gate::ExitZero,
               rollback: Some(root_cmd(&format!("rm -f {EMERGENCY_DROPIN}"), root)) },
        Step { on: Where::Device, what: "sshd accepts the configuration",
               cmd: root_cmd("sshd -t && echo CONFIG_OK", root), gate: Gate::Contains("CONFIG_OK".into()),
               rollback: Some(root_cmd(&format!("rm -f {EMERGENCY_DROPIN}"), root)) },
        Step { on: Where::Device, what: "sshd re-reads it",
               cmd: root_cmd("systemctl reload sshd", root), gate: Gate::ExitZero,
               rollback: Some(root_cmd(&format!("rm -f {EMERGENCY_DROPIN}; systemctl reload sshd"), root)) },
        Step { on: Where::Device, what: "the backup key, if one was staged at /tmp/emergency.pub",
               cmd: "[ ! -s /tmp/emergency.pub ] || grep -qxFf /tmp/emergency.pub \"$HOME/.ssh/authorized_keys\" || cat /tmp/emergency.pub >> \"$HOME/.ssh/authorized_keys\"".into(),
               gate: Gate::ExitZero,
               rollback: Some("[ ! -s /tmp/emergency.pub ] || { grep -vxFf /tmp/emergency.pub \"$HOME/.ssh/authorized_keys\" > \"$HOME/.ssh/authorized_keys.new\"; mv \"$HOME/.ssh/authorized_keys.new\" \"$HOME/.ssh/authorized_keys\"; }".into()) },
    ]
}

/// Install the rescue chord watcher and its console from `staged` (a directory on the phone holding
/// `fp5-rescued`, `console.sh`, `fp5-rescue.service` and `fp5-rescue-console.service`; see
/// `system/fp5-rescue`). Root.
pub fn rescue_console_plan(staged: &str, root: Root) -> Vec<Step> {
    let install = format!(
        "install -D -m 755 {staged}/fp5-rescued /usr/local/bin/fp5-rescued && \
         install -D -m 755 {staged}/console.sh /usr/local/lib/fp5-rescue/console.sh && \
         install -D -m 644 {staged}/fp5-rescue.service /etc/systemd/system/fp5-rescue.service && \
         install -D -m 644 {staged}/fp5-rescue-console.service /etc/systemd/system/fp5-rescue-console.service && \
         systemctl daemon-reload");
    let undo = "systemctl disable --now fp5-rescue.service; rm -f /usr/local/bin/fp5-rescued /usr/local/lib/fp5-rescue/console.sh /etc/systemd/system/fp5-rescue.service /etc/systemd/system/fp5-rescue-console.service; systemctl daemon-reload";
    vec![
        Step { on: Where::Device, what: "the console tools exist (chvt, agetty, buffyboard)",
               cmd: "command -v chvt && command -v agetty && command -v buffyboard && echo TOOLS".into(),
               gate: Gate::Contains("TOOLS".into()), rollback: None },
        Step { on: Where::Device, what: "the watcher sees the power and volume-down keys",
               cmd: root_cmd(&format!("chmod +x {staged}/fp5-rescued && {staged}/fp5-rescued --probe"), root),
               gate: Gate::Contains("/dev/input/event".into()), rollback: None },
        Step { on: Where::Device, what: "install the rescue watcher and console",
               cmd: root_cmd(&install, root), gate: Gate::ExitZero, rollback: Some(root_cmd(undo, root)) },
        Step { on: Where::Device, what: "start the watcher at boot",
               cmd: root_cmd("systemctl enable --now fp5-rescue.service", root), gate: Gate::ExitZero,
               rollback: Some(root_cmd(undo, root)) },
        Step { on: Where::Device, what: "the watcher is running",
               cmd: root_cmd("sleep 2; systemctl is-active --quiet fp5-rescue.service && echo WATCHING", root),
               gate: Gate::Contains("WATCHING".into()), rollback: Some(root_cmd(undo, root)) },
    ]
}

/// Every part that can be replayed without arguments, in a sensible order.
pub fn all_parts(root: Root) -> Vec<(&'static str, Vec<Step>)> {
    vec![
        ("hatch launcher alias", hatch_launcher_plan()),
        ("keyboard service", rusty_board_service_plan()),
        ("speaker-chain packages", audio_packages_plan(root)),
        ("emergency USB access", emergency_access_plan(root)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosmic_setup::run_plan;

    fn all_steps(root: Root) -> Vec<Step> {
        let mut v: Vec<Step> = all_parts(root).into_iter().flat_map(|(_, s)| s).collect();
        v.extend(tideshell_swap_plan("/tmp/tideshell.new"));
        v.extend(camera_app_plan("/tmp/snapshot-cam.new"));
        v.extend(hatch_swap_plan("/tmp/hatch-egui.new"));
        v.extend(emergency_access_plan(root));
        v.extend(rescue_console_plan("/tmp/fp5-rescue", root));
        v
    }

    #[test]
    fn nothing_in_any_plan_carries_a_password() {
        for root in [Root::NoPrompt, Root::PasswordOnStdin] {
            for s in all_steps(root) {
                for c in std::iter::once(&s.cmd).chain(s.rollback.iter()) {
                    for bad in ["147147", "password=", "echo $PW", "sudo -S -p '' sh -c 'echo"] {
                        assert!(!c.contains(bad), "step `{}` leaks `{bad}`: {c}", s.what);
                    }
                }
            }
        }
    }

    #[test]
    fn every_step_that_changes_the_phone_can_be_undone_or_says_why_not() {
        // Steps that mutate and have no rollback must be on this short, reviewed list: an upgrade
        // cannot be un-upgraded by a one-liner, and a backup copy is harmless to leave.
        let allowed = ["align PipeWire with them (camera plugin left alone)", "keep the running build as tideshell.prev", "keep the installed camera as snapshot-cam.prev"];
        for s in all_steps(Root::NoPrompt) {
            let mutates = ["apk add ", "apk upgrade", "mv ", "cp ", "ln -s", "enable", "systemctl --user restart", "printf"]
                .iter().any(|m| s.cmd.contains(m)) && !s.cmd.contains("apk add -s");
            if mutates && s.rollback.is_none() {
                assert!(allowed.contains(&s.what), "`{}` changes the phone and has no rollback: {}", s.what, s.cmd);
            }
        }
    }

    #[test]
    fn the_camera_steps_really_write_a_runnable_wrapper_and_a_valid_entry() {
        let home = std::env::temp_dir().join(format!("fp5-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        for what in ["the camera's launcher wrapper", "the camera's launcher entry"] {
            let step = camera_app_plan("/x").into_iter().find(|s| s.what == what).unwrap();
            let out = std::process::Command::new("sh").arg("-c").arg(&step.cmd).env("HOME", &home).output().unwrap();
            assert!(out.status.success(), "{what}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let wrapper = std::fs::read_to_string(home.join(".local/bin/snapshot-cam-phone")).unwrap();
        assert!(wrapper.starts_with("#!/bin/sh\n") && wrapper.contains("SNAPSHOT_CAM_TURN=90\n"), "{wrapper}");
        assert!(wrapper.contains(&format!("SNAPSHOT_CAM_SOURCE=\"gst:libcamerasrc camera-name={MAIN_CAMERA}\"")), "{wrapper}");
        assert!(wrapper.contains("exec \"$HOME/.local/bin/snapshot-cam\" \"$@\""), "$HOME and $@ must survive unexpanded: {wrapper}");
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(home.join(".local/bin/snapshot-cam-phone")).unwrap().permissions().mode() & 0o111 != 0);
        let text = std::fs::read_to_string(home.join(".local/share/applications/snapshot-cam.desktop")).unwrap();
        let exec = text.lines().find(|l| l.starts_with("Exec=")).expect("an Exec line");
        assert!(exec.ends_with("/.local/bin/snapshot-cam-phone") && !exec.contains(' '), "a bare path, nothing to quote: {exec}");
        assert!(text.starts_with("[Desktop Entry]\nType=Application\nName=Camera\n"), "{text}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_emergency_dropin_is_valid_sshd_config_and_limits_passwords_to_usb() {
        let d = emergency_dropin();
        assert!(d.contains(&format!("Match LocalAddress {USB_ADDR}\n")) && d.contains("PasswordAuthentication yes"));
        // Nothing above the Match block: the password rule must be inside it, not global.
        let before = d.split("Match ").next().unwrap();
        assert!(!before.contains("PasswordAuthentication"), "global password rule leaked: {before}");
        // Really parse it: sshd -t on a config that includes it (a host key is needed to start).
        let dir = std::env::temp_dir().join(format!("fp5-sshd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(dir.join("hk")).status().unwrap();
        assert!(keygen.success(), "ssh-keygen must exist where this test runs");
        std::fs::write(dir.join("drop.conf"), &d).unwrap();
        std::fs::write(dir.join("sshd_config"), format!("HostKey {}\nUsePAM no\nInclude {}\n", dir.join("hk").display(), dir.join("drop.conf").display())).unwrap();
        let sshd = ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"].into_iter().find(|p| std::path::Path::new(p).exists()).expect("sshd must exist where this test runs");
        let out = std::process::Command::new(sshd).arg("-t").arg("-f").arg(dir.join("sshd_config")).output().unwrap();
        assert!(out.status.success(), "sshd -t: {}", String::from_utf8_lossy(&out.stderr));
        // And it rejects a broken one, so the check above can fail.
        std::fs::write(dir.join("drop.conf"), "Match LocalAddress\n    PasswordAuthentication maybe\n").unwrap();
        let bad = std::process::Command::new(sshd).arg("-t").arg("-f").arg(dir.join("sshd_config")).output().unwrap();
        assert!(!bad.status.success(), "sshd -t must refuse a broken drop-in");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shell_is_never_restarted_unless_a_session_exists() {
        let plan = tideshell_swap_plan("/tmp/tideshell.new");
        let gate = plan.iter().position(|s| s.what.starts_with("a graphical session")).expect("a session gate");
        let restart = plan.iter().position(|s| s.cmd.contains("systemctl --user restart tideshell")).expect("a restart");
        let install = plan.iter().position(|s| s.cmd.starts_with("mv ")).expect("an install");
        assert_eq!(gate, 0, "the session check is the FIRST thing");
        assert!(gate < install && install < restart);
        // And the gate really does fail with no session: a run that answers every command with an empty
        // success must not get past it.
        let mut ran = vec![];
        let r = run_plan(&plan, &mut |c| { ran.push(c.to_string()); Ok(String::new()) }, &mut |_| {});
        assert!(r.is_err(), "no SESSION in the output must stop the plan");
        assert_eq!(ran.len(), 1, "nothing may run after the failed session gate: {ran:?}");
    }

    #[test]
    fn a_failed_health_check_puts_the_old_shell_back_newest_first() {
        let plan = tideshell_swap_plan("/tmp/tideshell.new");
        let mut ran: Vec<String> = vec![];
        let mut run = |c: &str| -> Result<String, String> {
            ran.push(c.to_string());
            if c.contains("ls /run/user") { return Ok("SESSION".into()); }
            if c.contains("ldd") { return Ok("LIBS_OK".into()); }
            if c.contains("is-active") { return Ok("inactive".into()); } // the health check fails
            Ok(String::new())
        };
        let mut lines = vec![];
        let r = run_plan(&plan, &mut run, &mut |l| lines.push(l.to_string()));
        assert!(r.is_err());
        let restores = ran.iter().filter(|c| c.starts_with("cp -p") && c.contains("tideshell.prev") && c.contains("systemctl --user restart tideshell")).count();
        assert!(restores >= 1, "the old build must be restored AND the shell restarted: {ran:?}");
    }

    #[test]
    fn the_retired_autostart_entry_leaves_the_autostart_folder() {
        // Regression: `mv rusty-board.desktop rusty-board.desktop.disabled` IN PLACE left the keyboard
        // starting twice, because the xdg-autostart generator still reads the renamed file. The entry
        // must move to a directory the generator does not scan.
        let plan = rusty_board_service_plan();
        let retire = plan.iter().find(|s| s.what.contains("retired")).expect("a retire step");
        assert!(retire.cmd.contains("autostart-disabled"), "must move OUT of autostart/: {}", retire.cmd);
        assert!(!retire.cmd.contains(".desktop.disabled"), "an in-place rename keeps it in autostart/: {}", retire.cmd);
        // and a plan step checks that no entry naming rusty-board is left behind, whatever its extension
        let check = plan.iter().find(|s| s.what.contains("exactly one launcher")).expect("an ownership check");
        assert!(check.cmd.contains("autostart") && check.cmd.contains("rusty-board"));
        assert!(crate::cosmic_setup::passes(&check.gate, true, "ONE_OWNER\n"));
        assert!(!crate::cosmic_setup::passes(&check.gate, true, "1\n"));
    }

    #[test]
    fn the_running_check_cannot_be_fooled_by_the_word_inactive() {
        // `is-active` prints "inactive" when it is not; a gate on the substring "active" would pass.
        for s in [rusty_board_service_plan().pop().unwrap(), tideshell_swap_plan("/x").pop().unwrap()] {
            assert_eq!(s.gate, Gate::Contains("RUNNING".into()), "{}", s.what);
            assert!(!crate::cosmic_setup::passes(&s.gate, true, "inactive"));
            assert!(crate::cosmic_setup::passes(&s.gate, true, "RUNNING\n"));
        }
    }

    #[test]
    fn the_audio_install_is_dry_run_first_and_leaves_the_camera_stack_alone() {
        let plan = audio_packages_plan(Root::NoPrompt);
        let dry = plan.iter().position(|s| s.cmd.contains("apk add -s")).expect("a dry run");
        let real = plan.iter().position(|s| s.cmd.contains("apk add ") && !s.cmd.contains("apk add -s")).expect("an install");
        assert!(dry < real);
        let text: String = plan.iter().map(|s| s.cmd.clone()).collect();
        assert!(!text.contains("spa-libcamera") && !text.contains("libcamera"), "an audio install must not touch the camera stack");
        assert!(AUDIO_PACKAGES.contains(&"bankstown-lv2") && !PIPEWIRE_ALIGN.contains(&"pipewire-spa-libcamera"));
    }

    #[test]
    fn root_work_goes_through_root_cmd_so_the_password_never_reaches_a_command_line() {
        let pw = audio_packages_plan(Root::PasswordOnStdin);
        let installs: Vec<&Step> = pw.iter().filter(|s| s.cmd.contains("apk add") || s.cmd.contains("apk upgrade")).collect();
        assert!(installs.len() >= 3);
        for s in installs { assert!(s.cmd.starts_with("sudo -S -p ''"), "`{}`: {}", s.what, s.cmd); }
    }
}
