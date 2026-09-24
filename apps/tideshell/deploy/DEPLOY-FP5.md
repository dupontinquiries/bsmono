# Deploying tideshell to the Fairphone 5 — directives

For the agent doing the deployment. Follow the stages **in order**. Each ends
in a **gate**: if it fails, stop, do not improvise past it, and report the raw
output listed under "Report". Raw command output beats summaries.

tideshell is a Wayland client (layer-shell surfaces). It does **not** replace
cosmic-comp and cannot break the session by itself: stopping the process
removes all its surfaces. The only step that touches the compositor is the
optional stage 7, and it has a rollback.

## What is and is not verified (read before judging results)

Verified against a **nested** cosmic-comp epoch-1.8.0 in a sandbox, mouse as
one finger: edge strips, swipe-up switcher over real windows, card tap
activates, drawer, hold arc, trackpad over EI (with patch 0002), fat cursor,
right bar with exclusive zone, fractional scale 1.5, honest failure reports.

**Never run on the phone.** Unknown until you run it: real multi-touch,
whether the digitizer reports touches at the very screen edges, the FP5's
real scale (~2.5) and rotation, cosmic-panel interactions, the Adreno EGL
path. Expect surprises there; the trace is designed to explain them.

## Stage 0 — get the code (on the Mac)

```sh
git clone -b tideshell c19unified-025-tideshell-deploy.bundle mono
cd mono && git log --oneline -1          # note this commit in your report
```

Everything for deployment is in `apps/tideshell/deploy/`:

| file | what |
|---|---|
| `DEPLOY-FP5.md` | this document |
| `preflight.sh` | read-only phone checks, PASS/WARN/FAIL |
| `fp5-touch-diag.sh` | which layer drops touch; edge-touch test |
| `fp5-camera-diag.sh` | why the camera works under plasma but not cosmic, link by link |
| `install-speaker-chain.sh` | build bankstown as a LOADABLE LV2 bundle (the circulating installer produces one lilv cannot read), check the LADSPA stages, install tideshell's speaker sink, verify. `--check` changes nothing |
| `tideshell-settings` | Display (text size, control size, target floor, cold mode), Speaker (the audio chain), COSMIC. Writes the shell's own prefs file and sends `tideshell ctl reload-prefs`, so a change applies without restarting the shell |
| `typein` (built from `tools/typein`) | type text and press keys with dotool under any keyboard layout: reads cosmic's own `xkb_config` and sets `DOTOOL_XKB_LAYOUT`, so `typein warp enter` types "warp" instead of ",apl". Emacs control macros: `C-a`, `M-x`, `C-{x c}` |
| `focus-probe.sh` | characterise the ultra-wide's VCM: range, step size, resonance, hold current, write rate. Hit Enter when you hear it; the report says which named step made the noise |
| `50-tideshell-powerkey.conf` | logind: short power press to the shell, long press powers off |
| `io.rustyboard.Keyboard.service` | D-Bus activation: tideshell's keyboard toggle starts rusty-board |
| `70-tideshell-keys.rules` | udev fallback: the keys by evdev (only without `tideshell install-keys`) |
| `build-in-podman.sh` + `Containerfile.aarch64` | build on the Mac in an emulated Alpine (unverified) |
| `GITMIND-DEPLOY.md` | the deploy as a one-button plan for gitmind/depot |
| `AMBIENCE-API.md` | how snapclone (or anything) names the place tideshell is at |
| `pack-src.sh` | packs the 11 crates the phone build needs |
| `run-in-session.sh` | runs tideshell inside the cosmic session from ssh |
| `tideshell.service` | systemd user unit (autostart), stage 6 only |
| `cosmic-comp/0001-*.patch` | touch focus (tap focuses the window) |
| `cosmic-comp/0002-*.patch` | lets tideshell open cosmic-comp's EI socket (trackpad) |

Phone access: the reverse tunnel, `127.0.0.1:8022`. scp needs the port as a
flag: `scp -P 8022 FILE user@127.0.0.1:~/` (not `host:8022:path`).

## Stage 1 — preflight (phone, read-only)

```sh
scp -P 8022 apps/tideshell/deploy/preflight.sh user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'sh preflight.sh' | tee preflight.txt
```

Run while logged into the cosmic session on the phone.

**Gate:** last line `PREFLIGHT OK`. Any `FAIL` → stop. `WARN`s are allowed
but must be in the report (they predict which features will degrade:
missing session bus → no trackpad; missing rusty-board → Keyboard mode
reports a failure; cosmic-panel → possible overlap with the strips/bar).

