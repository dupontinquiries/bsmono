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
               cmd: format!("mkdir -p \"{UNIT_DIR}/rusty-board.service.d\" && printf '[Service]\\nExecStart=\\nExecStart={BIN}/rusty-board-egui\\nRestart=always\\nRestartSec=1\\nReadWritePaths=%%t\\n' > \"{dropin}\" && systemctl --user daemon-reload"),
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

/// What vidkit-player needs on this phone to play a pasted PAGE link (a direct .mp4 needs
/// neither): `yt-dlp` resolves the page and `ffmpeg` muxes/reads the streams. The phone shipped
/// with neither, so "play from clipboard" found the URL and then could not do anything with it.
pub const VIDEO_TOOLS: &[&str] = &["yt-dlp", "ffmpeg"];

pub fn video_tools_plan(root: Root) -> Vec<Step> {
    let add = VIDEO_TOOLS.join(" ");
    vec![
        Step { on: Where::Device, what: "the packages resolve (dry run, nothing installed)",
               cmd: root_cmd(&format!("apk add -s {add}"), root), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "install yt-dlp and ffmpeg for vidkit-player",
               cmd: root_cmd(&format!("apk add {add}"), root), gate: Gate::ExitZero,
               rollback: Some(root_cmd(&format!("apk del {add}"), root)) },
        Step { on: Where::Device, what: "both are on the path",
               cmd: "command -v yt-dlp && command -v ffmpeg && echo TOOLS".into(),
               gate: Gate::Contains("TOOLS".into()), rollback: None },
    ]
}

/// COSMIC's panel and dock reserve an exclusive zone even while autohidden, which left the
/// tideshell bar (anchored top+bottom) short of the bottom of the screen. Both are live config
/// files: cosmic-panel applies them without a restart.
pub fn panel_zones_plan() -> Vec<Step> {
    let dir = "$HOME/.config/cosmic";
    let set = |value: &str| {
        format!(
            "for n in Panel Dock; do d={dir}/com.system76.CosmicPanel.$n/v1; mkdir -p $d && printf '{value}' > $d/exclusive_zone; done"
        )
    };
    vec![
        Step { on: Where::Device, what: "the panel and dock stop reserving a zone",
               cmd: set("false"), gate: Gate::ExitZero, rollback: Some(set("true")) },
        Step { on: Where::Device, what: "both files say false",
               cmd: format!("cat {dir}/com.system76.CosmicPanel.Panel/v1/exclusive_zone {dir}/com.system76.CosmicPanel.Dock/v1/exclusive_zone"),
               gate: Gate::Contains("falsefalse".into()), rollback: None },
    ]
}

/// The FP5 kernel boots with every core on the `performance` governor (measured 2026-10-01: all 8
/// cores `performance`, cpu7 at 2.7 GHz with the phone idle, no cpupower service, nothing in
/// tideshell's prefs), which pins the clocks high and is the first suspect for "hot just browsing
/// apps". `schedutil` follows the load. Written through systemd-tmpfiles so it survives a reboot.
pub const CPU_GOVERNOR: &str = "schedutil";

pub fn cpu_governor_plan(root: Root) -> Vec<Step> {
    let conf = "/etc/tmpfiles.d/fp5-cpu-governor.conf";
    let put = format!("printf 'w /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor - - - - {CPU_GOVERNOR}\\n' > {conf} && systemd-tmpfiles --create {conf}");
    let undo = format!("rm -f {conf}; for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do echo performance > $f; done");
    vec![
        Step { on: Where::Device, what: "schedutil is an available governor",
               cmd: "grep -qw schedutil /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors && echo AVAILABLE".into(),
               gate: Gate::Contains("AVAILABLE".into()), rollback: None },
        Step { on: Where::Device, what: "set schedutil now and at every boot",
               cmd: root_cmd(&put, root), gate: Gate::ExitZero, rollback: Some(root_cmd(&undo, root)) },
        Step { on: Where::Device, what: "no core is still on performance",
               cmd: "cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor | sort -u".into(),
               gate: Gate::Contains(CPU_GOVERNOR.into()), rollback: None },
    ]
}

