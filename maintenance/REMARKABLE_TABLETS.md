# reMarkable tablets: capabilities and quirks

What the monorepo knows about running its apps on reMarkable tablets, from a
session that got egui apps running on a Paper Pro. **Seen** = observed on a
real Paper Pro (OS with xovi + AppLoad). **Source** = read in AppLoad /
Oxide / reMarkable docs, not observed. Everything else is marked unverified.

## Models

| model | codename | SoC / CPU | panel | notes |
|---|---|---|---|---|
| reMarkable 1 | zero-gravitas | i.MX6SL, armv7 | 1404×1872 mono | not tested |
| reMarkable 2 | zero-sugar | i.MX7D, 2× A7, armv7 | 1404×1872 mono | SSH without developer mode; its dropbear refuses ed25519 keys |
| Paper Pro | ferrari | i.MX8MM, aarch64 | 1620×2160 colour (Gallery 3) | **seen**: hostname `imx8mm-ferrari`, 4 CPUs in dmesg, 1.9 GB RAM (~1.3 GB free) |
| Paper Pro Move | chiappa | i.MX93, aarch64 | 954×1696 colour | not tested |
| Paper Pure | tatsu | i.MX93, aarch64 | 1404×1872 mono | not tested |

Model detection order (gitmind `eink_fleet::parse_probe`): `/sys/devices/soc0/machine`
prefixes (Oxide's), device tree model, codename in `/etc/motd`, codename in hostname,
SoC + panel size.

## Access

* **SSH user** `root`: reMarkable documents it as "root *at the time of writing*", so
  gitmind treats it as a default (`--user`, `GITMIND_RM_USER`, a field on the Tablets page).
* **Password**: Settings → General → Help → About → Copyrights and licenses, under
  General information (username and password). OS updates reset it and the host key.
* **Paper Pro family needs developer mode** for any SSH, and enabling it
  **factory-resets** the tablet.
* **USB**: every tablet is `10.11.99.1`. **Seen**: plugging an rM2 in after a Paper Pro
  gives OpenSSH's "REMOTE HOST IDENTIFICATION HAS CHANGED". gitmind now files tablet host
  keys per *device* (`HostKeyAlias` from the USB serial read via sysfs before SSH, in its
  own `tablet_known_hosts`), and stores surveys per device. Unverified: whether the
  tablets' USB gadget reports a stable serial (the MAC is the flagged fallback).
* **Wi-Fi SSH is off by default** on the Paper Pro family, even with developer mode.
  **Seen**: "Connection refused" on the LAN address. Fix once over USB: `rm-ssh-over-wlan on`.
* **USB from a phone**: **seen** on one Fairphone 5 (postmarketOS), the phone's xHCI
  host controller died enumerating the tablet (`HC died` in dmesg). The user reports
  another FP5 works after installing and loading the right modules. A laptop, or Wi-Fi
  after the one USB step, avoids it.

## System

* Root filesystem read-only; `/etc` is a tmpfs overlay (writes vanish at reboot).
  **`/home` survives reboots and OS updates.** gitmind writes only under
  `/home/root/.local/share/gitmind` and never remounts `/` (a test asserts no command
  sends mount/remount). The dropbear host keys are already bind-mounted from
  `/home/root/.dropbear`.
* **Userland is BusyBox** (**seen**): `ps -o`, `head -1` and `od -A` fail; use
  `head -n 1`, plain `ps`. Present: `python3.12`, `curl`, `tar`, `rsync`, `openssl`,
  `journalctl`, `systemctl`, `bluetoothctl`, `btmon`, `hciconfig`, `evtest`, `opkg`.
* **No audio stack** (**seen**): no speakers; BlueZ is there, but no PipeWire,
  PulseAudio, bluez-alsa or `aplay`. Audio means a Bluetooth headset plus an audio
  bridge (below).

## xovi and AppLoad

* xovi does not survive a reboot: `/home/root/xovi/start` (**seen**) writes a systemd
  drop-in into a tmpfs and restarts xochitl. `systemctl restart xochitl` afterwards
  keeps xovi until the next reboot.
* **AppLoad caches each manifest's environment until xochitl restarts** (**seen**:
  an edited `EINK_QTFB_MODES` was ignored across many launches and ↻ taps; a restart
  applied it). gitmind reports when a deploy changed a manifest and has
  `eink restart-ui` / a Restart UI button.
