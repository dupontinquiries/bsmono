# FP5 queue: what the owner has asked for, in the order it is being worked

Written 2026-10-01 from a debugging session in which the owner dictated ideas as a journal and said to
do them all "in a reasonable order". Status is what is TRUE of the code and the phone, not what was
intended. See [FP5_PHONE_LEARNINGS.md](FP5_PHONE_LEARNINGS.md) for what was measured.

Legend: **done** = committed and deployed; **built** = committed, not seen working on the device;
**blocked** = waiting for something only the owner can supply; **open** = not started.

## Order

| # | item | status | notes |
|---|------|--------|-------|
| 1 | Carousel peeks carry live info (CPU, memory, age) | built | `appinfo.rs` + `appinfo_io.rs`; strip layout unseen |
| 2 | Peek quick actions (copy selected text, screenshot the app to the clipboard) behind a long press | open (screenshot itself is built: drawer tile, `ctl screenshot`, power + volume-down) | needs a sheet; actions are `wl-paste -p`/`wl-copy` and a screenshot piped to `wl-copy --type image/png` |
| 3 | Low-light noise in the camera photo pipeline | open | code-only: `snapshot-cam` stacking; testable with synthetic noise |
| 4 | iPad-style pointer: a trackpad that works from the touchscreen AND from a real mouse/trackpad | open | tideshell trackpad mode exists (EI pointer); needs the morphing round cursor and mouse input |
| 5 | Brightness: remove COSMIC's OSD slider, show it in the sidebar / a dynamic-island pill | open | `cosmic-osd` is respawned by the session within a second; cannot be shadowed without root |
| 6 | Landscape: bar vs board zone, asymmetric carousel peeks | open (portrait carousel verified symmetric with the keyboard up; landscape unverified) | peeks created after the board are shortened by its zone; the slide board exists for landscape typing |
| 7 | Tideshell "chrome" (the glossy look of COSMIC's OSD and window frames) | blocked | which element? COSMIC's is stock styling, nothing was applied |
| 8 | Camera: periodic ambient samples -> palette for the lockscreen, bars and the board | blocked | the reader side (`tideshell.palette`) can be built; running the camera needs the owner's go-ahead (it froze the phone once) |
| 9 | Camera: tap to focus, probe the 3D ToF sensor | blocked | same go-ahead |
| 10 | Dictation: whisper tiny model in `~/models` on the phone | blocked | the `voice` build is deployed; no model exists on either machine |
| 11 | Idle profiling round | built | `apps/tideshell/deploy/idle-profile.py`; the governor plan needs root |
| 12 | cosmic-panel protocol-error leak | open | bounded by a watchdog; root cause is cosmic-panel/cosmic-comp (`cosmic_corner_radius_layer_v1`) |
| 13 | Hatch opening finicky | built | repaints every frame for 1.5 s after launch; cause inferred |
| 15 | Lock vs desktop state and display power-off bughunt | built | found and fixed: the wake press counted as a morse dot (stale loop clock), the re-blank timer firing late; by design and still open: with a morse unlock a short press on the lit lockscreen is a symbol, not sleep, so only a 1.5 s hold turns the display off there |
| 16 | Phone setup plans from the GUI | built | gitmind-egui Network page; not yet applied from the GUI |
| 17 | reMarkable Paper Pro: probe, COSMIC on it | probed | see `maintenance/REMARKABLE_TABLETS.md` (probe of 2026-10-02): remote session via VNC needs pointer injection; local needs a Wayland-to-qtfb bridge |
| 18 | clawbound: the bundle at 192.168.2.232:5002 (`WIP-clawbound`, 51 commits, 5 new cats, dunes map) and LAN multiplayer | open | branch is in the repo as `refs/remotes/bundle/WIP-clawbound`, not merged; the game already has `netcode`, `lockstep.rs`, `netplay.rs`, `lanlobby.rs` |
| 19 | vidkit-player: ghost on-screen keyboard | built | cause proven by A/B on the phone: `fonts::install` enabled IME on every window; removed. vidkit-player rebuilt and deployed; the other apps pick it up when rebuilt. Typing into a focused field not yet confirmed |
| 20 | vidkit-player: the top bar should overlay the video (the "chrome"), not block it | open | media controls as an overlay for now; later decide sidebar vs top slide-down vs press-and-hold vs a hatch tile |
| 21 | bo-suite-egui: media controls, designed properly (not vidkit's rugged style) | open | |
| 22 | bsettings (rusty-board-settings) integrated into tideshell | open | |
| 23 | hatch should look like Windows 8 (live tiles) | open | |
| 24 | Swipe-up switcher: move the all-apps button to the bottom | open | |
| 25 | studio: tap-and-drag on empty timeline space; rolling-tape portrait timeline; multiple scopes for the empty vertical space; command palette swipe on touch | open | pinch-zoom already works |
| 26 | Tablet as a smart second screen: Rust VNC viewer with touch first; then dashboards (monitoring, smarthome, photos, snapclone notifications), voiceterm notes sync + handwriting recognition, aperture/logscope layouts, AI chat updates | open | no Qt-Wayland bridge for now; the phone's pointer-injection gap blocks touch back through VNC |
| 27 | Multi-day battery life | built | `battery-log.py` runs on the phone (one line a minute; first reading 2.8 W screen on, governor performance, ZERO suspends since boot); design question: suspend vs tap-to-wake; governor plan needs root |
| 28 | Camera focus (tap-to-focus) and the 3D ToF probe | blocked | needs the go-ahead to run libcamera again (froze the phone once) |
| 29 | Fingerprint sensor for PAM | open | no sensor node in the device tree, not a quick fix; postmarketOS lists it broken; honest feasibility write-up still to do |
| 30 | Get past the cosmic-greeter no-keyboard barrier for dogfooding | open | autologin plan exists (`autologin_plan`) |
| 31 | True black on the OLED | built | pure black went 0% -> 55% of a screenshot on the phone; apps pick it up when rebuilt with the new uikit-egui |
| 32 | Drawer tile sub-labels are clipped ("Do not di", "Flashligh", "Screensho") | open | |
| 33 | Known pre-existing test failures | open | uikit-egui `callout` (slate/Error contrast), clawbound `sentinel` x3 and `touch` x2 |
| 34 | Hand tracking for a cold-glove keyboard / sign-language + letters input mode (`handtrack-rs`) | open | the monorepo already has a hand-kit corpus; camera work needs the owner's go-ahead |
| 35 | Volume voicing pad has no audible effect | diagnosed | the chain is a PipeWire filter-chain; PipeWire cannot start here (`libpipewire-module-filter-chain` missing), PulseAudio is what runs. Decision: move to PipeWire or voice in Pulse |
| 36 | Switcher closing: unblur over time | built | alpha-modifier fade implemented; blur protocol has no strength control; not verified on device |
| 37 | Pull the lockscreen effect into the wallpaper | open | |
| 38 | Sidebar scale issue in carousel landscape | open | recovers after the delayed poll; cause not found |
| 39 | Crestfall (clawbound god) is a copy of Oremantle | blocked | owner supplies the ability outline |
| 40 | vidkit-player: touch-drag scrub dead on the phone | built | the GPU path returned before `view.show`; gestures now shared (`View::handle_gestures`). Unconfirmed on device |
| 41 | `VIDKIT_URI=` starts vidkit playing without the clipboard | built | |
| 42 | vidkit-player top bar overlays the video | built | was a `TopBottomPanel`; now a floating Area that follows the chrome. Unconfirmed on device |
| 43 | Clipboard history (as on Plasma): a hatch-like app or hatch tile with a clipboard icon; pick an entry; scan for YouTube URLs and dufs video requests and offer them to vidkit | open | needs a history store (a resident `wl-paste --watch` writer) |
| 44 | hatch search field takes one character at a time on the phone | open, NOT reproduced | the field has a pinned `id_salt`; a headless test (typing across frames, with the notice strip and an error line appearing above it) keeps the whole query; a parent-id-shift fix was tried, did not change the test either way, and was backed out. Next: ask what exactly happens (clears? loses focus? keyboard hides?), and log egui's Ime/Text events on the phone |
| 45 | hatch: per-app memory / battery profile, logged and shown on the tile | open | needs a resident sampler (private mem via RssAnon+RssShmem, as in tideshell `appinfo_io`, plus battery attribution from `battery-log`'s per-process CPU) writing a log hatch reads; tile look and demo notice strip are done |
| 46 | Ribbed-key haptics: emulate a BlackBerry keyboard so a thumb can find the middle key by feel, walking | open | needs a vibrator device: on the phone only `evtest` exists, no `fftest`, and no vibrator LED node was found under /sys/class/leds; check `ff` capability of input event4 and the `aw8` haptic driver before designing. Slow-moving held touch = "feeling the ribs": pulse when the contact crosses a key boundary, a stronger one on the centre key |
| 47 | hatch should overlay carousel mode's sidebars instead of conceding space | experiment | `HATCH_FULLSCREEN=1` / `--fullscreen` opens it fullscreen; seen on the phone it covers the bar too. Needs a decision: overlay everything, or leave the bar |
| 48 | vidkit: some YouTube videos (a Drake one) will not play; direct-URL (big buck bunny) scrubbing does not work | open | God's Plan (xWggTb45brM) resolves and plays on the phone, so the failing URL is needed; the direct-URL path (`open_direct_url`) only probes the file, and errors surface only in the banner, not a log |
| 49 | vidkit: after switching apps the video fast-forwards through a backlog | built | `playback::is_stale` cuts to the present (>1.5 s behind, once per 5 s); not seen on the device |
| 50 | cosmic-osd stopped (SIGSTOP) so its brightness slider cannot cover the screen | built | volume/layout pop-ups from COSMIC go quiet too; `TIDESHELL_QUIET_OSD=0` undoes it |
| 51 | Greeter without a keyboard still blocks dogfooding | blocked on the owner | the `autologin` plan is opt-in and needs root: `gitmind fp5 apply user@phone autologin --password-stdin` (sudo over ssh needs a tty/password) |
| 14 | Older: logscope service test, switcher drawer redesign, home widget, steno multitouch, PAM shim, emergency access | open | see the C20 queues |

## Done tonight (all deployed unless noted)

Auto-rotate hold 900 ms and recovery from a boot race; Dvorak-correct typing and chords; keyboard height
after steno; no 45 s focus-grace popup on COSMIC; hold Wi-Fi/Bluetooth/mobile-data to open settings;
xwaylandvideobridge hidden; tap-to-wake storm removed; carousel reflow settles instead of chasing;
hatch repaint; single-character TextEdit ids (9 fields); `gitmind fp5 list|plan|apply`; the slide board.