/// The FP5's panel is OLED: a pixel at the usual "dark theme" ground (20, 20, 25) is a lit grey and only
/// (0, 0, 0) is off, so every dark UI looked washed out and burned power. `~/.config/uikit/oled` = `on`
/// makes `uikit_egui::apply_palette` (every themed app) and tideshell's own and published palettes use a
/// true-black ground. Per user, and read by apps built with it; tideshell restarts to pick it up.
pub fn oled_black_plan() -> Vec<Step> {
    let f = "$HOME/.config/uikit/oled";
    vec![
        Step { on: Where::Device, what: "true black on the OLED panel",
               cmd: format!("mkdir -p \"$HOME/.config/uikit\" && printf 'on\\n' > \"{f}\""),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{f}\"")) },
        Step { on: Where::Device, what: "the switch is on",
               cmd: format!("grep -x on \"{f}\" && echo OLED_ON"),
               gate: Gate::Contains("OLED_ON".into()), rollback: None },
    ]
}

/// xwaylandvideobridge (KDE) keeps an invisible X11 window and a tray icon for screen-sharing X11
/// apps. KWin and GNOME hide it; COSMIC shows it. Nothing on the phone shares X11 windows, so it
/// is not started: a per-user autostart entry with `Hidden=true` shadows the system one.
pub fn hide_video_bridge_plan() -> Vec<Step> {
    let f = "$HOME/.config/autostart/org.kde.xwaylandvideobridge.desktop";
    vec![
        Step { on: Where::Device, what: "xwaylandvideobridge no longer autostarts",
               cmd: format!("mkdir -p \"$HOME/.config/autostart\" && printf '[Desktop Entry]\\nType=Application\\nName=Xwayland Video Bridge\\nHidden=true\\n' > \"{f}\" && {{ pkill -x xwaylandvideobr; true; }}"),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{f}\"")) },
        Step { on: Where::Device, what: "the override says Hidden",
               cmd: format!("grep -x Hidden=true \"{f}\" && echo HIDDEN"),
               gate: Gate::Contains("HIDDEN".into()), rollback: None },
    ]
}

/// Past this much anonymous memory (kB) cosmic-panel is restarted by the watchdog.
pub const PANEL_RSS_LIMIT_KB: u32 = 800_000;

/// `cosmic-panel` on the FP5 (cosmic 1.9.0) can hit a Wayland protocol error on its
/// `cosmic_corner_radius_layer_v1` object right after the session starts and then spin on the dead
/// connection: ~7 MB/s leaked and a core burned until the phone runs out of memory (5 GB seen).
/// Measured: 0 such errors in the boot before 02:49 on 2026-10-01, then 460k and 1M in the next
/// two, and 92k within minutes of a boot WITHOUT autologin (so autologin is not the cause). Not
/// caused by the panel/dock config (zones, autohide, outputs and radius were each
/// changed and the leak stayed). This does not fix it; it bounds it.
pub fn panel_watchdog_plan() -> Vec<Step> {
    let script = format!("{BIN}/panel-watchdog");
    let unit = format!("{UNIT_DIR}/panel-watchdog.service");
    vec![
        Step { on: Where::Device, what: "the watchdog script (restarts cosmic-panel past its memory limit)",
               cmd: format!("mkdir -p \"{BIN}\" && printf '#!/bin/sh\\nwhile sleep 20; do p=$(pidof -s cosmic-panel) || continue; kb=$(grep RssAnon /proc/$p/status | tr -dc 0-9); [ \"${{kb:-0}}\" -gt {PANEL_RSS_LIMIT_KB} ] && pkill -x cosmic-panel; done\\n' > \"{script}\" && chmod +x \"{script}\""),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{script}\"")) },
        Step { on: Where::Device, what: "its user unit",
               cmd: format!("mkdir -p \"{UNIT_DIR}\" && printf '[Unit]\\nDescription=Restart cosmic-panel when it leaks\\n[Service]\\nExecStart=%%h/.local/bin/panel-watchdog\\nRestart=always\\n[Install]\\nWantedBy=default.target\\n' > \"{unit}\" && systemctl --user daemon-reload"),
               gate: Gate::ExitZero, rollback: Some(format!("rm -f \"{unit}\"; systemctl --user daemon-reload")) },
        Step { on: Where::Device, what: "the watchdog is enabled and started",
               cmd: "systemctl --user enable --now panel-watchdog.service".into(),
               gate: Gate::ExitZero, rollback: Some("systemctl --user disable --now panel-watchdog.service".into()) },
        Step { on: Where::Device, what: "the watchdog is running",
               cmd: "sleep 1; systemctl --user is-active --quiet panel-watchdog.service && echo RUNNING".into(),
               gate: Gate::Contains("RUNNING".into()), rollback: None },
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