**Report:** all of `preflight.txt`.

## Stage 2 — touch diagnostics (phone)

```sh
scp -P 8022 apps/tideshell/deploy/fp5-touch-diag.sh user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'sh fp5-touch-diag.sh'         # static checks
ssh -p 8022 user@127.0.0.1 'sh fp5-touch-diag.sh live'    # then tap the screen for 15 s
```

During the live run, tap: the centre, then **as close as possible to the
very top edge, the very bottom edge and the right edge** (a few taps each).
tideshell's gestures start in 28 px strips at those edges; if the digitizer
reports nothing within ~30 px of an edge, the design needs changing and you
should stop after this stage.

Then check touch-to-focus by hand: with two windows open, tap the window that
is **not** focused and type on the external keyboard. Note which window gets
the text.

**Gate:** touches reach libinput (`TOUCH_DOWN` count > 0). Edge taps are
reported at coordinates within ~30 px of each edge.

**Report:** both outputs; the edge result; which window got the typed text.

## Stage 3 — pack and transfer the source

On the Mac, in the clone:

```sh
sh apps/tideshell/deploy/pack-src.sh      # -> tideshell-src.tar.gz (a few MB)
scp -P 8022 tideshell-src.tar.gz user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'mkdir -p ~/src/tideshell && tar -xzf tideshell-src.tar.gz -C ~/src/tideshell'
```

## Stage 4 — build ON the phone

Build on the phone, dynamically linked. **Do not** use the Mac's static musl
cross-build for tideshell: it links libwayland-client and libEGL, and Mesa's
libEGL exists only as a shared library. (Separately: a *static* musl build of
any eframe app, e.g. hatch-egui, cannot `dlopen` libwayland/libEGL at
runtime, so it will likely fail to open a window. Verify before relying on it.)

```sh
ssh -p 8022 user@127.0.0.1
sudo apk add wayland-dev mesa-dev pkgconf     # package names unverified; add what the linker asks for
cd ~/src/tideshell/apps/tideshell/egui-frontend
cargo build --release --locked --no-default-features --features layer,ei --bin tideshell 2>&1 | tee ~/tideshell-build.log
install -Dm755 target/release/tideshell ~/.local/bin/tideshell
```

- `--no-default-features --features layer,ei` builds the real shell only:
  no eframe, no winit, no X11 (checked: zero of those crates in the graph).
- The first build compiles a few hundred crates. Let it finish; do not
  restart it midway.
- `--locked` uses the committed Cargo.lock. If it fails on a Rust version
  error, report `rustc --version` and the error; do not edit the lockfile.

**Gate:** `~/.local/bin/tideshell` exists; `ldd ~/.local/bin/tideshell`
shows libwayland-client and libEGL resolved (no "not found").

**Report:** the last 30 lines of `tideshell-build.log`, the `ldd` output,
build duration.

## Stage 5 — manual run inside the session (the real test)

```sh
scp -P 8022 apps/tideshell/deploy/run-in-session.sh user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'sh run-in-session.sh --show-strips'
```

`--show-strips` tints the edge strips purple so you can see them. The
engine trace streams to the terminal and `~/tideshell-run.log`. Stop with
Ctrl-C; its surfaces disappear.

Try each, and for each note what happened on screen **and** the trace lines:

