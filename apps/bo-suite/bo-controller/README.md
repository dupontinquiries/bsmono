# bo-controller

A Bang & Olufsen volume + streaming controller for ASE/Mozart devices, built with Rust + Qt/QML (via the `qmetaobject` crate).

## Project layout

```
bang-olufsen-volume/
├── Cargo.toml              # YOUR original manifest + one added dep (include_dir)
├── build.rs                # standard minimal qmetaobject 0.2 build script
├── .gitignore
├── README.md
├── src/
│   └── main.rs             # your patched code (products.json + qml/ fallbacks)
├── assets/
│   └── products.json       # productType → {emoji, icon} starter map
├── qml/
│   ├── qmldir              # registers the Bo design-system singleton
│   ├── Bo.qml              # design system: motion curves, materials, type
│   ├── volume_knobs.qml    # main shell — swipeable adaptive pages
│   ├── VolumeDial.qml      # inertial circular volume control
│   ├── PageDots.qml        # paged indicator (active dash elongates)
│   ├── ProductSilhouette.qml # product badge with stroke-reveal
│   ├── StepKey.qml         # −/+/mute keys with ripple + spring
│   ├── ControlCard.qml     # card container with entrance + breathing
│   └── InfoRow.qml         # key/value row for device info
└── examples/
    └── api_explorer.rs     # endpoint probe tool (cargo run --example api_explorer)
```

## Features (and per-product adaptation)

Beyond volume, the app now drives the device through its ASE/BeoZone API:

- **Mute** — the Stage's `BeoSound:/muted` path isn't whitelisted (it 500s),
  so mute is implemented as *volume → 0 with restore*: it remembers your
  level, drops to 0, and restores on un-mute. A manual volume nudge clears
  mute automatically.
- **Sources** — lists the device's inputs and switches between them. Each
  source gets a glyph by type (line-in, Bluetooth, radio, TV/HDMI, AirPlay,
  network). On its own swipe page.
- **Now Playing** — title/artist/source with play-pause and skip transport,
  polled live every 3s. Shows a mini animated equalizer while playing.
- **Standby / power** — read and toggle the device's standby state.
- **Stream** — push a URL to the device (Mozart PlayQueue).
- **Auto-route KDE media** — the original PipeWire-detection passthrough.

**Per-product adaptation** is driven by each entry's `controls` array in
`assets/products.json`. The app shows only the pages and controls a product
supports:

- **Speakers / portables / discs** (Stage, Balance, Level, A5, A9, Emerge):
  volume, mute, now-playing, stream, sources, power.
- **TVs** (Beovision Eclipse/Contour): volume, mute, now-playing, sources,
  power — no streaming.
- **Headphones** (Beoplay H100/HX): volume + mute only — no sources, power,
  streaming, or now-playing page.

Pages the product doesn't support are dropped from the swipe set entirely,
and the page-dot count adjusts to match.

## Color system: the warm bridge

Earlier builds were a stark black-and-gold binary. There's now a **warm
bronze/taupe mid-tone family** in `Bo.qml` (`bronze`, `bronzeDim`,
`bronzeWarm`) that bridges the near-black backgrounds and the gold accent,
used for surfaces, the dial track, idle watermark strokes, and the backdrop
gradient. The progression reads ink → warm surface → bronze → accent → bone
instead of jumping straight from black to gold. All three modes (day/dim/
night) carry their own bronze values.

## Device selector

The top area is now a **device selector** (`DeviceSelector.qml`). It shows
the current device with its silhouette and name; when more than one device
is known, tapping it unfolds a soft dropdown (settle-curve animation,
spring-press rows, accent on the active device) to switch between them.
Selecting a device swaps `active_url` and re-fetches all of its state.

The device list comes from `QtState.devices_json`, seeded in `main()` and
extendable via `add_device(name, url)` from QML. It is also populated
automatically by **mDNS/DNS-SD discovery**: a background thread (using
`mdns-sd`, which runs its own daemon and plays nicely with the tokio
worker) browses `_beoremote._tcp`, `_bangolufsen._tcp`, `_beozone._tcp`,
and `_http._tcp` (name-filtered to B&O). Resolved devices are merged into
the list — de-duped on URL and tagged `discovered`, shown with a "• found"
marker. The dropdown shows a quiet pulsing "Scanning network…" row.
`select_device(url)` performs the switch.

