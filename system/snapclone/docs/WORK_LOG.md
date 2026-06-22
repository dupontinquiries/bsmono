# SnapClone — Work Log

Session-level summary of significant decisions and major fixes.
Format: newest first. For tactical TODO see [TODO.md](TODO.md);
for strategic direction see [ROADMAP.md](ROADMAP.md).

---

## V32.31 — Landmarking foundation (data layer + UI; no DHT yet) + video recording preview aspect fix

### Bonus: video recording preview aspect distortion — fixed

User reported "video recording preview is stretched strangely on all
devices but in different ways... videos do seem to be fine on
playback." Diagnosed:

The recording pipeline's separate preview branch
(`VideoRecorder.cpp`) used `videoscale ! video/x-raw,width=320`
with no height constraint. GStreamer's videoscale fixate-step picks
the missing height based on internal heuristics — and that pick was
inconsistent across GStreamer versions / source PARs / source
resolutions. Some sources got correct aspect; others got squashed.

The actual H.264 → AV1 recording branch was unaffected (separate
output sink, separate caps), which explains why playback at the
recipient was fine. Only the in-app shooting preview was wrong.

Fix: constrain BOTH width AND height to 320×320 (square) and add
`add-borders=true`. Result:
- Landscape sources letterbox with top/bottom black bars
- Portrait sources pillar-box with left/right bars
- Content aspect always preserved, never stretched

The bars are invisible against `m_viewfinderLabel`'s black
background after Qt's `KeepAspectRatio` scaling. Cost is harmless
— a few extra pixels of black per side.

Carefully NOT re-introducing the previous integer-overflow bug
(GStreamer fixate with `width + PAR` constraint) — that bug
needed PAR constrained explicitly, which this fix doesn't do.
Fallback documented inline if some future device hits a new
overflow path.

---

### Landmark foundation

Builds the local-only foundation for user-tagged places. The
"BSSID-to-location" workaround for Mozilla Location Service's
shutdown — but on a per-user basis. User tags "I'm here, call this
place X"; future resolution can match against those tags when GPS
is unavailable.

**Staged rollout.** V32.31 ships only the local pieces. DHT
publish path is deliberately NOT wired — that needs a privacy
review (default scope, signature scheme, abuse mitigation, etc.)
before we point any user-tagged location at a public broadcast
channel. The schema includes signature + scope + geohash columns
now so Phase 2 publish needs no migration.

### Data layer

**`Core::LocationSource` enum (Types.hpp).** Tagged taxonomy of
where a location fix came from: Gps / Manual / Snap / Geocoded /
Network / WiFi / Landmark / Derived / Unknown. String mapping for
storage compat (existing `PeerLocation::source` is a free-form
QString — values "gps", "snap" etc. map cleanly). Helpers
`locationSourceToString` / `locationSourceFromString`. Used by
future LocationResolver to weight signals and by UI to show
provenance.

**`Core::Landmark` struct (Types.hpp).** The data model:
```
id              sc-landmark-<uuid>
creatorId       sc-... (owner's stable identity)
displayName     "My apartment"
lat, lon        WGS-84
accuracyM       horizontal accuracy at capture
geohash7        coarse cell (~76m) for DHT lookup (Phase 2)
ssid            associated WiFi SSID (optional)
bssidHash       SHA-256(MAC)[0..8] (optional)
photoBlurHash   8-byte aHash of heavily-blurred 64x64 (optional)
photoEdgeHash   4-byte Sobel edge fingerprint (optional)
scope           Private / Contacts / Public
createdAtMs     epoch ms
signature       ed25519 over canonical serialization (Phase 2)
```
With `LandmarkScope` enum (Private = 0, Contacts = 1, Public = 2)
and `distanceMTo(lat, lon)` Haversine helper.

V32.31 always creates Private. Phase 2 adds the scope picker +
publishes Contacts/Public to peers and the DHT respectively.

**Photo handling is privacy-preserving by design.** The original
photo NEVER leaves the publisher's device. What gets stored (and
eventually shared) is two lossy transforms:

- `LandmarkHash::blurHash(QImage) -> 8 bytes` — 3-pass separable
  box blur (~σ=4.5 gaussian equivalent) → 8x8 downscale → aHash.
  Heavy blur destroys text, faces, license plates while preserving
  building silhouettes / skyline contour / dominant color blocks.
  Matches "same place, roughly same angle."

- `LandmarkHash::edgeHash(QImage) -> 4 bytes` — light gaussian blur
  pre-pass (single 5-tap separable box, ~σ=1.4) → Sobel edge
  detection → threshold against global mean → 8x4 cell grid →
  one bit per cell. The pre-blur is critical for privacy: destroys
  high-frequency content (text glyphs, faces, pixel noise) BEFORE
  any edge extraction can recover it. What remains is macro-scale
  structure (rooflines, horizon, silhouettes) — the very thing
  that makes a landmark distinctive. Discards color and brightness
  entirely; robust to lighting/season changes that defeat blur
  hashing.

  (Blur-before-Sobel ordering was user-flagged as the privacy-correct
  default. Initial implementation ran Sobel on the raw downscale,
  which would preserve text/face edges into the fingerprint. Fixed
  in the same turn.)

Both are LOSSY one-way transforms. From an 8-byte blur hash or
4-byte edge hash, you cannot reconstruct the original image.
This is the privacy guarantee that lets the hashes eventually go
on a public DHT without leaking what the user actually photographed.

Hamming distance helper for matching:
- distance ≤ 10 bits / 64 = confident blur match
- distance ≤ 6 bits / 32 = confident edge match
- Use the better signal of the two when both available.

### Storage

**Migration 14: `landmarks` table.** Schema mirrors the struct.
Composite index on `geohash7` for "find landmarks near me"
Phase 2 lookups; index on `creator_id` for "my landmarks" UI.
No FK from `creator_id` → `users.id` deliberately — Phase 2
will ingest DHT-discovered landmarks from strangers whose
creators aren't in our local users table.

Multi-statement SQL in migration uses the existing semicolon-
split path (CREATE TABLE + 2 CREATE INDEX in one migration entry).

**StorageManager API additions:** `saveLandmark(Landmark) -> bool`,
`loadLandmark(id) -> optional<Landmark>`, `loadAllLandmarks()`,
`loadLandmarksNear(lat, lon, radiusM)`, `deleteLandmark(id)`.

`loadLandmarksNear` uses a SQL bounding-box pre-filter (cheap)
then precise Haversine in code (accurate). Results sorted by
distance ascending. The bounding-box uses cos(lat) scaling for
longitude to handle parallels shrinking near the poles. At our
data scale (<1k landmarks per user) the pre-filter saves more
than it costs.

### UI

**`AddLandmarkDialog`** (MobileDialog subclass). Inputs:
- Name (required, 64-char max)
- Current location, read from `AppController::lastKnownLocation`
  by the caller. If NaN, shows a "set location first" hint
  pointing at Settings → Privacy and disables Save.

No photo capture in V1 (deliberate — keep the dialog lean while
proving the data layer). Future revision adds a "use photo
from inbox" / "take photo now" tab pair.

Emits `saved(Landmark)` with all fields populated except `ssid`
/ `bssidHash` / photo hashes / signature (all empty in V1) and
`geohash7` (empty in V1; Phase 2 helper will populate before
publish). `scope = Private`, `createdAtMs = now`.

**"Add landmark" button in ContactsPage header.** Per design
discussion — primary entry point for landmark creation. Adjacent
to the QR scan button, uses a new `landmark.svg` icon (Feather-
style map-pin teardrop + inner circle). Click handler:

1. Read `AppController::lastKnownLocation(selfId)`
2. Construct `AddLandmarkDialog` with that lat/lon (or NaN)
3. On `saved` signal → `storageManager()->saveLandmark(lm)`

ContactsPage gained a `setController(AppController*)` setter
mirroring V32.29's SettingsPage pattern. MainWindow calls it
right after constructing the friends page. Non-owning.

### What's deliberately NOT in V32.31

- **DHT publish/discover.** Pending privacy review covering
  default scope behaviour, signature scheme, geohash precision
  tuning, abuse mitigation (rate limits, reputation lists, web-
  of-trust filter at display).

- **Landmark resolution.** No "I have no GPS, match my current
  WiFi BSSID against stored landmarks." That's Phase 2 — needs
  the BSSID scanning code (NetworkManager DBus API on Linux,
  permission gates on mobile) AND a LocationResolver that can
  weigh multiple weak signals.

- **Landmark list UI.** No "My landmarks" view yet — users can
  create them, but seeing/editing/deleting an existing landmark
  list is a Phase 2 deliverable. For V1, data lives in storage
  and resolves automatically; the lack of a list UI is intentional
  scope-cutting.

- **Photo capture in the AddLandmark dialog.** V1 saves the
  landmark with empty photo hashes. The image-hash helpers exist
  and are wired into the build; they just don't have a caller
  yet. Adding a photo-pick UI is the smallest extension after
  the data layer settles.

- **Tap-and-hold on snap photo → landmark card.** User's idea
  from the design conversation; slot for Phase 2 after the
  resolution engine exists (the card shows "you might be at:
  Café Vista [3 confirmations]" which requires a resolver to
  populate).

### Wire-format plan (for next privacy-review session)

When publishing a landmark on the DHT (Phase 2), the wire
record should stay minimal. Strawman ~134 bytes:
```
  ed25519_pubkey (32)
  lat            (8 LE double)
  lon            (8 LE double)
  acc_m          (2 LE uint, capped at 65535)
  pHash          (8)
  edgeHash       (4)
  ssidHash       (8 SHA-256 truncate)
  sig            (64)
```
Display name + photo bytes stay on the publisher; recipients
who want to see them request them peer-to-peer after discovering
the lightweight DHT record. This keeps DHT churn low and
keeps user-identifying content off the public broadcast.

Per the user's design conversation: re-publish-on-resolve
economy. When a recipient resolves their location via a
landmark, they cache it locally AND re-publish it for their
own ~10min DHT TTL window — landmarks "echo" through the
network while in active use; self-extinguish when nobody's
resolving them. Standard DHT pattern.

---


## V32.30 — Ambient theming polish + UX fixes (QR / long-press / scaling / crossfade)

This version split into three waves:
**Wave 1 (earlier in V32.30):** ambient theming — readability-safe
BG tinting, AmbientBlobs pulsing overlay, Content Creator Mode.
**Wave 2:** four user-reported polish bugs/requests (QR flicker,
long-press FSM, viewport-relative margins, swipe crossfade).
**Wave 3 (this entry):** ambient refinements — auto-refresh inbox,
EMA smoothing for camera mode.

### Wave 3.1: AmbientInboxDriver — auto-refresh on snapReceived

V32.29 shipped Mode A (Ambient: Inbox) as a one-shot derivation
on Settings carousel pick. Spec called for "re-derive on inbox-
update with 30s minimum between recomputes." Wave 3 ships that.

New class `AmbientInboxDriver` (Q_OBJECT). Subscribes to
`AppController::snapReceived`, runs an immediate derivation on
start (uses current inbox as seed), and re-derives on every
subsequent snap arrival — throttled to one derivation per 30s.

Throttle protects against:
- **Initial inbox load** (V32.20 path emits snapReceived for each
  restored snap)
- **Burst delivery** (peer reconnect flushing queued messages,
  group photo arrives as multiple snaps)

Idempotent start() with controller re-targeting. Safety guard:
checks `Theme::isSyntheticActive()` AND that the synthetic id is
`ambient_inbox` before applying — if the user has switched to
ambient_camera or a static palette since start(), the tick
silently skips rather than stomping their pick.

MainWindow owns the driver (lazy-created on first signal).
SettingsPage now emits `ambientInboxRequested(bool)` parallel to
`ambientCameraRequested`. The inbox-pick handler is no longer
in-handler-derivation; it just emits the signal. Switching to
static or camera mode emits both `ambientInboxRequested(false)`
AND `ambientCameraRequested(false)` for the mutual-exclusion
guarantee.

### Wave 3.2: EMA smoothing for camera mode

Walking around with the camera produced visible palette jitter as
the dominant hue swung frame-to-frame. Spec: 5-10s exponential
moving average.

Approach: split the deriver's pipeline into two halves at the
`(accent, background)` QColor stage. New API:
- `AmbientPaletteDeriver::deriveColors(QImage)` — returns
  `{QColor accent, QColor background}` without building a Palette
- `AmbientPaletteDeriver::fromColors(accent, bg, base)` —
  assembles a Palette from those colors

`AmbientCameraDriver` now holds `m_smoothedAccent` /
`m_smoothedBackground` member state. Each tick:

1. `deriveColors` produces fresh raw colors from the latest frame
2. EMA blend: `smoothed = α * fresh + (1-α) * smoothed`, with
   α=0.15 (≈4.3s half-life)
3. `fromColors(smoothed...)` builds the Palette
4. Apply via `setSyntheticPalette` + stylesheet refresh

Critical detail: blending happens in **HSL space, not RGB**. A
swing from green to magenta in RGB passes through grey at the
midpoint; in HSL we lerp hue along the shorter arc of the color
wheel (with explicit half-turn wrap-around handling). This gives
a perceptually smooth transition that tracks the camera view
rather than washing out.

First-tick seeding: `m_smoothedValid` starts false. The first
tick after start() bypasses the blend and seeds smoothed state
directly from raw values, so activation is immediate, not lagged.
Subsequent ticks blend normally.

`stop()` resets smoothing state so a fresh activation seeds clean
rather than blending against stale colors from a prior session.

### Wave 3.3: Pattern observation

The driver model is now consistent across both ambient modes:
- `AmbientInboxDriver` is event-driven, 30s-throttled
- `AmbientCameraDriver` is timer-driven, 1Hz, EMA-smoothed