| # | gesture | expected trace |
|---|---|---|
| 1 | nothing (just started) | `ei: Ready` (or `ei: Failed(...)` = no patch 0002 / no bus: fine, note it) |
| 2 | short swipe up from the bottom edge | `edge pending -> swipe-up`, `swipe-up -> Switcher`; switcher shows open windows |
| 3 | tap a window card | `-> Activate(n)`, `ok: sent Activate to n`; that window comes to front |
| 4 | long swipe up (past half the screen) | `swipe-up -> Launcher`, `-> SummonLauncher` (hatch opens if installed) |
| 5 | swipe down from the top edge | `edge pending -> drawer`, `drawer release open=true`; drawer shows |
| 6 | swipe sideways along the bottom edge | `quick-switch Back` + `-> Activate(n)` |
| 7 | press and hold on the bottom edge, slide onto Keyboard, lift | `edge pending -> hold`, `hold release -> Some(Keyboard)`, then `succeeded` or `FAILED ... exited N` (+ `keyboard mode off`) |
| 8 | hold -> Trackpad; drag on the pad; tap | pointer moves; tap clicks (needs `ei: Ready`) |
| 9 | drawer -> Shell settings -> Right-hand bar -> Apply | bar appears on the right; maximized windows narrow beside it (`reflow ... maximized window(s)`) |
| 11a | the power key, the right way (replaces the udev route below). FIRST `sudo ~/.local/bin/tideshell doctor power-key`: it lists every process reading the power key, the login manager's effective HandlePowerKey and the file that set it, held inhibitors and cosmic's bindings, and ends with a verdict naming each consumer and its fix. Then: nothing to run for the bindings -- tideshell writes them into cosmic's `custom` shortcuts at every start (idempotent; cosmic-comp reloads the file live, confirmed on the FP5; trace line `keys: cosmic custom shortcuts AlreadyBound|Written{...}`). It keeps your other bindings, saves your original once as `custom.before-tideshell`, and rebinds if the binary moved. `TIDESHELL_MANAGE_KEYS=0` in `~/.config/tideshell/env` turns this off; `tideshell install-keys` / `remove-keys` do it by hand. While tideshell is not running the power key does nothing (its `ctl` has no socket) rather than showing cosmic's shutdown dialog and the logind drop-in into the directory the doctor names (`/etc/systemd/logind.conf.d/` for systemd-logind, `/etc/elogind/logind.conf.d/` for elogind; see the file's header); restart tideshell; re-run the doctor until its verdict is empty | `keys: from cosmic-comp via tideshell ctl; logind power-key inhibitor held` and `ctl: listening`; a short power press -> cover + fade + `output power: set off`, NO shutdown; a long press (~5 s) still powers off; volume keys fade. Test the socket by hand: `tideshell ctl power-key` |
| 11b | keyboard: `install -Dm644 io.rustyboard.Keyboard.service ~/.local/share/dbus-1/services/` (and the rusty-board unit + binary as its own deploy); make sure squeekboard is not also running (`pgrep squeekboard`: one input method per seat, the second gets Unavailable) | tapping a text field shows the keyboard (input-method-v2, like plasma-mobile); the drawer's Keyboard tile starts it if needed |
| 11 | (fallback, only without 11a) FIRST: `sudo install -Dm644 70-tideshell-keys.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input` (or `sudo adduser user input` + relogin; sudo over ssh: `echo PASSWORD \| sudo -S ...`). Then at start read the `keys:` line; press the POWER key once; wait; triple-tap | `keys: <device names>; logind power-key inhibitor held` (if it says `cannot open ... input group`, run `sudo adduser user input`, log out and in, retry); power key -> `power key`, `cover shown`, `display off`, `output power: set off on 1 output(s)` and the screen goes dark WITHOUT powering off; triple tap -> `triple tap: wake`, `output power: set on`. If the phone powers off instead, the inhibitor was not held: report the `keys:` line |
| 12 | press volume up / down | `-> SetSlider(Volume, ...)` then `ok: volume fade 0.xx -> 0.yy` and the level changes smoothly (needs `wpctl`, i.e. PipeWire/WirePlumber; else `FAILED: wpctl ...`) |
| 14 | drawer -> drag on the Volume pad: left/right = level, up = cut through, down = soft | first drag: `FAILED: voicing sink installed at ~/.config/pipewire/pipewire.conf.d/tideshell-voicing.conf; restart pipewire ...` -> run `systemctl --user restart pipewire wireplumber`, then `wpctl set-default $(wpctl status | grep -m1 'tideshell voicing' | grep -oE '[0-9]+' | head -1)`; later drags: `ok: voicing +0.70: hp 250 Hz ...` and speech gets clearer / softer. Needs `pw-cli` (pipewire-tools) |
| 15 | put the phone in a pocket, then triple-tap through the fabric; take it out, wait a second, triple-tap | `proximity: iio-sensor-proxy` at start; in the pocket `proximity near: covered` and the taps do nothing; out of the pocket, after ~0.8 s, the triple tap wakes |
| 16 | camera pane: with the cover up (power key, wake), read the `camera:`, `imu:`, `compass:`, `mic:` lines; turn on the spot | `camera: started gst-launch-1.0` and the picture behind the clock with a red dot (needs `gst-launch-1.0` + the libcamera GStreamer plugin: `apk add gstreamer-tools gst-plugins-base libcamera-gstreamer`, names unverified); `imu: gyro ... accel ...`; turning the phone moves the heading number and the anchors stay on their spots. If the picture is missing, run `sh fp5-camera-diag.sh` (in the bundle) and report it. The mic: `mic: pw-record ...` then, in a noisy place, `ambient sound: floor -30 dBFS -> notification gain 0.9`; `shell.ambient.mic = off` disables it |
| 16a | which camera: `cam -l 2>/dev/null` lists them (on the FP5 seen: 1 front `.../i2c-bus@1/camera@10`, 2 back ultrawide `.../i2c-bus@1/camera@29`, 3 back main `.../i2c-bus@0/camera@1a`). Without a choice libcamerasrc opens #1, the FRONT one. Put `TIDESHELL_CAMERA=/base/soc@0/cci@ac4a000/i2c-bus@0/camera@1a` in `~/.config/tideshell/env` (read by tideshell.service and run-in-session.sh). Before adding `TIDESHELL_CAMERA_CAPTURE=WxH`, check the camera accepts the size: `timeout 10 gst-launch-1.0 libcamerasrc camera-name=<id> ! video/x-raw,width=1280,height=960 ! identity eos-after=30 ! fakesink && echo ok` (a size it cannot give fails with not-negotiated) | the `camera:` line shows `camera-name=` with the id; the pane shows the rear view |
| 16b | touch settings: `install -Dm755 out/tideshell-settings ~/.local/bin/` and `install -Dm644 tideshell-settings.desktop ~/.local/share/applications/`; open "Settings (touch)" from the launcher. Change Dark mode, then tap its label | the theme flips live; the details show `your setting: ~/.config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark` and `file says: false`; Reset returns it to the built-in default. Screen off after 30 s: the display goes off after 30 s idle (cosmic-idle reloads live) |
| 17 | cold mode: `shell.cold = on` (or a battery below 8 C) | trace `cold mode on`; the edge strips grow (purple with --show-strips), holds take longer, taps tolerate wobble; `auto` follows the battery temperature |
| 18 | gyro select: hold the bottom edge until the palette opens, keep the finger down, tilt the top edge away / roll left-right, lift | trace `tilt select Row(n)` / `Mode(n)` as you tilt; lifting activates the highlighted item |
| 19 | bright sun: walk outside with the drawer open | the shell's contrast rises with the light, flips to a light look around 8 000 lux, and `sunlight: apps to the light theme` switches COSMIC apps to light (restored below ~3 000 lux); `cat ~/.config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark` shows false meanwhile |
| 20 | voice route: `cat /run/user/$(id -u)/tideshell.voice-route` with a text app in front, then with the palette open; with comfortgov running, also while hot or below 20 % | `tiers=tiny,base reason=...` / `tiers=tiny reason=command...` / `tiers=tiny reason=phone is hot, low or busy...`; voiceterm reading this file is the follow-up |
| 13 | turn the phone to landscape, hold it there | `orientation: iio-sensor-proxy accelerometer` at start; after ~0.5 s in landscape `rotate LeftUp` (or RightUp) then `ok: rotated DSI-1 to 90`; the screen rotates. If it rotates the WRONG way, set `shell.rotate.flip = on`. Needs `wlr-randr` (`apk add wlr-randr`); a 45° tilt or laying it flat must not rotate. `shell.rotate.auto = off` disables |
| 21 | brightness on this panel: read `backlight write mode:` at start | `Safe` on the FP5 (its backlight is `ae94000.dsi.0`): one write per settled value, because the DPU driver corrupts the display under a stream of changes (pmaports #4274: garbage bars, flicker, the panel switching off). The slider still animates; the panel steps once when you let go. `shell.brightness.mode = smooth` re-enables the ramp only on a kernel with Marcel's DPU patch from that issue |
| 10 | at start: read the `ambient:` and `backlight:` lines; then cover the light sensor (top bezel) for 5 s, uncover | ideally `ambient: iio-sensor-proxy (lux)` and `backlight: <name> (max N) via logind ...`; screen dims ~3 s after covering, brightens quickly after uncovering. `no light sensor found` = time of day only (report it); `cannot set` = neither logind nor sysfs worked (report the line) |

Also capture screenshots if `grim` is available
(`WAYLAND_DISPLAY=... grim ~/ts-N.png`) for #2, #5, #9.

**Gate:** tideshell stays running, strips visible, #2 and #5 work.

**Report:** `~/tideshell-run.log`, the table filled in, screenshots, and for row 10 the output of `busctl --system list | grep -i sensorproxy` and `ls /sys/class/backlight/`; for row 11, `id` and `ls -l /dev/input/`.

If a gesture does nothing, the trace says why: no `edge pending` line = the
touch never reached the strip (edge dead zone / cosmic-panel on top); an
`unwired:` or `FAILED:` line names the missing piece.

## Stage 6 — autostart (only after stage 5 passed)

```sh
scp -P 8022 apps/tideshell/deploy/tideshell.service user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 '
  install -Dm644 tideshell.service ~/.config/systemd/user/tideshell.service &&
  systemctl --user daemon-reload && systemctl --user enable --now tideshell.service &&
  systemctl --user status tideshell --no-pager | head -5'
```

Logs: `journalctl --user -u tideshell -f`. Undo:
`systemctl --user disable --now tideshell.service`.

**Gate:** active after a log-out/log-in (it is `WantedBy=graphical-session.target`,
the same mechanism rusty-board's autostart already uses).

## Stage 7 (optional) — cosmic-comp patches

Only if needed:
- **0002** if stage 5 #1 showed `ei: Failed(... Access denied ...)` and you
  want the trackpad.
- **0001** if stage 2 showed taps do not focus windows.

Both are against **cosmic-comp epoch-1.8.0**; check the installed package
matches (`apk info -v cosmic-comp`). The on-phone recipe (~40 min):
```sh
cd ~/src && git clone --depth 1 -b epoch-1.8.0 https://github.com/pop-os/cosmic-comp && cd cosmic-comp
git apply --check ~/0002-dbus-ei-allow-tideshell.patch && git apply ~/0002-dbus-ei-allow-tideshell.patch
sudo apk add libinput-dev libseat-dev libdisplay-info-dev libxkbcommon-dev mesa-dev pixman-dev udev-dev   # names unverified
cargo build --release
```
then the binary swap and rollback below. Better long-term: the pmOS aport
with the patch in `source=`, built as an apk (gitmind's packaging.rs).

Swapping the binary (the fast test path):

```sh
sudo cp /usr/bin/cosmic-comp /usr/bin/cosmic-comp.orig
sudo install -m755 target/release/cosmic-comp /usr/bin/cosmic-comp
sudo systemctl restart display-manager      # logs the session out
```

**Rollback** (from ssh if the screen stays black): `sudo apk fix cosmic-comp`
then `sudo systemctl restart display-manager`. Do not do this stage without
ssh access confirmed working, and never on a phone whose tunnel is down.

## Stage 8 (optional) — comfortgov, the comfort governor

Separate daemon, `system/comfortgov` in the bundle (no dependencies). Build
it on the phone, probe, dry-run, and only then apply:

```sh
cd ~/src/tideshell/system/comfortgov && cargo build --release
./target/release/comfortgov --probe                # report this output
./target/release/comfortgov | head -50             # dry run: decisions, nothing written
sudo ./target/release/comfortgov --apply --hint /run/user/$(id -u)/comfortgov.input
```

The probe prints each cluster's OPP table and the heuristic knee; the
skin sensor it picked (report which); battery; MGLRU/PSI availability.
tideshell touches the hint file on every touch, so with both running the
governor sees your input. Undo: stop it; the kernel's defaults return at
the next boot (or `echo` the cpuinfo min/max back into scaling_min/max_freq).

**Report:** the probe output, a minute of dry-run log while you use the
phone (tap around, then leave it alone), and whether anything felt rough
or warm with --apply.

## Rollback summary

| what | undo |
|---|---|
| a running tideshell | Ctrl-C, or `pkill -x tideshell` |
| autostart | `systemctl --user disable --now tideshell.service` |
| cursor hidden after a crash in fat-cursor mode | start tideshell once (restores), or `rm ~/.config/cosmic/com.system76.CosmicComp/v1/cursor_hide_timeout` |
| settings | `rm ~/.config/tideshell/prefs` |
| patched cosmic-comp | `sudo apk fix cosmic-comp` |
| comfortgov limits | `sudo systemctl stop comfortgov` (or Ctrl-C), then reboot or restore cpuinfo min/max into scaling_min/max_freq |

## What to send back

1. Commit from stage 0.
2. `preflight.txt`.
3. Touch diag outputs + edge result + focus result.
4. Build log tail, `ldd`, duration.
5. `~/tideshell-run.log` + the stage-5 table + screenshots.
6. Anything you changed that is not in this document, and why.