### Volume-reactive watermark

The dial watermark's flowing strokes now track the **actual volume level**:
a continuous floor derived from `qtobj.volume` (louder = faster, brighter
strokes) plus brief pulses to full on volume changes and playback start.
Quiet is a slow ~15s loop; loud eases up to ~5s. So the watermark visibly
breathes faster as you turn the device up, and settles when you turn it
down — never frantic.

## Mozart features

Beyond the ASE volume path (confirmed working on the Stage), the Mozart
`/BeoZone/Zone/...` surface drives sources, now-playing + transport,
standby, streaming, and now **sound modes / presets**: the Connect page
shows selectable mode chips (`/BeoZone/Zone/Sound/Mode`, activated via
`/BeoZone/Zone/Sound/Mode/Active`). Modes appear only when the device
reports them and the product profile includes `soundmode`.

## Media library (high-speed loading)

A SQLite-backed catalog of your local audio files, integrated as a
**Library** swipe page. The design priority is instant loading of
thousands of files:

- A dedicated **worker thread owns the SQLite connection** (`src/media/`),
  so queries never block the UI. On launch it immediately publishes
  whatever is already cataloged — the list appears instantly, before any
  scan runs.
- The catalog uses a normalized, indexed schema (tracks, artists, tags,
  playlists) with fast queries (recent, by-artist, by-BPM-range,
  by-camelot, by-tag, with-counts). Subsequent launches are sub-200ms.
- A **3-phase scanner**: walk the music dir → parse filenames → (optional)
  audio analysis. Walk+parse of ~10k files takes seconds; the slow BPM/key
  analysis is a separate, resumable, cancellable phase.
- The QML **`ListView` is virtualized** — it recycles row delegates, so a
  library of thousands scrolls smoothly. Rows show title/artist/album with
  a BPM/key chip when analyzed.

### Configuration

- **Music roots:** `$BO_MUSIC_DIR` (colon-separated paths) or `~/Music`.
- **Database:** `$XDG_DATA_HOME/bo-controller/media.sqlite` (or
  `~/.local/share/bo-controller/media.sqlite`), created automatically.
- **Filename parser** expects `{artists} - {song} ({tags}) - {album}` but
  degrades gracefully on other formats.

```bash
BO_MUSIC_DIR=/home/you/Music cargo run --release
# then tap "Scan" on the Library page (or it shows existing catalog instantly)
```

### Audio analysis (optional)

BPM/key detection uses `bliss-audio`, which pulls ffmpeg/symphonia and is
slow to compile — so it's **behind a cargo feature, off by default**:

```bash
cargo build --release --features analysis
```

Without it, the library does everything except compute BPM/key (those
columns stay null and the chips hide). The `analyze`/`AnalyzeBacklog` path
and `key_detection` have no-op stubs when the feature is off, so the core
build stays fast and dependency-light.

### Dependencies added

`rusqlite` (bundled — no system SQLite needed), `walkdir`, `regex`,
`once_cell`, and optional `bliss-audio`.

## Spaces: soundscapes & multi-speaker (daemon-backed)

A **Spaces** swipe page drives ambient multi-stem soundscapes across
multiple speakers. The heavy lifting lives in a **separate headless daemon**
(`bo-muxd`, the `bo-mux` crate) so playback continues with the app closed
and can be systemd-managed; the controller GUI is a **thin client**.

- `src/mux_bridge.rs` is a `MuxBridge` QObject exposed to QML as `mux`. It
  speaks the daemon's NDJSON control protocol over
  `$XDG_RUNTIME_DIR/bo-mux/control.sock` — status, scene list, peers,
  play/stop, session start/leave.
- `qml/SpacesPage.qml` shows the scene picker, transport, and room/peers
  when the daemon is up; an **Install** card when it isn't; and an **Update**
  card when the installed version is older than what the GUI ships.
