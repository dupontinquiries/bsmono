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

## Later the same night  [measured unless noted]
* cosmic-osd shows its own brightness slider (a 79% pill, seen in a screenshot) whenever tideshell's
  auto-brightness writes the backlight. The session restarts cosmic-osd within a second of a kill and
  `~/.local/bin` is last in its PATH, so it cannot be shadowed or removed without root. Open: replace it with
  a tideshell dynamic-island pill in the bar, and find a way to silence the OSD.
* rusty-board popped up on an empty home screen 45 s after login: the focus grace fallback ("no Activate,
  assume the signal is broken"). Off on COSMIC now (`focus_policy::grace_for`).
* Hold the Wi-Fi/Bluetooth/mobile-data tile to open that cosmic-settings page (desktop ids
  `com.system76.CosmicSettings.{Wireless,Bluetooth,Network}`).
* xwaylandvideobridge: stopped from autostarting (`fp5_setup::hide_video_bridge_plan`). The kernel cuts comm
  to 15 characters, so `pkill -x xwaylandvideobridge` matches nothing; use `xwaylandvideobr`.
* hatch hides by quitting on Wayland, so each tap on Apps is a fresh process; it now repaints every frame for
  its first 1.5 s (cause inferred, not reproduced).
* Landscape: the bar is shortened by the board's zone by design, and with the board up only two widgets fit
  (a "+8" overflow). Landscape also leaves the keyboard 44% of the height. Ideas not built: two 3x3 Cardinal
  pads at the left and right edges with the middle free, a compact landscape board, or the board beside the bar.
* `shell.living` is off, and only snapclone reads `tideshell.palette` today: rusty-board and the bars do not.

## More quirks from the same night  [measured unless noted]
* **Sleep/wake storm.** cosmic reports the output "on" at every touch event while tideshell had it off, and the
  old rule answered each report with `DisplayPower(false)`: 132 on/off commands in one boot, ~5 ms apart, with
  a DSI command timeout and a `disp_cc_mdss_pclk0_clk status stuck at 'on'` warning in the kernel at the same
  instant (`brightness_store` -> `dsi_cmds2buf_tx ... ret=-110`, from logind's `(sd-bright)` helper). The
  triple tap could not complete through it, so tap-to-wake never worked. Now a lit panel while asleep stays
  black (OLED: free) and capturing taps, and is blanked ONCE after 5 s without a touch (`Sleep::quiet`).
  Hardware double-tap-to-wake is not available: the mainline goodix-berlin driver has no gesture knob and the
  touchscreen's SPI device has no `power/wakeup`; the software triple tap is the substitute.
* **Carousel flicker.** `reflow_for_zone` ran every loop and sent `SetMaximized(false)` then `(true)` to any
  maximized window reported wider than the zones leave; during a carousel step the zone total (bar + both
  peeks) and the window widths are in flux, so it chased the layout through distorted sizes. It now waits for
  the zone total to hold 700 ms and ignores windows whose state changed within 500 ms. The two carousel
  peeks are not symmetric when the keyboard is up: same-layer zones are applied in creation order, so the
  peek created after the board is shortened by it and the other is not (not fixed).
* **Voice build.** `--features voice` needs `alsa-lib-dev cmake clang clang-dev llvm-dev linux-headers g++
  make` in the Alpine container (no `c++` otherwise: whisper-rs-sys fails). A build script that copies the
  binary after a failed build ships the PREVIOUS binary under the new name (same size to the byte): copy only
  on success. No whisper model exists on the laptop or phone; the engine looks in `$WHISPER_MODELS_DIR`,
  `~/models`, `~/.local/share/rusty-board/models`, ... and already has a fingerprint corpus
  (`audio_kit::fingerprint`, remembers committed utterances) that skips whisper for phrases said before.
* **Slide board** (`slide_board.rs`): a thumb rail per hand, slide for the column, vertical offset from where
  the finger came down for the row (home / up past 0.5 row = top / down past 0.33 row = bottom), types on
  lift. `UP_ROWS`, `DOWN_ROWS`, `RAIL_ROWS` are the numbers to tune first.
* The single-character TextEdit checker flagged 9 more fields in photosync, ambient-learner and the rusty-board
  companion (fixed); hatch, vidkit-player and tideshell were clean.
* Kernel process names are cut to 15 characters: `pkill -x xwaylandvideobridge` matches nothing.
* A fixed-target `sudo`/`systemctl reboot` over ssh now needs a terminal; the owner reboots.

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
  Replay: `gitmind fp5 list | plan [PART] | apply USER@HOST [PART]` (the CLI; gitmind-egui has no page for it yet).
  The cosmic-panel leak (protocol error on `cosmic_corner_radius_layer_v1@55`, ~7 MB/s, 5 GB seen, first seen at the 02:49 boot; it
  continued after autologin was reverted, so autologin is NOT the cause; not caused by the panel/dock config) is only bounded by the `panel memory watchdog` part.

## Git hygiene in this repo
The real index holds the owner's staged work. Commit through a temp index, `export GIT_INDEX_FILE=… &&
git read-tree HEAD && git add <mine> && git commit` **in one Bash call** (the variable does not survive to
the next call, and a bare commit sweeps the owner's staging); resync with `git reset -q HEAD -- <paths>`.

## Ornith-1.5-9B on the M2 Max  [measured]
`kernel-validate --bench` panicked on a hybrid model (raw `new_cache()`); fixed to `new_cache_for`. Result:
5.08 tok/s at context 544, 6.91 at 40; 190 of 197 ms/token is GPU wait, ~28 GB/s effective (~7% of 400 GB/s),
the same as the 3B. See `apps/logscope/perf-loop/LEDGER.md`.

## Auto-rotate dead after a reboot: no accelerometer in IIO (2026-10-03)

Found on the phone, boot of 02:27: rotation did nothing and tideshell logged
`orientation: HasAccelerometer: ... ServiceUnknown: The name is not activatable`.

* `/sys/bus/iio/devices` held only the two PMIC ADCs, so there was no accelerometer for
  `iio-sensor-proxy` to find. udev starts the proxy when an accelerometer appears; here it ran 21 s
  (02:27:44 to 02:28:05) and exited cleanly, so nothing owned `net.hadess.SensorProxy`.
* The sensors come from the ADSP through `hexagonrpcd-adsp-sensorspd`. Its log is the same on the good
  boot (02:15) and the bad one: `Could not open /mnt/vendor/persist/sensors/registry/registry/../sns_reg_version`
  and `Tried to open ... temp.json for writing`; that directory does not exist on the phone. So those
  errors are not the difference. What differed was whether the ADSP sensor service came up far enough
  to expose the IMU.
* tideshell already asks again every 3 s (`OrientationFeed`), so it recovers by itself once the proxy
  exists. Nothing in the shell was at fault.
* Recovery needs root: `sudo systemctl restart hexagonrpcd-adsp-sensorspd.service`, then
  `ls /sys/bus/iio/devices` for a third device, then `systemctl status iio-sensor-proxy`.
* Not known: why the ADSP sensor path comes up on some boots and not others. A reboot is the blunt fix.

## The keyboard hid behind the palette sheet (2026-10-03)

Tideshell's palette sheet is a `Layer::Overlay` surface with a scrim; rusty-board defaulted to `Layer::Top`, so the keyboard
was summoned and mapped but drawn UNDER the scrim (a blank band where it should be; `rusty-board.geom` read 216 all the while).
Fix, user-level and replayable: `~/.config/systemd/user/rusty-board.service.d/layer.conf` with
`Environment=RUSTY_BOARD_LAYER=overlay`, then `systemctl --user daemon-reload && restart rusty-board`. Verified by screenshot:
the board draws over the sheet and the search bar sits right above it. The cost: it also sits above other overlays (hatch, switcher), which is where
it belongs. Debug aid: `rusty-board.ctl` is a DGRAM socket (`show`/`hide`), phone `nc` has no `-U`, use python.

## cosmic-panel spun a core for hours: corner radius vs the autohide handle (2026-10-03)  [measured]
Found from `~/.local/share/tideshell/battery.jsonl` (the minute log): `cosmic-panel`, `cosmic-session` and
`systemd-journal` burned CPU together in bursts (01-02h and 11-12h, the reboots), up to ~2 cores
(`cpu_s` 129 per minute), and the persistent journal was 1 GB. On a fresh boot the journal showed
`Protocol error 1 on object cosmic_corner_radius_layer_v1@55`, 2,027,492 suppressed messages in 27 s.
Error 1 of that interface is `radius_too_large`: the panel asks for a corner radius bigger than half its own layer, and
`Panel` is `autohide: OnOverlap` with `handle_size: 4`, so hidden it is ~4 px thick and `border_radius: 12` (Dock: 160!)
cannot fit. The connection dies, the panel spins on it, and every restart of the panel aborts its eight applets
(`cosmic-applets` SIGABRT, "wayland dispatch: Backend error: Broken pipe"): 311 core dumps today, 2.7 GB in
`/var/lib/systemd/coredump` (1,205 files), each launching a 168 MB `drkonqi-coredump-launcher`.
Fix: `~/.config/cosmic/com.system76.CosmicPanel.{Panel,Dock}/v1/border_radius` = `0` (were 12 and 160). Verified: panel 0 CPU ticks
over 5 s with no protocol errors, load down, minute log `cpu_s` 129 -> 65 -> 4.5. NOT done (needs root, the owner's call): cap or
disable coredump storage (`/etc/systemd/coredump.conf.d`), vacuum the 1 GB journal and the cores. Also: the journal is persistent
here, but `battery.jsonl` is what showed the pattern first.

## Photo quality, measured from a day's roll (2026-10-03)  [measured]
39 rear-camera photos (720x1280, saved with `-front` in the name until the facing fix): the darkest 1% of pixels sits at
52/255 (median over photos), the median luma at 161, p95/p99 are 255, 20-60% of pixels have a channel at >=250, nearly
none are below 20, and the near-neutral surfaces lean green/cyan (G/R ~1.0-1.5). A 4-6% linear black lift reads as ~70/255 in
sRGB, which fits libcamera's soft ISP having no black-level for the imx858 here. Clipped highlights are in the frame
itself, so software can only lift the black, balance the neutrals and darken what is not burnt (`snapshot-cam/src/autotone.rs`);
the rest needs the exposure turned down at capture, which wants the camera run. `~/.config/snapshot-cam/settings.conf`
had `lens = no lens` (the wide sensor's distortion uncorrected) and `wb = auto`.
Sensors from the device tree (`/sys/firmware/devicetree/base/soc@0/cci@*/i2c-bus@*/camera@*`): camera@1a imx800 (orientation 1,
back), camera@29 imx858 (back, main, lens motor), camera@10 s5kjn1 (orientation 0 = FRONT); all rotation 270.

## The COSMIC panel is off (2026-10-03)
Tideshell draws the bar, drawer and switcher, so the stock COSMIC panel was a second, unwanted bar along the bottom (and its
applets cost CPU, and crashed whenever the panel restarted). `com.system76.CosmicPanel/v1/entries` set to `[]` removes the panel and the
dock live; cosmic-panel keeps running idle. To bring it back: restore `entries.prev`. (The `border_radius` fix above only matters
when the panel is on.)


## The 7.5 s freeze every minute was `nmcli dev wifi` (2026-10-03)

Found with the stall log (`journalctl --user -u tideshell | grep stall`): `slow work ~7.4-7.7 s`, exactly 60 s apart, with
nothing in front and the phone locked. Two clues decided it: the interval (a 60 s timer in the main loop) and a sampler
that showed the main thread asleep in `poll()` (how `Command::output()` waits) with the child
`nmcli -t -f ACTIVE,SSID,BSSID dev wifi` alive for the whole stall. `nmcli dev wifi` defaults to `--rescan auto`, which
asks for a new scan when results are over 30 s old and waits for it; awake and recently scanned it takes 0.02 s (which
is why timing it by hand found nothing), asleep and locked it took ~7.5 s. Fixed by reading cached results
(`list --rescan no`) on a worker thread (`sensors::WifiPoller`), which also stops a Wi-Fi scan every minute from keeping
the radio awake.

Things this taught: (1) time a suspect under the CONDITION it fails in (screen off), not the condition it is tested in;
(2) the phone's login shell is zsh and kills background jobs on logout even with `setsid` -- a long sampler needs
`systemd-run --user --unit=NAME --property=StandardOutput=file:PATH sh script`; (3) `date +%N` does not exist in the
busybox here: time with `/proc/uptime` differences over a loop.

## Camera focus: what the field log showed (2026-10-03)

`SNAPSHOT_CAM_AF_TRACE=1` (the phone wrapper sets it) writes `~/.cache/snapshot-cam-af.log`. First log, 15 taps: most
searches read 0.2-0.5 across the whole lens range (smooth skin in dim light is sensor noise) and the ones that locked did
so at contrast 1.3-1.6 on that noise, leaving the lens at an arbitrary spot (1023, 895, 446). `v4l2-ctl` takes ~7 ms per
move, so a move being queued but not applied is NOT the cause. Fixed in tapfocus (median of 3 frames per probe,
re-measure the landing, refuse if it does not reproduce). Not yet fixed: automatic (3x3) focus in `autofocus.rs` has the
same driver. Focus stack and HDR found no V4L2 focus/exposure control on the camera node and silently took no photo
(the lens is a separate sub-device): focus stack now drives the lens; HDR still needs the virtual-bracket path.
