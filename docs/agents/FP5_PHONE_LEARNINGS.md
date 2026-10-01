# Fairphone 5 (postmarketOS, COSMIC) — what a session on the real phone learned

Written 2026-10-01 from one long session that ran against the owner's phone
(`user@192.168.2.231`, ssh key `~/.ssh/id_phone`; laptop is the Asahi M2 Max `montroyal`).
Tags: **[measured]** = observed on the device, command or log given; **[docs]** = taken from a
document or upstream source, not re-verified; **[unverified]** = built or reasoned, not seen working.

## Which machine am I on
`whoami`, `fastfetch` or `ip a` before anything destructive. A plain Bash call is the laptop; a call
through `ssh … user@192.168.2.231` is the phone. `sudo reboot` on the phone is allowed; never on the
laptop. The phone needs the owner to log in again afterwards (`printf 'type 147147\nkey enter\n' | sudo dotool`).

## The display wedge — do not blame polling first  [measured]
After ~20 minutes the phone's frame drawing became unusable. Kernel log: `[dpu error]enc35 frame done
timeout`, then ~22 lines/s of `ctl start interrupt wait failed` / `wait for commit done returned -22`
(33,793 in one boot), starting right after a kernel call trace and tideshell's brightness writes timing
out. `cosmic-comp`'s render thread (`surface-DSI-1`) sat at ~95% of a core. Only a reboot cleared it; the
next boot had 0 errors and a smooth screen. Check `journalctl -k -b | grep -c "dpu error"` before
patching cosmic or any poller. Panel is `panel_raydium_rm692e5` (command mode), kernel 7.2.2. The
shell's brightness mode was already `Safe` (pmaports#4274).

## Keys  [measured]
* cosmic-comp, tideshell and cosmic-session all hold the power key; cosmic's `XF86PowerOff` shortcut is
  routed to `tideshell ctl power-key` (press only, no release).
* Volume keys were **never read** while cosmic routed the power key: tideshell skipped evdev entirely.
  Fixed: evdev stays open for the volume nodes (`pmic_resin` = down, `gpio-keys` = up) and for the power
  node in release-only mode (the press length for the morse unlock).
* Cold mode (`shell.cold = auto`) turns the volume keys into a steering selection; that looks like
  "volume buttons don't work".
* A `systemctl poweroff` once came from a process in `user@10000.service` (logind log line) while the
  owner was using the hatch; cause not identified (cosmic's power dialog is the suspect).

## Rotation  [measured]
`iio-sensor-proxy` only reports CHANGES, so tideshell's "hold before rotating" (now 900 ms, was 450: less twitchy; the sensor proxy's own tilt angle is not configurable from the shell) never completed
(it waited for a second reading). Now judged by the clock (`AutoRotate::poll`). Both landscapes and, after
the display reboot, portrait work. Sensor mounting: all three cameras report `camera_sensor_rotation = 270`;
upright = a clockwise quarter turn (owner confirmed "rot was 90 deg clockwise").

Boot race: after a reboot tideshell asked iio-sensor-proxy `HasAccelerometer` at login, got `false`, and
the orientation thread returned; the proxy found the sensor 17 s later (journal 03:53:39 vs 03:53:56), so
rotation was dead until a restart. It now asks every 3 s. The accelerometer is NOT an IIO sysfs device here
(`/sys/bus/iio` has only the PMIC ADCs): only the proxy sees it, and the proxy gives orientation, not the
gravity vector. So `PAST_DEG` (extra turn past the switch point, needs `on_accel`) is inert on this phone;
the 900 ms hold is the only thing making rotation less twitchy. Making it real needs a raw-accelerometer
source (or a longer hold).

## Camera  [measured unless noted]
* libcamera's soft ISP debayers on the GPU through EGL: it shares the one GPU with the compositor. A
  probe once hung the phone; the memory note `fp5-camera-gpu-hang` says ask first.
* `snapshot-cam` was never on the phone until now. Installed as `~/.local/bin/snapshot-cam` plus a wrapper
  `snapshot-cam-phone` (sets `SNAPSHOT_CAM_TURN=90` and `SNAPSHOT_CAM_SOURCE=gst:libcamerasrc
  camera-name=/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29`) because a desktop `Exec=` cannot quote part of
  an argument. Camera 1 (`camera@29`) is the main sensor with the lens motor.
* The owner's burst photos were a plain average of 6 frames and smeared moving subjects; `stack::average`
  now only blends pixels that agree with the reference frame (GHOST_DIFF). The v4l2 controls error in the
  app comes from reading controls off `/dev/video0` (the video decoder); the real controls are on subdevs.
  Tap-to-focus is not wired. **[unverified]** whether the rotation and burst changes look right.
* Volume-button capture worked with the screen off (owner).

## Fingerprint sensor  [measured]
There is none visible: no node in the device tree mentioning fpc/egis/fingerprint, the only SPI device is
the touchscreen (`goodix,gt9897`), `fprintd` reports "No devices available". Needs identification of the
sensor plus a device-tree entry and driver; not a quick fix.

## Emergency access  [docs + unverified]
postmarketOS's initramfs debug shell (`/usr/share/initramfs/init_functions.sh`): holding volume-down at
boot (`iskey KEY_LEFTCTRL KEY_VOLUMEDOWN`) drops to a shell with buffyboard (console on-screen keyboard),
getty, an ACM gadget getty and `telnetd -l /sbin/pmos_getty` on the USB network (172.16.42.1). The hook is
not installed here (`/etc/mkinitfs/hooks` empty) and it only exists during boot. For a running system:
`system/fp5-rescue` (hold power, tap volume-down three times → login on tty3 with buffyboard) and the gitmind
plans `emergency_access_plan` (sshd password login on 172.16.42.1 only) and `rescue_console_plan`.
`usb0` already carries 172.16.42.1 from `usb-moded`'s developer profile.

## Login  [measured]
cosmic-greeter's on-screen keyboard did not let the owner in. `autologin_plan` adds greetd's
`[initial_session]` (user `user`, `start-cosmic`) to `/etc/greetd/cosmic-greeter.toml` (backup
`.before-autologin`); applied by hand on 2026-10-01 02:31, effective at the next boot, greeter kept as
`default_session`. Tideshell's lock is a cover (swipe) plus the power-key morse unlock
(`shell.unlock.morse`); PAM in tideshell is not built.

## Keyboard (rusty-board-egui)  [measured]
No `tracing` subscriber was installed, so every focus/input-method log line was dropped. Now logs to the
journal (`RUST_LOG`, warn by default). cosmic sends input-method Activate/Deactivate correctly and the
layer surface maps/unmaps. The magnifier bubble is replaced by a press pulse. Open: steno multitouch, and
the bug where switching away from steno leaves the keyboard short.

### Layer surface: unmap is a trap on cosmic-comp  [measured with WAYLAND_DEBUG=1 + RUSTY_BOARD_LOOP_STATS=1]
Hiding the board with a null-buffer commit (the protocol's "unmap") works once; the NEXT show sends
`set_anchor/set_size/set_exclusive_zone` + commit and cosmic-comp **never sends a configure**. The board
then holds its exclusive zone and paints nothing: a black band under the apps and a right bar that stops
short (a bar anchored top+bottom is laid out inside every other surface's zone). A resize-nudge on the
unmapped surface got no answer either, 300 ms after 300 ms. A resize of a *mapped* surface is always
answered, so hide now PARKS (1 px, exclusive zone 0, a transparent 1 px buffer; the EGL config has alpha)
and show resizes back. Two traps found on the way:
* a nudge placed after the loop's "is a frame due?" check never runs when nothing is due;
* egui lays the board out against the surface height it is given and then asks for a surface that tall:
  tell it the full height while parked or before the configure, or it comes back tiny.
The loop-stats line prints `mapped / configured / frame_pending`, which names the stuck gate directly.
Also: `rusty-board.service` has `ProtectSystem=strict`, so `$XDG_RUNTIME_DIR/rusty-board.ctl` could not be
bound ("Read-only file system") and every ctl command (cold, strip, voice, the new `show`/`hide`) was lost;
`ReadWritePaths=%t` fixes it (in the unit and in the `fp5_setup` drop-in; printf needs `%%t`).
Chromium on the phone did not drive the board's input-method focus in one test (address bar focused, no
Activate logged); GTK apps do. Unresolved: probably needs its Wayland text-input flag.

### Typing came out as Dvorak  [measured, fix unverified on device]
The phone's COSMIC layout is `us`/`dvorak` (`~/.config/cosmic/com.system76.CosmicComp/v1/xkb_config`; the
console keymap too). On the input-method-v2 path the board typed every character as a QWERTY evdev keycode
through `zwp_virtual_keyboard_v1`, and cosmic-comp maps keycodes with the SEAT's layout, ignoring the keymap
the client uploads: `i` -> `c`, `s` -> `o`, and Ctrl+C was Ctrl+J. Plasma Mobile used the v1 keysym path,
which is layout independent, which is why it "worked there". Now: text goes through
`zwp_input_method_v2.commit_string` while a field is active, and the char -> keycode table (chords, no-focus
fallback) is generated from the layout in COSMIC's `xkb_config` via xkbcommon. Named keys (BackSpace,
Return, arrows) are the same keycodes in every layout and are unchanged.

### Height after a short board  [measured]
steno -> qwerty stayed short: the wanted surface height subtracted the central panel's bottom from the screen
bottom, and on a screen shorter than the keys that goes negative (-100), so the board asked for the short
height again. Panel space is now read from `ctx.available_rect()` before the central panel.

## Dock and the right bar  [measured]
A dock with `exclusive_zone: true` shortens the tideshell bar even when autohidden. `exclusive_zone false`
on both COSMIC panels (live config) lets the bar reach the bottom.

## Palette (tideshell)
Order is now list, search, then the three mode buttons, hung off the bottom edge. It opens with 4 rows and
opens in full when a hold reaches the search bar (or the field is tapped); the search does not move under
the thumb. With no physical keyboard (`/proc/bus/input/devices`: `kbd` handler + LED events; virtual and
dotool devices do not count) tideshell sends `show`/`hide` on the rusty-board ctl socket, because its search
field is not a text-input client. All unit-tested; the on-device feel is not yet confirmed.

## Video player on the phone  [measured]
vidkit-player is installed, but the phone has no `yt-dlp` and no `ffmpeg` (`apk search` has both), so a
pasted PAGE link cannot resolve; `wl-paste` itself works (data-control is exposed). Plan:
`fp5_setup::video_tools_plan` (needs root). The picker's filter field auto-focused on open and held the
on-screen keyboard up; it now only auto-focuses if no touch has been seen.

## Display wedge, second occurrence  [measured]
Trace before the first `[dpu error]`: `brightness_store` -> `rm692e5_bl_update_status` ->
`mipi_dsi_dcs_set_display_brightness_large` -> `dsi_link_clk_set_rate` -> `_set_opp`, i.e. a sysfs backlight
write while a frame was in flight; ~22 errors/s followed, `cosmic-panel` grew ~6 MB/s (2.6 GB) and the UI
crawled. Reboot cleared it (0 errors after). The writer is not identified (tideshell's `Safe` path or
COSMIC's own control). `sudo`/`systemctl reboot` over ssh now ask for interactive authentication, so the
reboot needed the owner (power button).

## Not done (as of 2026-10-01 04:10)
Who writes the backlight (see above); a raw accelerometer for the rotation margin; apk yt-dlp/ffmpeg (root);
the `show`/`hide` summon and palette feel on the device; pinch-expand animation for the board (patch kept off-tree: scaling the layer also scales pointer positions
and broke two hold tests; an in-board approach is needed); steno multitouch and the short-keyboard bug;
camera tap-to-focus and highlights; the phone-to-laptop model path; the unexplained power-offs (two more
this session: read the previous boot's journal next time it is up).

## Build and deploy that worked
* Build for the phone in the arm64 Alpine container (`apps/tideshell/deploy/build-in-podman.sh`, image
  `tideshell-aarch64`); for other crates run `cargo build --release` in that image with the repo copied in.
* **`pack-src.sh` uses `git archive HEAD`**: uncommitted work is silently not in the build.
* Copy binaries with `ssh … 'cat > file' < binary`; plain `scp` hung. Under zsh keep ssh options in an array
  (`K=(-i … -o IdentitiesOnly=yes -o BatchMode=yes); ssh $K …`): an unquoted string is not split.
* Start test apps with `systemd-run --user --unit=… -E WAYLAND_DISPLAY=wayland-1`; a background `nohup` over
  ssh dies with the session.
* Everything above that touches the phone is meant to be replayable from gitmind's `fp5_setup` plans.

## Git hygiene in this repo
The real index holds the owner's staged work. Commit through a temp index, `export GIT_INDEX_FILE=… &&
git read-tree HEAD && git add <mine> && git commit` **in one Bash call** (the variable does not survive to
the next call, and a bare commit sweeps the owner's staging); resync with `git reset -q HEAD -- <paths>`.

## Ornith-1.5-9B on the M2 Max  [measured]
`kernel-validate --bench` panicked on a hybrid model (raw `new_cache()`); fixed to `new_cache_for`. Result:
5.08 tok/s at context 544, 6.91 at 40; 190 of 197 ms/token is GPU wait, ~28 GB/s effective (~7% of 400 GB/s),
the same as the 3B. See `apps/logscope/perf-loop/LEDGER.md`.