* **AppLoad closes the connection *and the window* when a client sends
  `SET_REFRESH_MODE`** (**seen**, journal: `Associated` → `Disassociating` →
  `Closing`, milliseconds after init). AppLoad's own shim (the KOReader path) never
  sends it. So **there is no fast waveform through AppLoad** on this OS: eink-kit
  stays in AppLoad's default mode (`EINK_QTFB_MODES` is an opt-in experiment that now
  exits with the reason instead of reconnecting to nothing). Probably worth reporting
  upstream to rm-appload.
* Touch forwarding (**source**, `FBController::touchEvent`): stationary points are sent
  as PRESS again whenever any point moves; cancelled or ungrabbed touches send no
  RELEASE; 5 simultaneous points trigger a full refresh; a drag down from the top
  100 px is AppLoad's gesture. **Seen** consequence: a resting palm left every button
  dead. eink-kit now treats a repeat PRESS as a move, releases contacts silent for
  2.5 s, lets a new finger take the pointer from a contact still for 0.4 s, and
  releases off-screen so a palm never clicks.
* AppLoad logs `[QTFB]` / `[AppLoad]` lines under `xochitl` in `journalctl`, including
  `Process for … finished with exit code N`.
* Tools such as `alpine-run` must not be registered as AppLoad apps (**seen**: one sat
  in the sidebar printing its usage). gitmind installs `TOOLS` to `bin/` only.

## Performance (seen)

* Full 1620×2160 frame through eink-kit's CPU rasterizer: **140 ms** with 4 threads
  (`eink-demo --snapshot` on the tablet).
* eink-kit never renders faster than every 120 ms and caps motion at 6 Hz; idle apps
  draw nothing. Priority is steady, low frame rates, not FPS.

## What runs, and how

* **eink-kit** (`shared/eink-kit`): egui on the panel via AppLoad/qtfb (or Oxide/Blight),
  CPU rasterizer, damage tracking, refresh policy, touch/pen, one log line per launch
  (time, pid, `EINK_*` as received) in `EINK_LOG`.
* **eframe shim** (`shared/eink-kit/eframe-shim`): a crate named `eframe` swapped in at
  build time (`--config patch.crates-io.eframe.path=…`, `EINK_SHIM=1`), so ordinary
  eframe apps build unchanged. Compile-checked this way: snapclone-egui, bo-suite-egui,
  logscope-egui, voiceterm-egui, rusty-board-egui, aperture-egui (UI), studio (UI).
  Not run on the tablet yet. photosync is not eframe-based.
* **Seen running on the Paper Pro**: eink-demo, unendlich-eink (after the xochitl
  restart), ambient-learner-egui, tomescope-eink, stoat-egui (the latter three were
  hit by the refresh-mode window loss before that fix).
* **Per-device features**: enrolment runs a hardware survey (`cockpit::capabilities`);
  builds drop default features the device cannot use (a Paper Pro drops audio, midi,
  voice/whisper, GPU scenes), declared per app in `[package.metadata.gitmind.needs]`.
* **Packages like a laptop** (`cockpit::alpine_root`): an Alpine root under `/home`
  managed by Alpine's `apk.static` (signature-checked), bootstrapped with only the
  tablet's curl + python3; `alpine-run` runs programs inside it (private mount
  namespace, /dev /run /tmp /home bound). Mirror: Alpine's CDN or **depot** (new Alpine
  projection, served byte-for-byte so Alpine's signatures still apply); gitmind checks a
  depot can satisfy an install (dependency closure) first. `--userland alpine` builds a
  dynamic musl app against the root's Alpine release and launches it through
  `alpine-run`, installing its `DT_NEEDED` libraries as `so:` packages. Bluetooth audio
  is meant to be `bluez-alsa` in that root. **None of the Alpine-root runtime has been
  run on a tablet yet.**

## Open questions

* A dynamic app running inside the Alpine root on a tablet; bluez-alsa + a headset.
* USB serial stability per tablet (identity keying).
* rM1/rM2 and the Move/Pure: little or no hands-on testing; armv7 Alpine-root builds
  are not wired.
* A fast waveform: Oxide/Blight has per-update waveforms but was not tried.

## Merge notes (for c19unified)

* Base: `6b03cd4af` (tip of `c19unified-018-qr-encoder-runctl-and-kit-checker.bundle`).
  This work is the commits after it on `main`.
* New crates: `shared/eink-kit` (+ `eframe-shim`, `alpine-run`),
  `games/unendlich/crates/unendlich-eink` (added to the unendlich workspace members).