Both follow the same lifecycle (lazy-create-on-signal, start/stop
via SettingsPage emission, mutual exclusion via paired stop signals).
A future `AmbientLocationDriver` ("chrome follows where you are
geographically") would slot in here with the same shape.

### Bug 1: QR code flickering and disappearing — fixed

`ExpandedQrOverlay::start` had **two nested `QGraphicsOpacityEffect`s**
running simultaneously: one on the dialog (`fx`) fading 0→1 over 220ms,
and one on the QR `QLabel` (`qrFx`) ALSO fading 0→1 over 280ms with an
80ms delay for a staggered feel. Qt's effect composition for nested
opacity effects produces brief 0-opacity frames where the child appears
to vanish — both effects compete for the same paint pipeline and
neither knows about the other.

The user's report ("QR flickering and disappearing") matched this
exactly. Fix: removed the inner per-label fade. The single outer
dialog fade carries the QR content up cleanly. Comment in the code
documents the discovery so future "let's add a nested fade" tempters
know to avoid it.

### Bug 2: Tap-and-hold to edit never opening — PressGesture FSM

The V32.24 implementation used `QPushButton::pressed` + `QTimer` +
`QPushButton::released` signals. Worked on desktop mouse — failed
reliably on touch. Diagnosis: when the chain has upstream
`WA_AcceptTouchEvents` widgets (notably `SwipeNavigator`),
`pressed()` doesn't always fire on touch — Qt's touch-to-mouse
synthesis can be defeated. The 500ms timer would never start.
A scroll-area drag-threshold can also swallow the press while it
decides "is this a scroll or a tap."

Built `PressGesture` — a small QObject FSM watching BOTH touch
(TouchBegin / TouchUpdate / TouchEnd / TouchCancel) AND mouse
(Press / Move / Release) events. State diagram:

```
  Idle ─[press]─▶ Pressing ─[500ms]─▶ Held(emit longPress)
   ▲                │
   │                ├─[release before 500ms]─▶ emit tap, → Idle
   │                │
   │                └─[move > 16px]──────────▶ emit dragCanceled, → Idle
   │
   └─[release|cancel from Held]──▶ Idle
```

Move-cancel threshold (16px) distinguishes a press from a drag-scroll
attempt. Guards in MouseButtonPress / TouchEnd prevent double-firing
from the touch+mouse-synth same-finger sequence. Public signals:
`tap()`, `longPress()`, `dragCanceled()`.

ContactsPage edit button now uses it. The 500ms threshold matches
Android's long-press standard. Tooltip text on the button updated
to "Long-press to edit contact" so the affordance is discoverable.

The FSM is reusable — anywhere else in the app that wants distinct
tap vs long-press behaviour can attach one. Likely candidates:
contact card itself (long-press for context menu), inbox row
(long-press for delete confirm), wayfinder peer pin (long-press
for peer detail).

### Issue 3: 300% OS scaling too big — viewport-relative margins

At 300% scaling the hard-coded `Theme::SPACE_MD` (20px) values stay
20 physical pixels, which is a small fraction of a high-DPI canvas,
but the surrounding content scales 3× so the page padding looks
under-sized relative to everything else. Or — depending on which
way you read it — the absolute screen real estate consumed by
margins doesn't grow with screen size.

Added HTML-vh/vw analogues to `Theme.hpp`:

- `vhPx(pct)` — pct% of screen logical HEIGHT, in pixels
- `vwPx(pct)` — pct% of screen logical WIDTH
- `vminPx(pct)` — orientation-independent (min of vh/vw)

Two convenience accessors built on these:

- `pageSideMarginPx()` = `vminPx(3.0) + SPACE_SM` ≈ 3% of shorter
  screen side plus 12px floor
- `pageTopMarginPx()` = `vhPx(1.5) + SPACE_SM`

These match the CSS recipe `calc(50px + 5vmin)` — a screen-relative
term combined with a fixed baseline. The floor ensures even tiny
canvases get *some* inset; the percentage term grows it on roomier
surfaces.

Applied to root margins of Inbox, Contacts (top bar), Settings
(scroll container). Not applied wholesale — buttons, font sizes,
and individual card padding stay fixed because the OS DPI scaling
already handles those.

### Request 4: 90ms crossfade on page swipe

`SwipeNavigator::startCommitAnimation` now attaches a brief opacity
fade to the OUTGOING page during commit: 1.0 → 0.65 over
`min(90ms, slideDuration/2)`. Floor at 0.65 (not 0) so the page
stays visually grounded mid-flight — without that floor, fast swipes
look like the page vanished. Cleanup via `QTimer::singleShot` at
`durationMs + 20ms` reaffirms opacity 1.0 and detaches the effect.

A real crossfade (incoming opacity 0→1 at the same x-position as
outgoing) would need a bigger restructure — currently pages slide
horizontally, no overlap. This is a softening of the departure
rather than a true crossfade, but it gives the page change less
"mechanical" feel.

### What's still open from the V32.30 user-flagged list

- **App stability** — user reported it's stable; no action.
- **Wayfinder GPS** — user is still composing their plan; the
  multi-source LocationSource plan is in flight (next session).

---

## V32.30 — Ambient theming polish: readability-safe tinting + pulsing blobs + Content Creator Mode

(Wave 1 of V32.30. Wave 2 above covers UX bugs filed by user after
Wave 1 shipped — QR flicker, long-press FSM, viewport-relative
margins, swipe crossfade.)

V32.29 shipped Ambient: Inbox, but feedback was clear: the saturated
background tint killed text contrast on the big panes. V32.30 fixes
that AND addresses the underlying design goal — keep the chrome alive
WITHOUT sacrificing legibility. It also lands Mode B (Content Creator
Mode / Ambient: Camera).

### Fix: BG tinting no longer overwhelms text

V32.29's deriver averaged neutral pixels and adjusted the result's
lightness. Output: fully-saturated backgrounds that warred with the
text colors inherited from the base palette.

V32.30 rewrites the background derivation:
- **Pull base palette's BG_BASE into HSL** to use as the lightness
  anchor (so existing text contrast still works)
- **Borrow the derived background's HUE only**
- **Cap saturation at 0.18** — a quiet tint, not a wash
- All five BG slots (BG_DEEP/BG_BASE/BG_CARD/BG_RAISED/BG_HOVER)
  flow from this same lightness-anchored recipe with small deltas
  for the raised-card visual hierarchy

The numeric cap (0.18) was chosen so combined with the AmbientBlobs
overlay opacity cap (also 0.18, see below) the total ambient effect
stays within safe text-contrast bounds across the dark palettes
we ship as base.

### New: AmbientBlobs overlay

`include/ui/AmbientBlobs.hpp` + `src/ui/AmbientBlobs.cpp`. A widget
that paints 2-5 (default 3) large soft-edged radial-gradient
"blobs" in the active palette's accent colors. Each blob:
- Drifts via independent Lissajous-style x/y phase advance
  (45-70s drift periods, irrational ratios so paths don't repeat)
- Pulses radius ±15% (6-10s period) and opacity 0.06–0.18
  (8s-ish period, slightly offset from radius → "breath" not "throb")
- Cycles through the four palette accent colors (ACCENT, ACCENT_SOFT,
  ACCENT_DIM, GLOW_ONLINE)
- Reads the current palette EACH FRAME so palette changes propagate
  without re-seeding

Critical: `paintEvent` early-returns when `Theme::isSyntheticActive()`
is false. **Zero render cost when on a static palette.** The 30fps
timer also stops on hide.

Installed once, parented to MainWindow's wrapper QWidget, sized via
eventFilter on wrapper-resize. `WA_TransparentForMouseEvents` so
clicks pass through to widgets below. Z-order is **raised**
(NOT lowered) because the global stylesheet gives every QWidget a
BG_BASE fill — putting blobs under everything would make them
invisible. With alpha ≤ 0.18, painting on top of content is still
legible and the blobs become an atmospheric layer rather than a
decorative backdrop.

### New: Ambient: Camera ("Content Creator Mode")

Live-viewfinder driven palette. Walk into a forest, the UI goes
green. Stand under sodium streetlights, it goes amber.

Pieces shipped:
- `AmbientPaletteDeriver::fromImage(QImage, base)` — new entry point.
  Same edge-band saturation-bucket pipeline as fromImagePaths, but
  takes an in-memory QImage. Refactored shared
  `prepareForSample(QImage)` and `sampleImage(QImage)` helpers out
  of the path-loading variant.
- `CameraWidget::previewFrameAvailable(QImage)` — public re-emit
  signal added. Fires from `onPreviewFrame` with the user-rotation-
  corrected frame at the backend's native ~30fps. Consumers throttle
  themselves.
- `AmbientCameraDriver` (include + src) — QObject that subscribes
  to `previewFrameAvailable`, stores the latest frame (cheap QImage
  shallow-copy, no allocation per frame), and re-derives the
  synthetic palette at 1Hz via QTimer. Iteration chains: each
  derivation builds on the previous synthetic palette, so the
  chrome continuously evolves with what the camera sees. Stop()
  disconnects + idles the timer.
- `"ambient_camera"` carousel entry in Settings → Theme. Click
  emits `SettingsPage::ambientCameraRequested(true)`. MainWindow
  owns the driver lifecycle.

Battery model: derivation runs only on the 1Hz tick, NOT on every
frame. Frames flow at ~30Hz but the driver discards 29 of every 30
(stores them as latest-frame; the timer pulls). Total cost: one
64×64 image scale + ~750 pixel reads per second when active.
Negligible compared to the camera backend itself.

Lifecycle is mutually exclusive across all carousel picks:
- Pick a static palette → stops ambient-camera + clears synthetic
- Pick ambient_inbox → stops ambient-camera + sets inbox-derived synthetic
- Pick ambient_camera → emits start signal; MainWindow lazy-creates
  the driver and calls start(m_cameraPage)

Camera page must be visible for frames to actually arrive (the
camera backend stops when the widget is hidden). So ambient_camera
on a non-camera page won't update until the user swipes to camera,
which is the right behavior — Content Creator Mode is meaningful
when you're framing a shot.

### Pattern observation

Five different ambient/synthetic consumers now plug into the same
infrastructure:
1. `AmbientPaletteDeriver::fromImagePaths` (Mode A: inbox)
2. `AmbientPaletteDeriver::fromImage` (Mode B: live frame)
3. `AmbientPaletteDeriver::fromAccentColor` (future: per-contact themes)
4. `AmbientCameraDriver` (orchestrator for #2)
5. `AmbientBlobs` (visual overlay reading the active palette)

All flow through `Theme::setSyntheticPalette` / `Theme::activePalette` /
`Theme::isSyntheticActive`. The Theme.hpp infrastructure built in
V32.29 holds up cleanly under three different consumers.

### Open follow-ups

- **Auto-refresh on inbox change** for Mode A. Currently one-shot
  on pick; spec calls for 30s throttled re-derive on inbox-update.
- **Temporal smoothing for Mode B.** Walking the camera around
  produces visible palette jitter as the dominant hue swings.
  An exponential moving average (5-10s window) of the last N
  derived accents would calm this down. V1 doesn't bother because
  the visual effect is already mostly chill due to the 0.18 sat
  cap — but power users may want it.
- **k-means clustering** for accent selection (currently saturation-
  bucket averaging — collapses multi-color frames to a muddy mid-tone).
- **Persistence option.** Synthetic doesn't survive app restart.
  Some users may want a "Stay in Ambient mode" toggle.
- **Apply AmbientBlobs to wayfinder canvas specifically.** The
  current installation is window-wide. Wayfinder might benefit
  from blobs scaled to the radar-canvas size and bias-placed
  toward populated quadrants.

---

## V32.29 — Wayfinder visibility + dynamic theme V1 (Ambient: Inbox)

Two unrelated efforts shipped in the same version.

### Part A: Wayfinder visibility fixes

Deep investigation of "wayfinder doesn't show users/snaps" turned up
THREE concrete bugs that collectively explain most of the empty-state
reports.

### Bug 1: AppController::lastKnownLocation bypassed LocationManager

The method took ANY peerId and went straight to storage with a 5-min
TTL. For peer queries that was fine, but for SELF queries it missed
two cases:

1. **Manual override**: `LocationManager::lastKnownLocation` has a
   QSettings-backed manual override path (used for desktop testing
   without GeoClue, or by users who prefer hand-set "approximate"
   location). That override only lives inside LocationManager — it
   never flows to storage. So manual lat/lon was read by
   `sendLocationSnap` (which uses `d->location->...` directly) but
   ignored by Wayfinder (which uses `AppController::lastKnownLocation`).
2. **Brief pre-storage-write window**: between a GPS fix arriving at
   LocationManager and the AppController slot writing it to storage,
   Wayfinder would see no fix.

Fix: for self-id queries, consult LocationManager FIRST. Manual
override always wins over stale stored GPS. Storage is the fallback
for cross-restart persistence.

### Bug 2: Self-fix TTL was 5 minutes — too tight for stationary users

A user with one GPS fix at startup, sitting still for 6 minutes,
would see Wayfinder switch to the "no location fix yet" empty state.
GPS hadn't refreshed (they didn't move; the platform may not push
new fixes when stationary), so storage went stale, and the 5-min
gate dropped it.

Fix: TTL bumped to 30 minutes. Wayfinder's design ALREADY humanizes
"2h ago" / "3d ago" for offline peer data — multi-minute staleness
of OUR position is well within the design tolerance.

### Bug 3: Startup purge removed 10-min-old locations on every cold start

`purgeStaleLocations(10 * 60 * 1000)` ran in `initialize()`. Opening
the app the next morning wiped every peer's last-known location —
Wayfinder would then show ONLY peers who shared locations live
during the current session. The whole point of persisting
`peer_locations` was cross-restart continuity; the 10-min purge
defeated it.

Fix: purge bumped to 7 days. Gives 1 week of "where was Alice last
week" without unbounded growth. Per-contact UI policy
(humanizeAgo "3d ago") already conveys age, so users aren't
misled by old data.

### Bonus: manual location override UI

The Wayfinder empty-state message said "Set a manual location in
Settings → Privacy" — but there was no such UI. The QSettings keys
(`location/manualLat`, `location/manualLon`) were read by
LocationManager but could only be set programmatically.

Fix: added a "Manual location override" row to
SettingsPage::buildPrivacySection with two QLineEdits (validated
to lat/lon ranges) and a Clear button. editingFinished commits to
QSettings. Both fields must be non-empty for the override to
activate. Empty fields → override removed → falls back to GPS.

### Bonus: Wayfinder refreshes when known-peer comes online

Previously `onPeerConnected` only updated existing in-memory nodes.
If a peer wasn't yet in `m_nodes` (e.g. their stored location was
loaded on a previous session but cleared by the 10-min purge — now
moot, but the race window still exists during a session where
peer_locations gets a fresh entry), they didn't appear until the
30s tick. Now: if `loadPeerLocation(id)` has a stored entry AND the
peer isn't in `m_nodes`, fire `refreshLayout()`. Avatars are cached
after first load so the cost is minimal.

### Combined effect

A real-device test that previously showed an empty Wayfinder should
now show:

- The self-center dot whenever LocationManager has any fix
  (live OR manual override) OR storage has a fix less than 30min old
- Peer nodes for every contact with ANY persisted location in the
  last 7 days
- Floating snap thumbnails for any peer who's sent us a photo snap
  in the inbox (existing behavior, was just being starved of nodes
  to attach to)

The "manual location override" Settings UI also makes the empty-state
hint message accurate for the first time — desktop testers can
actually set their location now and verify the rendering path.

### Part B: Dynamic theme V1 — Ambient: Inbox

The themes-from-images TODO has been sitting for a while; this lays
the infrastructure and ships the inbox-derived mode. Camera-derived
(Mode B) follows the same pipeline and is a future session.

#### Theme.hpp: synthetic palette plumbing

Existing infrastructure: `kPalettes[]` is a constexpr array of
`Palette` structs holding `const char *` color literals (bound to the
binary). `paletteIndex()` returns a mutable static int indexing into
that array; `-1` sentinel triggers re-read from QSettings.

New: `-2` sentinel means a synthetic palette is active. The synthetic
lives in a `SyntheticPaletteStorage` struct held by a function-static
inside `syntheticStorage()`. It holds `std::array<std::string, 24>`
for durable string storage plus a `Palette` whose `const char *`
fields point into those strings (rebuilt on each
`setSyntheticPalette()` call).

New entry points:
- `setSyntheticPalette(srcPalette, id, name)` — copies all 23 color
  literals from src into the storage's owned strings and flips
  `paletteIndex` to `-2`
- `clearSyntheticPalette()` — flips `paletteIndex` to `-1`, forcing
  re-read of the user's last static pick from QSettings on next
  `activePalette()` call
- `isSyntheticActive()` — for the carousel to draw correct selection

`activePalette()` checks the synthetic sentinel first; the rest of
the Theme accessors (`ACCENT()`, `BG_BASE()`, etc.) flow through it
unchanged. The synthetic is **deliberately NOT persisted** to
QSettings — picking any other carousel entry, or app restart, brings
back the user's last static pick. Spec-aligned: synthetic is a
session-level overlay, not a permanent state.

#### AmbientPaletteDeriver: edge-band saturation bucketing

New class in `ui/AmbientPaletteDeriver.{hpp,cpp}`. Stateless;
`fromImagePaths(paths, basePalette) -> Palette`. Pipeline:

1. For each path (up to 12), load via `QImage`, scale to 64×64
   `SmoothTransformation`, convert to ARGB32.
2. Iterate pixels in the edge bands only: rows in [0..3] ∪ [60..63],
   cols in [0..3] ∪ [60..63]. The center is deliberately ignored —
   anti-saliency heuristic, since central subjects shouldn't drive
   the ambient palette; the backdrop should.
3. For each edge pixel, compute HSL. If `sat ≥ 0.20` AND lightness in
   [0.10, 0.90], add to colorful bucket. Else add to neutral bucket.
4. Average each bucket separately into two QColors.
5. Colorful → accent; neutral → background tint.

Output strategy is intentionally minimal: only `BG_DEEP / BG_BASE /
BG_CARD / BG_RAISED / BG_HOVER / ACCENT / ACCENT_SOFT / ACCENT_DIM /
GRAD_*` and `GLOW_ONLINE` get overwritten. Text/danger/success/border
slots are inherited from the base palette. This preserves the
carefully-tuned contrast and semantic-state colors while letting the
ambient mood come through.

If a bucket is empty (e.g. all-grey inbox produces no colorful
samples), the corresponding slots stay from base — no muddy fallback
guess, no random accent invention.

#### Settings carousel: "Ambient: Inbox" entry

`kEntries[]` gains an `{"ambient_inbox", "Ambient: Inbox", #7FB1A8, #1A1A1F}`
chip. The chip's swatch is static; the actual palette is computed
on click.

Click handler is special-cased: instead of `Theme::setPalette(id)` +
QSettings write, it:
1. Collects photo-snap localPaths from the inbox via the new
   `SettingsPage::setController()` hookup (storage reachable now)
2. Filters out empty paths, missing files, and videos
3. Calls `AmbientPaletteDeriver::fromImagePaths(paths,
   Theme::activePalette())` — base is whatever's currently displayed
4. Calls `Theme::setSyntheticPalette(derived, "ambient_inbox",
   "Ambient: Inbox")`
5. Reapplies the global stylesheet via `qApp->setStyleSheet(...)`

Picking any other carousel entry calls `Theme::setPalette(staticId)`
which resets `paletteIndex` to -1 — naturally clears the synthetic.

The `saved` lookup at carousel-build time is now aware of synthetic
state: if `Theme::isSyntheticActive()`, treat the synthetic's `id`
as the saved pick so the correct chip is highlighted on Settings
re-entry within a session.

#### Lifetime carefulness

`buildAmbientPalette` (inside the deriver cpp) uses a thread_local
`QList<QByteArray>` scratch to hold the hex strings. The Palette
it returns points into those bytes. The bytes live until the next
call to `buildAmbientPalette` (which clears the list at top of
function).

`setSyntheticPalette` copies every string into its own owned storage
before returning. So as long as nobody calls `fromImagePaths` /
`fromAccentColor` a second time between the deriver returning and
`setSyntheticPalette` being called, the pointers stay valid. The
current carousel handler does exactly this back-to-back chain; safe
by construction. If a future caller wants to retain the returned
Palette for longer, they need to copy it via setSyntheticPalette
immediately.

#### What's NOT in this session

- **Mode B (camera viewfinder)** — same pipeline, fed by live frames.
  Has battery-drain concerns (sample only Nth frame, only when camera
  page is parked, not during swipe transitions). Separate session.
- **Auto-refresh on inbox change** — currently the derivation is
  one-shot on pick. The spec calls for 30s minimum between recomputes
  on inbox update; not yet wired. Manual re-pick works as a workaround
  meanwhile.
- **k-means clustering for accent** — used simpler saturation bucket
  averaging. Inboxes with multiple dominant saturated colors will
  produce a muddy mid-color accent. Acceptable for V1; k-means
  refinement (pick the brightest of three clusters) is a follow-up.
- **Persistence** — synthetic doesn't survive app restart. Some users
  may want it to; could be a future "Stay in Ambient mode" toggle.

---

## V32.28 — AppContext consumer wiring + Wayfinder first-location refresh

Continuing the V32.21 AppContext rollout. Three more consumers wired,
plus an unrelated Wayfinder UX fix.

### UdpChannel retransmit cadence is now context-adaptive

`UdpChannel::onRetransmitTimer` previously fired every 1500ms regardless
of app state. When pocketed on cellular with no user activity, that
1.5s cadence keeps spending battery + cellular bytes on retransmits
nobody is waiting for.

Fix: `setAppContext` setter (matching the V32.21/V32.27 network-
consumer pattern), a tick counter, and a top-of-handler check:
```
if (m_appContext && ctx.prefersConservative()
                 && !ctx.userJustActed(5000)
                 && (m_retransmitTickCount % 2 == 0)) {
    skipDataRetransmits = true;
}
```
The skip applies ONLY to the chunk-retransmit loop. CONNECT
retransmits stay at full cadence above the skip — relay fallback
timing has to remain bounded (20 attempts × 1.5s = 30s window
before we give up on direct UDP).

User-just-acted overrides: a snap-send within 5s flips back to full
cadence so the user doesn't perceive a stall during a press-send
flow.

### NetworkManager LAN-isolation threshold is now context-aware

Previously `kIsolatedLanThreshold = 2` was static. On a cellular
connection, LAN literally cannot work — there's no shared subnet
to route over. Giving it 2 attempts before falling back to relay
wastes 6+ seconds of user time.

Same pattern: `setAppContext` on NetworkManager, derive
`effectiveThreshold = (ctx.connectionType == Cellular) ? 1 : 2`
at the gate. Log line tags the reduced-threshold case so device
traces are clearly attributable.

### DhtDiscovery bootstrap retry stretches on reachability flap

`bootstrapCheckTimer` previously always fired 30s after `start()`.
If the user changes networks (laptop closes/opens, phone roams)
during those 30s, the retry fires while routing is still settling
and the bootstrap probes get dropped — we then wait the full
re-announce cycle (55 min) before we'd try again.

Fix: `DhtDiscovery::setAppContext`; at the start() site, derive
`bootstrapDelayMs = ctx.reachabilityRecentlyFlapped(5000) ? 60000 : 30000`.
Gives the OS time to re-acquire DNS + routing before we burn
bootstrap UDP probes.

### AppController reconnect scheduler is user-activity-aware

`peerDisconnected` handler used a fixed 10000ms delay before
trying to reconnect known contacts. That's appropriate for "they
flapped briefly, give them time to come back" but wrong when the
user is actively trying to send/view a snap (they want it NOW).

Fix: when `ctx.userJustActed(2000)`, reconnect delay drops to
1000ms. The 2s window aligns with shutter→send latency so we
don't catch a stale user-active reading from a previous action.
Exponential backoff for repeated failures is still TBD — this is
the bimodal step (1s vs 10s); a graded ladder is a follow-up.

### FSM-misalignment logs include AppContext snapshot

Required for offline tuning of the FSM coverage tables.
`FSMValidator::logMisalignment[RateLimited]` gained an optional
`AppContextProvider*` parameter. When non-null, the log appends:
```
ctx[appActive=1,screenOn=1,page=camera,conn=wifi,metered=0,
    powerSaver=0,onAc=1,userAct=1,ssidRecent=0,reachFlap=0]
```
Single bracket, no inner spaces — grep-friendly. UdpChannel
passes its `m_appContext`; future FSM-using consumers follow.

Implementation pattern: `contextSummary(ctx)` is defined inline
in `FSMValidator.hpp` AFTER the namespace close, with
`#include "core/AppContext.hpp"` between the two namespace blocks.
This way the template body (in the first block) only needs a
forward declaration of AppContextProvider, while template
instantiation at the call site (in UdpChannel.cpp, which already
includes the full AppContext.hpp) sees the full helper definition.

### Wayfinder: refresh on snap from new-on-map peer

User reported "wayfinder doesn't update after receiving a snap."
Investigation: `SnapRouter::cacheSnapSenderLocation` correctly
writes to `peer_locations` on every inbound snap. But
`WayfinderPage::onSnapReceived` only updates existing in-memory
nodes — if the peer wasn't yet on the map AND their snap brought
the first location fix, the pin didn't appear until the 30s
refresh tick or manual page re-enter.

Fix: when a snap arrives from a peer NOT in `m_nodes` AND it
carries `hasLocation()`, fire `refreshLayout()`. The existing-node
path keeps its pulse-only repaint (no layout rebuild) to avoid
the avatar-reload cost on every snap.

### Storage migration 13: users.last_connected_at

Added in V32.27, wired in V32.28. `StorageManager::touchLastConnected(UserId)`
fires from both the inbound and outbound `peerConnected` paths in
AppController, recording ms-since-epoch. Distinct from `last_seen_at`
(which fires on bare presence signals like gossip beacons) — this
only fires after full session completion. Feeds future staleness-
based eviction policies (e.g. "drop contacts we haven't talked to
in 90 days"); no consumer yet.

### Bookkeeping closures during the audit

Three TODOs in `docs/TODO.md` were already-done from V32.24
but never got ticked: edit-contact icon + long-press, contacts
startup load, and the (now-resolved) MainWindow stale comment.
Also closed the "migration runner doesn't rollback on failure" —
inspection showed `applyMigration` already wraps in transaction
and rolls back on both per-statement and commit failure.

---

## V32.27 — Persistence + LRU + context-aware LAN isolation

Three storage-and-network polish items, all groundwork for V32.28+.

### DhtCache global LRU pruning

Per-peer cap of 15 was already in place; without a global cap the
table grew unbounded across many peers. Added a 256-row global cap
to `cacheDhtResult`: COUNT(\*) first (cheap), only fire DELETE when
over. Eviction order: `fail_count DESC, cached_at ASC` — drop the
unreachable rows before the merely old ones.

### NetworkManager LAN-isolation context-aware (started here)

Setter + member added in V32.27; effective use in the gate landed
together. See V32.28 above for the full description.

### Storage migration 13: schema-only add of `users.last_connected_at`

Migration entry added to `kMigrations` list. Schema-repair list
(`kContactCols`) extended for upgrade-from-old-build case.
`touchLastConnected` setter added but not yet wired — that
finished in V32.28.

---

## V32.26 — Drag-down-to-close gesture + camera lifecycle debounce

Two UX fixes, both addressing reported sluggishness perceptions.

### Drag-down-to-close on ConversationDetailDialog

User asked for a drag-down-on-empty-space gesture to dismiss the
conversation thread view (V32.20's mailbox-bundles drill-down).
Reasoning: the explicit "Close" button works but feels heavy on
mobile — modern bottom-sheet UX is "swipe down anywhere on chrome
to dismiss," and the V32.20 sheet had nowhere to grab.

Implementation:
- Restructured the dialog header into two rows. The top row holds
  a small visual drag-handle pill (48×4px, rounded, in
  TEXT_DISABLED colour — the standard iOS/Material indicator that
  this surface is draggable). The bottom row holds the existing
  title + Close button.
- Installed an eventFilter on the header frame from the dialog.
  Tracks mouse-press on the header, captures globalY, watches
  mouse-move; when downward delta exceeds 60px the dialog calls
  reject(). MobileDialog's reject() already animates the bottom-
  sheet slide-out on mobile, so the gesture-to-animation handoff
  is seamless.
- The QScrollArea below the header is left alone: it already owns
  vertical drag for content flick-scroll, and overriding it would
  break native scrolling. The header IS the "empty space" the user
  can grab — drag-handle pill makes that affordance visible.

V1 limitation noted in the source: no live rubber-band of the sheet
following the finger. Doing that would fight
MobileDialog::repositionSheet() which owns the sheet's geometry
on the inline path. User sees the dialog vanish on threshold-cross
instead of following their drag in real-time. Promotable to
MobileDialog (so all sheet-style dialogs gain it) with proper
rubber-band as a v2 — but the V1 gesture works without that
complexity.

Threshold 60px is intentionally generous so the gesture doesn't
fire accidentally during a sideways swipe or normal scroll attempt
that happened to start on the header. The Close button (a child
interactive widget) gets its own clicks naturally — the eventFilter
on the *header frame* doesn't see press events that hit the button
because the button's own mouse-press handler consumes them first.

### CameraWidget lifecycle debounce

Filed in the V32.24 "page-switch smoothness" TODO as suspect #1:
fast swipes past the camera page were tearing down + rebuilding
the gstreamer backend (~1.5s kernel-release wait on FP5) on every
show/hide transition.

Fix: showEvent and hideEvent now route through a 250ms debounce
timer instead of calling startCamera/stopCamera directly. A
show-followed-by-hide within 250ms cancels the queued start; the
inverse cancels the queued stop. Only "real" visibility transitions
(intent to actually stay on the page) reach the backend lifecycle.

The timer is lazily allocated on first show/hide. 250ms matches
typical swipe-animation duration — the user can't visit-and-leave
the camera page in less than that unless they're swiping past
without intent.

Trade-off: first show (cold app launch on camera page) gets a
250ms delay before backend init begins. Acceptable because the
existing kernel-release-wait dominates anyway. Camera scan mode is
unaffected: `shouldProcessFrames()` (the frame-drop predicate) is
independent of the backend lifecycle.

### Open follow-ups

- The other four page-switch suspects (Wayfinder paint during
  animation, synchronous signal cascades, tab-bar badge flashes,
  swipe-passthrough touch leakage) remain unaddressed. They may
  resolve on their own once #1 stops the camera teardown — many
  reported stutters were almost certainly the gst pipeline reset.
  Re-verify with real-device traces before pulling in more fixes.
- Drag-to-close in ConversationDetailDialog is contained to that
  one dialog. If users like it, promote to MobileDialog base
  (with proper rubber-band) so SnapComposerDialog, ContactEditor,
  etc all benefit.

### Build

No new dependencies. QTimer + QMouseEvent already in scope. New
SVG asset wasn't needed (drag-handle pill is a styled QFrame).

---

## V32.25 — Recording header-write guard + symmetric NetworkStats rerank

Two unrelated targeted fixes, both addressing reported behaviour
patterns rather than crashes.

### Recording: output-grace guard in `stop()`

User reported corrupted .webm files on the FP5/Alpine vp8 path.
ffprobe diagnosis: `EBML header parsing failed` — webmmux never
wrote its header. Root cause hypothesis: `m_proc->terminate()` runs
before the gst-launch pipeline transitions to PLAYING, so SIGTERM
is delivered while the muxer is still in NULL/READY state. The user
likely starts a recording and immediately stops it (< 500ms), which
on slow ARM is faster than the camera-init + pipeline-build + state-
transition timeline.

Fix in `VideoRecorder::stop()`: before sending terminate, check
`m_outputPath`'s size. If zero AND we're within 1.5s of recording
start, sleep-poll the file every 100ms for up to (1500ms - elapsed)
total, bailing as soon as the file grows non-zero (meaning webmmux
wrote at least the EBML header). Once size > 0, the file is
finalizable; SIGTERM with `-e` flag will produce a valid (short)
recording.

The wait is bounded so a permanently-broken pipeline (e.g. missing
codec, EBUSY camera) doesn't hang stop(). After the grace window,
we terminate anyway and let the existing post-mortem `size < 1024`
check in the `finished()` handler raise `recordingFailed()` cleanly.

Common case: recording for > ~half a second. File has bytes, the
guard is a single QFile::size() stat, proceed straight to terminate.
Zero overhead where it doesn't matter.

Trade-off: blocks the UI thread for up to 1.5s during stop. The user
just tapped stop and is staring at "saving..." — that's an acceptable
window. Alternative (event-loop processing via QCoreApplication) was
considered but rejected as overkill: stop is rare, the user expects
a brief pause, and processEvents during teardown invites re-entrancy
bugs.

Pairs with the existing `m_proc->waitForStarted(2000)` at start time
— that confirms the *process* spawned; this confirms the *pipeline*
streamed. Together they bound both ends of the "recording must
produce output" guarantee.

### Symmetric NetworkStats rerank

Filed observation from earlier sessions: `NetworkStats::rankClassesForPeer`
(V31 learned per-peer routing priorities) was applied only on the
cache-hit code path in `connectByDhtId`. The live-DHT-lookup path
that fires when cache is empty appended results to the address queue
in raw arrival order, then `tryNextPeerAddress` drained them in
arrival order. Result: cold-start ran with the heuristic prior only,
and only the second-and-later connect attempts benefited from
learning. Opposite of what we want — the connect that hurts most is
the first one.

Refactor:
- Extracted the inline rerank logic into a static helper
  `applyNetworkStatsRerank(QList<PeerAddress>&, UserId, callSite)`
  in the anon namespace at the top of `AppController_commands.cpp`.
- Cache-hit path now calls the helper instead of inlining the sort.
- Live-DHT path calls the helper inside the singleShot(0) lambda
  that fires `tryNextPeerAddress`. The deferral means addresses
  arriving in the same event-loop tick are seen by the rerank
  before draining begins.

Limitation that mirrors the cache-hit behaviour: addresses that
arrive AFTER the singleShot tick fires go to the queue tail in raw
arrival order. We don't continuously re-sort because:
1. Sort churn while a connect attempt is in flight could disrupt
   it (the head of the queue is the current target)
2. DHT results don't arrive in tight bursts; spacing them across
   ticks means most realistically arrive before draining starts
3. Late-arriving addresses going to the tail is the same behaviour
   as the cache-hit path which also doesn't re-sort post-construction

Logs at Info level include the `callSite` tag (`"cache-hit"` vs
`"live-DHT"`) so real-device traces can confirm both call sites
fire as expected.

### Build

No new dependencies. `QThread` (used for `msleep` in the recording
guard) is already in `Qt6::Core`. `QDateTime` (used for the deadline)
is already included.

---



## V32.24 — Edit-contact icon + long-press, contacts startup load

Three changes off one user session's TODO filings. The edit-contact
icon work is V1 (icon + mistap-prevention shipped, real-device
verification pending). The contacts-startup wiring is shipped as
the one-line fix it turned out to be.

### Edit-contact button: pencil icon + long-press

**Was:** `QPushButton(QStringLiteral("edit"))` at 11px font, sitting
in a `TOUCH_MIN×TOUCH_MIN` cell. The text label was nearly invisible
on mobile — the button was the right size to tap, but you couldn't
*see* what it was. User reported it as "icon not showing on mobile,"
which was almost right: there was no icon, just unreadable text.

**Now:**
- New SVG asset `assets/icons/edit.svg` — Feather-style diagonal
  pencil using `stroke="currentColor"` so `SvgIcon::secondary` can
  tint it to `TEXT_SECONDARY` at render time
- Registered in `qt_add_resources` block of `src/ui/CMakeLists.txt`
  (keeps in-binary so flatpak/Alpine deployments don't need the
  filesystem fallback path)
- Button rendered with `setIcon(SvgIcon::secondary("edit", QSize(20,20)))`
- 20×20 icon inside the `TOUCH_MIN` cell — gives ~8px breathing
  room around the icon for the tap hitbox

**Mistap prevention** — was discussed in an earlier session but not
yet shipped. The edit button sits right next to the share-location
button in a tight horizontal row; the user can easily hit it
mid-scroll or while reaching for the location share. Switched from
bare `clicked` → `openEditor` to:
- **Long-press (500ms hold) opens the editor** — matches Android's
  default long-press threshold
- **Short tap surfaces a QToolTip** ("Tap and hold to edit")
  anchored at the button's bottom-center, auto-dismissing after 1.5s
- The hold-detector is a per-button QTimer parented to the button
  (dies with it). A small heap-allocated `bool *holdFired` flag
  (parent-owned via `destroyed` signal) prevents the `clicked`
  handler from firing after a successful long-press

Why long-press and not "tap → confirm dialog": the confirm dialog
eats two more taps every time and steals the tap-and-hold gesture
the user already knows from every other mobile app. Long-press
matches the standard interaction. Real-device verification on FP5
still pending: earlier attempts at long-press-as-edit got abandoned
when the touch grab interfered with SwipeNavigator, but those were
on the card container, not on a child button — child-widget long-press
shouldn't conflict with the swipe gesture which is on the parent.

### Contacts startup load — V32.24

**Was:** the friends page sat empty for ~1 second after launch
before contacts appeared. User reported "may need an abstraction
to make it snappier."

**Diagnosis:** the abstraction wasn't the issue. `setContacts` on
the friends page was called from EXACTLY ONE site in MainWindow
— the post-delete refresh handler. There was no initial load.
Contacts trickled in via `peerConnectionStatus` /
`peerStatUpdate` signals that fired per-peer as connections came
up, which is why they appeared *eventually* but not immediately.
`loadContacts` itself is a single indexed SELECT on `users` —
milliseconds.

**Fix:** mirrored the V32.20 inbox-init pattern. Right after
`m_inboxPage->reloadConversationsFromStorage()` in MainWindow.cpp,
added:

```cpp
if (m_friendsPage && m_controller->storageManager()) {
    const auto contacts = m_controller->storageManager()
        ->loadContacts(Core::UserId{ m_controller->stableId() });
    m_friendsPage->setContacts(contacts);
}
```

One block, no new abstraction. If real-device testing shows there's
still perceptible delay after this fix, then we revisit with profiling
and possibly invent a `Contact::List` shared model that all UI pages
subscribe to. But the obvious fix first.

### Filed but not shipped this session

- **Corrupted .webm recordings on the vp8 fallback path.** ffprobe
  reports `EBML header parsing failed` on a fresh recording from
  FP5/Alpine. Most plausible root cause: recorder process is
  killed before pipeline reaches PLAYING state (we currently
  `waitForStarted(2000)` which only confirms process spawn, not
  streaming). Webmmux writes its header on the first cluster — if
  the user records very briefly and stops, no cluster, no header,
  unplayable file. Fix sketch in the filed TODO: parse gst-launch
  stdout for "Setting pipeline to PLAYING" before allowing stop.
  Pairs naturally with the audio-normalization work also filed.

### Build

- `edit.svg` added under `assets/icons/`, registered in the qrc
  list in `src/ui/CMakeLists.txt`. No new dependencies — `QToolTip`
  is part of `Qt6::Widgets` already linked.

---



## V32.22 — Video playback fast-forward-then-frozen fix

Laptop log showed audio playing correctly while video went through a
fast-forward animation at the start, then froze on a single frame for
the rest of the duration. Classic GStreamer pipeline-without-sync
symptom: video branch races to completion as fast as the decoder
delivers frames; audio branch (autoaudiosink) is clocked to real
time. The two branches operate on independent timelines.

### Root cause

The viewer pipeline tees `decodebin` into two branches:

```
filesrc ! decodebin name=dec
  dec. ! queue ! videoconvert ! videoscale ! jpegenc ! multifilesink ...
  dec. ! queue ! audioconvert ! audioresample ! autoaudiosink
```

`autoaudiosink` defaults to `sync=true` (it inherits from GstBaseSink
and is registered as a clock provider). `multifilesink` defaults to
`sync=false` because its typical use case (capturing snapshots to
disk) doesn't want clock-throttling — bursts of frames are desirable
for "save every frame for later analysis" workflows.

But our viewer uses multifilesink with `max-files=1` as a poor-man's
video display: keep one rolling JPEG that a Qt poll timer renders to
the screen at 20 Hz. For that, we need multifilesink to wait for each
frame's running time before writing — i.e. sync to the pipeline clock,
which the audio sink already provides.

Without `sync=true`: decodebin pumps frames to multifilesink as fast
as the decoder works, multifilesink writes the JPEG file in a tight
loop with each successive video frame, the Qt poll only ever sees
the most-recent file, multifilesink finishes its work in well under a
second, the JPEG file is now frozen at the last frame, audio plays
the remaining duration over the still image. Hence: fast-forward then
freeze.

### Fix

Single token append:

```cpp
<< QStringLiteral("multifilesink")
<< QStringLiteral("location=...") << QStringLiteral("max-files=1")
<< QStringLiteral("sync=true");   // V32.22
```

Works for both audio-having and audio-less videos. With audio,
autoaudiosink is the clock; with no audio, GStreamer's system clock
is the fallback. Either way each frame's running time is honored.

### What the recorder still does

The companion recording pipeline (`VideoRecorder.cpp`) also has a
multifilesink-driven preview branch. Left alone — that branch runs
during *live capture* where the camera source provides the clock and
frames arrive at the live rate. No race possible. Don't add sync=true
there blindly; the live-capture case doesn't need it and adding it
could introduce its own latency surprises.

### Observation filed but not acted on

The same log shows ~1 self-originated punch dropped every 20 seconds
(matches our maintenance-punch interval). Send-side
`initiateHolePunch` already refuses loopback targets, so we're the
*receiver* of these — likely a stale instance of the app still bound
to UDP/9876 on the host. Startup zombie-probe currently checks TCP
only. Filed in `🧪 Needs investigation` for future work; not a crash,
just noise.

---



## V32.21 — AppContext: app-state coupling for the FSM (V1)

V32.21 introduces the AppContext infrastructure described in the design
brief from the V32.20 session. Goal: give the network layer awareness of
app state (foreground/background, screen on/off, current page, connection
type, metered, power state, recent SSID changes, recent user actions)
so it can scale aggression / backoff sensibly instead of running fixed
constants regardless of what's happening to the device.

V32.21 ships the infrastructure plus **one consumer** as a proof point
that the channel works end-to-end. Future sessions wire more consumers.

### New files

- **`include/core/AppContext.hpp`** — defines:
    - `AppContext` value-type struct with the fields above + helpers
      `userJustActed(ms)`, `ssidJustChanged(ms)`,
      `reachabilityRecentlyFlapped(ms)`, `isIdle()`,
      `prefersConservative()`.
    - `AppContextProvider` QObject owning the snapshot and broadcasting
      `contextChanged(AppContext)`. Hookup methods (`attachPowerManager`,
      `attachApplicationState`, `attachNetworkInformation`) plus manual
      note methods (`setCurrentPage`, `noteUserAction`, `noteSsidChange`,
      `noteReachabilityFlap`).
- **`src/core/AppContext.cpp`** — implementation. Listens to
  `QGuiApplication::applicationStateChanged`,
  `QNetworkInformation::transportMediumChanged`,
  `QNetworkInformation::isMeteredChanged`, plus PowerManager signals
  (latter no-op until PowerManager is instantiated — flagged in the
  filed follow-up).
  `applyIfChanged` guarantees the `contextChanged` signal only fires
  on real transitions so subscribers don't get signal storms from
  redundant pollers reporting unchanged values.

### Wired in AppController

- Pimpl gets a `std::unique_ptr<AppContextProvider> appContext`
- Public `AppController::appContext()` accessor (returns non-owning pointer)
- Constructed early in `initialize()` (after Session, before Network)
  so by the time NAT is built, the context is ready to be attached
- `attachApplicationState()` and `attachNetworkInformation()` called
  immediately. `attachPowerManager` deferred (PowerManager not yet
  instantiated; safe to call later)
- `noteReachabilityFlap()` called from the existing
  `QNetworkInformation::reachabilityChanged` handler — feeds online/
  offline transitions into the context
- `noteSsidChange()` called from the IP-change poll handler when a new
  primary interface IP is detected (couldn't tell "SSID change" vs
  "kernel chose different interface" without OS-specific helpers; both
  produce the same downstream consequence, so we treat them
  equivalently)

### Wired in MainWindow

- `setCurrentPage(name)` called from `SwipeNavigator::pageChanged` so
  the network layer knows whether the user is on camera / inbox /
  friends / wayfinder / settings
- `noteUserAction()` called from `CameraWidget::snapCaptured` (shutter
  tap) and `InboxWidget::snapOpened` (snap view) — both strong signals
  that the user is actively waiting for snappy network behaviour

### First consumer: NatTraversal punch volley adaptation

NatTraversal gets a new `setAppContext(Core::AppContextProvider*)` setter
plus a non-owning `m_appContext` member. `initiateHolePunch` derives an
effective burst count from the context:

- Default: `kPunchCount` (60 packets × 150ms = 9s burst) — historical
  fixed behaviour
- If `context.prefersConservative()` (idle + metered/cellular/power-saver)
  AND NOT `context.userJustActed(5000)`: halve the burst to 30 packets
  (~4.5s). The maintenance punch every 20s continues regardless, so the
  CGNAT mapping stays open with much less cellular data while the user
  is pocketed
- If `context.userJustActed(5000)` (shutter / send / open within the
  last 5 seconds): always use the full burst regardless of conservative
  mode — the user is expecting snappy delivery, this is the wrong moment
  to save 30 UDP packets

The effective count is computed ONCE at the start of the volley and
captured into the punch lambda by value, so a context flip mid-burst
doesn't produce a weird "halfway through I changed my mind" effect.
AppController attaches the context to NAT at NAT construction time so
the very first punch volley uses adaptive behaviour.

### Why only one consumer this session

The infrastructure is generic but the wins are very consumer-specific.
Wiring all of NatTraversal + UdpChannel + AppController reconnect + DHT
bootstrap in one session is too much surface area to test confidently —
I want to see whether the punch-volley adaptation actually moves the
needle on real device logs before deciding which lever to pull next.
The relevant logs already exist (cafe-wifi punch storms, cold-start
reconnect delays); a few days of running with V32.21 should produce
clear "yes the burst halving works" or "no, the maintenance interval
matters more" evidence.

### Build

- `AppContext.cpp` added to `src/core/CMakeLists.txt`, header added to
  the AUTOMOC list
- `Qt6::Gui` and `Qt6::Network` added to `snapclone_core`'s link list —
  previously only `Qt6::Core` and `Qt6::Positioning` were linked, but
  AppContext needs `QGuiApplication` (Gui) and `QNetworkInformation`
  (Network)

### Open follow-ups (filed in TODO)

- PowerManager is built but never instantiated. The `attachPowerManager`
  path is wired but no-op until someone calls it with a real instance.
  This means `powerSaver` / `onAc` / `screenOn` stay at optimistic
  defaults. Worth wiring next.
- Wire more consumers: UdpChannel retransmit timer (back off when
  `prefersConservative()`), AppController reconnect scheduler (be
  more immediate when `userJustActed`), DHT bootstrap retry interval
  (stretch when `reachabilityRecentlyFlapped(5000)`)
- LAN-isolation threshold (currently 2 from V32.15) → drop to 1 when
  `connectionType == Cellular` since LAN can't possibly work
- Persistence: the V32 NetworkStats persistent profile (also filed) and
  AppContext are independent but pair naturally — a per-context decision
  log would feed offline tuning

---



## V32.20 — Conversation-grouped inbox (UI half of V32.15's groundwork)

V32.15 added the storage primitives (`Snap::conversationKey`,
`ConversationSummary` struct, `loadConversations`, `loadConversationSnaps`)
but the inbox UI still showed a flat list of snaps — one row per snap
regardless of whether they came from the same person. Past a handful of
snaps that collapses into noise. V32.20 wires the UI to the existing
storage primitives.

### New components

- **`ConversationDetailDialog`** (new files in `include/ui/` + `src/ui/`):
  bottom-sheet MobileDialog showing all snaps in one 1:1 thread, oldest
  at top. Outbound snaps right-aligned with accent background, inbound
  left-aligned with raised background — classic chat-bubble convention.
  Pure presentation: emits `snapTapped(SnapId)` and lets the parent
  InboxWidget handle viewer launch. This keeps the `m_openViewers` dedup
  map (V32.7 fix for Qt FFmpeg crashes on parallel viewers) authoritative
  in one place across both the list and the detail dialog. Auto-scrolls
  to the bottom on open via a queued single-shot — mirrors typical chat
  UX where the newest message is visible immediately.

  Card widget uses a small QFrame subclass (`SnapCard`) that intercepts
  mouse-press and emits via a captured callback. Avoided full Q_OBJECT
  subclassing because each dialog needs only one signal (snapTapped),
  not one per card.

  Thread snapshot is taken at construction-time; the dialog doesn't
  live-update if new snaps arrive while it's open. Documented in header.
  Live-updating is a v2 feature; not worth the complexity for V1.

- **`InboxWidget`** extended additively:
    - New API: `setSelfId(UserId)`, `setStorage(StorageManager*)`,
      `reloadConversationsFromStorage()`. MainWindow calls all three
      right after the controller is initialized.
    - Two display modes selected by whether `m_selfId` is set:
      conversation mode (one row per thread) or legacy flat mode
      (one row per snap, preserved as the fallback for startup race
      protection).
    - `addSnap` / `removeSnap` / `clearSnaps` maintain a
      `m_conversations` aggregate alongside the existing `m_snaps`.
    - List items in conversation mode store the OTHER-PARTY UserId in
      Qt::UserRole instead of a snap id. `onItemDoubleClicked` branches
      on mode and dispatches either to `ConversationDetailDialog`
      construction or to the legacy snap-open path.
    - `openSnapById` extracted as a private helper. Both the flat-list
      tap path and the conversation-detail `snapTapped` relay use it,
      so the viewer dedup map + lifecycle are managed in exactly one
      place. Crucially: if the requested snap isn't already in
      `m_snaps` (because conversation mode loaded the thread directly
      from storage, bypassing `m_snaps`), the helper falls back to
      `m_storage->loadSnap(id)` and inserts into `m_snaps` on demand.

### Wired through MainWindow

`connectSignals()` calls the three new InboxWidget setters right after
the `if (m_controller)` guard, before any `snapReceived → addSnap`
connection. Order matters: must set the selfId / storage / reload before
addSnap can fire, otherwise the very first inbound snap would render in
legacy flat mode and confuse the unread-count bookkeeping.

### Edge cases handled

- **Double-add protection**: `addSnap` checks `m_snaps.contains()` before
  incrementing `unreadCount` so the same snap arriving twice (live + later
  also via some race) doesn't inflate the badge.
- **Empty selfId**: gates conversation mode off. Necessary because
  setSelfId might not have been called by the time addSnap fires
  (defense in depth, though MainWindow now wires setSelfId first).
- **Total unread sync**: `reloadConversationsFromStorage` sums per-row
  unread into `m_unreadCount` and emits `unreadCountChanged` so the
  tab-bar badge refreshes after the restore.

### Risks documented for on-device verification

- `removeSnap` in conversation mode doesn't fully recompute the affected
  row's `lastSnapAt`/`lastMediaType` from in-memory state because
  `m_snaps` only holds inbound snaps. If the removed snap was the row's
  "last", the row's preview will be slightly stale until a
  `reloadConversationsFromStorage()` runs. Acceptable for V1 — caller
  can refresh if it's noticeable.
- Detail dialog snapshots the thread at open time. New snaps arriving in
  that thread while the dialog is open don't appear until the user
  reopens. Documented in header.

### Build

`ConversationDetailDialog.cpp` added to `src/ui/CMakeLists.txt` sources,
header added to AUTOMOC list.

---



## V32.19 — Bug-hunt round 2: post-V32.18 audit

After the V32.18 MobileDialog dangling-pointer fix went in, did a systematic
sweep for the same class of defect across the codebase. Five more real
bugs found and fixed.

### CameraWidget transcoder replacement clobbers running encode

When recording stops and the transcoder for the recording is created:

```cpp
if (m_transcoder) m_transcoder->deleteLater();
m_transcoder = new Video::VideoTranscoder(this);
```

If a previous transcode is still in flight when a new recording finishes,
the old transcoder is queued for delete BUT its `transcodeFinished` /
`transcodeFailed` signal connections to `onTranscodeFinished` /
`onTranscodeFailed` are still wired. The old transcoder's signal can fire
*after* the new transcoder has been assigned to `m_transcoder`. The slot
then runs `if (m_transcoder) { m_transcoder->deleteLater(); ... }` — and
kills the **new** transcoder mid-encode, sending the wrong recording.

Fix: `disconnect(m_transcoder, nullptr, this, nullptr);` before the
`deleteLater()`.

### NatTraversal::m_tcpStun synchronous-signal-during-abort crash

Two call sites (constructor reset path and `restartStunOnly` path) used:

```cpp
m_tcpStun->abort();
m_tcpStun->deleteLater();
m_tcpStun = nullptr;
```

`QAbstractSocket::abort()` emits `errorOccurred` *synchronously* during
the call. The errorOccurred slot does its own
`if (m_tcpStun) { m_tcpStun->deleteLater(); m_tcpStun = nullptr; }`. So
when control returns from `abort()`, `m_tcpStun` is already nullptr, and
the next line `m_tcpStun->deleteLater()` segfaults.

Fix: cache the pointer, null the member first, call `disconnect()` to
suppress signals, *then* abort + deleteLater on the local copy. Both
sites updated.

### JamiAdapter DHT callback context leaks

The DHT PUT and mailbox-GET completion callbacks allocate a `PutCtx` /
`GetCtx` heap struct passed through OpenDHT's C API, then post a queued
event to the main thread to log+delete it:

```cpp
QMetaObject::invokeMethod(c->self.data(), [c, ok]() {
    // ... log ...
    delete c;
}, Qt::QueuedConnection);
```

If `c->self.data()` is null (the DhtDiscovery has been destroyed since the
PUT was issued), `invokeMethod` with a null receiver fails to schedule
the event in Qt 6 — and **the lambda never runs, leaking `c`.** Same
risk if `self` was valid at post time but destroyed before dispatch:
Qt removes pending events for a destroyed receiver, lambda body never
runs, leak.

Fix: snapshot data and delete the context BEFORE posting the queued
event. The lambda then captures plain values + a QPointer for the
log emission, and `delete c` is guaranteed to run exactly once on the
callback thread regardless of receiver lifetime.

Three sites fixed: multi-addr PUT, connection-request PUT, mailbox-GET
done callback.

### Bogus QPointer-from-raw-pointer check

In the mailbox-GET blob callback, the code was:

```cpp
DhtDiscovery *disc = c->disc;
QMetaObject::invokeMethod(disc, [disc, fromId, rawBytes]() {
    if (!QPointer<DhtDiscovery>(disc)) return;
    // ... use disc ...
});
```

`QPointer<T>(raw)` only auto-nulls if `raw` was already known dead to
Qt's QObject machinery. Constructing a QPointer fresh from a stale raw
pointer can't recover liveness info — the check `if (!QPointer<...>(disc))`
is equivalent to `if (disc == nullptr)`, which it isn't. Fixed by
constructing a `QPointer<DhtDiscovery>` from a still-valid `disc` at
callback time and capturing **that** QPointer in the lambda.

### Minor: uninitialised SnapComposerDialog::m_textMode

`m_textMode` was declared without a default initialiser in the header.
Both constructors set it via init list so this was safe in practice,
but added `{false}` defensively in case a future constructor forgets.

### Audited but found clean

- All other `QPropertyAnimation *m_anim` members in the codebase
  (PagePeelHint, TabBar, SwipeNavigator) — none use `DeleteWhenStopped`,
  so the V32.18 dangling pattern can't recur there
- `QProcess` lifecycle in GstLaunchBackend, CameraBridgeManager,
  VideoRecorder, VideoTranscoder, VideoSnapViewerDialog — most use the
  defensive disconnect+reparent+deleteLater pattern; one site in
  GstLaunchBackend has a `kill(); waitForFinished(); deleteLater()`
  sequence without prior disconnect, but the connected slots don't
  dereference the process pointer so it's safe by accident. Left as-is.
- WebSocketTransport disconnect path — already disconnects all signals
  before abort
- Storage SQL queries — all check `exec()` return values defensively
- Wire-format parsers (SnapRouter v3/v4/v5, UdpChannel datagram,
  JamiAdapter DHT payload) — all bound-check length fields before
  reading, cap absurdly large values, and check `QDataStream::status()`
- Iteration with mutation — `pruneDiscoveryMap`'s erase pattern is
  correct; `UdpChannel::onRetransmitTimer` emits inside the loop but
  the slot doesn't modify `m_peers` re-entrantly
- Concurrency boundary at FirewallPortManager worker threads — uses
  proper QMetaObject::invokeMethod(this, ..., QueuedConnection) so
  thread crossings stay safe even if worker outlives the manager

---

## V32.18 — Dangling-pointer crash on second snap-send (composer→picker)

Mobile log showed photo tap, MobileDialog #1 (composer), MobileDialog #2
(picker) then app restart line. Different timing from V32.16 pulse crash
(7s gap → that was a slow-burn animation overload) — this is tight, ~2s
between composer-open and crash.

### Root cause: MobileDialog::m_anim dangling raw pointer

`m_anim` was a raw `QPropertyAnimation *`. `animateSheetIn()` did:

```cpp
m_anim = new QPropertyAnimation(this, "pos", this);
...
m_anim->start(QAbstractAnimation::DeleteWhenStopped);
```

When the slide-in animation finished, `DeleteWhenStopped` deleted the
animation, **but the m_anim member raw pointer was not nulled**. The
member still pointed at freed memory. When the user later tapped Send
on the composer, `accept()` → `animateSheetOut()` ran:

```cpp
if (m_anim) { m_anim->stop(); m_anim = nullptr; }   // DANGLE — m_anim
                                                     // was freed already
```

`if (m_anim)` saw the non-null stale pointer, called `stop()` on freed
memory → crash.

Why this only bit recently: the bug existed from V28 when MobileDialog
was first written, but didn't fire in V32.14 and earlier because the
composer-to-picker handoff didn't exist. V32.15's always-picker UX
introduced a second MobileDialog opened after the first had completed
its slide-in (the first being the composer, second being the picker).
The crash needs slide-in to have *completed* before the next animate
call, which is why it's order-of-2-seconds rather than immediate.

### Fix: QPointer for m_anim and m_scrim

`m_anim` is now `QPointer<QPropertyAnimation>`. When DeleteWhenStopped
fires, QPointer auto-nulls — the `if (m_anim)` guard becomes
meaningful again.

Also opportunistically converted `m_scrim` to QPointer. The scrim is
parented to the root window (MainWindow), not to the dialog. If root
dies first during app shutdown the scrim is destroyed via parent
ownership, leaving the dialog destructor's `m_scrim->hide()` call to
crash. Same defence.

### Audit

Searched for the same pattern elsewhere — raw pointer member + stored
animation + DeleteWhenStopped. MobileDialog::m_anim was the only hit.
Three other classes have raw `QPropertyAnimation *m_anim` members
(PagePeelHint, TabBar, SwipeNavigator) but none use DeleteWhenStopped:
they let parent ownership clean up at widget destruction, so the
pointer can never dangle while the parent lives.

Local-scope `new QPropertyAnimation(...); anim->start(DeleteWhenStopped);`
patterns (used by toasts, hint fades, etc.) are safe because the
pointer goes out of scope at the end of the function before deletion
matters.

---



## V32.17 — Wire format v5: tier byte for tiered delivery

V32.4 scaffolded `EncodeTier` and `TieredDeliveryPolicy` but the wire
format had no way to identify *which* tier a given snap represented,
so the multi-tier delivery story stayed theoretical — the same logical
snap couldn't arrive twice at different qualities because the
receiver had no way to disambiguate.

V32.17 fixes that by extending the wire format additively, mirroring
the pattern v3→v4 used for `videoDurationMs`:

- New constant `kWireVersion5 = 5`
- `buildV3()` always emits v5, takes new `EncodeTier tier = Standard`
  param, writes the byte after `videoDurationMs`
- Parser handles v3/v4/v5 with version-gated reads; v3/v4 senders
  only ever emitted Standard so the default is correct for them
- Version dispatcher accepts v5 alongside v3/v4
- `sendV3()` plumbed through `routeOutbound`, SYNC-resend (with the
  Qt lambda capture lists too), NEED-after-expired-offer, and the
  offer-redelivery path — every site that reaches the encoder

Snap struct gained `Core::Media::EncodeTier tier = Standard;`.
`Types.hpp` now includes `core/EncodeTier.hpp`.

Storage migration 12 adds `tier INTEGER NOT NULL DEFAULT 2` (Standard).
`saveSnap` writes it, `snapFromQuery` reads it, all four `SELECT`
statements include the column. `repairSchema` has an idempotency entry
matching migration 11's video-columns pattern.

### What this does NOT yet do

The receiver still uses last-write-wins on snap id — a later HighQuality
arrival overwrites an earlier Thumbnail. This is good enough for first
deployment (lower-quality previews still appear instantly, get replaced
by better versions when they arrive) but doesn't yet implement the full
"keep all tiers, lazy-upgrade UI when better arrives" model. That needs:

- A `(snap_id, tier)` compound key — current schema makes `id` primary
- ANNOUNCE/HAVE/NEED extended to carry tier
- TieredDeliveryPolicy actually calling the encoder multiple times
  (currently single-shot)
- Inbox UI distinguishing "loading better version" vs final tier

All filed in TODO.md as follow-up.

### Wayfinder-empty-after-snap also filed

Reported anecdotally: after a fresh snap arrives, the Wayfinder map
doesn't update to show that peer's last-known location. Suspect inbound
snap loc_lat/loc_lon don't get mirrored into `peer_locations`. Filed as
a natural verification opportunity during the upcoming 3-device session,
which produces inbox traffic for the third party we can watch
wayfinder-side.

---

## V32.11 — UI crash/use-after-free bughunt

Systematic pass on UI code looking for crash patterns. Found and fixed
several real latent issues:

### QPointer for SwipeNavigator scroll-target tracking
`m_scrollTarget` and `m_skipGestureForArea` were raw `QAbstractScrollArea *`.
If the user's touch is being processed and the underlying scroll area
gets destroyed (page navigation animation, snap arriving and triggering
list re-layout, dialog opening), the next TouchUpdate would deref a
dangling pointer and crash. Now `QPointer<QAbstractScrollArea>` with
auto-null on destruction. Access sites already null-check via
`m_scrollTarget ? ... : nullptr` which works with QPointer's bool
conversion unchanged.

### QProcess parent-destructor race
`VideoRecorder`, `VideoTranscoder`, `GstLaunchBackend`,
`VideoSnapViewerDialog` all use `m_proc->kill(); m_proc->deleteLater()`.
When this happens from a destructor (or as part of one — e.g.
`stop()` called from `~GstLaunchBackend`), the deleteLater is
preempted by Qt's parent-child cleanup chain. Since `m_proc` is a
child of the destructing object (via `new QProcess(this)`), the
QObject base destructor synchronously deletes it during its child
cleanup phase — BEFORE the event loop processes the deleteLater.

If the SIGKILL hasn't been fully processed by the OS by then,
QProcess::~QProcess prints "QProcess: Destroyed while process running"
(which the user saw in V32.9-era logs).

**Fix:** `setParent(nullptr)` BEFORE `deleteLater`. The QProcess
becomes orphaned and survives the parent's destruction cascade. It
lives until the next event loop iteration when deleteLater fires —
by which time the kill has been processed. Applied to all four
sites consistently.

### Dead-code cleanup
`VideoSnapViewerDialog::startQtMediaPlayback` had
`m_video->installEventFilter(this)` but no eventFilter override — Qt
routed events to the default impl which returns false. Removed with
a comment noting how to add tap-to-toggle if desired in the future.

### Patterns verified safe
- All 29 `connect(... [this] ...)` lambdas use `this` as receiver →
  Qt auto-disconnects on destruction.
- All 42 `QTimer::singleShot` calls have explicit receiver widgets →
  auto-cancel on receiver destruction.
- Dialog cleanup: `WA_DeleteOnClose` everywhere, MobileDialog scrim
  cleanup explicit in destructor.
- InboxWidget's `m_openViewers` uses QPointer + lookup-by-key (not
  captured iterator) — survives addSnap mutations and viewer
  destruction.
- VideoRecorder's `stop()` waitForFinished(3000) for graceful EOS:
  the local event loop CAN re-enter but only the recorder's own state
  mutations happen, and the connected slots only do deleteLater on
  the recorder (which deferred-delete handles correctly).
- TouchFeedback shimmer/spring animations all parented to receiver
  widget → auto-cleanup on widget destruction.

### Files touched
- `include/ui/SwipeNavigator.hpp` — QPointer for scroll targets, add
  QPointer include
- `src/ui/SwipeNavigator.cpp` — no .cpp changes (QPointer assignment
  is implicit from raw pointer)
- `src/ui/video/VideoRecorder.cpp` — setParent(nullptr) before
  deleteLater in destructor
- `src/ui/video/VideoTranscoder.cpp` — same
- `src/ui/video/VideoSnapViewerDialog.cpp` — same + remove dead
  installEventFilter
- `src/ui/camera/GstLaunchBackend.cpp` — same in stop()

---

## V32.10 — Video snap viewer: gst-launch primary, Qt media fallback

**Note:** the entry below describes the earlier dual-backend-with-Qt-primary
design. The actual current code uses **gst-launch as primary on Linux**
and falls back to Qt media only when gst-launch isn't available
(Windows/macOS or stripped Linux). Backend selection happens at construction
based on `gstLaunchAvailable()`. Override via `SNAPCLONE_VIEWER_BACKEND=qt`
env var for testing. The gst pipeline was also rewritten to use the proven
`matroskademux name=d ! d.video_0/audio_0` static-pad pattern from the
transcoder (was previously `decodebin name=dec ! dec.` with dynamic pads
which races on pad-add ordering). Audio probe via `gst-discoverer-1.0` to
skip the audio branch on video-only files.

### Two backends, picked at runtime

**Primary: `QMediaPlayer` + `QVideoWidget`.** Native Qt path. Best when
it works — hardware-friendly decode, integrated audio output, in-process
pause/seek/scrub. Connects to `QMediaPlayer::errorOccurred` AND to
`QVideoSink::videoFrameChanged` to distinguish two failure modes:
- Error **before first frame painted** → Qt's pipeline is broken for
  this file (or this machine). Fail over to GStreamer.
- Error **after frames have painted** → transient. Log and ignore.

**Fallback: `gst-launch-1.0` as a child process.** Pipeline:
```
filesrc location=... ! decodebin name=dec
  dec. ! queue ! videoconvert ! videoscale ! video/x-raw,width=640
       ! jpegenc quality=85 ! multifilesink location=/tmp/<pid>.jpg max-files=1
  dec. ! queue ! audioconvert ! audioresample ! autoaudiosink
```

Video frames written to a tmpfs JPEG file (atomically replaced per
frame by `multifilesink max-files=1`). The dialog polls the file at
20 Hz, mtime-checks to skip redundant decodes, scales-to-fit and
paints into a QLabel that replaces the QVideoWidget. Audio goes to
`autoaudiosink` — PipeWire/PulseAudio/ALSA, whatever the system has,
no Qt involvement.

Same `decodebin` element handles both .webm (AV1+Opus from our
transcoder) and .mkv (H.264+Opus from the recorder when transcode
falls back).

Caps drop the `pixel-aspect-ratio=1/1` constraint that bit V32.6/V32.7
videoscale-integer-overflow paths in the recorder and transcoder.

### Per-process Qt-media-broken sticky flag

Once `QMediaPlayer` has failed for one snap during the session, we
skip it for all subsequent snaps and go straight to gst-launch. Saves
~500 ms of "black screen → error → fail-over" per snap, which would
otherwise be every snap on a machine where Qt's pipeline is broken.

Flag is `false → true` only. If Qt media starts working mid-session
(somehow), we stay on gst for consistency. A restart re-attempts Qt.

### What Qt-backend keeps

Pause/play via the play button, seek via slider, time display, tap-on-
video-toggle-play (planned). These rely on `QMediaPlayer::duration`,
`positionChanged`, `setPosition` — none of which exist for our gst
process. In gst-backend mode those controls are disabled and the time
label shows "Playing…". When the gst process exits cleanly (EOS), the
play button flips to ▶ for the user to know it's done. Replay requires
closing + reopening the snap (next step: in-place replay support).

### Hard failure: both backends down

`showHardFailureOverlay()` is the last resort. Fires when:
- File isn't on disk (snap expired / corrupted local cache)
- Qt fails AND gst-launch isn't installed
- gst process exits non-zero (pipeline failed to negotiate)

Parents to whichever video display widget is currently visible
(QVideoWidget or the gst-mode QLabel) and shows a "save it and open
in another player" message. The save button stays enabled so the
user can still extract the file.

### Teardown safety

`errorOccurred` is connected to a lambda that defers fail-over via
`QTimer::singleShot(0, ...)`. Calling `teardownQtMedia()` directly
from inside the connected slot would disconnect the signal we're
handling — Qt tolerates this but it's brittle. The singleShot lets
Qt's signal dispatch unwind first.

`teardownQtMedia()`/`teardownGst()` are idempotent and called from
both the fail-over path and the destructor. Disconnect before
terminate, kill if terminate doesn't finish in 1 s, deleteLater
rather than direct delete.

### Known limitations (deliberate)

- **No pause/seek in gst mode** — would require either pausing the
  child process (SIGSTOP, hacky and stops audio with awkward
  resume) OR using gst-launch's stdin to send a SEEK event (which
  it doesn't accept). Proper fix would be using GStreamer C bindings
  rather than the gst-launch CLI, which is a much bigger dep.
- **Audio/video sync** — video lags audio by up to one poll tick
  (50 ms). For 5-30 s snaps this is imperceptible. For longer
  videos a frame-precise sync would need an `appsink` + manual
  pull-based timing, again the C bindings.
- **Resolution capped at 640 wide** — the JPEG re-encode budget on
  mobile is the constraint. Source video can be any size; downscaled
  for display. Users can save the file and view it natively at full
  resolution. Tradeoff is that we get reliable playback on every
  postmarketOS/Asahi machine where the bridge runs.

### Files touched
- `include/ui/video/VideoSnapViewerDialog.hpp` — backend members,
  helper method declarations, doc rewrite
- `src/ui/video/VideoSnapViewerDialog.cpp` — full rewrite

---

## V32.9 — Touch UX + snap-open crash + caps fixes

### QR code not clickable on touchscreen (issue 1)
**Cause:** SwipeNavigator's TouchBegin handler walks the parent chain
looking for "interactive" widget types (QPushButton, QLineEdit, etc.)
and consumes the touch only when none match. Plain QLabels — including
the QR-code label in Settings — don't match, so SwipeNavigator
consumed the TouchBegin and the QR's event filter never saw a
MouseButtonRelease.

**Fix:** introduced a custom `swipenav-tappable` Qt property. Widgets
that set this property to true are treated as interactive by both
SwipeNavigator's TouchBegin check AND `forwardTap`'s walk. The QR
label now sets `setProperty("swipenav-tappable", true)` and receives
taps reliably. Easy opt-in for any future QLabel that wants taps.

### Snap-open crash via Qt FFmpeg sws_context (newly diagnosed)
**Cause:** Qt6's FFmpeg backend (FFmpeg 8.1.1 on this host) calls
sws_getContext with `dstH = -srcH` to flag a vertical flip — an idiom
older FFmpeg versions accepted but 7.x+ rejects. The result is a NULL
sws context that gets used downstream, leading to crashes.

Compounding factor: tapping the same snap twice (the user observed
"first tap looked unresponsive" and re-tapped 450 ms later) opened
TWO VideoSnapViewerDialogs, each with its own QMediaPlayer racing
over the same file. The 300 ms debounce was insufficient.

**Fixes:**
1. **Dedupe by snap id.** `InboxWidget` now tracks open viewer
   dialogs in a `QHash<QString, QPointer<QWidget>>`. A second open
   request for the same snap raises/activates the existing viewer
   instead of constructing another.
2. **Bumped open-debounce to 750 ms** to absorb tap repeats during
   first-time media-backend warmup (Qt+FFmpeg init takes ~500 ms).
3. **VideoSnapViewerDialog.errorOccurred shows an error overlay.**
   Previously the slot only logged a warning. Now it paints a clear
   "This video can't be played here, save it and open elsewhere"
   panel over the QVideoWidget and stops the player. The save button
   remains enabled so the user can extract the file. Silent failure
   was reading as a crash to users; explicit failure is debuggable.

This is a Qt-side bug we can't fully fix from our code. Long-term
alternatives: switch to a `QVideoSink` + manual frame painting path
(sidesteps the broken sws flip), or use a child gst-launch playbin
with `xvimagesink`/`glimagesink` embedded in a QWindow.

### Recorder preview-branch caps integer overflow (newly diagnosed)
**Symptom from log:**
```
ERROR: from element ... GstVideoScale:videoscale0:
Error calculating the output scaled size - integer overflow
```

**Cause:** the V32.6 preview-branch caps `video/x-raw,width=320,pixel-aspect-ratio=1/1`
forces videoscale to compute height-from-PAR for some source resolutions
(observed at 480×640 from virtual-cam-rear on FP5). The fixate-size
calculation overflows internally and the pipeline fails to preroll.

**Fix:** dropped `pixel-aspect-ratio=1/1` from the preview caps. Caps
become `video/x-raw,width=320` — videoscale picks a height that
preserves source aspect via the standard fixate path, no PAR math.
For 480×640 source: 320×426. For 1920×1080: 320×180. All non-overflow.

### Transcoder svtav1enc v4.1.0 not-negotiated (newly diagnosed)
**Symptom:** matroskademux failed with `not-negotiated (-4)` after
upgrading to svtav1enc v4.1.0. The error propagates upstream from
wherever caps couldn't be agreed.

**Cause:** same `pixel-aspect-ratio=1/1` trick in the transcoder's
videoscale caps as in the recorder's preview branch. svtav1enc 4.1.0
appears stricter about colorimetry/PAR constraints feeding it; the
old version tolerated the PAR-based fixate that newer videoscale
now rejects.

**Fix:** dropped the PAR constraint in `VideoEncoderBackend.cpp`'s
shared pipeline-builder. Same rationale and same effect as the
recorder fix above.

### Notes on issue 3 (3-device connection)
Looking at the log: sc-051d7c06 DID connect to sc-7c271ed3 via the
Magic Wormhole relay at 01:32:21, exchanged ECDH keys, and bidirectionally
shipped snaps. The "connection not working" complaint must have been
about either (a) the direct LAN path failing (which the log confirms —
DHT lookups timed out repeatedly), or (b) the third device. Without
a log from the third device — or one that captures the failure
specifically — I can't pinpoint it. Filed in TODO for next session:

- [ ] **3-device connect investigation.** Need synchronized logs from
  all three devices in the same session, with WHICH device couldn't
  reach which. Likely culprits to check: DHT mailbox slot contention,
  simultaneous-open race in UDP punch logic, session consolidation
  when two transports complete at once (relay + UDP).
- [ ] **QProcess: Destroyed while process running** warnings observed
  twice on the FP5 — recorder process not being awaited at destruction.
  These can crash on some Qt builds.

### Files touched
- `src/ui/SwipeNavigator.cpp` — `swipenav-tappable` property check in
  TouchBegin + forwardTap interactive walk
- `src/ui/SettingsPage.cpp` — QR label sets `swipenav-tappable=true`
- `include/ui/InboxWidget.hpp` — `m_openViewers` + QPointer include
- `src/ui/InboxWidget.cpp` — dedupe by snap id, 750 ms debounce,
  track cleanup in destroyed lambda
- `src/ui/video/VideoSnapViewerDialog.cpp` — error overlay on errorOccurred
- `include/ui/video/VideoSnapViewerDialog.hpp` — `m_errorOverlay` member
- `src/ui/video/VideoRecorder.cpp` — drop PAR from preview branch caps
- `src/ui/video/VideoEncoderBackend.cpp` — drop PAR from transcoder
  videoscale caps

---

## V32.8 — Fairphone 5 rear ultrawide camera support

### The bug
The FP5 has two back-facing sensors (main wide IMX858 + ultrawide
S5KJN1) but camera-bridge only ever exposed one of them. Three
independent issues converged:

1. **`pick_via_cam_list` returned first "Back" only.**
   `find_node_for("back")?` discards any second back-located camera
   reported by libcamera. The ultrawide was being silently dropped.
2. **Bridge `Config` had no slot for a second rear.**
   `Config { pw_node_rear, pw_node_front }` and three threads.
   No infrastructure to spawn an ultrawide virtual node.
3. **Qt-side hardcoded camera list.**
   `GstLaunchBackend::availableDevices` only knew rear/front/tracking.
   Even if the bridge somehow exposed ultrawide, the dropdown
   wouldn't include it.

Bonus discovery: `camera-view.desktop` invokes
`camera-view --camera ultrawide` but `view.rs`'s arg parser didn't
accept `--camera` at all, erroring out immediately on launch. That's
exactly the "not propagating through camera-view rust stack" symptom.

### The fix

**Bridge `capture.rs`** — new `PickedCameras { rear, front, extra_rear: Vec<String> }`
returned by `pick_camera_nodes_multi`. Parses `cam --list` and keeps
ALL back-located entries (first → primary rear, rest → `extra_rear`).
Legacy `pick_camera_nodes` retained as a thin `#[allow(dead_code)]`
wrapper for back-compat.

**Bridge `main.rs`** — added `pw_node_rear_ultrawide` to `Config` plus
`PW_NODE_REAR_ULTRAWIDE` env override. New `run_rear_ultrawide`
thread function spawns `virtual-cam-rear-ultrawide` ("Rear Camera
(Ultrawide)") with the same SENSOR_FLIP_REAR rotation. Spawned ONLY
when either env-supplied or discovered from `cam --list` — single-rear
devices skip the thread cleanly, no wasted resources.

Thread spawn order remains [rear-main, front, tracking, rear-ultrawide]
with 800 ms staggers so each i2c bus gets time to settle before the
next consumer opens it.

**Qt `GstLaunchBackend.cpp`** — added ultrawide to `kNames` (the pw-dump
scan list) and to `availableDevices`'s `kKnown` table with a new
`baseline` flag. The previous race-avoidance logic ("list optimistically
when any virtual-cam is visible, since they all spawn together") now
applies ONLY to baseline cams. Ultrawide is gated on confirmed presence
in pw-dump — listing a never-coming node on a single-rear phone would
let users pick an option that hangs forever.

**`view.rs`** — added `-c`/`--camera <node>` arg and `CAMERA_NODE` env
var (with legacy `PW_SOURCE_NODE` retained). New `resolve_camera_shorthand`
maps `rear`/`ultrawide`/`front`/`tracking` to their full virtual-cam-*
names so the .desktop file's "ultrawide" actually works. Help text
and module doc updated.

**`test-rear-camera.sh`** — added a section 3b that runs `cam --list`
and counts Back/Front entries, so users can verify locally that their
device reports a second back camera before expecting the ultrawide to
appear. Virtual-node check downgrades missing ultrawide from FAIL to
INFO (it's optional, expected absent on most non-FP5 devices).

**`camera-view.desktop`** — kept the `--camera ultrawide` invocation
now that view.rs supports it, with a comment noting the FP5-only
expectation.

### What happens on non-FP5 devices
Single-rear: `cam --list` reports one Back entry → `extra_rear` is
empty → `run_rear_ultrawide` never spawns → Qt's dropdown never
shows "Rear Camera (Ultrawide)". Behavior is identical to before
V32.8 for these devices.

### What to verify on FP5
Run `./scripts/test-rear-camera.sh`:
- Section 3b should show "Back-facing cameras detected: 2"
- Section 4 should list `virtual-cam-rear-ultrawide` as present
- The Snap app's camera dropdown should show "Rear Camera (Ultrawide)"

If the dropdown still doesn't show ultrawide, capture the
`cam --list` output and check whether libcamera reports the second
sensor as `Location: Back` or something else (External / etc.). If
it's tagged differently we'd need to extend the detection logic to
recognize that label.

### Files touched
- `camera-bridge/src/capture.rs` — added `PickedCameras` + `pick_camera_nodes_multi`
- `camera-bridge/src/main.rs` — Config field, run_rear_ultrawide, multi-picker call
- `camera-bridge/src/view.rs` — --camera arg, shorthand resolver
- `camera-bridge/desktop/camera-view.desktop` — comment about FP5 requirement
- `camera-bridge/scripts/test-rear-camera.sh` — cam --list section + ultrawide check
- `src/ui/camera/GstLaunchBackend.cpp` — known-list update, baseline flag gating

---

## V32.7 — Settings page UI for FSM / debug toggles

Surfaces the three `Debug::DebugFlag` toggles (NoFsmTuning,
NoMonitoringBlocking, VerboseFsm) in the Settings page so A/B testing
doesn't need env vars or CLI flags.

### Behaviour
- **Live-apply on toggle.** Flipping a checkbox immediately calls
  `Debug::DebugFlags::instance().set(flag, on, "settings page")`, so
  the runtime sees the change without a restart. The whole reason
  these flags exist is "is X faster with this off?" → one click, see
  if it's faster, click back. A Save-gated UI here would add friction.
- **Save persists.** `saveSettings()` calls
  `DebugFlags::saveToSettings()` which writes the `debug/no_fsm_tuning`,
  `debug/no_monitoring_blocking`, `debug/verbose_fsm` keys to QSettings.
  Loaded on next launch by `DebugFlags::loadFromSettings()` (which
  runs after env/CLI parsing).
- **Source-of-truth is the singleton.** `loadSettings()` reads the
  checkbox state from `DebugFlags::instance().is(...)`, NOT directly
  from QSettings. If env or CLI flipped a flag at startup, the
  checkbox reflects what's ACTUALLY active rather than the stale
  on-disk state. Avoids the checkbox lying about reality.
- **Signal-blocked initial load.** `QSignalBlocker` around each
  `setChecked()` during load so we don't fire the toggled() lambdas
  and accidentally re-apply (and dirty the page) on page open.

### Section placement
Inserted between System Diagnostics and Privacy — same "diagnostic
tools" cluster. Title: "Debug & FSM Tuning". Intro line explains the
default-OFF / production-behaviour convention. Tooltips on each
checkbox explain what specifically the flag disables and why.

### Adding more flags later
Pattern is straightforward: add a `Debug::DebugFlag` enum value, a
`name()` mapping, then in `buildDebugFlagsSection`:
1. Declare a new `QCheckBox *m_dbg<Name>` in the header
2. Create + addWidget in `buildDebugFlagsSection`
3. Call `bind(m_dbg<Name>, DebugFlag::<Name>)` to wire the live-apply
4. Add to `loadSettings()` initial-state block

QSettings persistence and env/CLI parsing happen automatically through
the singleton.

### Files touched
- `include/ui/SettingsPage.hpp` — added three `QCheckBox*` members,
  `buildDebugFlagsSection` declaration
- `src/ui/SettingsPage.cpp` — added `#include "debug/DebugFlags.hpp"`,
  inserted section between Diagnostics and Privacy, added load/save
  blocks for the singleton

---

## V32.6 — Live viewfinder preview during recording via tee branch

### The problem
v4l2 single-consumer: only one process can have `/dev/video0` open at
a time. Our recording flow stopped Qt's camera, waited 1500 ms for
kernel release, then handed the device to gst-launch. During the
entire recording window the viewfinder showed either a stale frozen
frame (pre-V32.5) or a static "RECORDING" placeholder (V32.5). Users
want to actually see what they're recording.

### The solution — tee in the recorder pipeline
gst-launch's `tee` element splits a stream so the SAME source can
feed multiple downstream branches. The recorder's pipeline becomes:

```
v4l2src ! tee name=t
  t. ! queue ! videoconvert ! caps_I420 ! openh264enc ! h264parse ! mux.
  t. ! queue (leaky=downstream max-buffers=1)
     ! videorate ! caps_10fps
     ! videoscale ! caps_width=320
     ! videoconvert ! jpegenc quality=70
     ! multifilesink location=/tmp/snapclone-preview-<pid>.jpg max-files=1
```

Encoder branch unchanged — same H.264 quality. Preview branch is
purely additive: ~5% extra CPU for the JPEG path at 10 fps and 320 px
wide. `leaky=downstream max-buffers=1` ensures the preview branch
NEVER backpressures the encoder branch — if jpegenc or the filesystem
can't keep up, preview frames drop. The recording stays at full fps.

`multifilesink max-files=1` atomically replaces a single file per
frame so readers always see a coherent JPEG.

### Viewfinder side
`CameraWidget` polls the preview file at 20 Hz during recording. On
each tick it checks the file's mtime; if unchanged from the last
read, it skips the JPEG decode entirely (avoids 20 Hz busy-decode
when the recorder produces 10 fps). User rotation override is applied
the same way as live frames. The poll timer starts after
`m_recorder->start()` returns success and stops in both
`recordingStopped` and `recordingFailed` handlers.

The "RECORDING" placeholder from V32.5 still paints initially (during
the 1500 ms kernel-release wait before gst-launch even runs); once
the first preview JPEG hits the filesystem, it gets replaced with
live frames.

### Recorder bitrate bumps for higher-quality source-to-AV1
User's reasoning: lower preview resolution to save CPU for a
higher-quality first-pass H.264 source feeding the AV1 transcode.
Applied this by bumping H.264 source bitrates from 4 → 8 Mbps:

- `x264enc bitrate=8000` (was 4000) — kbps units
- `openh264enc bitrate=8000000` (was 4000000) — bps units
- `vp8enc target-bitrate=8000000` (was 4000000) — bps units

Kept `complexity=low` on openh264enc — realtime constraint on ARM
(must hit 30 fps or buffers drop). The preview branch eats the ~5%
CPU budget that would otherwise have gone to `complexity=medium`.

The intermediate H.264 file is deleted post-transcode (in
`onTranscodeFinished`), so the higher bitrate doesn't bloat persistent
storage — it just gives the AV1 second pass more visual information
to compress optimally.

### What this DOES NOT do (yet)
- The 1500 ms kernel-release wait at the START of recording is still
  there. The viewfinder shows the "RECORDING" placeholder during that
  window (no preview JPEG exists yet). Could be reduced by polling for
  `multifilesink`'s first file write, but that's a separate session.
- HW-accelerated encode/decode (V4L2 M2M on FP5) still TODO.

### Files touched
- `include/ui/video/VideoRecorder.hpp` — added `previewFilePath()`,
  `m_previewPath`
- `src/ui/video/VideoRecorder.cpp` — pipeline rewrite (tee + preview
  branch), bitrate bumps, preview file cleanup on success/failure
- `include/ui/CameraWidget.hpp` — added `startPreviewPolling()`,
  `stopPreviewPolling()`, `m_previewPollTimer`, `m_previewLastMtime`;
  forward-declared QTimer
- `src/ui/CameraWidget.cpp` — implemented helpers; calls into them
  from launchRecorder lambda (start), recordingStopped (stop),
  recordingFailed (stop)

---

## V32.5 — Viewfinder freeze on recording start + audio robustness

### "Camera freezes when I start holding the record button"

Root cause (revised diagnosis): the v4l2 device is single-consumer, so
when recording starts we must call `m_backend->stop()` to release
`/dev/video0` for the recorder's gst-launch. The Qt backend's frame
callbacks stop, but the **last frame remains painted on the viewfinder
QLabel** — looks identical to a hung/dead camera.

This is the entire ~1.5s kernel-release window PLUS the entire recording
duration. User reasonably interprets that as the camera being broken
and concludes "the file must be empty too." (Spoiler: it's not — the
recorder is happily writing to disk; the recording log shows the file
filling normally.)

Fix in CameraWidget::onRecordStarted (when we pause the Qt backend):
clear the stale viewfinder pixmap immediately and replace it with a
prominent "RECORDING" placeholder — large red dot + bold text, on a
dark gray background. The user now sees an unambiguous state change
(viewfinder → recording placeholder) and can't mistake it for a freeze.

The small red dot indicator in the corner still shows elapsed time as
before; the new placeholder is additive.

### Future: actual live preview during recording (TODO)

The placeholder is a band-aid — what the user really wants is to see
what they're recording. v4l2 single-consumer prevents the obvious
"two consumers on /dev/video0" approach. Logged in TODO.md as three
candidate strategies:
1. Tee branch in recorder pipeline → low-res JPEG via multifilesink;
   viewfinder polls (hacky cross-platform baseline)
2. xvimagesink/glimagesink embedded in our QWindow (lower latency,
   platform-specific)
3. PipeWire bidirectional stream (cleanest, but excludes Asahi-style
   v4l2-direct path)

Probably option 1 first.

### Camera resume after recording (separate fix this rev)

Symptom: viewfinder stayed paused for the FULL transcode window
(5-30 s on desktop) before coming back, because `onTranscodeFinished`
was the only resume site. But the transcoder reads a file, not the
camera — the camera was free for the whole transcode.

Fix: resume the viewfinder in the `recordingStopped` handler (500 ms
defer for kernel STREAMOFF/STREAMON ordering). The "Encoding NN%"
indicator continues to overlay the live viewfinder during background
transcode. The `onTranscodeFinished` resume is now a no-op safety net.

### Audio: re-encode opus on output instead of -c:a copy

Symptom: audio sometimes missing from output webm even though ffmpeg's
input scan showed an opus stream and the mapping line said `(copy)`.

Root cause: matroska's opus framing isn't byte-identical to webm's
opus framing. ffmpeg usually re-muxes correctly but edge cases (mono
variant, unusual channel layout, partial frames at EOS) produce a
webm with zero-length audio packets — players show no audio.

Fix in FFmpeg backend: `-c:a libopus -b:a 96k -ac 2 -ar 48000`. Always
re-encode to a known-good 96 kbps stereo opus at 48 kHz. CPU cost ~5%,
negligible vs AV1 video encode. Corrupt source audio now fails LOUDLY
instead of silently producing empty audio.

Fix in gstreamer backends: replaced `opusparse` passthrough with
`opusdec → audioconvert → audioresample → caps → opusenc bitrate=96000`.
Same robustness rationale, matches the ffmpeg path.

### Audio probe timeout 2 s → 5 s

Observed timing out on a desktop with pipewire+wireplumber where
device enumeration takes ~3 s on first call after boot. The "assume
available" fallback was adding the audiosrc branch optimistically;
if pipewire wasn't actually ready, gst-launch would fail at audio
device open. Bumped to 5 s; subsequent records use the fast path
(probe completes in <200 ms once pipewire is warm).

---

## V32.4.1 — Build break fixes for V32.4

### Issue 1: tierName ambiguity
The `EncodeTier` enum lives in `SnapClone::Core::Media`. I'd redefined
`tierName(EncodeTier)` ALSO in `SnapClone::UI::Video` as a "convenience
forwarding wrapper" alongside the type alias. When unqualified
`tierName(spec.tier)` was called from inside `SnapClone::UI::Video`,
ADL on the EncodeTier argument made BOTH `::Core::Media::tierName`
and `::UI::Video::tierName` visible — ambiguous overload.

Fix: removed the UI::Video wrapper. The type alias for EncodeTier
stays (for ergonomics) but the function call sites use the fully-
qualified `SnapClone::Core::Media::tierName(t)`. A short comment in
the header documents WHY we don't redefine the function locally, so
the next person doesn't re-introduce the wrapper.

### Issue 2: GCC 15 -Wnull-dereference on unique_ptr chain
`if (self->m_backend) { self->m_backend->start(); }` triggered a
false-positive null-dereference warning because GCC 15's analyzer
can't see through `unique_ptr::operator bool` + `operator->` across
inline expansion of the standard library implementation.

Fix: cache the raw pointer once into a local, then check + use the
local. Same logic, but the analyzer sees a single clearly-checked
pointer.

Both fixes are minimal and shouldn't affect runtime behavior.

---

## V32.4 — Tiered media delivery + pluggable encoder backends

### Three things in this rev

**1. Reversed V32.3's aggressive default downscale.**
The 480p default was a "fix the symptom" move that compromised the
product goal (high-quality video messages). Now defaults are: keep
source resolution, 2500 kbps. Downscaling becomes a deliberate
per-snap choice driven by the TieredDeliveryPolicy below — not a
blanket setting.

**2. Pluggable encoder backend abstraction.**
`include/ui/video/VideoEncoderBackend.hpp` defines `IVideoEncoderBackend`:
each backend declares its binary, args, and progress-parse regex.
Priority-ordered fallback chain on startup, probed once and cached:

| Priority | Backend          | Binary           | Notes |
|----------|------------------|------------------|-------|
| 1        | ExternalCodec    | `snapcodec`      | Scaffold for in-development codec; falls through to FFmpeg/gst when binary absent; override via `SNAPCLONE_CODEC=/path/to/binary` |
| 2        | FfmpegLibSvtAv1  | `ffmpeg`         | Newer SVT-AV1 features; accurate `frame=N` progress; preferred over gst-svtav1enc when both available |
| 3        | GstSvtAv1        | `gst-launch-1.0` | Current production default (V32.2 pipeline) |
| 4        | GstVp9           | `gst-launch-1.0` | Fallback when svtav1enc missing |
| 5        | GstVp8           | `gst-launch-1.0` | Last-ditch |

VideoTranscoder no longer hardcodes the pipeline — it asks the chosen
backend for `binary()` and `buildArgs(spec)`. Adding a new backend is
~80 lines in `VideoEncoderBackend.cpp`.

The external-codec scaffold honors `SNAPCLONE_CODEC` env var for
out-of-tree testing. Expected CLI shape:

```
snapcodec encode --in INPUT --out OUTPUT --tier {thumbnail|preview|standard|highquality}
                 [--width N] [--bitrate K] [--no-audio]
```

with progress on stderr as `progress: NN` lines. When your codec ships,
update the regex/args in `ExternalCodecBackend` if needed.

**3. TieredDeliveryPolicy** (`include/core/TieredDeliveryPolicy.hpp`,
`src/core/TieredDeliveryPolicy.cpp`).
Decides which quality tiers to send for a given snap based on:
- Source size and duration
- Per-peer NetworkStats profile (success rate, median latency, class)
- DebugFlag::NoFsmTuning (when ON, bypass policy → Standard only)

Default heuristics (start simple, tune over time):
- Source < 200 KB → `[Standard]` (too small to tier)
- Same-LAN peer with good history + source < 2 MB → `[Standard]`
- Source > 5 MB → `[Thumbnail, Preview, Standard, HighQuality]`
- Cellular / CGNAT / slow peer → `[Thumbnail, Preview, Standard]`
- Default → `[Preview, Standard]`

`choose(Context)` is a pure function — no I/O, no Qt signals, easy to
unit-test by varying Context. The CALLER (AppController, when it
processes Send) is responsible for invoking the transcoder once per
tier in the returned plan and transmitting each result with the
appropriate tier marker.

`include/core/EncodeTier.hpp` holds the shared enum (used by both
core/ and ui/video/) so the dependency direction stays clean —
core never includes from ui/.

### What's wired vs. what's scaffolded

Wired now:
- VideoTranscoder uses the backend abstraction (Standard tier path
  still produces the same output as before V32.4).
- TieredDeliveryPolicy::choose() is implemented and unit-testable.

Scaffolded but NOT wired:
- AppController invoking `choose()` and producing N transcodes per
  snap. That requires wire-format extension (tier byte on Snap frames)
  and SnapRouter ANNOUNCE/HAVE/NEED logic to handle multi-tier
  versions of the same snap ID. **Tier byte transmission is the next
  major piece of work** — separate session, larger change.
- HighQuality tier per-peer toggle in Settings UI.
- ExternalCodec binary integration — turns on automatically when the
  binary appears on PATH.

### Why per-snap tiering as a CHOICE, not always-on

Two devices on same Gigabit LAN: tiering is overhead. A full clip
arrives in ~80 ms; thumbnails just clog the pipe.

Tiny snaps (5-second 480p ~200 KB): thumbnail isn't meaningfully
smaller than the original. Skip it.

Cellular peer at ~50 KB/s: thumbnail + preview + full is essential
or the user waits 10+ minutes.

The policy module captures this decision in one testable function.

---

## V32.3 — Transcoder speed (5-10× faster)

### Symptom
After V32.2 fixed the hang, transcoding still took 20-30s for a 5s
720p input on a desktop machine where "the same gst pipeline by hand
takes 0.5s." Reading the log carefully showed three independent
sources of waste, all stacking up.

### Cause 1: `add-borders=true` letterboxing
Desktop camera (1920×1440, 4:3) targeting 720×1280 (9:16 portrait)
produced output that was ~50% black bars. The encoder still had to
compress those bars. Estimated waste: 30-40% of encode time on
content the user doesn't care about.

Fix: dropped `add-borders=true`. Constrain width only via the caps
filter (`video/x-raw,width=W,pixel-aspect-ratio=1/1`); height floats
to whatever the source aspect requires. 4:3 input → 4:3 output. 16:9
input → 16:9 output. No black bars.

### Cause 2: SVT-AV1 preset=10 is way overkill for ephemeral snaps
preset=10 with default GOP=161, mini-GOP=16, look-ahead enabled is
quality-tuned. For a 5-30s clip that auto-deletes after viewing we
want speed, not quality.

Fix: preset=13 (fastest), plus `parameters-string=lookahead=0:pred-struct=0`
to disable scene look-ahead (irrelevant for short clips) and B-frames
(slightly larger output, faster encode). Net ~3-5× faster encode for
~25% larger output — well within cellular budget.

Same treatment for VP9 fallback (`cpu-used=8 deadline=1`, was just
`deadline=1`) and VP8 fallback (`cpu-used=16 deadline=1`).

### Cause 3: videoconvert BEFORE videoscale
For a 1920×1440 NV12 source the previous pipeline did:
  decodebin → videoconvert (NV12→I420, ~8 MB/frame work)
            → videoscale (drops 90% of the pixels we just converted)

Fix: reversed order to scale first, convert second:
  decodebin → videoscale (drops to 480×360 NV12, ~260 KB/frame)
            → videoconvert (NV12→I420 on the small frame, ~16× less work)

This is independent of the encoder speedup and stacks with it.

### Cause 4: Default resolution too high
720×1280 was the default. For ephemeral snaps on phone screens, 480
width is plenty. Halving the resolution quarters the pixel count and
proportionally cuts encode time.

Fix: default `targetWidth` 720 → 480, `targetHeight` 1280 → 854
(hint only — height now floats by aspect ratio).

### Expected result
Stacked: ~3× from preset, ~2× from no-borders, ~4× from scale-first,
~4× from lower default resolution. Some of these overlap; rough
expectation is ~10× faster on a 1920×1440 desktop input, ~5× faster
on a phone-native 1280×720 input.

### Side note: progress meter
SVT-AV1 doesn't emit `Progress: NN%` lines (only the multi-line config
banner during init). The current progress meter is driven by the time
fallback (`m_estimateTimer` creeps 1% per 3s) until output-size growth
takes over. With the new speed, the encode often finishes before the
fallback emits anything past 5% — so the user sees the bar jump from
0 to "Done." That's a UX bug to address separately; tracking in
docs/TODO.md.

### Pipeline shape now

```
filesrc ! matroskademux name=d
d.video_0 ! queue ! decodebin ! videoscale
       ! video/x-raw,width=W,pixel-aspect-ratio=1/1
       ! videoconvert ! video/x-raw,format=I420
       ! queue ! svtav1enc preset=13 parameters-string=lookahead=0:pred-struct=0
                 target-bitrate=K ! mux.
[if hasAudio:]
d.audio_0 ! queue ! opusparse ! queue ! mux.
webmmux name=mux ! filesink location=<out>
```

---

## V32.2 — Video transcoder hang fix (parsebin+decodebin negotiation)

### Symptom
User reported transcode taking very long with no significant cpu activity
on either the main thread or the gst thread — classic signature of a
pipeline stuck in PREROLL waiting for a negotiation that never completes.

### Root causes (three contributing bugs)

**(1) `parsebin ! decodebin` chain.** decodebin already contains a parser
stage internally. Chaining parsebin in front of it makes decodebin
re-detect caps from already-parsed output, and the two get into a
negotiation deadlock at PREROLL — gst-launch sits in PAUSED forever with
no cpu. The standard patterns are EITHER `decodebin` alone OR
`<codec>parse ! <codec>dec` explicitly, never `parsebin ! decodebin`.

The previous comment claimed parsebin was needed because openh264enc
produces byte-stream H.264 that decodebin couldn't handle. That's true
when feeding the encoder OUTPUT through decodebin without h264parse —
but in OUR case the encoder output goes through `matroskamux` first
(via the recorder), and matroskamux stores it in AVC form. decodebin
reading from matroskademux gets AVC packets and handles them fine on
its own.

**(2) Missing `video/x-raw,format=I420` caps filter.** svtav1enc accepts
only I420 or I420_10LE input (per gst-inspect-1.0). Without the explicit
filter, videoconvert picks whatever format the upstream prefers (often
NV12 on ARM or YUY2 from a USB cam) → svtav1enc refuses → PREROLL hang
(no error, no log, just stuck). VP8/9 also prefer I420 so the same
filter works for the fallback chain.

**(3) Missing `queue` after `opusparse`.** webmmux has to pull from both
video and audio branches. A single queue at each branch start isn't
enough when video encoding catches up — audio backpressure stalls the
video encoder and vice versa. Adding a second queue between opusparse
and the muxer decouples them.

### Fix
- Replaced `parsebin ! decodebin` with `decodebin` alone
- Added `video/x-raw,format=I420` caps filter (with width/height when
  scaling, format-only when not)
- Added second `queue` between encoder caps and encoder
- Added `queue` after opusparse before mux

### Diagnostic improvements
- Added `-m` to gst-launch args — emits state-transition messages on
  stderr ("Pipeline is PREROLLING", "PLAYING", state changes per element).
  Without `-m`, gst-launch is silent during PREROLL and we have nothing
  to diagnose hangs with.
- Bumped stderr handler from qCDebug to qCInfo so we see the state
  messages live during a hang.
- Bumped pipeline-args log from qCDebug to qCInfo with `args.join(' ')`
  so the user can copy-paste to reproduce manually: `gst-launch-1.0
  <pasted-args>` and watch exactly where the stall happens.
- Added `if (!m_proc) return;` null guard in stderr lambda — when the
  process is torn down asynchronously the lambda can fire one more
  time on the deleting-shortly object.

### Pipeline shape now

```
filesrc ! matroskademux name=d
d.video_0 ! queue ! decodebin ! videoconvert
       ! video/x-raw,format=I420[,width=W,height=H]
       ! queue ! <encoder> <args> ! mux.
[if hasAudio:]
d.audio_0 ! queue ! opusparse ! queue ! mux.
webmmux name=mux ! filesink location=<out>
```

---

## V32.1 — Build break fixes

### Compile errors from V32 introduction

- **`lcUdp` passed where `QLoggingCategory&` expected.** `Q_LOGGING_CATEGORY`
  declares the symbol as a *function* returning `QLoggingCategory&`, not as
  a variable. My V32 code passed the function name directly, which the
  compiler caught as "invalid initialization of reference from expression
  of type `const QLoggingCategory&()`." Fixed by calling it: `lcUdp()`.
  Future use sites should follow the same pattern (and any FSMValidator
  adopters should remember this).
- **`klass` variable shadowed** in `NetworkManager::connectToPeer`. The
  outer function declared `const auto klass = AddressClass::classify(...)`
  at the top. My V32 lambda for the `connected` signal also declared
  `const auto klass = ...` for the *connected* address class. The lambda
  captures the enclosing scope, so the inner declaration shadowed the
  outer. Compiler `-Wshadow` warning (treated as error in our build).
  Renamed inner to `connectedKlass`.

### Pre-existing use-after-free fixed

- **`JamiAdapter_dht_impl.cpp::lookupPeer` done_cb** had `delete c` followed
  by `c->alive->load(...)`. The `disc` and `userId` were saved off before
  the delete, but `alive` was read AFTER. GCC 15's `-Wuse-after-free`
  caught it. Fixed by also saving the `shared_ptr<atomic<bool>>` to a
  local before the delete. This was latent — the shared_ptr's control
  block survives the delete of `c` because we hold a copy of the
  shared_ptr in the value_cb closures, so in practice the load was
  reading freed-but-still-mapped memory and returning the right answer
  by luck. The fix removes the luck.

### Narrowing warnings silenced

- `scoreProfile` did arithmetic mixing `qint64` and `double` constants.
  GCC's `-Wconversion` flagged `(1000 - p.medianMs)` as narrowing because
  the result is `qint64` and gets implicitly promoted to `double` for the
  multiplication. Restructured to convert `p.medianMs` to `double` once
  at the top of the if-chain so all subsequent arithmetic is pure
  `double`.

### Documentation

Yes, all the changes from V28 through V32 are documented across
`docs/WORK_LOG.md` (per-session detail) and `docs/TODO.md` (followups).
The session catalog `/mnt/transcripts/journal.txt` keeps a brief
external pointer to each session transcript. No documentation drift
from the recent rapid changes; the work-log entries are written at
the same time as the code changes.

---

## V32 — NetworkStats learning + debug toggle infrastructure

### Two complementary pieces in this rev

**NetworkStats** (`include/network/NetworkStats.hpp`,
`src/network/NetworkStats.cpp`) — per-peer connectivity profile that
learns across sessions. Records every connect attempt as
`(peerId, addressClass, outcome, latencyMs)` and rank-orders address
classes per peer based on success rate × recency × latency.

Wired into:
- `NetworkManager::onTransportConnected` (success) and
  `onTransportDisconnected` when `connecting && !connected` (timeout).
- `AppController_commands.cpp` cache-hit ordering: after the heuristic
  same-LAN-first sort, we apply a stable_sort by learned per-peer class
  priority. The heuristic provides a sensible default; the learner
  overrides it when it has evidence the heuristic is wrong for a
  specific peer.

This addresses the "good double-check via other methods" angle — the
same-LAN-first heuristic is a pure structural assumption. If Alice's
phone has a flaky WiFi NIC, the heuristic actively makes things worse
by trying the LAN address first every time. With learned stats we
know "Alice's LAN succeeds 20% of the time at 8s, public succeeds
95% at 1s" and route accordingly.

**DebugFlags** (`include/debug/DebugFlags.hpp`,
`src/debug/DebugFlags.cpp`) — process-wide debug toggles that can be
flipped via:
- Environment variables (test pipelines, CI):
  `SNAPCLONE_NO_FSM_TUNING=1`, `SNAPCLONE_NO_MONITORING_BLOCKING=1`,
  `SNAPCLONE_VERBOSE_FSM=1`
- CLI flags: `--no-fsm-tuning`, `--no-monitoring-blocking`,
  `--verbose-fsm`
- Live toggling via `DebugFlags::instance().set(...)` (Settings page
  UI is a future addition)
- Persisted in QSettings under `debug/` group

Every flip is logged at WARNING level so the logs make it obvious when
monitoring is being bypassed. End-of-session emits a summary:

    Debug flag summary at shutdown (session 12345 ms):
      no_fsm_tuning           OFF (was ON for 8000 ms across 2 toggles)
      no_monitoring_blocking  OFF (never enabled)
      verbose_fsm             ON  (was ON for 4000 ms across 1 toggle)

The summary also includes per-peer NetworkStats success-rate data so
post-mortems can see what worked in this session.

### Why these go together

The whole point of the FSM monitoring (V31) is to gather signals
without changing behaviour. NetworkStats is one of the consumers of
that signal — it records the connection outcomes that the FSM transitions
imply. `NoFsmTuning` toggles whether the LEARNING is APPLIED to routing
decisions (the data is still recorded so we can A/B compare).
`NoMonitoringBlocking` toggles whether validators produce log output
at all (useful when other code paths flood logs and we want quiet).

### Limits / future

- NetworkStats is in-memory only. Persistence via StorageManager is
  TODO — without it, the learner cold-starts on every app launch.
- The score function (`scoreProfile`) uses fixed weights (0.6, 0.25,
  0.15). Tuning these requires real-world data which we don't have yet.
- Settings-page UI for live-toggling debug flags is TODO.
- The reranking only applies on cache hits in `connectByDhtId`. The
  live-DHT-lookup path also gets candidate addresses; it should
  consult NetworkStats too.

---

## V31 — FSM alignment as defense-in-depth

### What this addresses

The previous defensive passes caught **parse-level** exploits (bad ports,
oversized fields, bogus addresses). They don't catch **sequence-level**
ones: messages where every individual field is valid but the order is
wrong. Examples:

- A peer sends `ACK` for a session we never started → we update phantom
  state, may corrupt window-tracking
- A peer sends `CHUNK` after they've cleanly disconnected → replay
  attack or session-confusion bug
- A peer sends `CONNECT` twice without us ack'ing the first → state
  pile-up, possible DoS surface
- Storage state diverges from network state in protocol-impossible ways

### Design

New header `include/defense/FSMValidator.hpp` — tiny, header-only,
pure-functional. No state owned by the validator; callers manage their
own FSM and ask the validator to log when transitions don't match.

Rate-limited misalignment logging (5s window, with counter). Repeated
misalignments from one peer = strong signal of either a protocol bug
on our side, a peer-side bug, or an active attack.

### First adopter: UdpChannel

Added explicit `PeerState::Phase { Idle, Connecting, Established }`
on top of the existing `connected` boolean. The phase changes when
we send CONNECT (→ Connecting), receive CONNECT or CONNECT_ACK
(→ Established), or when a peer is pruned for inactivity (→ Idle).

Inbound frames now go through `isLegalInbound(phase, type)` before
processing. Illegal transitions are logged but NOT rejected — the
protocol has been operating under "accept everything" rules and
tightening that risks breaking peers running older code. The log
gives us a low-risk way to OBSERVE whether tightening would be safe.

Legal transitions:
- `Idle`        → only `CONNECT` accepted (everything else implies
  the peer thinks we have state we don't)
- `Connecting`  → `CONNECT_ACK`, `CONNECT` (simultaneous open),
  `KEEPALIVE` (tolerance)
- `Established` → all five frame types

### Why this is a good double-check

Parse-level validators answer "is this message well-formed?" — they
trust the protocol layer to be in a sensible state. FSM-level
validators answer "is this message appropriate to RECEIVE NOW?" —
they catch exploits that wear a well-formed disguise.

Specifically helps with hardening that other methods don't catch:
- Pre-handshake data injection (parse-level says "valid CHUNK frame,"
  FSM says "we never connected to you, why are you sending CHUNKs")
- Replay after disconnect (parse-level passes, FSM catches that
  the peer is in Idle and we shouldn't see anything but CONNECT)
- Future bugs in our own state-management code (an internal bug
  that leaves phase = Idle while sending CHUNKs surfaces in the log
  rather than silently corrupting state)

### Scope limit

Did NOT model:
- SnapRouter per-snap-per-peer state (OFFER → NEED → SNAP → ACK chain)
  — would require per-snap state tracking, too much surface area for
  this pass; revisit if real exploit data shows up
- Version negotiation (peer "is or isn't v3 capable") — monotonic
  state, no misalignment risk

---

## V30 — Hardening untrusted input + media file extensions

### Untrusted-input validation
- **Gossip JSON port range** — `obj["port"].toInt()` returns 0 on type
  mismatch and silently clamps on overflow. A malicious or buggy peer
  could advertise `port: 99999` which gets truncated to a valid-looking
  number, or `port: "lol"` which becomes 0 (causing connectToPeer with
  port 0). Now explicit `0 < portRaw <= 65535` check before narrowing
  to quint16. Two sites fixed: top-level peer announce + piggy-backed
  peer-list entries.
- **QR scan port range** — same class of bug. `toUShort()` returns 0 on
  overflow; a QR code claiming port 99999 was silently interpreted as
  port 0. Now uses `toUInt(&ok)` with explicit range check.
- **Unknown MsgType from peer** (V29 work) — now logged rate-limited
  instead of silently dropped, so version-skew or malicious traffic is
  visible to operators.

### writeMediaFile extension correctness
- **Was:** every video saved as `.mp4` regardless of actual container.
  Our recorder produces `.mkv` (matroskamux) and transcoder produces
  `.webm` (webmmux + VP8/9/AV1 + Opus). Receivers got files with the
  wrong extension that confused external players and file managers.
- **Now:** detect container by magic bytes — EBML header for WebM/MKV
  (chooses `.webm` since our pipeline targets webmmux), `ftyp` at
  offset 4 for MP4, `moov`/`mdat` for MOV, JPEG/PNG magic for images.
  Falls back to media-type default; unknown video → `.bin` rather
  than lying with `.mp4`.

### Hunt false positives documented
- **Virtual destructors on 4 base classes** (CameraBackend, VideoRecorder,
  IDiscovery, ITransport) — all inherit from QObject which has a virtual
  destructor and use `override = default;` which guarantees correct
  override. The hunt regex didn't account for inheritance chains.
- **30+ singleShot lambdas without explicit guard** — all use `this`
  as receiver, which is documented Qt behaviour to auto-disconnect.
- **One dead-looking QTimer** in ContactsPage — actually started via
  signal-slot wiring (`textChanged → QTimer::start`). Clever debounce
  pattern, kept.
- **Storage migration runner** — already does per-migration transactions
  with rollback. Idempotent `ALTER TABLE ADD COLUMN` skip via column
  existence check. Migration version persisted outside transaction so
  it survives a crash between commit and PRAGMA.

---

## V29 — Defence in depth + functional refactor

### Bugs caught (& fixed)

- **`isPrivate` would have been a compile error** after the same-LAN-first
  refactor in V28. Previous code declared a local `isPrivate` lambda
  inside the cache-hit block, then the same name was referenced at
  line 358 outside the lambda's scope. The refactor removed the
  lambda — line 358 was relying on it. Fixed by switching to
  `AC::isPrivate` from the AddressClassification helper.
- **`SnapComposerDialog` 50ms focus singleShot captured `this` by raw
  reference** while the receiver was `m_editor` (a child of `this`).
  If the dialog is destroyed in the 50ms window, the lambda runs with
  a stale `this`. Fixed by capturing `QPointer<QWidget>` to m_editor
  by value.
- **`SnapRouter::handleInboundV2/V3` switch on `MsgType` had no
  default case**. Unknown bytes from a peer were silently dropped
  with zero log visibility. A malicious peer could fill our network
  layer with unknown types and we'd have no idea. Added rate-limited
  warning (every 30s, "N unknown MsgTypes from peer X").
- **AppController_connect `t->deleteLater()` mid-lambda** — the timer
  pointer was used in subsequent lines of the SAME lambda body after
  deleteLater. Safe in practice (deleteLater queues to next event
  loop) but brittle. Reordered to put deleteLater at the END of the
  lambda.

### Functional / readability improvements

- **`AddressClassification.hpp` gained `isSameLan24(a, b)`** — the
  same-/24 detection logic that was inline in AppController_commands
  is now reusable. Header-only, no state.
- **AppController_commands cache-hit ordering uses `namespace AC =`
  alias** + helper functions. Replaces the inline isPrivate/isSameLanAs
  lambdas with `AC::isPublic(...)` and `AC::isSameLan24(...)`. Reads
  declaratively as "if public → publicAddrs; else if same-LAN → fast
  path; else → far-LAN."

### Hunt results (false positives documented)

- **`.first()/.last()` calls without isEmpty()** — all guarded by
  container-population sequence (append immediately before, or earlier
  contains/isEmpty check). Confirmed safe.
- **Range-for over `m_sessions` with `erase` inside** — looked unsafe
  but: (a) unordered_map erase only invalidates the iterator to the
  erased element, (b) we immediately `return`. Safe.
- **Many `singleShot(longDelay, this, [this](){...})`** — the receiver
  is `this`. Qt auto-disconnects when the receiver is destroyed; the
  lambda will not fire on a stale `this`. Safe.
- **`deleteLater()` followed by use in same scope** — most are inside
  a queued lambda where the deleteLater queues to the NEXT event loop
  tick, so the subsequent use is on a still-valid object. Documented;
  one case (`AppController_connect`) was reordered for clarity even
  though it was technically safe.

---

## V28 — Bug hunt, defensive code, network adaptiveness

### Network adaptiveness
- **Same-LAN-first address ordering** in `AppController_commands.cpp`.
  Previous code put public addresses first, LAN last — meant two phones
  on the same WiFi waited ~10s for NAT hairpin to time out before
  falling through to the LAN address that would have worked instantly.
  Detection: same `/24` (same broadcast domain on consumer routers).
- **AddressClassification helper** at `include/network/AddressClassification.hpp`.
  Pure functional, header-only. Classifies any IPv4/IPv6 string into
  loopback / link-local / RFC1918 / RFC6598-CGNAT / ULA / public.
  Replaces 32 scattered inline subnet checks. Used in NetworkManager
  for richer reject reasons + CGNAT informational logs.
- **WebSocket pong tracking.** Keepalive previously sent pings every
  30s but didn't track replies. Now counts unanswered pings and
  force-disconnects after 3 misses (90s), letting the reconnect path
  try alt addresses immediately instead of waiting on TCP's 2-5
  minute stale-detection.

### Defensive code
- **VideoRecorder & VideoTranscoder destructors hardened.** Same
  pattern as `GstLaunchBackend::stop()` — disconnect signals BEFORE
  `kill()`, drop `waitForFinished()` (which runs a nested event loop
  and is the root cause of multiple historical crashes), use
  `deleteLater()`.
- **UdpChannel input validation tightened.** Fixed wrong header-size
  check (was 13 bytes, should be 17), removed unsigned-underflow risk
  on `raw.size() - kFixedHdr`, capped `total` chunk count
  (1M = 48GB ceiling), enforced `seq < total` invariant. Prevents a
  malicious peer from making us allocate huge buffers.

### Self-feedback-loop guards
- Triple-layer defense against loopback / self-id packets:
  1. `NatTraversal::onPunchReadyRead` drops PUNCH and SCUD frames
     from loopback sources
  2. `NetworkManager::connectToPeer` refuses loopback / self-id
     targets at the top level
  3. `UdpChannel::connectToPeer` refuses loopback peers as a backstop
- Counter-style rate-limited warnings (every 5s, "dropped N frames")
  with BUILD-MARKER strings so `strings build/src/SnapClone | grep
  DROP-V2` confirms the new code is compiled in.

### Documentation
- Created `docs/VISION.md` (what we're building & why)
- Created `docs/ROADMAP.md` (strategic priorities, stage-by-stage)
- Created `docs/TODO.md` (tactical work, with status tags)
- Created `docs/DEVELOPING.md` (orientation for new contributors)
- Created this file: `docs/WORK_LOG.md`

---

## V27 → V28 transition (multiple smaller passes)

### V4l2 desktop reliability
- **Root cause finally identified:** "Buffer pool activation failed"
  is a USB bandwidth contention issue, not memory. The USB host
  controller refuses STREAMON because the previous client's
  periodic-bandwidth slot wasn't released yet.
- **Fix:** `QtCameraBackend::stop()` does FULL teardown (destroys
  QCamera, QMediaCaptureSession, QImageCapture, QVideoSink) instead
  of just calling `m_camera->stop()`. The kernel then processes
  VIDIOC_STREAMOFF + USB alt-setting reset. Combined with a fixed
  1500ms wait before gst-launch starts, this resolves the issue
  on Asahi/Fedora with FaceTime HD.
- Removed bogus VIDIOC_REQBUFS readiness polling — it tests memory
  contention, but the actual gating ioctl is STREAMON which has no
  pre-checkable equivalent.

### Mobile crash on send (root cause)
- The `ContactPickerDialog::contactSelected` signal fired synchronously
  inside the Send button click handler, which then called `accept()`.
  Re-emitting `snapCaptured` synchronously re-entered the outer
  handler, which called `m_swiper->goToPage(kPageInbox)` triggering
  `CameraWidget::hideEvent` → `m_backend->stop()` →
  `QProcess::waitForFinished(500)` — and **`waitForFinished` runs a
  nested event loop** while the picker dialog was still visible and
  mid-animation. Half-destroyed widget tree → segfault on next layout.
- **Fix:** Defer the re-emit via
  `QMetaObject::invokeMethod(... Qt::QueuedConnection)` so the
  picker's animation/cleanup completes before page transition.
- **Also fixed:** GstLaunchBackend::stop() removed `waitForFinished`
  and added pre-kill signal disconnect.

### Recordings → CacheLocation
- Per XDG Base Directory Spec, recordings are non-essential and
  regenerable. Moved from `AppLocalDataLocation` (persistent app
  data) to `CacheLocation` (`~/.cache/SnapClone/recordings/` on Linux).
  Backup tools auto-exclude `.cache/`.

### Transcoder reliability
- **Encoder fallback chain:** Probe `svtav1enc` → `vp9enc` → `vp8enc`
  via `gst-inspect-1.0`. svtav1enc is preferred but not always
  available (`gst-plugins-rs` packaging varies).
- **Audio probe:** `gst-discoverer-1.0` detects whether input has an
  audio track. Without this, video-only inputs hung PREROLL forever
  waiting on a `d.audio_0` pad that never linked.
- **Progress meter robustness:** AV1 buffers ~30-60 frames before
  emitting bytes, so size-based progress sticks at 0% for 10-30s.
  Combined with a time-based monotonic creep (1%/3s capped at 60%)
  so users always see motion.
- **NV12 explicit conversion** in `QtCameraBackend::onVideoFrame`.
  Qt's `frame.toImage()` produces tiled garbage on Asahi/aarch64.
  Manual YUV→RGB via fixed-point BT.601 limited-range math, only
  for `Format_NV12`; other formats unchanged.

### Wire format v4
- Bumped `buildV3` to write `kWireVersion4` with `videoDurationMs`
  after `durationSecs`, before the F_SYNC/F_DATA tail.
- Receiver accepts both v3 and v4; reads videoDurationMs only when
  `version >= kWireVersion4`.
- Fixed pre-existing bug in SYNC re-send: previously lost mediaType
  + durationSecs (videos arrived as broken 5s images).
- DB schema migration #11 already in place from earlier session;
  storage now correctly persists videoDurationMs through restart.

---

## Earlier sessions (V20-V26 summary)

Detailed transcripts in `/mnt/transcripts/` for sessions V20-V27.
Highlights:

- **V20-V22:** OpenDHT integration; STUN reflexive detection; UPnP
  port mapping; multi-address DHT announces.
- **V23-V24:** SnapRouter v3 wire format; SYNC reconciliation;
  encrypted DHT mailbox for offline peers.
- **V25:** Wayfinder geographic friend view; per-contact stats;
  WiFi-SSID location indicators.
- **V26:** MobileDialog scrim cleanup (3-bug fix with 15s safety
  timer); v4l2 buffer pool first-pass investigation; openh264enc +
  h264parse fix.
- **V27:** Recording lifecycle hardening; ShutterButton TouchCancel
  handling; Wayfinder fresh-snap pulse animation; PollinatorPage
  removed (consolidated into ContactsPage).

---

## Style note

When a session lands non-obvious fixes, prefer a short entry here
that captures:
1. What was broken
2. What the root cause turned out to be (not just the symptom)
3. The fix
4. Anything to watch for if the same area is touched again

This file is for future-us, not for changelogs to users. Be candid
about wrong turns — they're useful information.