const GREETD_CONF: &str = "/etc/greetd/cosmic-greeter.toml";

/// The `[initial_session]` block greetd runs ONCE at boot instead of showing the greeter: the owner
/// is logged straight into the COSMIC session, and tideshell's own cover is the lock screen. The
/// greeter stays the `default_session`, so a logout (or a crash of the session) still lands on a
/// login screen rather than a dead VT.
pub fn autologin_block(user: &str) -> String {
    format!("\n[initial_session]\ncommand = \"start-cosmic\"\nuser = \"{user}\"\n")
}

/// Skip the cosmic-greeter login at boot (its on-screen keyboard did not let the owner in). The
/// original file is kept as `.before-autologin` and put back on any failure. Root. Takes effect at
/// the next boot; nothing is restarted.
pub fn autologin_plan(user: &str, root: Root) -> Vec<Step> {
    let bak = format!("{GREETD_CONF}.before-autologin");
    let append = format!("printf '%s' {} >> {GREETD_CONF}", crate::deploy::shell_quote(&autologin_block(user)));
    let restore = root_cmd(&format!("[ ! -e {bak} ] || cp -p {bak} {GREETD_CONF}"), root);
    vec![
        Step { on: Where::Device, what: "the greetd config is the cosmic-greeter one",
               cmd: format!("grep -q 'default_session' {GREETD_CONF} && echo GREETD"), gate: Gate::Contains("GREETD".into()), rollback: None },
        Step { on: Where::Device, what: "the session command exists",
               cmd: "command -v start-cosmic && echo SESSION_CMD".into(), gate: Gate::Contains("SESSION_CMD".into()), rollback: None },
        Step { on: Where::Device, what: "keep the greeter config as .before-autologin",
               cmd: root_cmd(&format!("[ -e {bak} ] || cp -p {GREETD_CONF} {bak}"), root), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "log the owner in at boot (initial_session)",
               cmd: root_cmd(&format!("grep -q '^\\[initial_session\\]' {GREETD_CONF} || {append}"), root), gate: Gate::ExitZero,
               rollback: Some(restore.clone()) },
        Step { on: Where::Device, what: "the block is there exactly once",
               cmd: format!("grep -c '^\\[initial_session\\]' {GREETD_CONF}"), gate: Gate::Contains("1".into()), rollback: Some(restore) },
    ]
}