- The GUI can **install/auto-update the daemon itself**: `MuxBridge::install`
  copies the bundled `bo-muxd` + systemd unit files into place, writes a
  version stamp, runs `systemctl --user daemon-reload`, and enables the
  socket. Bundle the payload with `scripts/stage-mux-payload.sh` so the
  button finds it.

The page only appears for products whose profile includes `spaces`
(speakers/portables/discs — not headphones). See the `bo-mux` crate for the
engine, protocol, and the honest sync-reality discussion.

## Roadmap (multi-turn integration)
- **Turn 1 (done):** Real product SVG visuals + the day/dim/night adaptive
  theme ("smooth nighttime vibes").
- **Turn 2 (done):** Media library with high-speed loading of thousands of
  files (SQLite catalog + 3-phase scanner + virtualized list).
- **Turn 3:** Key/BPM analysis, now-playing HUD, manual track overrides,
  artist/tag search.
- **Turn 4:** Live notifications (long-poll push instead of polling),
  firmware diff page, per-band equalizer.

## Nighttime vibes (day / dim / night)

The whole interface shifts through three moods, driven by the `Bo` design
singleton:

- **day** — full contrast, cool neutral.
- **dim** — evening: lower luminance, slightly warmer text.
- **night** — deep and warm: backgrounds drop near-black, text warms to a
  soft bone, the product accent pulls toward amber and dims so it *glows*
  rather than glares in a dark room, and every animation eases ~35% longer
  for a calmer, slower late-night cadence.

Tap the ☀/◗/☾ button in the header to cycle **auto → day → dim → night**.
In **auto**, the mode follows the clock (night 10pm–6am, dim mornings/
evenings, day otherwise) and re-checks every few minutes so it drifts into
night on its own. The `dark_mode` plumbing that was unused is now the basis
for this.

## Product visuals

The header badge shows a per-product silhouette. Drop real artwork at
`assets/products/<productType>.svg` (e.g. `assets/products/BeosoundStage.svg`)
and it loads automatically, fading in with the reveal animation. With no
SVG present, a hand-drawn silhouette (soundbar, disc, screen, headphones,
column, slab, spine) is used — so it always looks finished. See
`assets/products/README.md`.

### Dial watermark

Behind the volume dial on the Sound page, the product silhouette also
appears as a large **watermark peeking from the top-left**: clipped along a
45° diagonal through its midpoint (only the upper-left triangle shows),
~30–50% opacity, and a 4–16px soft blur. Its outline is drawn as a flowing
dashed stroke — a slow, calm linear loop at rest that speeds up and
brightens when you change volume or audio is playing (the `activity` value
pulses on volume changes and holds a low floor while a track plays).

The blur uses `QtGraphicalEffects`' GaussianBlur, loaded defensively at
runtime via `Qt.createQmlObject` — if the module isn't available on your Qt
build, the watermark still renders (just un-blurred) rather than failing.
If you'd rather force a specific path, the component is `qml/DialWatermark.qml`.

## The UI: Scandinavian motion design

`qml/volume_knobs.qml` is a complete, working controller built around
B&O's design language — restraint, precision, materiality, and motion
that has weight and intention. It's a **swipeable three-page experience**:

- **Sound** — an inertial circular volume dial (the hero)
- **Stream** — source URI + play/stop with a live equalizer status strip
- **Connect** — auto-route toggle + device identity

Swipe horizontally between pages; a refined indicator (the active dash
elongates and takes the accent) tracks position.

### Motion + materials, centralized

Everything draws from `Bo.qml`, a design-system singleton holding the
motion curves, durations, materials, and type scale. One source of
truth means the whole app shares one physical character. The signature
curves: `settle` (well-damped arrival with a hair of overshoot),
`spring` (confident push with anticipation), `smooth` (calm, attention-
free).

### Little touches

- **Inertial dial** — flick the volume ring and it keeps turning,
  decelerating under friction like a real weighted aluminium wheel,
  then settles. Hits the 0/100 rail and stops.
- **Detents** — a faint pulse + tick scale as the value crosses each
  10% mark, the visual analogue of B&O's tactile wheel clicks.