* Changed: `apps/gitmind` (cockpit: `eink_fleet`, `alpine_root`, `capabilities`,
  `tablet_identity`, `elf_deps`, `finding`; `deploy.rs` gains a per-host ssh option
  registry; cli; gui Tablets/Diagnostics pages and `show.rs`), `apps/depot` (Alpine
  projection, API type, catalog refresh), `apps/tomescope` (app moved into the library;
  `desktop`/`eink` features), `apps/aperture` (`voice` feature), `apps/studio` (Opus
  behind `audio`), `apps/rusty-board` (two pre-existing compile errors fixed),
  `shared/scene-kit` (GPU-free `glow_scene` under `cfg(eink)`), `shared/vidkit`
  (e-ink hint).
* Lock files touched: gitmind, tomescope, unendlich, and the new crates' own.
  gitmind's cockpit now depends on `apps/depot/crates/depot-core` by path.
* Known failures that predate this work: gitmind cockpit
  `flow::launch_consistency_tests::every_registered_app_is_offered_in_the_store`
  (stoat-egui registered but not in `store_groups()`). studio's
  `tcp_carrier_tests::a_word_crosses_the_socket_and_comes_out_of_the_far_sink` is
  timing-sensitive and failed once under full-suite load (passed alone 5/5).

## Probe of 2026-10-02 (Paper Pro at 192.168.2.185, OS 3.20.0.92) and the COSMIC question

Read-only, over ssh as root on the LAN (Wi-Fi ssh was already on). **Seen** unless marked.

* Hardware/OS: `imx8mm-ferrari`, Linux 6.6.52 (Codex Linux 5.0.57, scarthgap), 4 cores, 1.9 GB RAM
  (1.4 GB available), up 11 days. `/` is 514 MB and 100% full; `/home` is a 46 GB encrypted volume
  with 41 GB free. DRM has `card0` and `card0-LVDS-1` only: **no render node, no `/dev/fb*`** (so no
  GPU for a Wayland compositor; software rendering is the only option). Internet works (the Alpine
  CDN answers).
* xochitl runs as `xochitl --system` with `LD_PRELOAD=/home/root/xovi/xovi.so`. xovi extensions:
  appload, fileman, framebuffer-spy, literm, qt-command-executor, qt-resource-rebuilder,
  random-suspend-screen, webserver-remote, xovi-message-broker. AppLoad apps registered:
  ambient-learner-egui, eink-demo, **hatch-egui**, koreader, paperbound-eink, **rusty-board-egui**,
  snapclone-egui, stoat-egui, tomescope-eink, unendlich-eink. Their manifests use `EINK_BACKEND=qtfb`,
  `EINK_PROFILE=fast-color`.
* Left on the device from earlier work: `vnsee` + `vnsee-gui` (a VNC client for the panel),
  an ArchLinuxARM tarball (543 MB), KOReader, `qtfb-shim`, `eink-perf/` and `perf-track/` samplers,
  rustup/cargo, nix. There is no `gcc`, `git` or `apk` on the device itself (python3, curl, rsync, tar,
  `chroot` only).

### Can COSMIC run on it?

Two routes, with different honesty levels:

1. **Remote COSMIC, local glass (feasible now, uses parts that exist).** Run the session where it
   already runs (the Fairphone) and show it on the tablet through VNC with `vnsee`. cosmic-comp on the
   phone advertises `ext_image_copy_capture` and `ext_output_image_capture_source`, and `wayvnc 0.10` /
   `neatvnc` are in the phone's repos, so a VNC server can capture the output. **Gap:** cosmic-comp does
   not advertise `zwlr_virtual_pointer`, which wayvnc uses for touch/mouse, so the picture arrives but
   taps do not (the same missing-injection problem as tideshell's EI patch). Needs either a pointer path
   (EI once cosmic-comp has the patch, or uinput) or a different VNC server. **Unverified:** latency and
   how the e-ink refresh policy copes with a full desktop.
2. **Local COSMIC on the tablet (unproven).** Needs cosmic-comp in an Alpine root under `/home`
   (`cockpit::alpine_root`; the packages exist for aarch64 on the phone's Alpine-based distro, whether
   Alpine's own repos carry them is **unchecked**), a software GLES renderer, and a bridge from the
   compositor's output to qtfb (AppLoad window) plus touch back in. With 1.4 GB free, no GPU and a
   compositor that expects one, expect this to be heavy; nothing about it has been tried.

Either way the unit of work is the same missing piece: **a Wayland-output to qtfb bridge** (route 2) or
**pointer injection into cosmic-comp** (route 1). Route 1 first: it needs no changes on the tablet.