/// Every part that can be replayed without arguments, in a sensible order.
pub fn all_parts(root: Root) -> Vec<(&'static str, Vec<Step>)> {
    vec![
        ("hatch launcher alias", hatch_launcher_plan()),
        ("keyboard service", rusty_board_service_plan()),
        ("speaker-chain packages", audio_packages_plan(root)),
        ("video tools for vidkit-player", video_tools_plan(root)),
        ("panel and dock zones", panel_zones_plan()),
        ("panel memory watchdog", panel_watchdog_plan()),
        ("hide xwaylandvideobridge", hide_video_bridge_plan()),
        ("true black (OLED)", oled_black_plan()),
        ("cpu governor", cpu_governor_plan(root)),
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
        v.extend(video_tools_plan(root));
        v.extend(panel_zones_plan());
        v.extend(panel_watchdog_plan());
        v.extend(hide_video_bridge_plan());
        v.extend(oled_black_plan());
        v.extend(cpu_governor_plan(root));
        v.extend(rescue_console_plan("/tmp/fp5-rescue", root));
        v.extend(autologin_plan("user", root));
        v
    }

    #[test]
    fn the_video_and_zone_plans_say_what_they_change() {
        let v = video_tools_plan(Root::NoPrompt);
        assert!(v.iter().any(|s| s.cmd.contains("apk add yt-dlp ffmpeg") && s.rollback.as_deref().is_some_and(|r| r.contains("apk del yt-dlp ffmpeg"))));
        let z = panel_zones_plan();
        assert!(z[0].cmd.contains("CosmicPanel.$n/v1") && z[0].cmd.contains("printf 'false'"));
        assert!(z[0].rollback.as_deref().unwrap().contains("printf 'true'"), "undo restores the zone");
        assert!(all_parts(Root::NoPrompt).iter().any(|(n, _)| n.contains("panel and dock")));
    }

    #[test]
    fn the_watchdog_script_is_a_runnable_loop_with_the_limit_in_it() {
        let w = panel_watchdog_plan();
        let cmd = &w[0].cmd;
        assert!(cmd.contains("pidof -s cosmic-panel") && cmd.contains("pkill -x cosmic-panel"), "{cmd}");
        assert!(cmd.contains(&format!("-gt {PANEL_RSS_LIMIT_KB}")), "{cmd}");
        assert!(cmd.contains("${kb:-0}"), "an unset reading must not become a syntax error: {cmd}");
        assert!(w[1].cmd.contains("ExecStart=%%h/.local/bin/panel-watchdog"), "{}", w[1].cmd);
        assert!(w.iter().filter(|s| s.rollback.is_some()).count() == 3);
    }

    #[test]
    fn the_video_bridge_is_shadowed_by_a_hidden_autostart_entry() {
        let p = hide_video_bridge_plan();
        assert!(p[0].cmd.contains("Hidden=true") && p[0].cmd.contains("org.kde.xwaylandvideobridge.desktop"), "{}", p[0].cmd);
        assert!(p[0].rollback.as_deref().unwrap().contains("rm -f"));
        assert!(p[0].cmd.contains("pkill -x xwaylandvideobr;"), "comm is truncated to 15 chars: {}", p[0].cmd);
    }

    #[test]
    fn the_governor_plan_persists_schedutil_and_can_go_back() {
        let p = cpu_governor_plan(Root::NoPrompt);
        assert!(p[1].cmd.contains("scaling_governor - - - - schedutil") && p[1].cmd.contains("tmpfiles"), "{}", p[1].cmd);
        assert!(p[1].rollback.as_deref().unwrap().contains("echo performance"));
        assert!(p[2].gate == Gate::Contains("schedutil".into()), "the check must name the governor, not just run");
    }

    #[test]
    fn the_oled_plan_writes_the_per_user_switch_uikit_reads_and_can_undo_it() {
        let p = oled_black_plan();
        assert!(p[0].cmd.contains(".config/uikit/oled") && p[0].cmd.contains("printf 'on"), "{}", p[0].cmd);
        assert!(p[0].rollback.as_deref().unwrap().contains("rm -f"));
        assert!(all_parts(Root::NoPrompt).iter().any(|(n, _)| n.contains("OLED")));
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
        let allowed = ["align PipeWire with them (camera plugin left alone)", "keep the running build as tideshell.prev", "keep the installed camera as snapshot-cam.prev", "keep the greeter config as .before-autologin"];
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
    fn the_autologin_block_is_valid_toml_for_greetd_and_keeps_the_greeter() {
        // The real file on the phone, plus the block: it must still parse, with both sessions.
        let file = "[terminal]\nvt = \"1\"\n\n[general]\nservice = \"cosmic-greeter\"\n\n[default_session]\ncommand = \"cosmic-greeter-start\"\nuser = \"cosmic-greeter\"\n";
        let text = format!("{file}{}", autologin_block("user"));
        let n_init = text.matches("[initial_session]").count();
        assert_eq!(n_init, 1);
        let after = text.split("[initial_session]").nth(1).unwrap();
        assert!(after.contains("command = \"start-cosmic\"") && after.contains("user = \"user\""));
        assert!(text.contains("[default_session]") && text.contains("cosmic-greeter-start"), "the greeter stays the fallback");
        // The append step is idempotent: its guard greps for the header it adds.
        let step = autologin_plan("user", Root::NoPrompt).into_iter().find(|s| s.what.starts_with("log the owner in")).unwrap();
        assert!(step.cmd.contains("initial_session") && step.cmd.contains("||"), "{}", step.cmd);
        assert!(step.rollback.is_some());
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