- **Metallic sheen** — a moving highlight travels the filled arc,
  suggesting brushed aluminium catching light.
- **Breathing idle** — at rest the dial handle has an almost
  imperceptible glow pulse, so the control feels alive.
- **Stroke-reveal silhouette** — the product badge inks itself in like
  a technical drawing on launch and on product change, with an accent
  halo bloom.
- **Ripple + spring keys** — the −/+/mute keys bloom a soft ripple from
  the touch point and squash-release with a spring curve.
- **Card choreography** — cards rise and fade into place with a slight
  stagger; active cards gain a barely-there breathing border glow.
- **Animated accent** — switch products and the entire accent color
  cross-fades over ~640ms, rippling through dial, keys, dots, and the
  atmospheric background gradient.

### Adapts per product

Driven by `assets/products.json`: each device carries an `accent`,
`accentSoft`, `shape` (silhouette), `tagline`, and a `controls`
whitelist. The accent themes everything; the silhouette matches the
form factor (soundbar, disc, screen, headphones, column, slab, spine);
and pages/controls the product doesn't support are dropped. Add a new
device by adding a key matching its `productType`.

Everything binds to the real `QtState` API. Restyle freely — the Rust
side only depends on property/method names.

## Cargo.toml — what changed

Exactly one line added vs. your original:

```toml
include_dir = "0.7"
```

Everything else (qmetaobject, reqwest, serde_json, tokio, cstr,
urlencoding, serde, dbus, the [[example]] block, the qmetaobject
build-dependency) is byte-for-byte your original.

## Build & run

```bash
cargo build --release
./target/release/bo-controller
```

Set the device IP in `src/main.rs`:
```rust
state.active_url = "http://192.168.2.12".to_string();
```

## Probe your device's API

```bash
cargo run --example api_explorer -- http://192.168.2.12
```

Reports which ASE/BeoZone/Mozart endpoints your specific product +
firmware actually expose. Useful before wiring new features.

## Runtime asset resolution (the fix)

Both `products.json` and the `qml/` directory are located at runtime by
searching, in order:

1. `$BO_PRODUCTS_JSON` / `$BO_QML_DIR` (explicit override)
2. `./assets/products.json` / `./qml/` (project root)
3. `<exe_dir>/assets/...` / `<exe_dir>/qml/` (binary next to assets)
4. `<exe_dir>/../share/bo-controller/...` (installed layout)
5. `~/.local/share/bo-controller/...` (user install)
6. `/usr/share/bo-controller/...` and `/usr/local/share/bo-controller/...`

If none is found, the binary uses copies **embedded at compile time**, so
it runs from any directory — even with no assets/ or qml/ present. (The
qml/ embedded copy extracts to a temp dir on launch, cleaned up on exit.)

This fixes the original `Failed to read product file at assets/products.json`
panic that happened when running the binary from outside the project root.

To override at runtime without rebuilding:
```bash
BO_PRODUCTS_JSON=/path/to/products.json BO_QML_DIR=/path/to/qml \
  ./target/release/bo-controller
```

## Build prerequisites (system)

- C++ compiler (g++/clang)
- Qt 5.6+ or Qt 6 with `qmake` on PATH
- QtQuick / QtQml runtime modules

On Debian/Ubuntu/KDE Plasma Mobile:
```bash
sudo apt install build-essential qtbase5-dev qtdeclarative5-dev \
     qml-module-qtquick-controls2 qml-module-qtquick-layouts \
     qml-module-qtquick-window2
```

## Recent additions

- **Real mute** — the MUTE key toggles the device's actual mute state via
  the ASE `BeoSound:/muted` endpoint (not just setting volume to 0). The
  dial dims and reads "MUTED" when active; state is fetched on launch and
  kept in sync. Exposed as `qtobj.muted` (property) + `qtobj.toggle_mute()`
  / `qtobj.refresh_mute()` (methods).
- **Automatic state sync** — a light 3-second poll keeps volume and mute
  in step when they're changed from the physical dial, the official B&O
  app, or a remote. Pauses while you're actively dragging the dial so it
  never fights your input.
