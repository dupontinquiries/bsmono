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
| 11 | at start: read the `keys:` line; then press the POWER key once; wait; triple-tap the screen | `keys: <device names>; logind power-key inhibitor held` (if it says `cannot open ... input group`, run `sudo adduser user input`, log out and in, retry); power key -> `power key`, `cover shown`, `display off`, `output power: set off on 1 output(s)` and the screen goes dark WITHOUT powering off; triple tap -> `triple tap: wake`, `output power: set on`. If the phone powers off instead, the inhibitor was not held: report the `keys:` line |
| 12 | press volume up / down | `-> SetSlider(Volume, ...)` then `ok: volume fade 0.xx -> 0.yy` and the level changes smoothly (needs `wpctl`, i.e. PipeWire/WirePlumber; else `FAILED: wpctl ...`) |
| 14 | drawer -> drag on the Volume pad: left/right = level, up = cut through, down = soft | first drag: `FAILED: voicing sink installed at ~/.config/pipewire/pipewire.conf.d/tideshell-voicing.conf; restart pipewire ...` -> run `systemctl --user restart pipewire wireplumber`, then `wpctl set-default $(wpctl status | grep -m1 'tideshell voicing' | grep -oE '[0-9]+' | head -1)`; later drags: `ok: voicing +0.70: hp 250 Hz ...` and speech gets clearer / softer. Needs `pw-cli` (pipewire-tools) |
| 15 | put the phone in a pocket, then triple-tap through the fabric; take it out, wait a second, triple-tap | `proximity: iio-sensor-proxy` at start; in the pocket `proximity near: covered` and the taps do nothing; out of the pocket, after ~0.8 s, the triple tap wakes |
| 13 | turn the phone to landscape, hold it there | `orientation: iio-sensor-proxy accelerometer` at start; after ~0.5 s in landscape `rotate LeftUp` (or RightUp) then `ok: rotated DSI-1 to 90`; the screen rotates. If it rotates the WRONG way, set `shell.rotate.flip = on`. Needs `wlr-randr` (`apk add wlr-randr`); a 45° tilt or laying it flat must not rotate. `shell.rotate.auto = off` disables |
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
matches (`apk info -v cosmic-comp`). Source: `git clone --depth 1 -b
epoch-1.8.0 https://github.com/pop-os/cosmic-comp`, then
`git apply --check` each patch before applying. Build per the runbook's
on-phone route (`cargo build --release`), or better as an apk from the
pmOS aport with the patches in `source=`.

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
